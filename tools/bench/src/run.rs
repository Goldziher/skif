//! Runs hyperfine for skif and the optional comparison tools, then writes the summary.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::corpus;
use crate::parity;
use crate::proc::capture;
use crate::provenance::{self, GithubRunner, Machine, Toolchain};
use crate::sources;
use crate::tools_install;
use crate::type_aware;
use skif_bench::generate::{self, Options, PackageManager};

/// ADR 0001: manifest and policy rules on 500 packages.
const MANIFEST_TARGET_PACKAGES: usize = 500;
const MANIFEST_TARGET_MS: f64 = 50.0;
/// ADR 0001: JS analysis versus knip on the corpus.
const KNIP_SPEEDUP_TARGET: f64 = 5.0;
/// ADR 0001: parse and build the module graph of 10,000 files.
const GRAPH_TARGET_FILES: usize = 10_000;
const GRAPH_TARGET_MS: f64 = 2000.0;
/// The type-aware budget of ADR 0029, each limit on the run with `--type-aware` against the default run of the same
/// target, without types.
struct TypeAwareBudget {
    /// Typed over untyped mean on the largest corpus repository, the one whose untyped run takes longest.
    largest_ratio: f64,
    /// The median of typed over untyped mean across the corpus repositories.
    median_ratio: f64,
    /// Peak RSS of the typed run on any target.
    peak_rss_bytes: u64,
    /// Mean of the typed run on the generated module graph of [`GRAPH_TARGET_FILES`] files.
    graph_ms: f64,
    /// knip's mean over the typed run's wherever knip completes.
    knip_speedup: f64,
}

const TYPE_AWARE_BUDGET: TypeAwareBudget = TypeAwareBudget {
    largest_ratio: 3.0,
    median_ratio: 2.0,
    peak_rss_bytes: 4 << 30,
    graph_ms: 5000.0,
    knip_speedup: 5.0,
};
/// Version of the `summary.json` layout.
const SUMMARY_SCHEMA: u32 = 8;
const GENERATED_SEED: u64 = 1;
const GENERATED_MISMATCH_RATE: f64 = 0.05;
/// How long an install of a corpus repository may take, unless the corpus manifest sets its own.
const INSTALL_TIMEOUT: Duration = Duration::from_mins(30);
/// How long the run outside hyperfine of a command that exited with an error may take.
const ERROR_RERUN_TIMEOUT: Duration = Duration::from_mins(10);
/// How many of the last lines of that run's stderr go to the log.
const ERROR_LOG_LINES: usize = 40;
/// The program that reports the peak RSS of the command it runs ([`peak_rss_flag`]).
const TIME_PROGRAM: &str = "/usr/bin/time";
/// How long the run under [`TIME_PROGRAM`] that measures a command's peak RSS may take.
const PEAK_RSS_TIMEOUT: Duration = Duration::from_mins(10);

// Independent switches, one per command-line flag.
#[allow(clippy::struct_excessive_bools)]
pub struct RunOptions {
    pub skif: PathBuf,
    pub skif_args: Vec<String>,
    pub sizes: Vec<usize>,
    pub package_manager: PackageManager,
    pub corpus_file: PathBuf,
    pub corpus_dir: PathBuf,
    pub use_corpus: bool,
    /// Only these repositories of the bench corpus; empty for all of them.
    pub only: Vec<String>,
    /// The checkout the skif binary was built from; the harness's own checkout when `None`.
    pub skif_source: Option<PathBuf>,
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
    /// Return an error when a type-aware budget check ([`TYPE_AWARE_BUDGET`]) fails.
    pub check_budget: bool,
    /// Install the dependencies of the corpus repositories before timing knip, as `parity --install` does.
    pub install: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tool {
    /// The whole run, without types (the default).
    Skif,
    /// The whole run with `--type-aware`.
    SkifTyped,
    SkifManifest,
    /// The whole run with `--recursive`, timed only where the corpus repository holds nested roots. It lints more
    /// projects than any comparison tool reads, so it has no ratio and is no part of a check (ADR 0037).
    SkifRecursive,
    Sherif,
    Syncpack,
    Knip,
}

impl Tool {
    const fn label(self) -> &'static str {
        match self {
            Self::Skif => "skif",
            Self::SkifTyped => type_aware::TYPED_TOOL,
            Self::SkifManifest => "skif --no-js",
            Self::SkifRecursive => RECURSIVE_TOOL,
            Self::Sherif => "sherif",
            Self::Syncpack => "syncpack lint",
            Self::Knip => "knip --reporter json",
        }
    }

    /// The npm package (and its executable) of a comparison tool, or `None` for skif (given by path).
    const fn package(self) -> Option<&'static str> {
        match self {
            Self::Skif | Self::SkifTyped | Self::SkifManifest | Self::SkifRecursive => None,
            Self::Sherif => Some("sherif"),
            Self::Syncpack => Some("syncpack"),
            Self::Knip => Some("knip"),
        }
    }
}

/// The label of the informational row of a recursive skif run ([`Tool::SkifRecursive`]).
const RECURSIVE_TOOL: &str = "skif --recursive";

/// The two modes of the whole skif run, timed and measured on every corpus and graph target.
const SKIF_MODES: [Tool; 2] = [Tool::Skif, Tool::SkifTyped];

/// The comparison tools, each at the version `corpus.toml` pins.
const COMPARISON_TOOLS: [Tool; 3] = [Tool::Sherif, Tool::Syncpack, Tool::Knip];

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

/// Whether a measured run's exit makes its time that of a partial run: a status above 1, or none (a signal). A status
/// of 1 is a complete run that reported findings, as sherif, syncpack, knip and skif exit when they find something.
const fn is_error_exit(code: Option<i32>) -> bool {
    match code {
        Some(code) => code > 1,
        None => true,
    }
}

impl TryFrom<RawResult> for HyperfineResult {
    type Error = anyhow::Error;

    fn try_from(raw: RawResult) -> Result<Self> {
        let codes: Vec<Option<i32>> = if raw.measurements.is_empty() {
            raw.exit_codes
        } else {
            raw.measurements.iter().map(|m| m.exit_code).collect()
        };
        let error_runs = codes.iter().filter(|code| is_error_exit(**code)).count();
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
    /// The error line of one more run outside hyperfine when `error_runs` is not 0, so the summary names the cause.
    error_output: Option<String>,
    /// The failure `corpus.toml` declares for this tool on the target, when that run matched it: the row is shown as
    /// failing, and its times (still recorded) are compared with nothing, unless the failure is `timed`.
    failure: Option<ExpectedFailure>,
    /// Peak resident set size of one more run under [`TIME_PROGRAM`], for the skif rows of both modes
    /// ([`SKIF_MODES`]); `None` for the other rows and where it could not be measured.
    peak_rss_bytes: Option<u64>,
}

/// A declared comparison tool failure ([`corpus::KnipFailure`]) that a run matched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ExpectedFailure {
    /// Why the tool fails there, as `corpus.toml` says.
    reason: String,
    /// The error line of the run outside hyperfine (the row's `error_output`).
    error: String,
    /// The tool printed its whole report before the error, so its time is compared like a clean run's
    /// ([`corpus::KnipFailure::timed`]).
    timed: bool,
}

/// The declared failure `declared`, when the run outside hyperfine failed that way: its `matches` text is in that
/// run's stderr or its error line. `None` when it is in neither, so a changed failure is not taken for the declared one.
fn matched_failure(declared: &corpus::KnipFailure, stderr: &str, error_output: &str) -> Option<ExpectedFailure> {
    (stderr.contains(&declared.matches) || error_output.contains(&declared.matches)).then(|| ExpectedFailure {
        reason: declared.reason.clone(),
        error: error_output.to_owned(),
        timed: declared.timed,
    })
}

#[derive(Debug, Serialize)]
struct TargetResult {
    target: String,
    kind: &'static str,
    packages: Option<usize>,
    files: Option<usize>,
    /// What installed a corpus repository before it was timed ([`install_label`]); `None` when it was not installed.
    installed_with: Option<String>,
    /// The knip failure `corpus.toml` declares for the corpus repository (`bench-knip-failure`).
    declared_knip_failure: Option<corpus::KnipFailure>,
    /// The warmup and measured runs of every command of the target: the run's, or fewer where the corpus repository
    /// caps them (`bench-max-warmup`, `bench-max-runs`).
    warmup: u32,
    runs: u32,
    results: Vec<ToolResult>,
}

/// The outcome of an ADR 0001 target check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum CheckStatus {
    Pass,
    Fail,
    /// A row the check compares exited with an error in a measured run that `corpus.toml` does not declare, so its
    /// time may be that of a partial run; or a declared failure did not happen as declared, so the declaration is
    /// stale.
    Invalid,
    /// The compared tool failed on the target as `corpus.toml` declares, so there is no time to compare.
    NotMeasured,
}

impl CheckStatus {
    /// `Invalid` when one of the compared rows errored, else `Pass` or `Fail` by `met`.
    fn of(met: bool, rows: &[&ToolResult]) -> Self {
        if rows.iter().any(|row| row.error_runs > 0) {
            Self::Invalid
        } else if met {
            Self::Pass
        } else {
            Self::Fail
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "FAIL",
            Self::Invalid => "invalid",
            Self::NotMeasured => "not measured",
        }
    }
}

#[derive(Debug, Serialize)]
struct TargetCheck {
    target: String,
    description: String,
    status: CheckStatus,
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
    /// The commit (`-dirty` with local changes) of the skif source the binary under test was built from.
    source_rev: String,
    /// The commit of the harness that measured it, which the workflow builds from the dispatched ref.
    harness_rev: String,
    /// When the measurement ended, in seconds since the Unix epoch.
    measured_at: u64,
    /// The machine the measurement ran on, with the GitHub Actions runner when it ran in a workflow.
    machine: Machine,
    /// `machine` in one line ([`machine_text`]), as `summary.md` and the published tables write it.
    machine_text: String,
    /// The compiler, runtime and package managers around the measurement.
    toolchain: Toolchain,
    /// The 1, 5 and 15 minute load averages before the first and after the last measurement.
    load_before: Option<[f64; 3]>,
    load_after: Option<[f64; 3]>,
    hyperfine: String,
    /// Each comparison tool's `--version` output, from the install of the versions `corpus.toml` pins.
    tool_versions: BTreeMap<String, String>,
    warmup: u32,
    runs: u32,
    /// The corpus repositories a run narrowed with `--only` timed; empty when it timed all of them.
    corpus_only: Vec<String>,
    skipped_tools: Vec<Skipped>,
    targets: Vec<TargetResult>,
    adr_0001_checks: Vec<TargetCheck>,
    /// The checks of [`TYPE_AWARE_BUDGET`].
    type_aware_budget_checks: Vec<TargetCheck>,
}

/// Quotes one argument for hyperfine's no-shell command splitting.
fn quote(argument: &str) -> String {
    if argument.contains(char::is_whitespace) || argument.contains('"') {
        format!("\"{}\"", argument.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        argument.to_owned()
    }
}

/// The program and arguments of `tool`, whose executable is `program` (the skif binary for the skif rows); `extra`
/// holds skif's arguments for the target (the run's `--skif-arg`s, then the target's own, such as `--config`).
fn tool_argv(tool: Tool, program: &Path, extra: &[String], knip_args: &[String]) -> Vec<String> {
    let mut argv = vec![program.to_string_lossy().into_owned()];
    let fixed: &[&str] = match tool {
        Tool::Skif | Tool::SkifTyped | Tool::SkifManifest | Tool::SkifRecursive => {
            &[".", "--format", "json", "--no-install"]
        }
        Tool::Sherif => &["."],
        Tool::Syncpack => &["lint"],
        Tool::Knip => &["--reporter", "json"],
    };
    argv.extend(fixed.iter().map(|arg| (*arg).to_owned()));
    match tool {
        Tool::Skif => argv.extend_from_slice(extra),
        Tool::SkifTyped => {
            argv.push(type_aware::TYPE_AWARE_FLAG.to_owned());
            argv.extend_from_slice(extra);
        }
        Tool::SkifManifest => {
            argv.push("--no-js".to_owned());
            argv.extend_from_slice(extra);
        }
        Tool::SkifRecursive => {
            argv.push("--recursive".to_owned());
            argv.extend_from_slice(extra);
        }
        Tool::Knip => argv.extend_from_slice(knip_args),
        Tool::Sherif | Tool::Syncpack => {}
    }
    argv
}

/// A program and its arguments as one hyperfine command line, each argument quoted where needed.
fn command_line(argv: &[String]) -> String {
    argv.iter().map(|arg| quote(arg)).collect::<Vec<_>>().join(" ")
}

/// Runs each command of `target` whose row exited with an error in a measured run once more, outside hyperfine, prints
/// the end of its stderr to the log and records its error line in the row, so the cause is visible. A knip row whose
/// run fails as the target's declared knip failure says is marked with that failure and loses its ratio to skif.
fn explain_errors(
    target: &Target,
    argvs: &[(Tool, Vec<String>)],
    rows: &mut [ToolResult],
    scratch: &Path,
) -> Result<()> {
    for row in rows.iter_mut().filter(|row| row.error_runs > 0) {
        let Some((tool, argv)) = argvs.iter().find(|(tool, _)| tool.label() == row.tool) else {
            continue;
        };
        let Some((program, arguments)) = argv.split_first() else {
            continue;
        };
        let run = capture(
            Command::new(program).args(arguments).current_dir(&target.dir),
            scratch,
            &format!("error-{}", tool.label().replace(' ', "_")),
            ERROR_RERUN_TIMEOUT,
        )?;
        let exit = run.code.map_or_else(
            || "no status (a signal or the time limit)".to_owned(),
            |code| format!("status {code}"),
        );
        let stderr: Vec<&str> = run.stderr.lines().collect();
        let tail = stderr[stderr.len().saturating_sub(ERROR_LOG_LINES)..].join("\n");
        eprintln!(
            "bench: {} on {} exited with an error in {} measured runs; run once more, it exited with {exit}. The end of its stderr:\n{tail}",
            row.tool, target.name, row.error_runs
        );
        let error_output = run.failure_summary();
        if *tool == Tool::Knip
            && let Some(declared) = &target.knip_failure
        {
            row.failure = matched_failure(declared, &run.stderr, &error_output);
            if row.failure.as_ref().is_some_and(|failure| !failure.timed) {
                row.relative_to_skif = None;
            }
        }
        row.error_output = Some(error_output);
    }
    Ok(())
}

struct Target {
    name: String,
    kind: &'static str,
    packages: Option<usize>,
    files: Option<usize>,
    dir: PathBuf,
    tools: Vec<Tool>,
    /// knip's arguments on this target ([`corpus_tool_args`]): the corpus repository's `bench-knip-args`, then the
    /// knip comparison's `--config` where it sets `bench-knip-config`.
    knip_args: Vec<String>,
    /// skif arguments of this target ([`corpus_tool_args`]): the `--config` with the `entry` files of the corpus
    /// repository where it sets `bench-knip-config`. Never `--recursive`, which only [`Tool::SkifRecursive`] adds.
    skif_args: Vec<String>,
    /// What installed the corpus repository ([`install_label`]); `None` when it was not installed.
    installed_with: Option<String>,
    /// The corpus repository's `bench-knip-failure`.
    knip_failure: Option<corpus::KnipFailure>,
    /// hyperfine's warmup and measured runs of each command ([`corpus_runs`] on a corpus repository).
    warmup: u32,
    runs: u32,
}

fn hyperfine(target: &Target, commands: &[(Tool, String)], export: &Path) -> Result<()> {
    let mut command = Command::new("hyperfine");
    command
        .current_dir(&target.dir)
        .args(["-N", "--ignore-failure", "--style", "basic", "--warmup"])
        .arg(target.warmup.to_string())
        .arg("--runs")
        .arg(target.runs.to_string())
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
            let reference = if r.command == RECURSIVE_TOOL {
                None
            } else if r.command == Tool::Sherif.label() || r.command == Tool::Syncpack.label() {
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
                error_output: None,
                failure: None,
                peak_rss_bytes: None,
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
                status: CheckStatus::of(skif.mean_ms < MANIFEST_TARGET_MS, &[skif]),
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
                status: CheckStatus::of(skif.mean_ms < GRAPH_TARGET_MS, &[skif]),
            });
        }
        if target.kind == "corpus"
            && let (Some(skif), Some(knip)) = (find(Tool::Skif.label()), find(Tool::Knip.label()))
        {
            out.push(knip_check(target, skif, knip, KNIP_SPEEDUP_TARGET));
        }
    }
    out
}

/// The check that skif is at least `speedup_target` times faster than knip on a corpus target, for ADR 0001 and the
/// type-aware budget. A knip failure the target declares and the run matched leaves it not measured, unless the
/// failure is `timed` (knip printed its whole report first), which is checked as a clean run with the declared reason
/// beside it; a declared failure that did not happen (knip ran cleanly, or failed with another error) makes it
/// invalid, so a changed failure is noticed and the declaration updated.
fn knip_check(target: &TargetResult, skif: &ToolResult, knip: &ToolResult, speedup_target: f64) -> TargetCheck {
    let goal = format!("JS analysis >= {speedup_target}x faster than knip");
    let speedup = knip.mean_ms / skif.mean_ms;
    let errors: String = [skif, knip]
        .iter()
        .filter(|row| row.error_runs > 0)
        .map(|row| format!("; {} exited with an error in {} runs", row.tool, row.error_runs))
        .collect::<Vec<_>>()
        .concat();
    let measured = format!("measured {speedup:.1}x{errors}");
    let met = speedup >= speedup_target;
    let (description, status) = match (&target.declared_knip_failure, &knip.failure) {
        (_, Some(failure)) if failure.timed => (
            format!(
                "{goal} ({measured}, as corpus.toml declares, after its report: {})",
                failure.reason
            ),
            CheckStatus::of(met, &[skif]),
        ),
        (_, Some(failure)) if skif.error_runs == 0 => (
            format!("{goal} (not measured: knip fails here: {})", failure.reason),
            CheckStatus::NotMeasured,
        ),
        (Some(declared), None) => {
            let how = if knip.error_runs == 0 {
                "knip ran without an error"
            } else {
                "knip's error is another one"
            };
            (
                format!(
                    "{goal} ({measured}; corpus.toml declares that knip fails here with {}, but {how}: the declaration is stale)",
                    code_span(&declared.matches)
                ),
                CheckStatus::Invalid,
            )
        }
        _ => (format!("{goal} ({measured})"), CheckStatus::of(met, &[skif, knip])),
    };
    TargetCheck {
        target: target.target.clone(),
        description,
        status,
    }
}

/// The rows of both skif modes of a target, typed first, when it timed both.
fn mode_rows(target: &TargetResult) -> Option<(&ToolResult, &ToolResult)> {
    let find = |tool: Tool| target.results.iter().find(|row| row.tool == tool.label());
    Some((find(Tool::SkifTyped)?, find(Tool::Skif)?))
}

/// The median of `values`, `None` when there are none.
fn median_of(mut values: Vec<f64>) -> Option<f64> {
    values.sort_by(f64::total_cmp);
    let half = values.len() / 2;
    match values.len() {
        0 => None,
        len if len % 2 == 1 => Some(values[half]),
        _ => Some(f64::midpoint(values[half - 1], values[half])),
    }
}

/// The checks of [`TYPE_AWARE_BUDGET`] (ADR 0029): per target, the typed run's peak RSS, its time on the module graph
/// case and its speed against knip; across the corpus, the typed over untyped ratio on the largest repository (the
/// one whose untyped run takes longest) and the median of that ratio. A check of a row that errored is invalid, and
/// a peak RSS that was not measured is not measured.
fn budget_checks(targets: &[TargetResult]) -> Vec<TargetCheck> {
    let budget = &TYPE_AWARE_BUDGET;
    let mut out = Vec::new();
    for target in targets {
        let Some((typed, _)) = mode_rows(target) else {
            continue;
        };
        let limit_mib = mib(budget.peak_rss_bytes);
        out.push(match typed.peak_rss_bytes {
            Some(bytes) => TargetCheck {
                target: target.target.clone(),
                description: format!("peak RSS with types <= {limit_mib} MiB (measured {} MiB)", mib(bytes)),
                status: CheckStatus::of(bytes <= budget.peak_rss_bytes, &[typed]),
            },
            None => TargetCheck {
                target: target.target.clone(),
                description: format!("peak RSS with types <= {limit_mib} MiB (not measured)"),
                status: CheckStatus::NotMeasured,
            },
        });
        if target.kind == "graph" && target.files.is_some_and(|files| files >= GRAPH_TARGET_FILES) {
            out.push(TargetCheck {
                target: target.target.clone(),
                description: format!(
                    "whole run with types < {} ms (measured {:.1} ms)",
                    budget.graph_ms, typed.mean_ms
                ),
                status: CheckStatus::of(typed.mean_ms < budget.graph_ms, &[typed]),
            });
        }
        if target.kind == "corpus"
            && let Some(knip) = target.results.iter().find(|row| row.tool == Tool::Knip.label())
        {
            out.push(knip_check(target, typed, knip, budget.knip_speedup));
        }
    }
    let corpus: Vec<(&TargetResult, &ToolResult, &ToolResult)> = targets
        .iter()
        .filter(|target| target.kind == "corpus")
        .filter_map(|target| mode_rows(target).map(|(typed, untyped)| (target, typed, untyped)))
        .collect();
    let ratio = |typed: &ToolResult, untyped: &ToolResult| typed.mean_ms / untyped.mean_ms;
    if let Some((largest, typed, untyped)) = corpus
        .iter()
        .copied()
        .max_by(|a, b| a.2.mean_ms.total_cmp(&b.2.mean_ms))
    {
        let measured = ratio(typed, untyped);
        out.push(TargetCheck {
            target: largest.target.clone(),
            description: format!(
                "types on <= {}x types off on the largest repository, the one whose untyped run takes longest (measured {measured:.1}x: {} ms against {} ms)",
                budget.largest_ratio,
                format_ms(typed.mean_ms),
                format_ms(untyped.mean_ms)
            ),
            status: CheckStatus::of(measured <= budget.largest_ratio, &[typed, untyped]),
        });
    }
    if let Some(median) = median_of(
        corpus
            .iter()
            .map(|&(_, typed, untyped)| ratio(typed, untyped))
            .collect(),
    ) {
        let rows: Vec<&ToolResult> = corpus
            .iter()
            .flat_map(|&(_, typed, untyped)| [typed, untyped])
            .collect();
        out.push(TargetCheck {
            target: "corpus".to_owned(),
            description: format!(
                "median types on / types off across {} repositories <= {}x (measured {median:.1}x)",
                corpus.len(),
                budget.median_ratio
            ),
            status: CheckStatus::of(median <= budget.median_ratio, &rows),
        });
    }
    out
}

/// The `/usr/bin/time` flag that makes it print the peak RSS of the command it runs: GNU time's `-v` on Linux, BSD
/// time's `-l` on macOS; `None` elsewhere, where peak RSS is not measured.
const fn peak_rss_flag() -> Option<&'static str> {
    if cfg!(target_os = "linux") {
        Some("-v")
    } else if cfg!(target_os = "macos") {
        Some("-l")
    } else {
        None
    }
}

/// The peak RSS in bytes that `/usr/bin/time` printed last in `stderr`: GNU time's `-v` line `Maximum resident set
/// size (kbytes): N`, in kibibytes, or BSD time's `-l` line `N  maximum resident set size`, in bytes. `None` when
/// `stderr` has neither.
fn parse_peak_rss(stderr: &str) -> Option<u64> {
    stderr.lines().rev().find_map(|line| {
        let line = line.trim();
        if let Some(kib) = line.strip_prefix("Maximum resident set size (kbytes):") {
            return kib.trim().parse::<u64>().ok()?.checked_mul(1024);
        }
        line.strip_suffix("maximum resident set size")?.trim().parse().ok()
    })
}

/// The `/usr/bin/time` command line that measures the peak RSS of `argv` with `flag` ([`peak_rss_flag`]).
fn peak_rss_argv(flag: &str, argv: &[String]) -> Vec<String> {
    [TIME_PROGRAM.to_owned(), flag.to_owned()]
        .into_iter()
        .chain(argv.iter().cloned())
        .collect()
}

/// Runs each skif mode of `target` ([`SKIF_MODES`]) once more, outside hyperfine, under `/usr/bin/time`, and records
/// its peak RSS in its row. Where the platform has no flag for it or `/usr/bin/time` is missing, or its output names
/// no peak RSS, the row keeps none and the log says why.
fn measure_peak_rss(
    target: &Target,
    argvs: &[(Tool, Vec<String>)],
    rows: &mut [ToolResult],
    scratch: &Path,
) -> Result<()> {
    let flag = match peak_rss_flag() {
        Some(flag) if Path::new(TIME_PROGRAM).is_file() => flag,
        _ => {
            eprintln!(
                "bench: peak RSS is not measured on {}: no {TIME_PROGRAM} that reports it here",
                target.name
            );
            return Ok(());
        }
    };
    for (tool, argv) in argvs.iter().filter(|(tool, _)| SKIF_MODES.contains(tool)) {
        let Some(row) = rows.iter_mut().find(|row| row.tool == tool.label()) else {
            continue;
        };
        let timed = peak_rss_argv(flag, argv);
        let Some((program, arguments)) = timed.split_first() else {
            continue;
        };
        let run = capture(
            Command::new(program).args(arguments).current_dir(&target.dir),
            scratch,
            &format!("rss-{}", tool.label().replace(' ', "_")),
            PEAK_RSS_TIMEOUT,
        )?;
        row.peak_rss_bytes = parse_peak_rss(&run.stderr);
        if row.peak_rss_bytes.is_none() {
            eprintln!(
                "bench: no peak RSS for {} on {}: {TIME_PROGRAM} {flag} printed none ({})",
                row.tool,
                target.name,
                run.stderr_summary()
            );
        }
    }
    Ok(())
}

/// `bytes` in mebibytes, rounded to the nearest one.
const fn mib(bytes: u64) -> u64 {
    bytes.saturating_add(1 << 19) >> 20
}

/// `count` and `noun`, plural unless the count is 1: `1 warmup run`, `5 measured runs`.
fn count_of(count: u32, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
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
    let harness = if summary.harness_rev == summary.source_rev {
        String::new()
    } else {
        format!(" (harness at `{}`)", summary.harness_rev)
    };
    let _ = write!(
        out,
        "{} at `{}`{harness}; hyperfine `{}`, {} warmup runs, {} measured runs. Times are wall clock, warm file cache.\n\n",
        summary.skif, summary.source_rev, summary.hyperfine, summary.warmup, summary.runs
    );
    if !summary.corpus_only.is_empty() {
        let _ = write!(
            out,
            "Corpus: only {}, a partial run.\n\n",
            summary.corpus_only.join(", ")
        );
    }
    let _ = write!(
        out,
        "Machine: {}; load average {} before and {} after the run.\n\n",
        summary.machine_text,
        format_load(summary.load_before),
        format_load(summary.load_after)
    );
    if let Some(github) = &summary.machine.github {
        let _ = write!(out, "{}\n\n", runner_text(github));
    }
    let toolchain = toolchain_text(&summary.toolchain);
    if !toolchain.is_empty() {
        let _ = write!(out, "Toolchain: {toolchain}.\n\n");
    }
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
            code_span(&skipped.reason)
        );
    }
    for target in &summary.targets {
        let _ = write!(out, "## {}\n\n", target.target);
        if let Some(installed_with) = &target.installed_with {
            let _ = write!(out, "Installed with {}.\n\n", code_span(installed_with));
        }
        if (target.warmup, target.runs) != (summary.warmup, summary.runs) {
            let _ = write!(
                out,
                "{} and {}, as corpus.toml caps them here.\n\n",
                count_of(target.warmup, "warmup run"),
                count_of(target.runs, "measured run")
            );
        }
        out.push_str("| Tool | Mean (ms) | Std dev (ms) | Median (ms) | Min (ms) | Max (ms) | vs skif |\n");
        out.push_str("| --- | ---: | ---: | ---: | ---: | ---: | ---: |\n");
        for r in &target.results {
            if let Some(failure) = r.failure.as_ref().filter(|failure| !failure.timed) {
                let _ = writeln!(
                    out,
                    "| {} | fails: {} | - | - | - | - | - |",
                    r.tool,
                    table_cell(&failure.reason)
                );
                continue;
            }
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
        if target.results.iter().any(|r| r.tool == RECURSIVE_TOOL) {
            let _ = write!(
                out,
                "`{RECURSIVE_TOOL}` also lints the nested roots that are not workspace members. No comparison tool reads them, so the row has no ratio and is part of no check.\n\n"
            );
        }
        for r in target.results.iter().filter(|r| r.error_runs > 0) {
            let cause = r
                .error_output
                .as_deref()
                .map_or_else(String::new, |output| format!(": {}", code_span(output)));
            let consequence = match &r.failure {
                Some(failure) if failure.timed => format!(
                    "as corpus.toml declares, after its report, so its time is compared ({})",
                    one_line(&failure.reason)
                ),
                Some(_) => "as corpus.toml declares, so its time is compared with nothing".to_owned(),
                None => "so its time may be that of a partial run".to_owned(),
            };
            let _ = write!(
                out,
                "{} exited with an error in {} of {} runs, {consequence}{cause}.\n\n",
                r.tool, r.error_runs, target.runs
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
                table_cell(&check.description),
                check.status.label()
            );
        }
        out.push('\n');
    }
    type_aware_markdown(summary, &mut out);
    // Every section above ends in a blank line; the file ends in exactly one newline, as `poly fmt` writes it.
    let end = out.trim_end().len();
    out.truncate(end);
    out.push('\n');
    out
}

/// The section of `summary.md` on the two skif modes: both columns of every target that timed both, and the checks of
/// [`TYPE_AWARE_BUDGET`]. Nothing when there are neither.
fn type_aware_markdown(summary: &Summary, out: &mut String) {
    let modes: Vec<(&TargetResult, &ToolResult, &ToolResult)> = summary
        .targets
        .iter()
        .filter_map(|target| mode_rows(target).map(|(typed, untyped)| (target, typed, untyped)))
        .collect();
    if modes.is_empty() && summary.type_aware_budget_checks.is_empty() {
        return;
    }
    out.push_str("## Type awareness (ADR 0029)\n\n");
    if !modes.is_empty() {
        out.push_str(
            "| Target | types on, mean (ms) | types off (default), mean (ms) | on / off | types on, peak RSS (MiB) | types off (default), peak RSS (MiB) |\n",
        );
        out.push_str("| --- | ---: | ---: | ---: | ---: | ---: |\n");
        let rss = |row: &ToolResult| {
            row.peak_rss_bytes
                .map_or_else(|| "-".to_owned(), |bytes| mib(bytes).to_string())
        };
        for (target, typed, untyped) in modes {
            let _ = writeln!(
                out,
                "| {} | {} | {} | {:.1}x | {} | {} |",
                target.target,
                format_ms(typed.mean_ms),
                format_ms(untyped.mean_ms),
                typed.mean_ms / untyped.mean_ms,
                rss(typed),
                rss(untyped)
            );
        }
        out.push('\n');
    }
    if !summary.type_aware_budget_checks.is_empty() {
        out.push_str("| Target | Budget check | Result |\n| --- | --- | --- |\n");
        for check in &summary.type_aware_budget_checks {
            let _ = writeln!(
                out,
                "| {} | {} | {} |",
                check.target,
                table_cell(&check.description),
                check.status.label()
            );
        }
        out.push('\n');
    }
}

/// `text` with every run of whitespace (tabs, newlines, double spaces) made one space: `poly fmt` collapses them in
/// Markdown prose, so the summary has to be written that way to pass its check.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `text` (on [`one_line`]) as the text of a Markdown table cell: a `|` would end the cell, so it is escaped.
pub fn table_cell(text: &str) -> String {
    one_line(text).replace('|', "\\|")
}

/// What the summary writes for an empty reason.
const EMPTY_TEXT: &str = "(empty)";

/// `text` (on [`one_line`]) as a Markdown code span, so `poly fmt` leaves it as written: command output such as an
/// install's error line holds `_` pairs, `*` and URLs, which the formatter rewrites in prose. The fence is one backtick
/// longer than the longest run of backticks inside. Empty text, which no code span can hold, is [`EMPTY_TEXT`].
fn code_span(text: &str) -> String {
    let text = one_line(text);
    if text.is_empty() {
        return EMPTY_TEXT.to_owned();
    }
    let longest = text
        .split(|character: char| character != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(longest + 1);
    let pad = if text.starts_with('`') || text.ends_with('`') {
        " "
    } else {
        ""
    };
    format!("{fence}{pad}{text}{pad}{fence}")
}

/// The machine in one line: operating system, kernel, architecture, CPU model, CPUs, memory, cgroup limit and
/// hypervisor; an unknown OS or CPU is named as such, any other unknown fact is left out.
fn machine_text(machine: &Machine) -> String {
    let or_unknown = |value: Option<&str>, what: &str| value.map_or_else(|| format!("unknown {what}"), one_line);
    let mut cpus = format!(
        "{} logical CPUs",
        machine.logical_cpus.unwrap_or(machine.available_cpus)
    );
    if let Some(cores) = machine.physical_cores {
        let _ = write!(cpus, " ({cores} physical cores)");
    }
    if machine
        .logical_cpus
        .is_some_and(|logical| machine.available_cpus < logical)
    {
        let _ = write!(cpus, ", {} available to the run", machine.available_cpus);
    }
    let mut parts = vec![or_unknown(machine.os.as_deref(), "OS")];
    parts.extend(machine.kernel.as_deref().map(one_line));
    parts.push(machine.arch.clone());
    parts.push(or_unknown(machine.cpu.as_deref(), "CPU"));
    parts.push(cpus);
    parts.extend(machine.memory_bytes.map(|bytes| format!("{} GiB memory", gib(bytes))));
    parts.extend(
        machine
            .cgroup_cpu_limit
            .map(|limit| format!("cgroup limit of {limit} CPUs")),
    );
    parts.extend(
        machine
            .virtualization
            .as_deref()
            .map(|hypervisor| format!("hypervisor {}", one_line(hypervisor))),
    );
    parts.join(", ")
}

/// `bytes` in gibibytes, rounded to one decimal.
fn gib(bytes: u64) -> String {
    const GIB: u64 = 1 << 30;
    let tenths = bytes.saturating_mul(10).saturating_add(GIB / 2) / GIB;
    format!("{}.{}", tenths / 10, tenths % 10)
}

/// The GitHub Actions runner in one line: its environment and name, image and platform, and a link to the run.
fn runner_text(github: &GithubRunner) -> String {
    let field = |value: Option<&str>| value.map_or_else(|| "unknown".to_owned(), code_span);
    let mut text = format!(
        "Runner: {} {}, image {} {}, {} {}",
        github
            .runner_environment
            .as_deref()
            .map_or_else(|| "unknown".to_owned(), one_line),
        field(github.runner_name.as_deref()),
        field(github.image_os.as_deref()),
        field(github.image_version.as_deref()),
        github
            .runner_os
            .as_deref()
            .map_or_else(|| "unknown".to_owned(), one_line),
        github.runner_arch.as_deref().map_or_else(String::new, one_line),
    );
    let end = text.trim_end().len();
    text.truncate(end);
    if let Some(url) = &github.run_url {
        let attempt = github
            .run_attempt
            .map_or_else(String::new, |attempt| format!(", attempt {attempt}"));
        let _ = write!(text, "; [workflow run{attempt}]({url})");
    }
    text.push('.');
    text
}

/// The toolchain versions that were found, each as its name and the version in a code span, comma separated; empty
/// when none was.
fn toolchain_text(toolchain: &Toolchain) -> String {
    [
        ("rustc", &toolchain.rustc),
        ("node", &toolchain.node),
        ("npm", &toolchain.npm),
        ("pnpm", &toolchain.pnpm),
        ("yarn", &toolchain.yarn),
        ("bun", &toolchain.bun),
    ]
    .iter()
    .filter_map(|(name, version)| Some(format!("{name} {}", code_span(version.as_deref()?))))
    .collect::<Vec<_>>()
    .join(", ")
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

/// Installs the comparison tools from the lock in `lock_dir` (their pinned versions and every dependency's) into
/// `prefix` once ([`tools_install::install`]), so that hyperfine times their executables directly (not through `npx`,
/// whose resolution would be timed too), and returns each installed tool with its executable. A tool that cannot be
/// installed is recorded in `skipped`.
fn install_tools(lock_dir: &Path, prefix: &Path, skipped: &mut Vec<Skipped>) -> Result<Vec<(Tool, PathBuf)>> {
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
    if let Err(reason) = tools_install::install(lock_dir, prefix)? {
        return Ok(skip_all(reason));
    }
    let mut installed = Vec::new();
    for tool in COMPARISON_TOOLS {
        let program = tools_install::program(prefix, tool.package().unwrap_or_default())?;
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

/// What installed a corpus repository: its `install-command` where it sets one (which names the package manager's
/// version), else its package manager with the `--version` that package manager reports in the checkout, where
/// corepack's shim resolves the repository's own `packageManager`.
fn install_label(repo: &corpus::Repo, version: Option<&str>) -> String {
    if repo.install_command.is_empty() {
        format!("{} {}", repo.package_manager, version.unwrap_or("(unknown version)"))
    } else {
        repo.install_command.join(" ")
    }
}

fn corpus_target_name(repo: &corpus::Repo) -> String {
    format!("corpus: {} ({})", repo.name, repo.package_manager)
}

/// The skif and knip arguments of a corpus repository's timed runs: knip's `bench-knip-args`, and with
/// `bench-knip-config = "parity"` the configurations of the knip comparison, written into `scratch`: knip's in place
/// of the repository's own, and skif's with the `entry` files when there are any. Neither gets `--recursive`: the
/// compared rows read the scope the comparison tools read (ADR 0037).
fn corpus_tool_args(repo: &corpus::Repo, scratch: &Path) -> Result<(Vec<String>, Vec<String>)> {
    let mut skif = Vec::new();
    let mut knip = repo.bench_knip_args.clone();
    if repo.bench_knip_config.as_deref() == Some(corpus::PARITY_KNIP_CONFIG) {
        let path = |config: PathBuf| config.to_string_lossy().into_owned();
        knip.extend([
            "--config".to_owned(),
            path(parity::write_knip_config(&repo.entry, scratch)?),
        ]);
        if let Some(config) = parity::write_skif_entry_config(&repo.entry, scratch)? {
            skif.extend(["--config".to_owned(), path(config)]);
        }
    }
    Ok((skif, knip))
}

/// The warmup and measured runs of a corpus repository: the run's `warmup` and `runs`, each capped by the repository's
/// `bench-max-warmup` and `bench-max-runs`.
fn corpus_runs(repo: &corpus::Repo, warmup: u32, runs: u32) -> (u32, u32) {
    (
        repo.bench_max_warmup.map_or(warmup, |most| warmup.min(most)),
        repo.bench_max_runs.map_or(runs, |most| runs.min(most)),
    )
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
        tools: SKIF_MODES.to_vec(),
        knip_args: Vec::new(),
        skif_args: Vec::new(),
        installed_with: None,
        knip_failure: None,
        warmup: options.warmup,
        runs: options.runs,
    })
}

/// Runs the whole benchmark and writes `summary.json` and `summary.md` into `options.out`.
///
/// # Errors
/// Fails when hyperfine or skif is missing, the corpus cannot be fetched, a measurement fails, (with `options.enforce`)
/// an ADR 0001 target check fails or (with `options.check_budget`) a type-aware budget check fails; the summary is
/// written before those last errors.
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

    let harness_rev = provenance::source_rev();
    let source_rev = options
        .skif_source
        .as_deref()
        .map_or_else(|| harness_rev.clone(), provenance::rev_of);
    // Checks the names of `--only`, and that the comparison tool lock agrees with `[tools]`, before anything is
    // installed.
    corpus::select(&options.corpus_file, "bench", &options.only)?;
    let mut skipped = Vec::new();
    let prefix = std::path::absolute(options.generated_dir.join("tools")).context("resolving the tools directory")?;
    let installed = install_tools(&corpus::tool_lock_dir(&options.corpus_file), &prefix, &mut skipped)?;
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
            installed_with: None,
            knip_failure: None,
            warmup: options.warmup,
            runs: options.runs,
        });
    }
    if options.graph_files > 0 {
        targets.push(graph_target(options)?);
    }
    // The corpus checkouts stay locked until every target is measured.
    let suite = if options.use_corpus {
        Some(corpus::fetch_suite(
            &options.corpus_file,
            "bench",
            &options.only,
            &options.corpus_dir,
        )?)
    } else {
        None
    };
    if let Some(suite) = &suite {
        for (repo, dir) in &suite.repos {
            let mut tools = SKIF_MODES.to_vec();
            tools.push(Tool::SkifManifest);
            if repo.recursive {
                tools.push(Tool::SkifRecursive);
            }
            tools.extend(comparison.iter().copied());
            let checkout = fs::canonicalize(dir)?;
            let mut installed_with = None;
            // knip loads the configurations of the repository's tools, which import its own packages: without an
            // install it fails on them and the time is that of a partial run.
            if options.install && tools.contains(&Tool::Knip) {
                let scratch = options.generated_dir.join("install").join(&repo.name);
                if let Some(reason) = parity::install(repo, &checkout, &scratch, INSTALL_TIMEOUT)? {
                    eprintln!("bench: {} not installed ({reason}), knip is not timed there", repo.name);
                    tools.retain(|tool| *tool != Tool::Knip);
                    skipped.push(Skipped {
                        tool: Tool::Knip.label().to_owned(),
                        target: Some(corpus_target_name(repo)),
                        reason: format!("the install failed: {reason}"),
                    });
                } else {
                    let version = provenance::version_in(&repo.package_manager, &checkout, &scratch);
                    installed_with = Some(install_label(repo, version.as_deref()));
                }
            }
            let configs = std::path::absolute(options.generated_dir.join("configs").join(&repo.name))
                .context("resolving the configuration directory")?;
            let (skif_args, knip_args) = corpus_tool_args(repo, &configs)?;
            let (warmup, runs) = corpus_runs(repo, options.warmup, options.runs);
            targets.push(Target {
                name: corpus_target_name(repo),
                kind: "corpus",
                packages: None,
                files: None,
                dir: checkout,
                tools,
                knip_args,
                skif_args,
                installed_with,
                knip_failure: repo.bench_knip_failure.clone(),
                warmup,
                runs,
            });
        }
    }

    let machine = provenance::machine();
    let toolchain = provenance::toolchain(
        &std::path::absolute(options.generated_dir.join("versions")).context("resolving the versions directory")?,
    );
    let load_before = provenance::load_average();
    let mut measured = Vec::new();
    for (index, target) in targets.iter().enumerate() {
        let skif_args: Vec<String> = options.skif_args.iter().chain(&target.skif_args).cloned().collect();
        let argvs: Vec<(Tool, Vec<String>)> = target
            .tools
            .iter()
            .map(|&tool| (tool, tool_argv(tool, &program_of(tool), &skif_args, &target.knip_args)))
            .collect();
        let commands: Vec<(Tool, String)> = argvs.iter().map(|(tool, argv)| (*tool, command_line(argv))).collect();
        let export = raw.join(format!("{index:02}.json"));
        hyperfine(target, &commands, &export)?;
        let mut results = tool_results(&read_results(&export)?);
        let scratch = std::path::absolute(options.generated_dir.join("errors").join(format!("{index:02}")))
            .context("resolving the error output directory")?;
        explain_errors(target, &argvs, &mut results, &scratch)?;
        measure_peak_rss(target, &argvs, &mut results, &scratch)?;
        measured.push(TargetResult {
            target: target.name.clone(),
            kind: target.kind,
            packages: target.packages,
            files: target.files,
            installed_with: target.installed_with.clone(),
            declared_knip_failure: target.knip_failure.clone(),
            warmup: target.warmup,
            runs: target.runs,
            results,
        });
    }

    let summary = Summary {
        schema: SUMMARY_SCHEMA,
        skif: skif_version(&skif),
        source_rev,
        harness_rev,
        measured_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_secs()),
        machine_text: machine_text(&machine),
        machine,
        toolchain,
        load_before,
        load_after: provenance::load_average(),
        hyperfine: version,
        tool_versions,
        warmup: options.warmup,
        runs: options.runs,
        corpus_only: options.only.clone(),
        skipped_tools: skipped,
        adr_0001_checks: checks(&measured),
        type_aware_budget_checks: budget_checks(&measured),
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
    for check in summary
        .adr_0001_checks
        .iter()
        .chain(&summary.type_aware_budget_checks)
        .filter(|check| matches!(check.status, CheckStatus::Invalid | CheckStatus::NotMeasured))
    {
        eprintln!(
            "bench: {} {}: {}",
            check.status.label(),
            check.target,
            check.description
        );
    }
    let errors: Vec<String> = [
        (options.enforce, &summary.adr_0001_checks, "ADR 0001 target"),
        (
            options.check_budget,
            &summary.type_aware_budget_checks,
            "type-aware budget (ADR 0029)",
        ),
    ]
    .into_iter()
    .filter(|(enforced, _, _)| *enforced)
    .filter_map(|(_, checks, what)| failed_checks(checks, what))
    .collect();
    if !errors.is_empty() {
        bail!("{}", errors.join("; "));
    }
    Ok(())
}

/// `<n> <what> check(s) failed` when any of `checks` failed, each printed to the log first; `None` when none did.
fn failed_checks(checks: &[TargetCheck], what: &str) -> Option<String> {
    let failed: Vec<&TargetCheck> = checks
        .iter()
        .filter(|check| check.status == CheckStatus::Fail)
        .collect();
    for check in &failed {
        eprintln!("bench: FAILED {}: {}", check.target, check.description);
    }
    (!failed.is_empty()).then(|| format!("{} {what} check(s) failed", failed.len()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// The hyperfine command line of `tool`.
    fn tool_command(tool: Tool, program: &Path, extra: &[String], knip_args: &[String]) -> String {
        command_line(&tool_argv(tool, program, extra, knip_args))
    }

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

    /// A summary of these targets and checks, measured on [`machine`] in 2 runs after 1 warmup run.
    fn summary_of(targets: Vec<TargetResult>, checks: Vec<TargetCheck>) -> Summary {
        Summary {
            schema: SUMMARY_SCHEMA,
            skif: "skif 0.0.0".into(),
            source_rev: "0123456789ab".into(),
            harness_rev: "ba9876543210".into(),
            measured_at: 1_794_268_800,
            machine: machine(),
            machine_text: machine_text(&machine()),
            toolchain: Toolchain {
                rustc: Some("rustc 1.99.0 (0123abcd 2026-10-01)".into()),
                node: Some("v26.13.0".into()),
                npm: Some("11.6.2".into()),
                pnpm: None,
                yarn: None,
                bun: None,
            },
            load_before: Some([1.5, 1.25, 1.0]),
            load_after: None,
            hyperfine: "hyperfine 1.20.0".into(),
            tool_versions: BTreeMap::from([("knip".to_owned(), "1.2.3".to_owned())]),
            warmup: 1,
            runs: 2,
            corpus_only: vec!["babel".into(), "vue-core".into()],
            skipped_tools: Vec::new(),
            targets,
            adr_0001_checks: checks,
            type_aware_budget_checks: Vec::new(),
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
            error_output: None,
            failure: None,
            peak_rss_bytes: None,
        };
        let summary = |checks: Vec<TargetCheck>, skipped: Vec<Skipped>| Summary {
            skipped_tools: skipped,
            ..summary_of(
                vec![TargetResult {
                    target: "t".into(),
                    kind: "generated",
                    packages: Some(1),
                    files: None,
                    installed_with: Some("pnpm 9.15.0".into()),
                    declared_knip_failure: None,
                    warmup: 1,
                    runs: 2,
                    results: vec![
                        row("skif --no-js"),
                        row("sherif"),
                        ToolResult {
                            error_runs: 2,
                            error_output: Some("ERROR: Error loading metro.config.js".into()),
                            ..row("syncpack lint")
                        },
                    ],
                }],
                checks,
            )
        };
        let check = TargetCheck {
            target: "t".into(),
            description: "d".into(),
            status: CheckStatus::Pass,
        };
        let skipped = Skipped {
            tool: "knip --reporter json".into(),
            target: Some("corpus: r (pnpm)".into()),
            reason: "the install failed: x".into(),
        };
        let with_skip = markdown(&summary(Vec::new(), vec![skipped]));
        assert!(
            with_skip.contains("Not timed: knip --reporter json on corpus: r (pnpm): `the install failed: x`."),
            "{with_skip}"
        );
        assert!(
            with_skip
                .contains("skif 0.0.0 at `0123456789ab` (harness at `ba9876543210`); hyperfine `hyperfine 1.20.0`"),
            "{with_skip}"
        );
        assert!(
            with_skip.contains(
                "Machine: Ubuntu 24.04.3 LTS, Linux 6.11.0-1018-azure, x86_64, AMD EPYC 7763 64-Core Processor, 4 logical CPUs (2 physical cores), 15.6 GiB memory, hypervisor Microsoft; load average 1.50 / 1.25 / 1.00 before and unknown after the run."
            ),
            "{with_skip}"
        );
        assert!(
            with_skip.contains(
                "Runner: github-hosted `GitHub Actions 1000012345`, image `ubuntu24` `20261102.1.0`, Linux X64; [workflow run, attempt 1](https://github.com/polylint/skif/actions/runs/19283746/attempts/1)."
            ),
            "{with_skip}"
        );
        assert!(
            with_skip.contains("Toolchain: rustc `rustc 1.99.0 (0123abcd 2026-10-01)`, node `v26.13.0`, npm `11.6.2`."),
            "{with_skip}"
        );
        assert!(with_skip.contains("Comparison tools: knip `1.2.3`."), "{with_skip}");
        assert!(
            with_skip.contains("## t\n\nInstalled with `pnpm 9.15.0`.\n\n| Tool |"),
            "{with_skip}"
        );
        assert!(
            with_skip.contains(
                "\n\nsyncpack lint exited with an error in 2 of 2 runs, so its time may be that of a partial run: `ERROR: Error loading metro.config.js`.\n"
            ),
            "{with_skip}"
        );
        assert!(
            with_skip.contains("\n\nCorpus: only babel, vue-core, a partial run.\n\n"),
            "{with_skip}"
        );
        let with_check = markdown(&summary(vec![check], Vec::new()));
        assert!(with_check.ends_with(" |\n"), "{with_check:?}");
        for text in [with_check, with_skip] {
            assert!(text.ends_with('\n'), "{text:?}");
            assert!(!text.ends_with("\n\n"));
            assert!(!text.contains("\n\n\n"));
            assert!(text.lines().all(|line| line == line.trim_end()));
        }
    }

    /// The machine of a GitHub-hosted x86 runner, as `provenance::machine` reads it there.
    fn machine() -> Machine {
        Machine {
            os: Some("Ubuntu 24.04.3 LTS".into()),
            kernel: Some("Linux 6.11.0-1018-azure".into()),
            arch: "x86_64".into(),
            cpu: Some("AMD EPYC 7763 64-Core Processor".into()),
            logical_cpus: Some(4),
            available_cpus: 4,
            physical_cores: Some(2),
            memory_bytes: Some(16_374_836 * 1024),
            cgroup_cpu_limit: None,
            virtualization: Some("Microsoft".into()),
            github: Some(GithubRunner {
                image_os: Some("ubuntu24".into()),
                image_version: Some("20261102.1.0".into()),
                runner_os: Some("Linux".into()),
                runner_arch: Some("X64".into()),
                runner_environment: Some("github-hosted".into()),
                runner_name: Some("GitHub Actions 1000012345".into()),
                run_attempt: Some(1),
                run_url: Some("https://github.com/polylint/skif/actions/runs/19283746/attempts/1".into()),
            }),
        }
    }

    #[test]
    fn the_machine_line_names_unknown_facts_and_leaves_out_missing_ones() {
        let bare = Machine {
            os: None,
            kernel: None,
            arch: "aarch64".into(),
            cpu: None,
            logical_cpus: None,
            available_cpus: 2,
            physical_cores: None,
            memory_bytes: None,
            cgroup_cpu_limit: None,
            virtualization: None,
            github: None,
        };
        assert_eq!(machine_text(&bare), "unknown OS, aarch64, unknown CPU, 2 logical CPUs");
        let limited = Machine {
            logical_cpus: Some(8),
            cgroup_cpu_limit: Some(1.5),
            ..bare
        };
        assert_eq!(
            machine_text(&limited),
            "unknown OS, aarch64, unknown CPU, 8 logical CPUs, 2 available to the run, cgroup limit of 1.5 CPUs"
        );
        assert_eq!(gib(16_374_836 * 1024), "15.6");
        assert_eq!(gib(1 << 30), "1.0");
        assert_eq!(gib(0), "0.0");
        let runner = GithubRunner {
            image_os: None,
            image_version: None,
            runner_os: None,
            runner_arch: None,
            runner_environment: None,
            runner_name: None,
            run_attempt: None,
            run_url: None,
        };
        assert_eq!(
            runner_text(&runner),
            "Runner: unknown unknown, image unknown unknown, unknown."
        );
        let none = Toolchain {
            rustc: None,
            node: None,
            npm: None,
            pnpm: None,
            yarn: None,
            bun: None,
        };
        assert_eq!(toolchain_text(&none), "");
    }

    /// An install's error line is written as `poly fmt` leaves it: in a code span, with whitespace runs collapsed.
    #[test]
    fn command_output_in_the_summary_is_written_as_the_formatter_leaves_it() {
        assert_eq!(
            code_span(
                "pnpm install failed: ERR_PNPM_OUTDATED_LOCKFILE  Cannot install with \"frozen-lockfile\" (see https://pnpm.io/x)"
            ),
            "`pnpm install failed: ERR_PNPM_OUTDATED_LOCKFILE Cannot install with \"frozen-lockfile\" (see https://pnpm.io/x)`"
        );
        assert_eq!(code_span("npm error\tcode  _not_ *x*\n"), "`npm error code _not_ *x*`");
        assert_eq!(code_span("run `npm ci` here"), "``run `npm ci` here``");
        assert_eq!(code_span("`tick`"), "`` `tick` ``");
        assert_eq!(code_span("a ``b`` c"), "```a ``b`` c```");
        assert_eq!(code_span(""), "(empty)");
        assert_eq!(code_span(" \t\n"), "(empty)");
        assert_eq!(
            one_line("AMD EPYC 7763 64-Core Processor   "),
            "AMD EPYC 7763 64-Core Processor"
        );
    }

    #[test]
    fn table_cells_escape_pipes_and_stay_on_one_line() {
        assert_eq!(
            table_cell("knip failed: a | b\nstderr:  c"),
            "knip failed: a \\| b stderr: c"
        );
        assert_eq!(table_cell("plain"), "plain");
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
        assert_eq!(
            line,
            "/bench/tools/node_modules/.bin/knip --reporter json --no-gitignore"
        );
        let target = |knip_errors: usize| {
            let mut knip = result("knip --reporter json", 1.0);
            knip.error_runs = knip_errors;
            TargetResult {
                target: "corpus: r (pnpm)".into(),
                kind: "corpus",
                packages: None,
                files: None,
                installed_with: None,
                declared_knip_failure: None,
                warmup: 1,
                runs: 2,
                results: tool_results(&[result("skif", 0.1), knip]),
            }
        };
        let clean = checks(&[target(0)]);
        assert_eq!(clean[0].status, CheckStatus::Pass);
        assert_eq!(
            clean[0].description,
            "JS analysis >= 5x faster than knip (measured 10.0x)"
        );
        let errored = checks(&[target(3)]);
        assert_eq!(
            errored[0].status,
            CheckStatus::Invalid,
            "a comparison whose knip errored is not measured, however fast skif was"
        );
        assert_eq!(
            errored[0].description,
            "JS analysis >= 5x faster than knip (measured 10.0x; knip --reporter json exited with an error in 3 runs)"
        );
        assert_eq!(CheckStatus::Invalid.label(), "invalid");
    }

    /// The knip failure create-t3-turbo declares, and the error line of a run that fails that way.
    fn declared() -> corpus::KnipFailure {
        corpus::KnipFailure {
            matches: "Cannot find module 'metro-cache'".into(),
            reason: "its Expo Metro config needs metro-cache".into(),
            timed: false,
        }
    }

    const METRO_ERROR: &str = "ERROR: Error loading apps/expo/metro.config.js (Cannot find module 'metro-cache')";

    fn metro_failure() -> ExpectedFailure {
        ExpectedFailure {
            reason: declared().reason,
            error: METRO_ERROR.to_owned(),
            timed: false,
        }
    }

    /// A corpus target timing skif at 0.1 s and knip at 1 s, whose knip row exited with an error in `knip_errors` runs,
    /// with the knip failure the repository declares and the one its knip run matched.
    fn corpus_target(
        knip_errors: usize,
        declared: Option<corpus::KnipFailure>,
        failure: Option<ExpectedFailure>,
    ) -> TargetResult {
        let mut knip = result("knip --reporter json", 1.0);
        knip.error_runs = knip_errors;
        let mut results = tool_results(&[result("skif", 0.1), knip]);
        results[1].failure = failure;
        TargetResult {
            target: "corpus: r (pnpm)".into(),
            kind: "corpus",
            packages: None,
            files: None,
            installed_with: None,
            declared_knip_failure: declared,
            warmup: 1,
            runs: 2,
            results,
        }
    }

    #[test]
    fn a_declared_failure_matches_the_stderr_or_the_error_line_of_the_rerun() {
        assert_eq!(
            matched_failure(&declared(), &format!("warn: slow\n{METRO_ERROR}\n"), "ERROR: other"),
            Some(ExpectedFailure {
                reason: declared().reason,
                error: "ERROR: other".to_owned(),
                timed: false,
            })
        );
        assert_eq!(matched_failure(&declared(), "", METRO_ERROR), Some(metro_failure()));
        let drizzle = "ERROR: Error loading packages/db/drizzle.config.ts";
        assert_eq!(matched_failure(&declared(), drizzle, drizzle), None);
    }

    #[test]
    fn a_matched_knip_failure_is_not_measured_and_a_stale_declaration_is_invalid() {
        let goal = "JS analysis >= 5x faster than knip";
        let declaration = "corpus.toml declares that knip fails here with `Cannot find module 'metro-cache'`";
        let mut skif_errored = corpus_target(15, Some(declared()), Some(metro_failure()));
        skif_errored.results[0].error_runs = 1;
        for (case, target, status, description) in [
            (
                "matched",
                corpus_target(15, Some(declared()), Some(metro_failure())),
                CheckStatus::NotMeasured,
                format!("{goal} (not measured: knip fails here: its Expo Metro config needs metro-cache)"),
            ),
            (
                "unmatched",
                corpus_target(15, Some(declared()), None),
                CheckStatus::Invalid,
                format!(
                    "{goal} (measured 10.0x; knip --reporter json exited with an error in 15 runs; {declaration}, but knip's error is another one: the declaration is stale)"
                ),
            ),
            (
                "declared but clean",
                corpus_target(0, Some(declared()), None),
                CheckStatus::Invalid,
                format!(
                    "{goal} (measured 10.0x; {declaration}, but knip ran without an error: the declaration is stale)"
                ),
            ),
            (
                "undeclared error",
                corpus_target(15, None, None),
                CheckStatus::Invalid,
                format!("{goal} (measured 10.0x; knip --reporter json exited with an error in 15 runs)"),
            ),
            (
                "skif errored beside a matched knip failure",
                skif_errored,
                CheckStatus::Invalid,
                format!(
                    "{goal} (measured 10.0x; skif exited with an error in 1 runs; knip --reporter json exited with an error in 15 runs)"
                ),
            ),
        ] {
            let found = checks(&[target]);
            assert_eq!(found.len(), 1, "{case}");
            assert_eq!(found[0].status, status, "{case}");
            assert_eq!(found[0].description, description, "{case}");
        }
        assert_eq!(
            serde_json::to_value(CheckStatus::NotMeasured).unwrap(),
            serde_json::json!("not-measured")
        );
        assert_eq!(
            serde_json::to_value(CheckStatus::Invalid).unwrap(),
            serde_json::json!("invalid")
        );
        assert_eq!(CheckStatus::NotMeasured.label(), "not measured");
    }

    #[test]
    fn a_declared_knip_failure_shows_as_failing_and_its_check_as_not_measured() {
        let mut target = corpus_target(2, Some(declared()), Some(metro_failure()));
        target.results[1].error_output = Some(METRO_ERROR.to_owned());
        target.results[1].relative_to_skif = None;
        let row = serde_json::to_value(&target).unwrap();
        assert_eq!(
            row["declared_knip_failure"],
            serde_json::json!({ "matches": "Cannot find module 'metro-cache'", "reason": "its Expo Metro config needs metro-cache", "timed": false })
        );
        assert_eq!(
            row["results"][1]["failure"],
            serde_json::json!({ "reason": "its Expo Metro config needs metro-cache", "error": METRO_ERROR, "timed": false })
        );
        assert_eq!(
            (row["warmup"].clone(), row["runs"].clone()),
            (serde_json::json!(1), serde_json::json!(2))
        );
        assert_eq!(row["results"][1]["relative_to_skif"], serde_json::Value::Null);
        assert_eq!(row["results"][0]["failure"], serde_json::Value::Null);
        let found = checks(std::slice::from_ref(&target));
        let text = markdown(&summary_of(vec![target], found));
        for expected in [
            "| knip --reporter json | fails: its Expo Metro config needs metro-cache | - | - | - | - | - |\n",
            "\n\nknip --reporter json exited with an error in 2 of 2 runs, as corpus.toml declares, so its time is compared with nothing: `ERROR: Error loading apps/expo/metro.config.js (Cannot find module 'metro-cache')`.\n",
            "| corpus: r (pnpm) | JS analysis >= 5x faster than knip (not measured: knip fails here: its Expo Metro config needs metro-cache) | not measured |\n",
        ] {
            assert!(text.contains(expected), "{expected}\n{text}");
        }
        assert!(
            text.contains("| skif | 100 | 1.0 | 100 | 100 | 100 | 1.0x |\n"),
            "{text}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_knip_run_that_fails_as_declared_is_marked_and_another_failure_is_not() {
        let dir = std::env::temp_dir().join(format!("skif-bench-expected-failure-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let argvs = vec![(
            Tool::Knip,
            vec![
                "sh".to_owned(),
                "-c".to_owned(),
                format!("echo \"{METRO_ERROR}\" >&2; exit 2"),
            ],
        )];
        let explained = |knip_failure: Option<corpus::KnipFailure>| {
            let target = Target {
                name: "corpus: r (pnpm)".into(),
                kind: "corpus",
                packages: None,
                files: None,
                dir: dir.clone(),
                tools: vec![Tool::Skif, Tool::Knip],
                knip_args: Vec::new(),
                skif_args: Vec::new(),
                installed_with: None,
                knip_failure,
                warmup: 1,
                runs: 2,
            };
            let mut knip = result("knip --reporter json", 1.0);
            knip.error_runs = 3;
            let mut rows = tool_results(&[result("skif", 0.1), knip]);
            explain_errors(&target, &argvs, &mut rows, &dir.join("scratch")).unwrap();
            rows.remove(1)
        };
        let matched = explained(Some(declared()));
        let other = explained(Some(corpus::KnipFailure {
            matches: "__DEV__ is not defined".into(),
            reason: "r".into(),
            timed: false,
        }));
        let undeclared = explained(None);
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(matched.failure, Some(metro_failure()));
        assert_eq!(matched.relative_to_skif, None);
        assert_eq!(matched.error_output.as_deref(), Some(METRO_ERROR));
        for row in [other, undeclared] {
            assert_eq!(row.failure, None);
            assert!(row.relative_to_skif.is_some_and(|ratio| (ratio - 10.0).abs() < 1e-9));
            assert_eq!(row.error_output.as_deref(), Some(METRO_ERROR));
        }
    }

    #[test]
    fn only_a_status_above_1_or_a_signal_is_an_error() {
        for (code, error) in [
            (Some(0), false),
            (Some(1), false),
            (Some(2), true),
            (Some(127), true),
            (None, true),
        ] {
            assert_eq!(is_error_exit(code), error, "{code:?}");
        }
    }

    #[test]
    fn a_tool_runs_outside_hyperfine_with_the_arguments_it_is_timed_with() {
        let extra = ["--recursive".to_owned()];
        let knip_args = ["--no-gitignore".to_owned()];
        let program = Path::new("/my tools/bin/x");
        for (tool, argv) in [
            (
                Tool::Skif,
                vec![
                    "/my tools/bin/x",
                    ".",
                    "--format",
                    "json",
                    "--no-install",
                    "--recursive",
                ],
            ),
            (
                Tool::SkifManifest,
                vec![
                    "/my tools/bin/x",
                    ".",
                    "--format",
                    "json",
                    "--no-install",
                    "--no-js",
                    "--recursive",
                ],
            ),
            (
                Tool::SkifTyped,
                vec![
                    "/my tools/bin/x",
                    ".",
                    "--format",
                    "json",
                    "--no-install",
                    "--type-aware",
                    "--recursive",
                ],
            ),
            (Tool::Sherif, vec!["/my tools/bin/x", "."]),
            (Tool::Syncpack, vec!["/my tools/bin/x", "lint"]),
            (
                Tool::Knip,
                vec!["/my tools/bin/x", "--reporter", "json", "--no-gitignore"],
            ),
        ] {
            assert_eq!(tool_argv(tool, program, &extra, &knip_args), argv, "{tool:?}");
            assert_eq!(
                tool_command(tool, program, &extra, &knip_args),
                std::iter::once("\"/my tools/bin/x\"")
                    .chain(argv[1..].iter().copied())
                    .collect::<Vec<_>>()
                    .join(" ")
            );
        }
    }

    #[test]
    fn the_comparison_tools_are_installed_from_their_lock_and_timed_by_path() {
        let bin = Path::new("/my tools/node_modules/.bin");
        assert_eq!(
            tool_command(Tool::Sherif, &bin.join("sherif"), &[], &[]),
            format!("{} .", quote(&bin.join("sherif").to_string_lossy()))
        );
        assert_eq!(
            tool_command(Tool::Syncpack, &bin.join("syncpack"), &[], &[]),
            format!("{} lint", quote(&bin.join("syncpack").to_string_lossy()))
        );
    }

    #[test]
    fn a_recursive_corpus_repository_is_compared_without_recursive_and_timed_recursively_beside_them() {
        let file = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/corpus.toml"));
        let (_, bench) = corpus::select(file, "bench", &[]).unwrap();
        let scratch = std::env::temp_dir().join(format!("skif-bench-recursive-{}", std::process::id()));
        for name in ["babel", "vscode", "next-js"] {
            let repo = bench.iter().find(|repo| repo.name == name).unwrap();
            assert!(repo.recursive, "{name}");
            let (args, _) = corpus_tool_args(repo, &scratch.join(name)).unwrap();
            assert!(!args.iter().any(|arg| arg == "--recursive"), "{name}: {args:?}");
        }
        fs::remove_dir_all(&scratch).ok();
        let program = Path::new("/s/skif");
        assert_eq!(
            tool_command(Tool::Skif, program, &[], &[]),
            "/s/skif . --format json --no-install"
        );
        assert_eq!(
            tool_command(Tool::SkifManifest, program, &[], &[]),
            "/s/skif . --format json --no-install --no-js"
        );
        assert_eq!(
            tool_command(Tool::SkifRecursive, program, &[], &[]),
            "/s/skif . --format json --no-install --recursive"
        );
        assert_eq!(Tool::SkifRecursive.label(), "skif --recursive");
        assert!(!SKIF_MODES.contains(&Tool::SkifRecursive));
        let vue = bench.iter().find(|repo| repo.name == "vue-core").unwrap();
        assert!(!vue.recursive);
    }

    #[test]
    fn the_recursive_row_has_no_ratio_and_no_check() {
        let rows = tool_results(&[
            result("skif", 0.1),
            result("skif --no-js", 0.02),
            result("skif --recursive", 5.0),
            result("sherif", 0.2),
            result("knip --reporter json", 1.0),
        ]);
        let by = |name: &str| rows.iter().find(|r| r.tool == name).unwrap().relative_to_skif;
        assert_eq!(by("skif --recursive"), None);
        assert!((by("sherif").unwrap() - 10.0).abs() < 1e-9);
        assert!((by("knip --reporter json").unwrap() - 10.0).abs() < 1e-9);
        let mut target = corpus_target(0, None, None);
        target.results = rows;
        let text = markdown(&summary_of(vec![target], Vec::new()));
        assert!(text.contains("| skif --recursive |"), "{text}");
        assert!(
            text.contains("`skif --recursive` also lints the nested roots"),
            "{text}"
        );
        assert!(text.contains("part of no check"), "{text}");
    }

    /// A bench repository `r` with these extra `corpus.toml` keys.
    fn bench_repo(keys: &str) -> corpus::Repo {
        toml::from_str(&format!(
            "name = \"r\"\nurl = \"u\"\nrev = \"0\"\npackage-manager = \"pnpm\"\nsuites = [\"bench\"]\n{keys}"
        ))
        .unwrap()
    }

    #[test]
    fn a_corpus_repository_caps_the_runs_of_its_commands_below_the_runs_asked_for() {
        let capped = bench_repo("bench-max-runs = 5\nbench-max-warmup = 1\n");
        assert_eq!(corpus_runs(&capped, 3, 15), (1, 5));
        assert_eq!(corpus_runs(&capped, 0, 2), (0, 2), "a lower --runs or --warmup is kept");
        assert_eq!(corpus_runs(&bench_repo(""), 3, 15), (3, 15));
        assert_eq!(corpus_runs(&bench_repo("bench-max-runs = 4\n"), 3, 15), (3, 4));
    }

    #[test]
    fn the_comparison_configuration_gives_knip_its_config_and_both_tools_the_entry_files() {
        let dir = std::env::temp_dir().join(format!("skif-bench-corpus-args-{}", std::process::id()));
        let scratch = dir.join("configs");
        let with_entry = bench_repo(
            "recursive = true\nentry = [\"src/main.ts\"]\nbench-knip-config = \"parity\"\nbench-knip-args = [\"--no-gitignore\"]\n",
        );
        let (skif, knip) = corpus_tool_args(&with_entry, &scratch).unwrap();
        let root = fs::canonicalize(&scratch).unwrap();
        let knip_config = root.join("knip-config.json");
        let skif_config = root.join("skif-entry.toml");
        assert_eq!(skif, ["--config", skif_config.to_str().unwrap()]);
        assert_eq!(knip, ["--no-gitignore", "--config", knip_config.to_str().unwrap()]);
        let knip_text = fs::read_to_string(&knip_config).unwrap();
        let skif_text = fs::read_to_string(&skif_config).unwrap();
        fs::remove_dir_all(&scratch).unwrap();
        let entry: serde_json::Value = serde_json::from_str(&knip_text).unwrap();
        assert!(
            entry["workspaces"]["."]["entry"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("src/main.ts")),
            "{knip_text}"
        );
        assert!(skif_text.starts_with("[js.workspaces.\".\"]\nentry = ["), "{skif_text}");
        assert!(skif_text.contains("\"src/main.ts\""), "{skif_text}");

        let (skif, knip) = corpus_tool_args(&bench_repo("bench-knip-config = \"parity\"\n"), &scratch).unwrap();
        assert_eq!(skif, [] as [String; 0], "no entry files, no skif configuration");
        assert_eq!(knip, ["--config", knip_config.to_str().unwrap()]);
        assert_eq!(fs::read_to_string(&knip_config).unwrap(), "{}\n");
        assert!(!skif_config.exists());
        fs::remove_dir_all(&scratch).unwrap();

        let (skif, knip) = corpus_tool_args(&bench_repo("bench-knip-args = [\"--no-gitignore\"]\n"), &scratch).unwrap();
        assert_eq!((skif, knip), (Vec::new(), vec!["--no-gitignore".to_owned()]));
        assert!(
            !scratch.exists(),
            "the repository's own knip configuration needs no file"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// The knip failure next-js declares, which knip prints its whole report before.
    fn timed_declared() -> corpus::KnipFailure {
        corpus::KnipFailure {
            matches: "Error loading jest.config.js".into(),
            reason: "its root Jest config requires the built next/jest".into(),
            timed: true,
        }
    }

    const JEST_ERROR: &str = "ERROR: Error loading jest.config.js (Cannot find module './dist/build/jest/jest')";

    fn jest_failure() -> ExpectedFailure {
        ExpectedFailure {
            reason: timed_declared().reason,
            error: JEST_ERROR.to_owned(),
            timed: true,
        }
    }

    #[test]
    fn a_timed_knip_failure_is_checked_like_a_clean_run_with_its_reason() {
        let goal = "JS analysis >= 5x faster than knip";
        let reason = "as corpus.toml declares, after its report: its root Jest config requires the built next/jest";
        let mut slow = corpus_target(5, Some(timed_declared()), Some(jest_failure()));
        slow.results[1].mean_ms = 300.0;
        let mut skif_errored = corpus_target(5, Some(timed_declared()), Some(jest_failure()));
        skif_errored.results[0].error_runs = 1;
        for (case, target, status, description) in [
            (
                "fast enough",
                corpus_target(5, Some(timed_declared()), Some(jest_failure())),
                CheckStatus::Pass,
                format!("{goal} (measured 10.0x; knip --reporter json exited with an error in 5 runs, {reason})"),
            ),
            (
                "too slow",
                slow,
                CheckStatus::Fail,
                format!("{goal} (measured 3.0x; knip --reporter json exited with an error in 5 runs, {reason})"),
            ),
            (
                "skif errored",
                skif_errored,
                CheckStatus::Invalid,
                format!(
                    "{goal} (measured 10.0x; skif exited with an error in 1 runs; knip --reporter json exited with an error in 5 runs, {reason})"
                ),
            ),
            (
                "unmatched",
                corpus_target(5, Some(timed_declared()), None),
                CheckStatus::Invalid,
                format!(
                    "{goal} (measured 10.0x; knip --reporter json exited with an error in 5 runs; corpus.toml declares that knip fails here with `Error loading jest.config.js`, but knip's error is another one: the declaration is stale)"
                ),
            ),
        ] {
            let found = checks(&[target]);
            assert_eq!(found[0].status, status, "{case}");
            assert_eq!(found[0].description, description, "{case}");
        }
    }

    #[test]
    fn a_timed_knip_failure_keeps_its_times_and_ratio_in_the_summary() {
        let mut target = corpus_target(2, Some(timed_declared()), Some(jest_failure()));
        target.results[1].error_output = Some(JEST_ERROR.to_owned());
        let found = checks(std::slice::from_ref(&target));
        let text = markdown(&summary_of(vec![target], found));
        for expected in [
            "| knip --reporter json | 1000 | 1.0 | 1000 | 1000 | 1000 | 10.0x |\n",
            "\n\nknip --reporter json exited with an error in 2 of 2 runs, as corpus.toml declares, after its report, so its time is compared (its root Jest config requires the built next/jest): `ERROR: Error loading jest.config.js (Cannot find module './dist/build/jest/jest')`.\n",
            "| corpus: r (pnpm) | JS analysis >= 5x faster than knip (measured 10.0x; knip --reporter json exited with an error in 2 runs, as corpus.toml declares, after its report: its root Jest config requires the built next/jest) | pass |\n",
        ] {
            assert!(text.contains(expected), "{expected}\n{text}");
        }
        assert!(!text.contains("fails:"), "{text}");
    }

    #[test]
    fn a_target_with_capped_runs_says_so_and_counts_its_errors_out_of_its_own_runs() {
        let mut capped = corpus_target(5, None, None);
        (capped.warmup, capped.runs) = (0, 5);
        let mut one = corpus_target(0, None, None);
        one.target = "corpus: t (pnpm)".into();
        (one.warmup, one.runs) = (1, 1);
        let uncapped = TargetResult {
            target: "corpus: s (pnpm)".into(),
            ..corpus_target(0, None, None)
        };
        let text = markdown(&summary_of(vec![capped, uncapped, one], Vec::new()));
        assert!(
            text.contains(
                "## corpus: r (pnpm)\n\n0 warmup runs and 5 measured runs, as corpus.toml caps them here.\n\n| Tool |"
            ),
            "{text}"
        );
        assert!(text.contains("## corpus: s (pnpm)\n\n| Tool |"), "{text}");
        assert!(
            text.contains("knip --reporter json exited with an error in 5 of 5 runs, so its time may be"),
            "{text}"
        );
        assert!(
            text.contains("## corpus: t (pnpm)\n\n1 warmup run and 1 measured run, as corpus.toml caps them here.\n\n"),
            "{text}"
        );
        assert_eq!(text.matches("as corpus.toml caps them").count(), 2, "{text}");
    }

    #[test]
    fn a_corpus_install_names_its_package_manager_version_or_its_command() {
        let file = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/corpus.toml"));
        let (_, bench) = corpus::select(file, "bench", &[]).unwrap();
        let repo = |name: &str| bench.iter().find(|repo| repo.name == name).unwrap();
        assert_eq!(install_label(repo("babel"), Some("4.10.3")), "yarn 4.10.3");
        assert_eq!(install_label(repo("vue-core"), None), "pnpm (unknown version)");
        assert_eq!(
            install_label(repo("npm-cli"), Some("10.9.0")),
            "npx --yes npm@11.6.2 ci --ignore-scripts"
        );
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
            installed_with: None,
            declared_knip_failure: None,
            warmup: 1,
            runs: 2,
            results: vec![ToolResult {
                tool: "skif --no-js".into(),
                mean_ms,
                stddev_ms: None,
                median_ms: mean_ms,
                min_ms: mean_ms,
                max_ms: mean_ms,
                relative_to_skif: None,
                error_runs: 0,
                error_output: None,
                failure: None,
                peak_rss_bytes: None,
            }],
        };
        assert_eq!(checks(&[target(10.0)])[0].status, CheckStatus::Pass);
        assert_eq!(checks(&[target(80.0)])[0].status, CheckStatus::Fail);
    }

    #[test]
    fn checks_flag_a_slow_graph_run_and_ignore_small_graphs() {
        let target = |files, mean_ms| TargetResult {
            target: "g".into(),
            kind: "graph",
            packages: Some(100),
            files: Some(files),
            installed_with: None,
            declared_knip_failure: None,
            warmup: 1,
            runs: 2,
            results: vec![ToolResult {
                tool: "skif".into(),
                mean_ms,
                stddev_ms: None,
                median_ms: mean_ms,
                min_ms: mean_ms,
                max_ms: mean_ms,
                relative_to_skif: None,
                error_runs: 0,
                error_output: None,
                failure: None,
                peak_rss_bytes: None,
            }],
        };
        assert_eq!(checks(&[target(10_000, 900.0)])[0].status, CheckStatus::Pass);
        assert_eq!(checks(&[target(10_000, 2500.0)])[0].status, CheckStatus::Fail);
        assert!(checks(&[target(500, 2500.0)]).is_empty());
    }

    /// A row of `tool` whose runs all took `mean_ms`.
    fn tool_row(tool: &str, mean_ms: f64) -> ToolResult {
        ToolResult {
            tool: tool.into(),
            mean_ms,
            stddev_ms: None,
            median_ms: mean_ms,
            min_ms: mean_ms,
            max_ms: mean_ms,
            relative_to_skif: None,
            error_runs: 0,
            error_output: None,
            failure: None,
            peak_rss_bytes: None,
        }
    }

    /// A target `name` of `kind` timing skif with `--type-aware` at `typed_ms` (peaking at `typed_mib` MiB, `None` when
    /// not measured), the default skif run without types at `untyped_ms` (peaking at 512 MiB) and, on a corpus target,
    /// knip at `knip_ms`.
    fn budget_target(
        name: &str,
        kind: &'static str,
        (typed_ms, typed_mib): (f64, Option<u64>),
        untyped_ms: f64,
        knip_ms: f64,
    ) -> TargetResult {
        let mut results = vec![
            ToolResult {
                peak_rss_bytes: typed_mib.map(|megs| megs << 20),
                ..tool_row("skif --type-aware", typed_ms)
            },
            ToolResult {
                peak_rss_bytes: Some(512 << 20),
                ..tool_row("skif", untyped_ms)
            },
        ];
        if kind == "corpus" {
            results.push(tool_row("knip --reporter json", knip_ms));
        }
        TargetResult {
            target: name.into(),
            kind,
            packages: None,
            files: (kind == "graph").then_some(GRAPH_TARGET_FILES),
            installed_with: None,
            declared_knip_failure: None,
            warmup: 1,
            runs: 2,
            results,
        }
    }

    /// Three corpus targets within the budget: types on at 1.5x types off everywhere, b the largest, knip at least
    /// 10x slower; and a graph target of 10,000 files at 4 s with types.
    fn within_budget() -> Vec<TargetResult> {
        vec![
            budget_target("a", "corpus", (300.0, Some(1024)), 200.0, 3000.0),
            budget_target("b", "corpus", (900.0, Some(3000)), 600.0, 30_000.0),
            budget_target("c", "corpus", (150.0, Some(4096)), 100.0, 3000.0),
            budget_target("g", "graph", (4000.0, Some(2048)), 3000.0, 0.0),
        ]
    }

    /// The target and description of every budget check of `targets` that did not pass, with its status.
    fn not_passed(targets: &[TargetResult]) -> Vec<(String, String, CheckStatus)> {
        budget_checks(targets)
            .into_iter()
            .filter(|check| check.status != CheckStatus::Pass)
            .map(|check| (check.target, check.description, check.status))
            .collect()
    }

    #[test]
    fn a_run_within_the_type_aware_budget_passes_every_check() {
        let checks = budget_checks(&within_budget());
        let described: Vec<(&str, &str)> = checks
            .iter()
            .map(|check| (check.target.as_str(), check.description.as_str()))
            .collect();
        assert_eq!(
            described,
            [
                ("a", "peak RSS with types <= 4096 MiB (measured 1024 MiB)"),
                ("a", "JS analysis >= 5x faster than knip (measured 10.0x)"),
                ("b", "peak RSS with types <= 4096 MiB (measured 3000 MiB)"),
                ("b", "JS analysis >= 5x faster than knip (measured 33.3x)"),
                ("c", "peak RSS with types <= 4096 MiB (measured 4096 MiB)"),
                ("c", "JS analysis >= 5x faster than knip (measured 20.0x)"),
                ("g", "peak RSS with types <= 4096 MiB (measured 2048 MiB)"),
                ("g", "whole run with types < 5000 ms (measured 4000.0 ms)"),
                (
                    "b",
                    "types on <= 3x types off on the largest repository, the one whose untyped run takes longest (measured 1.5x: 900 ms against 600 ms)"
                ),
                (
                    "corpus",
                    "median types on / types off across 3 repositories <= 2x (measured 1.5x)"
                ),
            ]
        );
        assert!(checks.iter().all(|check| check.status == CheckStatus::Pass));
        assert_eq!(failed_checks(&checks, "type-aware budget (ADR 0029)"), None);
    }

    #[test]
    fn each_broken_limit_of_the_type_aware_budget_fails_its_own_check() {
        let with = |index: usize, target: TargetResult| {
            let mut targets = within_budget();
            targets[index] = target;
            not_passed(&targets)
        };
        let fail =
            |target: &str, description: &str| vec![(target.to_owned(), description.to_owned(), CheckStatus::Fail)];
        assert_eq!(
            with(1, budget_target("b", "corpus", (2100.0, Some(3000)), 600.0, 30_000.0)),
            fail(
                "b",
                "types on <= 3x types off on the largest repository, the one whose untyped run takes longest (measured 3.5x: 2100 ms against 600 ms)"
            ),
            "the largest repository is over 3x; the median stays at 1.5x"
        );
        let mut slower = within_budget();
        slower[0] = budget_target("a", "corpus", (500.0, Some(1024)), 200.0, 3000.0);
        slower[2] = budget_target("c", "corpus", (250.0, Some(1024)), 100.0, 3000.0);
        assert_eq!(
            not_passed(&slower),
            fail(
                "corpus",
                "median types on / types off across 3 repositories <= 2x (measured 2.5x)"
            ),
            "the median is over 2x while the largest repository stays at 1.5x"
        );
        assert_eq!(
            with(2, budget_target("c", "corpus", (150.0, Some(4097)), 100.0, 3000.0)),
            fail("c", "peak RSS with types <= 4096 MiB (measured 4097 MiB)")
        );
        assert_eq!(
            with(0, budget_target("a", "corpus", (300.0, Some(1024)), 200.0, 1000.0)),
            fail("a", "JS analysis >= 5x faster than knip (measured 3.3x)")
        );
        assert_eq!(
            with(3, budget_target("g", "graph", (6000.0, Some(2048)), 3000.0, 0.0)),
            fail("g", "whole run with types < 5000 ms (measured 6000.0 ms)")
        );
        let mut broken = within_budget();
        broken[2] = budget_target("c", "corpus", (150.0, Some(4097)), 100.0, 3000.0);
        let checks = budget_checks(&broken);
        assert_eq!(
            failed_checks(&checks, "type-aware budget (ADR 0029)").as_deref(),
            Some("1 type-aware budget (ADR 0029) check(s) failed")
        );
    }

    #[test]
    fn an_unmeasured_peak_rss_is_not_measured_and_an_errored_mode_makes_its_checks_invalid() {
        let mut targets = within_budget();
        targets[0] = budget_target("a", "corpus", (300.0, None), 200.0, 3000.0);
        targets[1].results[1].error_runs = 2;
        assert_eq!(
            not_passed(&targets),
            [
                (
                    "a".to_owned(),
                    "peak RSS with types <= 4096 MiB (not measured)".to_owned(),
                    CheckStatus::NotMeasured
                ),
                (
                    "b".to_owned(),
                    "types on <= 3x types off on the largest repository, the one whose untyped run takes longest (measured 1.5x: 900 ms against 600 ms)".to_owned(),
                    CheckStatus::Invalid
                ),
                (
                    "corpus".to_owned(),
                    "median types on / types off across 3 repositories <= 2x (measured 1.5x)".to_owned(),
                    CheckStatus::Invalid
                ),
            ]
        );
        assert!(budget_checks(&[]).is_empty());
    }

    #[test]
    fn the_median_of_an_even_count_is_the_mean_of_the_middle_two() {
        assert_eq!(median_of(vec![3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median_of(vec![4.0, 1.0, 3.0, 2.0]), Some(2.5));
        assert_eq!(median_of(Vec::new()), None);
    }

    #[test]
    fn reads_the_peak_rss_that_gnu_and_bsd_time_print() {
        let gnu = "error: 3 unused exports\n\tCommand being timed: \"skif . --format json\"\n\tUser time (seconds): 10.52\n\tMaximum resident set size (kbytes): 3366912\n\tAverage resident set size (kbytes): 0\n\tExit status: 1\n";
        assert_eq!(parse_peak_rss(gnu), Some(3_366_912 * 1024));
        let bsd = "error: 3 unused exports\n        4.91 real        20.12 user         2.10 sys\n          3447717888  maximum resident set size\n                   0  average shared memory size\n          3290000000  peak memory footprint\n";
        assert_eq!(parse_peak_rss(bsd), Some(3_447_717_888));
        assert_eq!(parse_peak_rss("error: no time output\n"), None);
        assert_eq!(
            parse_peak_rss("\tMaximum resident set size (kbytes): unknown\n"),
            None,
            "a value that is not a number is no peak RSS"
        );
    }

    #[test]
    fn the_peak_rss_run_is_the_timed_command_under_time() {
        let argv = tool_argv(Tool::SkifTyped, Path::new("/s/skif"), &[], &[]);
        assert_eq!(
            peak_rss_argv("-v", &argv),
            [
                "/usr/bin/time",
                "-v",
                "/s/skif",
                ".",
                "--format",
                "json",
                "--no-install",
                "--type-aware"
            ]
        );
        assert_eq!(mib(3_447_717_888), 3288);
        assert_eq!(mib(4 << 30), 4096);
    }

    #[test]
    fn the_summary_shows_both_modes_and_the_budget_checks() {
        let targets = within_budget();
        let summary = Summary {
            type_aware_budget_checks: budget_checks(&targets),
            ..summary_of(targets, Vec::new())
        };
        let text = markdown(&summary);
        assert!(
            text.contains(
                "## Type awareness (ADR 0029)\n\n| Target | types on, mean (ms) | types off (default), mean (ms) | on / off | types on, peak RSS (MiB) | types off (default), peak RSS (MiB) |\n| --- | ---: | ---: | ---: | ---: | ---: |\n| a | 300 | 200 | 1.5x | 1024 | 512 |\n"
            ),
            "{text}"
        );
        assert!(text.contains("| g | 4000 | 3000 | 1.3x | 2048 | 512 |\n"), "{text}");
        assert!(
            text.contains(
                "| Target | Budget check | Result |\n| --- | --- | --- |\n| a | peak RSS with types <= 4096 MiB (measured 1024 MiB) | pass |\n"
            ),
            "{text}"
        );
        assert!(!text.contains("\n\n\n") && text.ends_with("|\n"), "{text}");
    }
}
