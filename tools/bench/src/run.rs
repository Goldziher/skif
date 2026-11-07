//! Runs hyperfine for skif and the optional comparison tools, then writes the summary.

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::corpus;
use crate::parity;
use crate::proc::find_on_path;
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
const SUMMARY_SCHEMA: u32 = 1;
const GENERATED_SEED: u64 = 1;
const GENERATED_MISMATCH_RATE: f64 = 0.05;
/// Seconds an install of a corpus repository may take, unless the corpus manifest sets its own.
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

    /// The executable that must be on `PATH`, or `None` for skif (given by path).
    const fn executable(self) -> Option<&'static str> {
        match self {
            Self::Skif | Self::SkifManifest => None,
            Self::Sherif => Some("sherif"),
            Self::Syncpack => Some("syncpack"),
            Self::Knip => Some("knip"),
        }
    }
}

#[derive(Debug, Deserialize)]
struct HyperfineExport {
    results: Vec<HyperfineResult>,
}

#[derive(Debug, Deserialize)]
struct HyperfineResult {
    command: String,
    mean: f64,
    stddev: Option<f64>,
    median: f64,
    min: f64,
    max: f64,
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

#[derive(Debug, Serialize)]
struct Summary {
    schema: u32,
    skif: String,
    host: String,
    hyperfine: String,
    warmup: u32,
    runs: u32,
    skipped_tools: Vec<String>,
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

fn tool_command(tool: Tool, skif: &Path, extra: &[String]) -> String {
    match tool {
        Tool::Skif => skif_command(skif, extra, false),
        Tool::SkifManifest => skif_command(skif, extra, true),
        Tool::Sherif => "sherif .".to_owned(),
        Tool::Syncpack => "syncpack lint".to_owned(),
        Tool::Knip => "knip --reporter json".to_owned(),
    }
}

struct Target {
    name: String,
    kind: &'static str,
    packages: Option<usize>,
    files: Option<usize>,
    dir: PathBuf,
    tools: Vec<Tool>,
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
    Ok(serde_json::from_str::<HyperfineExport>(&text)
        .with_context(|| format!("parsing {}", export.display()))?
        .results)
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
            out.push(TargetCheck {
                target: target.target.clone(),
                description: format!("JS analysis >= {KNIP_SPEEDUP_TARGET}x faster than knip (measured {speedup:.1}x)"),
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
        "hyperfine `{}`, {} warmup runs, {} measured runs. Times are wall clock, warm file cache.\n\n",
        summary.hyperfine, summary.warmup, summary.runs
    );
    if !summary.skipped_tools.is_empty() {
        let _ = write!(
            out,
            "Skipped (not installed): {}.\n\n",
            summary.skipped_tools.join(", ")
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

fn available_tools(skipped: &mut Vec<String>) -> Vec<Tool> {
    let mut tools = Vec::new();
    for tool in [Tool::Sherif, Tool::Syncpack, Tool::Knip] {
        let name = tool.executable().unwrap_or_default();
        if find_on_path(name).is_some() {
            tools.push(tool);
        } else {
            eprintln!("bench: {name} not installed, skipping");
            skipped.push(name.to_owned());
        }
    }
    tools
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
    let raw = options.out.join("raw");
    fs::create_dir_all(&raw).with_context(|| format!("creating {}", raw.display()))?;

    let mut skipped = Vec::new();
    let comparison = available_tools(&mut skipped);

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
                }
            }
            targets.push(Target {
                name: format!("corpus: {} ({})", repo.name, repo.package_manager),
                kind: "corpus",
                packages: None,
                files: None,
                dir: fs::canonicalize(&dir)?,
                tools,
            });
        }
    }

    let mut measured = Vec::new();
    for (index, target) in targets.iter().enumerate() {
        let commands: Vec<(Tool, String)> = target
            .tools
            .iter()
            .map(|&tool| (tool, tool_command(tool, &skif, &options.skif_args)))
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
        host: format!("{} {}", env::consts::OS, env::consts::ARCH),
        hyperfine: version,
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
        };
        let summary = |checks: Vec<TargetCheck>, skipped: Vec<String>| Summary {
            schema: SUMMARY_SCHEMA,
            skif: "skif 0.0.0".into(),
            host: "linux x86_64".into(),
            hyperfine: "hyperfine 1.20.0".into(),
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
        for text in [
            markdown(&summary(vec![check], Vec::new())),
            markdown(&summary(Vec::new(), vec!["knip".into()])),
        ] {
            assert!(text.ends_with(" |\n"), "{text:?}");
            assert!(!text.ends_with("\n\n"));
            assert!(!text.contains("\n\n\n"));
            assert!(text.lines().all(|line| line == line.trim_end()));
        }
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
            }],
        };
        assert!(checks(&[target(10_000, 900.0)])[0].passed);
        assert!(!checks(&[target(10_000, 2500.0)])[0].passed);
        assert!(checks(&[target(500, 2500.0)]).is_empty());
    }
}
