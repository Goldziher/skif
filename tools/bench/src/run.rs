//! Runs hyperfine for skif and the optional comparison tools, then writes the summary.

use std::collections::BTreeMap;
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::corpus::{self, Tools};
use crate::parity;
use crate::proc::{capture, find_on_path};
use crate::provenance;
use crate::sources;
use skif_bench::generate::{self, Options, PackageManager};

/// ADR 0001: manifest and policy rules on 500 packages.
const MANIFEST_TARGET_PACKAGES: usize = 500;
const MANIFEST_TARGET_MS: f64 = 50.0;
/// ADR 0001: JS analysis versus knip on the corpus.
const KNIP_SPEEDUP_TARGET: f64 = 5.0;
/// ADR 0001: parse and build the module graph of 10,000 files.
const GRAPH_TARGET_FILES: usize = 10_000;
const GRAPH_TARGET_MS: f64 = 2000.0;
/// Version of the `summary.json` layout.
const SUMMARY_SCHEMA: u32 = 2;
const GENERATED_SEED: u64 = 1;
const GENERATED_MISMATCH_RATE: f64 = 0.05;
/// How long an install of a corpus repository may take, unless the corpus manifest sets its own.
const INSTALL_TIMEOUT: Duration = Duration::from_mins(30);

pub struct RunOptions {
    pub skif: PathBuf,
    pub skif_args: Vec<String>,
    pub sizes: Vec<usize>,
    pub package_manager: PackageManager,
    pub corpus_file: PathBuf,
    pub corpus_dir: PathBuf,
    pub use_corpus: bool,
    pub generated_dir: PathBuf,
    pub out: PathBuf,
    pub warmup: u32,
    pub runs: u32,
    /// Total source files of the module graph case; `0` skips it.
    pub graph_files: usize,
    /// Packages the graph case spreads its files over.
    pub graph_packages: usize,
    /// Return an error when an ADR 0001 check fails.
    pub enforce: bool,
    /// Install the dependencies of the corpus repositories before timing knip, as `parity --install` does.
    pub install: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tool {
    Skif,
    SkifManifest,
    Sherif,
    Syncpack,
    Knip,
}

impl Tool {
    const fn label(self) -> &'static str {
        match self {
            Self::Skif => "skif",
            Self::SkifManifest => "skif --no-js",
            Self::Sherif => "sherif",
            Self::Syncpack => "syncpack lint",
            Self::Knip => "knip --reporter json",
        }
    }

    /// The npm package (and its executable) of a comparison tool, or `None` for skif (given by path).
    const fn package(self) -> Option<&'static str> {
        match self {
            Self::Skif | Self::SkifManifest => None,
            Self::Sherif => Some("sherif"),
            Self::Syncpack => Some("syncpack"),
            Self::Knip => Some("knip"),
        }
    }
}

/// The comparison tools, each at the version `corpus.toml` pins.
const COMPARISON_TOOLS: [Tool; 3] = [Tool::Sherif, Tool::Syncpack, Tool::Knip];

/// The pinned version of a comparison tool; `None` for skif.
fn pinned(tools: &Tools, tool: Tool) -> Option<&str> {
    match tool {
        Tool::Sherif => Some(tools.sherif.as_str()),
        Tool::Syncpack => Some(tools.syncpack.as_str()),
        Tool::Knip => Some(tools.knip.as_str()),
        Tool::Skif | Tool::SkifManifest => None,
    }
}

#[derive(Debug, Deserialize)]
struct HyperfineExport {
    results: Vec<RawResult>,
}

/// One command of a hyperfine export, in the layout of hyperfine 1.x (times and exit codes at the top level, the
/// `--command-name` in `command`) or 2.x (times under `summary.time_wall_clock`, exit codes per measurement, the
/// `--command-name` in `name` when it differs from the command line).
#[derive(Debug, Deserialize)]
struct RawResult {
    command: String,
    name: Option<String>,
    mean: Option<f64>,
    stddev: Option<f64>,
    median: Option<f64>,
    min: Option<f64>,
    max: Option<f64>,
    #[serde(default)]
    exit_codes: Vec<Option<i32>>,
    summary: Option<RawSummary>,
    #[serde(default)]
    measurements: Vec<RawMeasurement>,
}

#[derive(Debug, Deserialize)]
struct RawSummary {
    time_wall_clock: RawStats,
}

#[derive(Debug, Deserialize)]
struct RawStats {
    mean: f64,
    stddev: Option<f64>,
    median: f64,
    min: f64,
    max: f64,
}

#[derive(Debug, Deserialize)]
struct RawMeasurement {
    exit_code: Option<i32>,
}

#[derive(Debug)]
struct HyperfineResult {
    command: String,
    mean: f64,
    stddev: Option<f64>,
    median: f64,
    min: f64,
    max: f64,
    /// Runs that exited with a status above 1 or by a signal. The linters exit with 1 when they report findings;
    /// knip exits with 2 when it fails or cannot load a configuration file, so its time may be a partial run.
    error_runs: usize,
}

impl TryFrom<RawResult> for HyperfineResult {
    type Error = anyhow::Error;

    fn try_from(raw: RawResult) -> Result<Self> {
        let codes: Vec<Option<i32>> = if raw.measurements.is_empty() {
            raw.exit_codes
        } else {
            raw.measurements.iter().map(|m| m.exit_code).collect()
        };
        let error_runs = codes.iter().filter(|code| code.is_none_or(|code| code > 1)).count();
        let stats = match (raw.summary, raw.mean, raw.median, raw.min, raw.max) {
            (Some(summary), ..) => summary.time_wall_clock,
            (None, Some(mean), Some(median), Some(min), Some(max)) => RawStats {
                mean,
                stddev: raw.stddev,
                median,
                min,
                max,
            },
            _ => bail!("no times for {} in the hyperfine export", raw.command),
        };
        Ok(Self {
            command: raw.name.unwrap_or(raw.command),
            mean: stats.mean,
            stddev: stats.stddev,
            median: stats.median,
            min: stats.min,
            max: stats.max,
            error_runs,
        })
    }
}

#[derive(Debug, Serialize)]
struct ToolResult {
    tool: String,
    mean_ms: f64,
    stddev_ms: Option<f64>,
    median_ms: f64,
    min_ms: f64,
    max_ms: f64,
    /// `mean / skif mean` for the matching skif row; above 1 means slower than skif.
    relative_to_skif: Option<f64>,
    /// Measured runs that exited with an error (a status above 1, or a signal).
    error_runs: usize,
}

#[derive(Debug, Serialize)]
struct TargetResult {
    target: String,
    kind: &'static str,
    packages: Option<usize>,
    files: Option<usize>,
    results: Vec<ToolResult>,
}

#[derive(Debug, Serialize)]
struct TargetCheck {
    target: String,
    description: String,
    passed: bool,
}

/// A comparison tool left out of a target, or of the whole run (`target` is `None`), and why.
#[derive(Debug, Clone, Serialize)]
struct Skipped {
    tool: String,
    target: Option<String>,
    reason: String,
}

#[derive(Debug, Serialize)]
struct Summary {
    schema: u32,
    skif: String,
    /// The skif source commit (`-dirty` with local changes) the harness ran from.
    source_rev: String,
    /// When the measurement ended, in seconds since the Unix epoch.
    measured_at: u64,
    host: String,
    cpu: String,
    cores: usize,
    /// The 1, 5 and 15 minute load averages before the first and after the last measurement.
    load_before: Option<[f64; 3]>,
    load_after: Option<[f64; 3]>,
    hyperfine: String,
    /// Each comparison tool's `--version` output, from the install of the versions `corpus.toml` pins.
    tool_versions: BTreeMap<String, String>,
    warmup: u32,
    runs: u32,
    skipped_tools: Vec<Skipped>,
    targets: Vec<TargetResult>,
    adr_0001_checks: Vec<TargetCheck>,
}

/// Quotes one argument for hyperfine's no-shell command splitting.
fn quote(argument: &str) -> String {
    if argument.contains(char::is_whitespace) || argument.contains('"') {
        format!("\"{}\"", argument.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        argument.to_owned()
    }
}

fn skif_command(skif: &Path, extra: &[String], no_js: bool) -> String {
    // `extra` holds the target's own arguments (such as `--recursive`) after the run's `--skif-arg`s.
    let mut parts = vec![
        quote(&skif.to_string_lossy()),
        ".".to_owned(),
        "--format".into(),
        "json".into(),
        "--no-install".into(),
    ];
    if no_js {
        parts.push("--no-js".into());
    }
    parts.extend(extra.iter().map(|arg| quote(arg)));
    parts.join(" ")
}

/// The command line of `tool`, whose executable is `program` (the skif binary for the skif rows).
fn tool_command(tool: Tool, program: &Path, extra: &[String], knip_args: &[String]) -> String {
    let quoted = || quote(&program.to_string_lossy());
    match tool {
        Tool::Skif => skif_command(program, extra, false),
        Tool::SkifManifest => skif_command(program, extra, true),
        Tool::Sherif => format!("{} .", quoted()),
        Tool::Syncpack => format!("{} lint", quoted()),
        Tool::Knip => std::iter::once(format!("{} --reporter json", quoted()))
            .chain(knip_args.iter().map(|arg| quote(arg)))
            .collect::<Vec<_>>()
            .join(" "),
    }
}

struct Target {
    name: String,
    kind: &'static str,
    packages: Option<usize>,
    files: Option<usize>,
    dir: PathBuf,
    tools: Vec<Tool>,
    /// The corpus repository's `bench-knip-args`.
    knip_args: Vec<String>,
    /// skif arguments of this target: `--recursive` where the corpus repository sets `recursive`, as in `parity`.
    skif_args: Vec<String>,
}

fn hyperfine(options: &RunOptions, target: &Target, commands: &[(Tool, String)], export: &Path) -> Result<()> {
    let mut command = Command::new("hyperfine");
    command
        .current_dir(&target.dir)
        .args(["-N", "--ignore-failure", "--style", "basic", "--warmup"])
        .arg(options.warmup.to_string())
        .arg("--runs")
        .arg(options.runs.to_string())
        .arg("--export-json")
        .arg(export);
    for (tool, line) in commands {
        command.arg("--command-name").arg(tool.label()).arg(line);
    }
    eprintln!("bench: {} ({} tools)", target.name, commands.len());
    let status = command.status().context("running hyperfine")?;
    if !status.success() {
        bail!("hyperfine failed on {} ({status})", target.name);
    }
    Ok(())
}

fn read_results(export: &Path) -> Result<Vec<HyperfineResult>> {
    let text = fs::read_to_string(export).with_context(|| format!("reading {}", export.display()))?;
    serde_json::from_str::<HyperfineExport>(&text)
        .with_context(|| format!("parsing {}", export.display()))?
        .results
        .into_iter()
        .map(HyperfineResult::try_from)
        .collect::<Result<_>>()
        .with_context(|| format!("reading {}", export.display()))
}

fn tool_results(results: &[HyperfineResult]) -> Vec<ToolResult> {
    let mean_of = |label: &str| results.iter().find(|r| r.command == label).map(|r| r.mean);
    let manifest_mean = mean_of(Tool::SkifManifest.label());
    let full_mean = mean_of(Tool::Skif.label());
    results
        .iter()
        .map(|r| {
            let reference = if r.command == Tool::Sherif.label() || r.command == Tool::Syncpack.label() {
                manifest_mean.or(full_mean)
            } else {
                full_mean.or(manifest_mean)
            };
            ToolResult {
                tool: r.command.clone(),
                mean_ms: r.mean * 1000.0,
                stddev_ms: r.stddev.map(|s| s * 1000.0),
                median_ms: r.median * 1000.0,
                min_ms: r.min * 1000.0,
                max_ms: r.max * 1000.0,
                relative_to_skif: reference.map(|skif| r.mean / skif),
                error_runs: r.error_runs,
            }
        })
        .collect()
}

fn checks(targets: &[TargetResult]) -> Vec<TargetCheck> {
    let mut out = Vec::new();
    for target in targets {
        let find = |label: &str| target.results.iter().find(|r| r.tool == label);
        if target.kind == "generated"
            && target.packages == Some(MANIFEST_TARGET_PACKAGES)
            && let Some(skif) = find(Tool::SkifManifest.label())
        {
            out.push(TargetCheck {
                target: target.target.clone(),
                description: format!(
                    "manifest and policy rules < {MANIFEST_TARGET_MS} ms (measured {:.1} ms)",
                    skif.mean_ms
                ),
                passed: skif.mean_ms < MANIFEST_TARGET_MS,
            });
        }
        if target.kind == "graph"
            && let (Some(files), Some(skif)) = (target.files, find(Tool::Skif.label()))
            && files >= GRAPH_TARGET_FILES
        {
            out.push(TargetCheck {
                target: target.target.clone(),
                description: format!(
                    "parse and module graph of {files} files < {GRAPH_TARGET_MS} ms, whole run including every JS rule (measured {:.1} ms)",
                    skif.mean_ms
                ),
                passed: skif.mean_ms < GRAPH_TARGET_MS,
            });
        }
        if target.kind == "corpus"
            && let (Some(skif), Some(knip)) = (find(Tool::Skif.label()), find(Tool::Knip.label()))
        {
            let speedup = knip.mean_ms / skif.mean_ms;
            let errors = if knip.error_runs > 0 {
                format!("; knip exited with an error in {} runs", knip.error_runs)
            } else {
                String::new()
            };
            out.push(TargetCheck {
                target: target.target.clone(),
                description: format!(
                    "JS analysis >= {KNIP_SPEEDUP_TARGET}x faster than knip (measured {speedup:.1}x{errors})"
                ),
                passed: speedup >= KNIP_SPEEDUP_TARGET,
            });
        }
    }
    out
}

fn format_ms(value: f64) -> String {
    if value >= 100.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    }
}

fn markdown(summary: &Summary) -> String {
    let mut out = String::from("# skif benchmark results\n\n");
    let _ = write!(
        out,
        "{} at `{}`; hyperfine `{}`, {} warmup runs, {} measured runs. Times are wall clock, warm file cache.\n\n",
        summary.skif, summary.source_rev, summary.hyperfine, summary.warmup, summary.runs
    );
    let _ = write!(
        out,
        "Host: {}, {} ({} logical CPUs); load average {} before and {} after the run.\n\n",
        summary.host,
        summary.cpu,
        summary.cores,
        format_load(summary.load_before),
        format_load(summary.load_after)
    );
    if !summary.tool_versions.is_empty() {
        let versions: Vec<String> = summary
            .tool_versions
            .iter()
            .map(|(tool, version)| format!("{tool} `{version}`"))
            .collect();
        let _ = write!(out, "Comparison tools: {}.\n\n", versions.join(", "));
    }
    for skipped in &summary.skipped_tools {
        let _ = write!(
            out,
            "Not timed: {} on {}: {}.\n\n",
            skipped.tool,
            skipped.target.as_deref().unwrap_or("every target"),
            skipped.reason
        );
    }
    for target in &summary.targets {
        let _ = write!(out, "## {}\n\n", target.target);
        out.push_str("| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |\n");
        out.push_str("| --- | ---: | ---: | ---: | ---: | ---: | ---: |\n");
        for r in &target.results {
            let _ = writeln!(
                out,
                "| {} | {} | {} | {} | {} | {} | {} |",
                r.tool,
                format_ms(r.mean_ms),
                r.stddev_ms.map_or_else(|| "-".to_owned(), format_ms),
                format_ms(r.median_ms),
                format_ms(r.min_ms),
                format_ms(r.max_ms),
                r.relative_to_skif
                    .map_or_else(|| "-".to_owned(), |x| format!("{x:.1}x")),
            );
        }
        out.push('\n');
        for r in target.results.iter().filter(|r| r.error_runs > 0) {
            let _ = write!(
                out,
                "{} exited with an error in {} of {} runs, so its time may be that of a partial run.\n\n",
                r.tool, r.error_runs, summary.runs
            );
        }
    }
    if !summary.adr_0001_checks.is_empty() {
        out.push_str("## ADR 0001 targets\n\n| Target | Check | Result |\n| --- | --- | --- |\n");
        for check in &summary.adr_0001_checks {
            let _ = writeln!(
                out,
                "| {} | {} | {} |",
                check.target,
                check.description,
                if check.passed { "pass" } else { "FAIL" }
            );
        }
    }
    // Every section above ends in a blank line; the file ends in exactly one newline, as `poly fmt` writes it.
    let end = out.trim_end().len();
    out.truncate(end);
    out.push('\n');
    out
}

/// `1.50 / 1.20 / 0.90` for the 1, 5 and 15 minute load averages, `unknown` without them.
fn format_load(load: Option<[f64; 3]>) -> String {
    load.map_or_else(
        || "unknown".to_owned(),
        |[one, five, fifteen]| format!("{one:.2} / {five:.2} / {fifteen:.2}"),
    )
}

fn skif_version(skif: &Path) -> String {
    Command::new(skif).arg("--version").output().map_or_else(
        |_| "unknown".to_owned(),
        |o| String::from_utf8_lossy(&o.stdout).trim().to_owned(),
    )
}

fn hyperfine_version() -> Result<String> {
    let output = Command::new("hyperfine")
        .arg("--version")
        .output()
        .context("hyperfine is required: install it with `mise install` or `brew install hyperfine`")?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// The `npm install` arguments that put the pinned comparison tools into `prefix`.
fn tool_install_args(tools: &Tools, prefix: &Path) -> Vec<String> {
    let mut args: Vec<String> = ["install", "--no-save", "--no-package-lock", "--no-audit", "--no-fund", "--prefix"]
        .map(str::to_owned)
        .into();
    args.push(prefix.to_string_lossy().into_owned());
    args.extend(
        COMPARISON_TOOLS
            .iter()
            .filter_map(|&tool| Some(format!("{}@{}", tool.package()?, pinned(tools, tool)?))),
    );
    args
}

/// Installs the comparison tools at their pinned versions into `prefix` once, so that hyperfine times their
/// executables directly (not through `npx`, whose resolution would be timed too), and returns each installed tool
/// with its executable. A tool that cannot be installed is recorded in `skipped`.
fn install_tools(tools: &Tools, prefix: &Path, skipped: &mut Vec<Skipped>) -> Result<Vec<(Tool, PathBuf)>> {
    let mut skip_all = |reason: String| {
        for tool in COMPARISON_TOOLS {
            eprintln!("bench: {} is not timed: {reason}", tool.label());
            skipped.push(Skipped {
                tool: tool.label().to_owned(),
                target: None,
                reason: reason.clone(),
            });
        }
        Vec::new()
    };
    if find_on_path("npm").is_none() {
        return Ok(skip_all("npm is not installed, so the pinned tools cannot be".to_owned()));
    }
    fs::create_dir_all(prefix).with_context(|| format!("creating {}", prefix.display()))?;
    eprintln!("bench: installing the pinned comparison tools into {}", prefix.display());
    let run = capture(
        Command::new("npm").args(tool_install_args(tools, prefix)),
        prefix,
        "npm-install",
        INSTALL_TIMEOUT,
    )?;
    if run.timed_out || run.code != Some(0) {
        return Ok(skip_all(format!("npm install of the pinned tools failed: {}", run.failure_summary())));
    }
    let mut installed = Vec::new();
    for tool in COMPARISON_TOOLS {
        let program = prefix.join("node_modules").join(".bin").join(tool.package().unwrap_or_default());
        if program.is_file() {
            installed.push((tool, program));
        } else {
            skipped.push(Skipped {
                tool: tool.label().to_owned(),
                target: None,
                reason: format!("the pinned install has no {}", program.display()),
            });
        }
    }
    Ok(installed)
}

/// The `--version` output of an installed tool, its first line.
fn tool_version(program: &Path) -> String {
    Command::new(program).arg("--version").output().map_or_else(
        |_| "unknown".to_owned(),
        |output| {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .next()
                .unwrap_or("unknown")
                .trim()
                .to_owned()
        },
    )
}

fn corpus_target_name(repo: &corpus::Repo) -> String {
    format!("corpus: {} ({})", repo.name, repo.package_manager)
}

/// The skif arguments of a corpus repository: `--recursive` where it sets `recursive`, as `parity` runs it, so the
/// timed run reads the nested roots knip reads from the root.
fn corpus_skif_args(repo: &corpus::Repo) -> Vec<String> {
    if repo.recursive {
        vec!["--recursive".to_owned()]
    } else {
        Vec::new()
    }
}

/// The ADR 0001 module graph case: one pnpm workspace whose packages hold `graph_files` TypeScript files in total.
fn graph_target(options: &RunOptions) -> Result<Target> {
    let packages = options.graph_packages.max(1);
    let per_package = options.graph_files.div_ceil(packages);
    let dir = options.generated_dir.join(format!("graph-{}", packages * per_package));
    if dir.exists() {
        fs::remove_dir_all(&dir).with_context(|| format!("cleaning {}", dir.display()))?;
    }
    let mut generated = generate::build(&Options {
        packages,
        seed: GENERATED_SEED,
        package_manager: PackageManager::Pnpm,
        catalog: false,
        mismatch_rate: 0.0,
    })?;
    sources::add_sources(&mut generated, per_package);
    generate::write(&generated, &dir)?;
    Ok(Target {
        name: format!("graph: {} files in {packages} packages", packages * per_package),
        kind: "graph",
        packages: Some(packages),
        files: Some(packages * per_package),
        dir: fs::canonicalize(&dir)?,
        tools: vec![Tool::Skif],
        knip_args: Vec::new(),
        skif_args: Vec::new(),
    })
}

/// Runs the whole benchmark and writes `summary.json` and `summary.md` into `options.out`.
///
/// # Errors
/// Fails when hyperfine or skif is missing, the corpus cannot be fetched, a measurement fails, or (with
/// `options.enforce`) an ADR 0001 target check fails; the summary is written before that last error.
pub fn run(options: &RunOptions) -> Result<()> {
    let version = hyperfine_version()?;
    let skif = fs::canonicalize(&options.skif).with_context(|| {
        format!(
            "skif binary not found at {} (build it or pass --skif)",
            options.skif.display()
        )
    })?;
    // hyperfine runs in each target's directory, so the export path it is given must not be relative.
    let raw = std::path::absolute(options.out.join("raw")).context("resolving the output directory")?;
    fs::create_dir_all(&raw).with_context(|| format!("creating {}", raw.display()))?;

    let source_rev = provenance::source_rev();
    let (pins, _) = corpus::select(&options.corpus_file, "bench", &[])?;
    let mut skipped = Vec::new();
    let prefix = std::path::absolute(options.generated_dir.join("tools")).context("resolving the tools directory")?;
    let installed = install_tools(&pins, &prefix, &mut skipped)?;
    let comparison: Vec<Tool> = installed.iter().map(|(tool, _)| *tool).collect();
    let tool_versions: BTreeMap<String, String> = installed
        .iter()
        .filter_map(|(tool, program)| Some((tool.package()?.to_owned(), tool_version(program))))
        .collect();
    let program_of = |tool: Tool| -> PathBuf {
        installed
            .iter()
            .find(|(candidate, _)| *candidate == tool)
            .map_or_else(|| skif.clone(), |(_, program)| program.clone())
    };

    let mut targets = Vec::new();
    for &size in &options.sizes {
        let dir = options
            .generated_dir
            .join(format!("{}-{size}", options.package_manager.as_str()));
        if dir.exists() {
            fs::remove_dir_all(&dir).with_context(|| format!("cleaning {}", dir.display()))?;
        }
        let generated = generate::build(&Options {
            packages: size,
            seed: GENERATED_SEED,
            package_manager: options.package_manager,
            catalog: false,
            mismatch_rate: GENERATED_MISMATCH_RATE,
        })?;
        generate::write(&generated, &dir)?;
        let mut tools = vec![Tool::SkifManifest];
        tools.extend(comparison.iter().copied().filter(|tool| *tool != Tool::Knip));
        targets.push(Target {
            name: format!("generated: {size} packages ({})", options.package_manager.as_str()),
            kind: "generated",
            packages: Some(size),
            files: None,
            dir: fs::canonicalize(&dir)?,
            tools,
            knip_args: Vec::new(),
            skif_args: Vec::new(),
        });
    }
    if options.graph_files > 0 {
        targets.push(graph_target(options)?);
    }
    if options.use_corpus {
        let (_, repos) = corpus::fetch_suite(&options.corpus_file, "bench", &[], &options.corpus_dir)?;
        for (repo, dir) in repos {
            let mut tools = vec![Tool::Skif, Tool::SkifManifest];
            tools.extend(comparison.iter().copied());
            // knip loads the configurations of the repository's tools, which import its own packages: without an
            // install it fails on them and the time is that of a partial run.
            if options.install && tools.contains(&Tool::Knip) {
                let scratch = options.generated_dir.join("install").join(&repo.name);
                if let Some(reason) = parity::install(&repo, &fs::canonicalize(&dir)?, &scratch, INSTALL_TIMEOUT)? {
                    eprintln!("bench: {} not installed ({reason}), knip is not timed there", repo.name);
                    tools.retain(|tool| *tool != Tool::Knip);
                    skipped.push(Skipped {
                        tool: Tool::Knip.label().to_owned(),
                        target: Some(corpus_target_name(&repo)),
                        reason: format!("the install failed: {reason}"),
                    });
                }
            }
            targets.push(Target {
                name: corpus_target_name(&repo),
                kind: "corpus",
                packages: None,
                files: None,
                dir: fs::canonicalize(&dir)?,
                tools,
                knip_args: repo.bench_knip_args.clone(),
                skif_args: corpus_skif_args(&repo),
            });
        }
    }

    let load_before = provenance::load_average();
    let mut measured = Vec::new();
    for (index, target) in targets.iter().enumerate() {
        let skif_args: Vec<String> = options.skif_args.iter().chain(&target.skif_args).cloned().collect();
        let commands: Vec<(Tool, String)> = target
            .tools
            .iter()
            .map(|&tool| (tool, tool_command(tool, &program_of(tool), &skif_args, &target.knip_args)))
            .collect();
        let export = raw.join(format!("{index:02}.json"));
        hyperfine(options, target, &commands, &export)?;
        measured.push(TargetResult {
            target: target.name.clone(),
            kind: target.kind,
            packages: target.packages,
            files: target.files,
            results: tool_results(&read_results(&export)?),
        });
    }

    let summary = Summary {
        schema: SUMMARY_SCHEMA,
        skif: skif_version(&skif),
        source_rev,
        measured_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_secs()),
        host: format!("{} {}", env::consts::OS, env::consts::ARCH),
        cpu: provenance::cpu(),
        cores: provenance::cores(),
        load_before,
        load_after: provenance::load_average(),
        hyperfine: version,
        tool_versions,
        warmup: options.warmup,
        runs: options.runs,
        skipped_tools: skipped,
        adr_0001_checks: checks(&measured),
        targets: measured,
    };
    fs::write(
        options.out.join("summary.json"),
        serde_json::to_string_pretty(&summary)? + "\n",
    )?;
    let table = markdown(&summary);
    fs::write(options.out.join("summary.md"), &table)?;
    println!("{table}");
    eprintln!("bench: wrote {}/summary.{{md,json}}", options.out.display());
    let failed: Vec<&TargetCheck> = summary.adr_0001_checks.iter().filter(|check| !check.passed).collect();
    if options.enforce && !failed.is_empty() {
        for check in &failed {
            eprintln!("bench: FAILED {}: {}", check.target, check.description);
        }
        bail!("{} ADR 0001 target check(s) failed", failed.len());
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn result(command: &str, mean: f64) -> HyperfineResult {
        HyperfineResult {
            command: command.to_owned(),
            mean,
            stddev: Some(0.001),
            median: mean,
            min: mean,
            max: mean,
            error_runs: 0,
        }
    }

    /// `poly fmt --check` (rumdl) accepts the published `summary.md` as written: one blank line between blocks, no
    /// trailing whitespace, exactly one final newline.
    #[test]
    fn summary_markdown_is_formatted() {
        let row = |tool: &str| ToolResult {
            tool: tool.into(),
            mean_ms: 12.34,
            stddev_ms: None,
            median_ms: 150.0,
            min_ms: 1.0,
            max_ms: 2.0,
            relative_to_skif: Some(1.0),
            error_runs: 0,
        };
        let summary = |checks: Vec<TargetCheck>, skipped: Vec<Skipped>| Summary {
            schema: SUMMARY_SCHEMA,
            skif: "skif 0.0.0".into(),
            source_rev: "0123456789ab".into(),
            measured_at: 1_794_268_800,
            host: "linux x86_64".into(),
            cpu: "Intel(R) Xeon(R) CPU @ 2.10GHz".into(),
            cores: 4,
            load_before: Some([1.5, 1.25, 1.0]),
            load_after: None,
            hyperfine: "hyperfine 1.20.0".into(),
            tool_versions: BTreeMap::from([("knip".to_owned(), "6.44.1".to_owned())]),
            warmup: 1,
            runs: 2,
            skipped_tools: skipped,
            targets: vec![TargetResult {
                target: "t".into(),
                kind: "generated",
                packages: Some(1),
                files: None,
                results: vec![row("skif --no-js"), row("sherif")],
            }],
            adr_0001_checks: checks,
        };
        let check = TargetCheck {
            target: "t".into(),
            description: "d".into(),
            passed: true,
        };
        let skipped = Skipped {
            tool: "knip --reporter json".into(),
            target: Some("corpus: r (pnpm)".into()),
            reason: "the install failed: x".into(),
        };
        let with_skip = markdown(&summary(Vec::new(), vec![skipped]));
        assert!(
            with_skip.contains("Not timed: knip --reporter json on corpus: r (pnpm): the install failed: x."),
            "{with_skip}"
        );
        assert!(
            with_skip.contains("skif 0.0.0 at `0123456789ab`; hyperfine `hyperfine 1.20.0`"),
            "{with_skip}"
        );
        assert!(
            with_skip.contains(
                "Host: linux x86_64, Intel(R) Xeon(R) CPU @ 2.10GHz (4 logical CPUs); load average 1.50 / 1.25 / 1.00 before and unknown after the run."
            ),
            "{with_skip}"
        );
        assert!(with_skip.contains("Comparison tools: knip `6.44.1`."), "{with_skip}");
        for text in [markdown(&summary(vec![check], Vec::new())), with_skip] {
            assert!(text.ends_with(" |\n"), "{text:?}");
            assert!(!text.ends_with("\n\n"));
            assert!(!text.contains("\n\n\n"));
            assert!(text.lines().all(|line| line == line.trim_end()));
        }
    }

    #[test]
    fn reads_the_exports_of_hyperfine_1_and_2() {
        let v1 = r#"{"results": [{"command": "knip --reporter json", "mean": 0.5, "stddev": 0.01, "median": 0.4,
            "min": 0.3, "max": 0.6, "exit_codes": [2, 1, 2]}]}"#;
        let v2 = r#"{"results": [{"command": "knip --reporter json --no-gitignore", "name": "knip --reporter json",
            "measurements": [{"exit_code": 1}, {"exit_code": null}, {"exit_code": 0}],
            "summary": {"time_wall_clock": {"unit": "second", "count": 3, "mean": 0.5, "stddev": 0.01,
            "median": 0.4, "min": 0.3, "max": 0.6}}},
            {"command": "syncpack lint", "measurements": [{"exit_code": 1}],
            "summary": {"time_wall_clock": {"mean": 0.1, "median": 0.1, "min": 0.1, "max": 0.1}}}]}"#;
        let dir = std::env::temp_dir().join(format!("skif-bench-hyperfine-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let read = |name: &str, text: &str| {
            let path = dir.join(name);
            fs::write(&path, text).unwrap();
            read_results(&path).unwrap()
        };
        let first = read("v1.json", v1);
        let second = read("v2.json", v2);
        fs::remove_dir_all(&dir).unwrap();
        for result in [&first[0], &second[0]] {
            assert_eq!(result.command, "knip --reporter json");
            assert!((result.mean - 0.5).abs() < 1e-9 && (result.median - 0.4).abs() < 1e-9);
        }
        assert_eq!(first[0].error_runs, 2);
        assert_eq!(second[0].error_runs, 1, "a run killed by a signal is an error");
        assert_eq!(second[1].command, "syncpack lint");
        assert_eq!(second[1].error_runs, 0);
    }

    #[test]
    fn knip_gets_the_repository_arguments_and_its_errors_are_reported() {
        let knip = Path::new("/bench/tools/node_modules/.bin/knip");
        let line = tool_command(Tool::Knip, knip, &[], &["--no-gitignore".to_owned()]);
        assert_eq!(line, "/bench/tools/node_modules/.bin/knip --reporter json --no-gitignore");
        let mut knip = result("knip --reporter json", 1.0);
        knip.error_runs = 3;
        let target = TargetResult {
            target: "corpus: r (pnpm)".into(),
            kind: "corpus",
            packages: None,
            files: None,
            results: tool_results(&[result("skif", 0.1), knip]),
        };
        let found = checks(std::slice::from_ref(&target));
        assert!(found[0].passed);
        assert!(
            found[0].description.contains("knip exited with an error in 3 runs"),
            "{}",
            found[0].description
        );
    }

    #[test]
    fn the_comparison_tools_are_installed_at_their_pins_and_timed_by_path() {
        let pins = Tools {
            knip: "6.44.1".into(),
            syncpack: "15.3.3".into(),
            sherif: "1.13.0".into(),
        };
        let args = tool_install_args(&pins, Path::new("/bench/tools"));
        assert_eq!(&args[..1], ["install"]);
        let prefix = args.iter().position(|arg| arg == "--prefix").unwrap();
        assert_eq!(args[prefix + 1], "/bench/tools");
        assert!(args.contains(&"--no-save".to_owned()));
        assert_eq!(
            &args[args.len() - 3..],
            ["sherif@1.13.0", "syncpack@15.3.3", "knip@6.44.1"]
        );
        let bin = Path::new("/my tools/node_modules/.bin");
        assert_eq!(
            tool_command(Tool::Sherif, &bin.join("sherif"), &[], &[]),
            "\"/my tools/node_modules/.bin/sherif\" ."
        );
        assert_eq!(
            tool_command(Tool::Syncpack, &bin.join("syncpack"), &[], &[]),
            "\"/my tools/node_modules/.bin/syncpack\" lint"
        );
    }

    #[test]
    fn a_recursive_corpus_repository_is_timed_recursively() {
        let file = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/corpus.toml"));
        let (_, bench) = corpus::select(file, "bench", &[]).unwrap();
        let babel = bench.iter().find(|repo| repo.name == "babel").unwrap();
        assert!(babel.recursive);
        let args = corpus_skif_args(babel);
        assert_eq!(args, ["--recursive"]);
        assert_eq!(
            tool_command(Tool::Skif, Path::new("/s/skif"), &args, &[]),
            "/s/skif . --format json --no-install --recursive"
        );
        assert_eq!(
            tool_command(Tool::SkifManifest, Path::new("/s/skif"), &args, &[]),
            "/s/skif . --format json --no-install --no-js --recursive"
        );
        let vue = bench.iter().find(|repo| repo.name == "vue-core").unwrap();
        assert!(corpus_skif_args(vue).is_empty());
    }

    #[test]
    fn quotes_only_when_needed() {
        assert_eq!(quote("/usr/bin/skif"), "/usr/bin/skif");
        assert_eq!(quote("/my dir/skif"), "\"/my dir/skif\"");
    }

    #[test]
    fn relative_times_use_the_matching_skif_row() {
        let rows = tool_results(&[
            result("skif", 0.1),
            result("skif --no-js", 0.02),
            result("sherif", 0.04),
            result("knip --reporter json", 1.0),
        ]);
        let by = |name: &str| rows.iter().find(|r| r.tool == name).unwrap().relative_to_skif.unwrap();
        assert!((by("sherif") - 2.0).abs() < 1e-9);
        assert!((by("knip --reporter json") - 10.0).abs() < 1e-9);
    }

    #[test]
    fn checks_flag_a_slow_manifest_run() {
        let target = |mean_ms| TargetResult {
            target: "t".into(),
            kind: "generated",
            packages: Some(MANIFEST_TARGET_PACKAGES),
            files: None,
            results: vec![ToolResult {
                tool: "skif --no-js".into(),
                mean_ms,
                stddev_ms: None,
                median_ms: mean_ms,
                min_ms: mean_ms,
                max_ms: mean_ms,
                relative_to_skif: None,
                error_runs: 0,
            }],
        };
        assert!(checks(&[target(10.0)])[0].passed);
        assert!(!checks(&[target(80.0)])[0].passed);
    }

    #[test]
    fn checks_flag_a_slow_graph_run_and_ignore_small_graphs() {
        let target = |files, mean_ms| TargetResult {
            target: "g".into(),
            kind: "graph",
            packages: Some(100),
            files: Some(files),
            results: vec![ToolResult {
                tool: "skif".into(),
                mean_ms,
                stddev_ms: None,
                median_ms: mean_ms,
                min_ms: mean_ms,
                max_ms: mean_ms,
                relative_to_skif: None,
                error_runs: 0,
            }],
        };
        assert!(checks(&[target(10_000, 900.0)])[0].passed);
        assert!(!checks(&[target(10_000, 2500.0)])[0].passed);
        assert!(checks(&[target(500, 2500.0)]).is_empty());
    }
}
