//! The correctness gate of skif's cache (ADR 0014, "Cache rows and the cache-diff gate"): on every `bench` repository,
//! skif's JSON report from a run without the cache against runs with a cold store, a
//! warm store and, after each of K seeded source edits, the store the earlier runs left. A cache that changes a report
//! is a correctness defect, so any difference fails. So does a binary that shows no sign of using its cache: identical
//! reports from four uncached runs prove nothing.
//!
//! The runs also give a first, informational answer to what a warm cache saves: the wall time of single uncached,
//! cold, warm and one-edit runs and the size of the store. They are single runs, not a benchmark.

use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use skif_bench::rng::{Rng, hash_str};

use crate::cache_edit::{self, Edit, EditGuard};
use crate::corpus::{self, Repo};
use crate::proc::capture;
use crate::report_diff::{Diff, Report, diff_documents, read_report, render_diff};

/// The suite whose repositories are compared.
const SUITE: &str = "bench";
/// Top-level keys that are not output of the analysis: the version stamp of the binary, and the cache counters and
/// timing a cached run may report.
const IGNORED_SECTIONS: &[&str] = &["skif", "cache", "timing"];
/// The flag that turns a default-on cache off.
const NO_CACHE_FLAG: &str = "--no-cache";
/// The environment variables of the cache settings: on or off (`0`, `1`), the location and the size cap.
const ENV_ENABLED: &str = "SKIF_CACHE";
const ENV_LOCATION: &str = "SKIF_CACHE_DIR";
const ENV_MAX_SIZE: &str = "SKIF_CACHE_MAX_SIZE";
/// The section of a report that holds the cache counters, and the counter of entries read from the store.
const CACHE_SECTION: &str = "cache";
const HITS: &str = "hits";

/// The command line of `skif-bench cache-diff`.
pub struct Options {
    pub skif: PathBuf,
    pub corpus_file: PathBuf,
    pub corpus_dir: PathBuf,
    pub only: Vec<String>,
    pub edits: usize,
    pub seed: u64,
    pub control: Control,
    pub require_counters: bool,
    pub scratch: PathBuf,
    pub timeout: Duration,
}

/// What the gate turns the cache on and off with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Control {
    /// The command line flags the binary's `--help` offers.
    Flags,
    /// `SKIF_CACHE=0|1` and `SKIF_CACHE_DIR`, with no cache flag: only a default-on binary reads them.
    Env,
}

/// The cache settings of one run: what is added to the command line and what the environment holds. The three
/// variables are always set or removed, so nothing of the caller's environment reaches a run.
#[derive(Debug, Default, PartialEq, Eq)]
struct Settings {
    args: Vec<OsString>,
    /// Each variable with its value, `None` to remove it.
    env: Vec<(&'static str, Option<OsString>)>,
}

/// How the binary under test turns its cache on and off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheCli {
    /// The cache is off unless `--cache` is given (`--cache --cache-location DIR`).
    OptIn,
    /// The cache is on unless `--no-cache` is given (`--cache-location DIR`).
    DefaultOn,
}

impl CacheCli {
    /// Reads the mode from the binary's `--help`: a default-on CLI offers `--no-cache`.
    pub fn from_help(help: &str) -> Self {
        if mentions_flag(help, NO_CACHE_FLAG) {
            Self::DefaultOn
        } else {
            Self::OptIn
        }
    }

    fn uncached_args(self) -> Vec<OsString> {
        match self {
            Self::OptIn => Vec::new(),
            Self::DefaultOn => vec![NO_CACHE_FLAG.into()],
        }
    }

    fn cached_args(self, store: &Path) -> Vec<OsString> {
        let mut args: Vec<OsString> = match self {
            Self::OptIn => vec!["--cache".into()],
            Self::DefaultOn => Vec::new(),
        };
        args.extend(["--cache-location".into(), store.as_os_str().to_owned()]);
        args
    }

    /// The settings of a run without the cache (`store` is `None`) or with the store.
    fn settings(self, control: Control, store: Option<&Path>) -> Settings {
        match control {
            Control::Flags => Settings {
                args: store.map_or_else(|| self.uncached_args(), |store| self.cached_args(store)),
                env: vec![(ENV_ENABLED, None), (ENV_LOCATION, None), (ENV_MAX_SIZE, None)],
            },
            Control::Env => Settings {
                args: Vec::new(),
                env: vec![
                    (ENV_ENABLED, Some(if store.is_some() { "1" } else { "0" }.into())),
                    (ENV_LOCATION, store.map(|store| store.as_os_str().to_owned())),
                    (ENV_MAX_SIZE, None),
                ],
            },
        }
    }

    const fn describe(self, control: Control) -> &'static str {
        match (self, control) {
            (Self::OptIn, _) => "opt-in cache (uncached: no flag; cached: --cache --cache-location DIR)",
            (Self::DefaultOn, Control::Flags) => {
                "default-on cache (uncached: --no-cache; cached: --cache-location DIR)"
            }
            (Self::DefaultOn, Control::Env) => {
                "default-on cache, by environment (uncached: SKIF_CACHE=0; cached: SKIF_CACHE=1 SKIF_CACHE_DIR=DIR)"
            }
        }
    }
}

/// Whether `help` names `flag` as a whole word, so `--no-cache-location` is not `--no-cache`.
fn mentions_flag(help: &str, flag: &str) -> bool {
    let part_of_flag = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
    help.match_indices(flag).any(|(at, _)| {
        help[..at].chars().next_back().is_none_or(|c| !part_of_flag(c))
            && help[at + flag.len()..].chars().next().is_none_or(|c| !part_of_flag(c))
    })
}

/// Asks `skif --help` which cache CLI the binary has.
fn probe(skif: &Path) -> Result<CacheCli> {
    let output = Command::new(skif)
        .arg("--help")
        .output()
        .with_context(|| format!("running {} --help", skif.display()))?;
    if !output.status.success() {
        bail!(
            "{} --help failed: {}",
            skif.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let help = String::from_utf8_lossy(&output.stdout);
    if !mentions_flag(&help, "--cache-location") {
        bail!(
            "{} --help names no --cache-location: not a skif with a cache",
            skif.display()
        );
    }
    Ok(CacheCli::from_help(&help))
}

/// The cache hits a report counts: the sum of every `hits` number below its `cache` section, whatever the nesting
/// (the counters are per namespace); `None` when the report has no such section or the section counts no hits.
pub fn cache_hits(document: &Value) -> Option<u64> {
    fn sum(value: &Value, found: &mut Option<u64>) {
        match value {
            Value::Object(object) => {
                for (key, value) in object {
                    match value.as_u64() {
                        Some(hits) if key == HITS => *found = Some(found.unwrap_or(0) + hits),
                        _ => sum(value, found),
                    }
                }
            }
            Value::Array(items) => items.iter().for_each(|item| sum(item, found)),
            _ => {}
        }
    }
    let mut found = None;
    sum(document.get(CACHE_SECTION)?, &mut found);
    found
}

/// Why the run without the cache is not one: it counts cache hits.
fn uncached_used_the_cache(uncached: &Report) -> Option<String> {
    let hits = cache_hits(&uncached.as_ref().ok()?.0).filter(|hits| *hits > 0)?;
    Some(format!(
        "the run without the cache reports {hits} cache hit(s), so the cache was not off"
    ))
}

/// The evidence that the warm run read the cache: the note of its row, or why there is none. A report that counts
/// hits must count at least one. A report without counters is taken on the store the cold run wrote only from an
/// opt-in binary, which has no counters, and never with `require_counters`.
///
/// # Errors
/// Fails with the reason when nothing shows the warm run used a cache.
pub fn warm_evidence(cli: CacheCli, require_counters: bool, warm: &Value, store_bytes: u64) -> Result<String, String> {
    match cache_hits(warm) {
        Some(0) => Err("the warm run reports zero cache hits, so no cached result was compared".to_owned()),
        Some(hits) => Ok(format!("{hits} cache hit(s)")),
        None if store_bytes == 0 => {
            Err("the warm run reports no cache hits and the store is empty, so nothing shows a cache in use".to_owned())
        }
        None if require_counters || cli == CacheCli::DefaultOn => {
            Err("the warm run's report has no cache hit counter, so nothing shows it read the cache".to_owned())
        }
        None => Ok(format!(
            "no hit counter in this binary's report; the cold run wrote {store_bytes} byte(s)"
        )),
    }
}

/// The outcome of comparing a cached run with the uncached run of the same tree.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Identical,
    Different(Diff),
    /// A run produced no comparable report.
    Failed(String),
}

/// The report with its exit code as the `exit-code` section.
fn with_exit_code((document, code): &(Value, Option<i32>)) -> Value {
    let mut document = document.clone();
    if let Some(object) = document.as_object_mut() {
        object.insert("exit-code".to_owned(), json!(code));
    }
    document
}

/// The differences between two reports and exit codes, the [`IGNORED_SECTIONS`] left out.
fn diff_reports(a: &(Value, Option<i32>), b: &(Value, Option<i32>)) -> Diff {
    diff_documents(&with_exit_code(a), &with_exit_code(b), IGNORED_SECTIONS)
}

/// Compares a cached run's report with the uncached run's report of the same tree.
pub fn compare(uncached: &Report, cached: &Report) -> Outcome {
    match (uncached, cached) {
        (Ok(a), Ok(b)) => {
            let diff = diff_reports(a, b);
            if diff.is_empty() {
                Outcome::Identical
            } else {
                Outcome::Different(diff)
            }
        }
        (Err(reason), Ok(_)) => Outcome::Failed(format!("uncached run {reason}")),
        (Ok(_), Err(reason)) => Outcome::Failed(format!("cached run {reason}")),
        (Err(a), Err(b)) => Outcome::Failed(format!("uncached run {a}; cached run {b}")),
    }
}

/// One comparison: a cached run (`run`) against the uncached run of the tree `state`.
pub struct Row {
    pub repo: String,
    pub state: String,
    pub run: &'static str,
    pub outcome: Outcome,
    /// What the line adds after the status, such as whether an edit changed the uncached report.
    pub note: Option<String>,
}

/// The report text: one line per comparison, the differences under it, and a total.
pub fn render(rows: &[Row]) -> String {
    let mut out = String::new();
    for row in rows {
        let status = match &row.outcome {
            Outcome::Identical => "identical".to_owned(),
            Outcome::Different(_) => "DIFFERENT".to_owned(),
            Outcome::Failed(reason) => format!("FAILED: {reason}"),
        };
        let note = row.note.as_deref().map_or_default(|note| format!(" ({note})"));
        let _ = writeln!(out, "{:<20} {:<6} {status}{note}  [{}]", row.repo, row.run, row.state);
        if let Outcome::Different(diff) = &row.outcome {
            render_diff(&mut out, diff);
        }
    }
    let count = |keep: fn(&Outcome) -> bool| rows.iter().filter(|row| keep(&row.outcome)).count();
    let _ = writeln!(
        out,
        "{} comparison(s): {} identical, {} different, {} failed",
        rows.len(),
        count(|o| matches!(o, Outcome::Identical)),
        count(|o| matches!(o, Outcome::Different(_))),
        count(|o| matches!(o, Outcome::Failed(_))),
    );
    out
}

/// The verdict over all rows: an error when nothing was compared or any comparison is not identical.
///
/// # Errors
/// Fails on zero comparisons, and on any `DIFFERENT` or `FAILED` row.
pub fn verdict(rows: &[Row]) -> Result<()> {
    if rows.is_empty() {
        bail!("no comparison was made, so the cache was not checked");
    }
    let bad = rows
        .iter()
        .filter(|row| !matches!(row.outcome, Outcome::Identical))
        .count();
    if bad > 0 {
        bail!(
            "{bad} of {} comparison(s) differ between uncached and cached runs or have no report",
            rows.len()
        );
    }
    Ok(())
}

/// The informational single-run numbers of one repository.
#[derive(Debug, Default)]
struct Timing {
    repo: String,
    uncached: Option<Duration>,
    cold: Option<Duration>,
    warm: Option<Duration>,
    /// The cached run after the first applied edit.
    one_edit: Option<Duration>,
    /// The store after the cold run.
    store_bytes: Option<u64>,
}

fn render_timings(timings: &[Timing]) -> String {
    let ms = |time: Option<Duration>| time.map_or_else(|| "-".to_owned(), |t| format!("{} ms", t.as_millis()));
    let mut out =
        String::from("informational, single runs, not a benchmark (wall time per run; store after the cold run):\n");
    let _ = writeln!(
        out,
        "{:<20} {:>10} {:>10} {:>10} {:>10} {:>12}",
        "repo", "uncached", "cold", "warm", "one-edit", "store"
    );
    for timing in timings {
        let store = timing
            .store_bytes
            .map_or_else(|| "-".to_owned(), |bytes| format!("{} KiB", bytes.div_ceil(1024)));
        let _ = writeln!(
            out,
            "{:<20} {:>10} {:>10} {:>10} {:>10} {:>12}",
            timing.repo,
            ms(timing.uncached),
            ms(timing.cold),
            ms(timing.warm),
            ms(timing.one_edit),
            store
        );
    }
    out
}

/// The total size of the files below `dir`.
fn dir_size(dir: &Path) -> Result<u64> {
    let mut total = 0;
    for entry in fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        let meta = entry.metadata()?;
        total += if meta.is_dir() {
            dir_size(&entry.path())?
        } else {
            meta.len()
        };
    }
    Ok(total)
}

/// A finished skif run.
struct Run {
    report: Report,
    elapsed: Duration,
}

/// What every run of one repository shares.
struct Pass<'a> {
    skif: &'a Path,
    cli: CacheCli,
    control: Control,
    require_counters: bool,
    repo: &'a Repo,
    root: &'a Path,
    scratch: &'a Path,
    timeout: Duration,
}

impl Pass<'_> {
    /// Runs skif on the checkout, uncached or with `store`.
    fn run(&self, store: Option<&Path>, label: &str) -> Result<Run> {
        let mut command = Command::new(self.skif);
        command
            .args([".", "--format", "json", "--no-install"])
            .current_dir(self.root);
        if self.repo.recursive {
            command.arg("--recursive");
        }
        let settings = self.cli.settings(self.control, store);
        command.args(&settings.args);
        for (name, value) in &settings.env {
            match value {
                Some(value) => command.env(name, value),
                None => command.env_remove(name),
            };
        }
        let label = format!("{}-{label}", self.repo.name);
        let run = capture(&mut command, self.scratch, &label, self.timeout)?;
        Ok(Run {
            report: read_report(&run),
            elapsed: run.elapsed,
        })
    }

    fn row(&self, state: &str, run: &'static str, outcome: Outcome) -> Row {
        Row {
            repo: self.repo.name.clone(),
            state: state.to_owned(),
            run,
            outcome,
            note: None,
        }
    }

    /// The warm run's comparison: identical reports count only with evidence that the run read the cache.
    fn warm_row(&self, base: &Report, warm: &Report, store_bytes: u64) -> Row {
        let mut row = self.row("base", "warm", compare(base, warm));
        if let (Outcome::Identical, Ok((document, _))) = (&row.outcome, warm) {
            match warm_evidence(self.cli, self.require_counters, document, store_bytes) {
                Ok(evidence) => row.note = Some(evidence),
                Err(reason) => row.outcome = Outcome::Failed(reason),
            }
        }
        row
    }

    /// Uncached, cold, warm, then each planned edit (uncached and cached, restored after), then a cached run on the
    /// restored tree, pushing a row per comparison.
    fn compare_all(&self, plan: &[Result<Edit, String>], rows: &mut Vec<Row>) -> Result<Timing> {
        let store = self.scratch.join("store").join(&self.repo.name);
        if store.exists() {
            fs::remove_dir_all(&store).with_context(|| format!("emptying {}", store.display()))?;
        }
        fs::create_dir_all(&store).with_context(|| format!("creating {}", store.display()))?;
        let mut timing = Timing {
            repo: self.repo.name.clone(),
            ..Timing::default()
        };

        let uncached = self.run(None, "base-uncached")?;
        timing.uncached = Some(uncached.elapsed);
        let base = match uncached_used_the_cache(&uncached.report) {
            Some(reason) => Err(reason),
            None => uncached.report,
        };
        let cold = self.run(Some(&store), "base-cold")?;
        timing.cold = Some(cold.elapsed);
        let store_bytes = dir_size(&store)?;
        timing.store_bytes = Some(store_bytes);
        let cold_outcome = match compare(&base, &cold.report) {
            Outcome::Identical if store_bytes == 0 => {
                Outcome::Failed("the cold run wrote nothing to the store, so no cached run reads anything".to_owned())
            }
            outcome => outcome,
        };
        rows.push(self.row("base", "cold", cold_outcome));
        let warm = self.run(Some(&store), "base-warm")?;
        timing.warm = Some(warm.elapsed);
        rows.push(self.warm_row(&base, &warm.report, store_bytes));

        for (index, planned) in plan.iter().enumerate() {
            let number = index + 1;
            let edit = match planned {
                Ok(edit) => edit,
                Err(reason) => {
                    rows.push(self.row(&format!("edit {number}"), "cached", Outcome::Failed(reason.clone())));
                    continue;
                }
            };
            let state = format!("edit {number} {} {}", edit.kind.name(), edit.path);
            eprintln!("cache-diff: {} {state}", self.repo.name);
            let mut guard = EditGuard::apply(self.root, edit)?;
            let edited_uncached = self.run(None, &format!("edit{number}-uncached"))?;
            let edited_cached = self.run(Some(&store), &format!("edit{number}-cached"))?;
            guard.restore()?;
            if timing.one_edit.is_none() && edited_cached.report.is_ok() {
                timing.one_edit = Some(edited_cached.elapsed);
            }
            let note = match (&base, &edited_uncached.report) {
                (Ok(a), Ok(b)) if diff_reports(a, b).is_empty() => {
                    Some("the edit left the report unchanged".to_owned())
                }
                (Ok(_), Ok(_)) => Some("the edit changed the report".to_owned()),
                _ => None,
            };
            let mut row = self.row(
                &state,
                "cached",
                compare(&edited_uncached.report, &edited_cached.report),
            );
            row.note = note;
            rows.push(row);
        }

        let restored = self.run(Some(&store), "restored-cached")?;
        rows.push(self.row("restored", "cached", compare(&base, &restored.report)));
        Ok(timing)
    }
}

/// Runs the gate.
///
/// # Errors
/// Fails when a checkout cannot be fetched, skif cannot be started or has no cache, an edited file cannot be restored,
/// or any comparison differs, has no report, or none was made.
pub fn run(options: &Options) -> Result<()> {
    let skif = options
        .skif
        .canonicalize()
        .with_context(|| format!("{}: not a skif binary", options.skif.display()))?;
    let scratch = std::path::absolute(&options.scratch)?;
    let cli = probe(&skif)?;
    if options.control == Control::Env && cli == CacheCli::OptIn {
        bail!(
            "{} has an opt-in cache, which reads no {ENV_ENABLED}; --control env needs a binary whose --help offers \
             {NO_CACHE_FLAG}",
            skif.display()
        );
    }
    eprintln!(
        "cache-diff: {} has a(n) {}",
        skif.display(),
        cli.describe(options.control)
    );
    let suite = corpus::fetch_suite(&options.corpus_file, SUITE, &options.only, &options.corpus_dir)?;
    let mut rows = Vec::new();
    let mut timings = Vec::new();
    for (repo, root) in &suite.repos {
        let candidates = cache_edit::candidates(root)?;
        let mut rng = Rng::for_stream(options.seed, hash_str(&repo.name));
        let plan = cache_edit::plan(&mut rng, &candidates, options.edits, |path| {
            fs::read_to_string(root.join(path)).ok()
        });
        eprintln!("cache-diff: {}", repo.name);
        let pass = Pass {
            skif: &skif,
            cli,
            control: options.control,
            require_counters: options.require_counters,
            repo,
            root,
            scratch: &scratch,
            timeout: options.timeout,
        };
        timings.push(pass.compare_all(&plan, &mut rows)?);
    }
    println!("mode: {}", cli.describe(options.control));
    print!("{}", render(&rows));
    print!("{}", render_timings(&timings));
    verdict(&rows)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[allow(clippy::unnecessary_wraps, reason = "a run's report is a `Report`")]
    fn report(diagnostics: &[&str], code: i32) -> Report {
        let diagnostics: Vec<Value> = diagnostics
            .iter()
            .map(|file| json!({"rule": "unused-file", "message": "m", "location": {"path": file}}))
            .collect();
        Ok((
            json!({"version": 1, "skif": "0.8.0", "summary": {"warnings": diagnostics.len()}, "diagnostics": diagnostics}),
            Some(code),
        ))
    }

    #[test]
    fn identical_reports_are_identical_whatever_the_ignored_sections() {
        let mut cached = report(&["a.ts"], 1);
        let document = &mut cached.as_mut().unwrap().0;
        document["skif"] = json!("9.9.9");
        document["cache"] = json!({"hits": 10, "misses": 0});
        document["timing"] = json!({"total-ms": 3});
        assert_eq!(compare(&report(&["a.ts"], 1), &cached), Outcome::Identical);
    }

    #[test]
    fn a_changed_finding_or_exit_code_is_different_by_section() {
        let Outcome::Different(diff) = compare(&report(&["a.ts"], 1), &report(&["b.ts"], 1)) else {
            panic!("not different");
        };
        assert!(diff.sections.contains("diagnostics"));
        assert_eq!((diff.added.len(), diff.removed.len()), (1, 1));
        let Outcome::Different(diff) = compare(&report(&["a.ts"], 1), &report(&["a.ts"], 0)) else {
            panic!("not different");
        };
        assert_eq!(diff.sections.iter().collect::<Vec<_>>(), ["exit-code"]);
        let mut extra = report(&["a.ts"], 1);
        extra.as_mut().unwrap().0["incomplete-packages"] = json!(["x"]);
        assert!(matches!(compare(&report(&["a.ts"], 1), &extra), Outcome::Different(_)));
    }

    #[test]
    fn a_missing_report_fails_and_names_the_side() {
        let missing: Report = Err("timed out".to_owned());
        let Outcome::Failed(reason) = compare(&report(&[], 0), &missing) else {
            panic!("not failed");
        };
        assert_eq!(reason, "cached run timed out");
        let Outcome::Failed(reason) = compare(&missing, &report(&[], 0)) else {
            panic!("not failed");
        };
        assert_eq!(reason, "uncached run timed out");
    }

    fn row(outcome: Outcome) -> Row {
        Row {
            repo: "r".into(),
            state: "base".into(),
            run: "warm",
            outcome,
            note: None,
        }
    }

    #[test]
    fn the_verdict_fails_on_zero_work_a_difference_or_a_failure() {
        assert!(format!("{:#}", verdict(&[]).unwrap_err()).contains("no comparison"));
        assert!(verdict(&[row(Outcome::Identical), row(Outcome::Identical)]).is_ok());
        let different = compare(&report(&["a.ts"], 1), &report(&[], 1));
        assert!(verdict(&[row(Outcome::Identical), row(different)]).is_err());
        assert!(verdict(&[row(Outcome::Failed("timed out".into()))]).is_err());
    }

    #[test]
    fn the_report_lists_sections_and_totals() {
        let rows = [
            row(Outcome::Identical),
            row(compare(&report(&["a.ts"], 1), &report(&[], 1))),
            row(Outcome::Failed("cached run timed out".into())),
        ];
        let text = render(&rows);
        assert!(text.contains("DIFFERENT"), "{text}");
        assert!(text.contains("sections: diagnostics, summary"), "{text}");
        assert!(text.contains("- unused-file a.ts"), "{text}");
        assert!(text.contains("FAILED: cached run timed out"), "{text}");
        assert!(
            text.contains("3 comparison(s): 1 identical, 1 different, 1 failed"),
            "{text}"
        );
    }

    #[test]
    fn the_mode_probe_reads_no_cache_as_a_whole_flag() {
        let opt_in = "      --cache\n          Keep the parse results\n      --cache-location <DIR>\n";
        assert_eq!(CacheCli::from_help(opt_in), CacheCli::OptIn);
        let default_on = "      --no-cache\n          Do not read or write the cache\n      --cache-location <DIR>\n";
        assert_eq!(CacheCli::from_help(default_on), CacheCli::DefaultOn);
        assert_eq!(CacheCli::from_help("      --no-cache-location\n"), CacheCli::OptIn);
        assert_eq!(CacheCli::from_help("      --no-caches\n"), CacheCli::OptIn);
        assert_eq!(CacheCli::from_help("[--no-cache] ..."), CacheCli::DefaultOn);
    }

    #[test]
    fn hits_are_summed_over_the_namespaces_of_the_cache_section() {
        assert_eq!(cache_hits(&json!({"diagnostics": []})), None);
        assert_eq!(cache_hits(&json!({"cache": {"location": "/s"}})), None);
        assert_eq!(cache_hits(&json!({"cache": {"hits": 0, "misses": 4}})), Some(0));
        let per_namespace = json!({"cache": {
            "modules": {"hits": 7, "misses": 1, "bytes_written": 10},
            "manifests": {"hits": 2, "misses": 0, "bytes_written": 0},
        }});
        assert_eq!(cache_hits(&per_namespace), Some(9));
        let listed = json!({"cache": {"namespaces": [{"name": "modules", "hits": 3}]}});
        assert_eq!(cache_hits(&listed), Some(3));
        assert_eq!(
            cache_hits(&json!({"summary": {"hits": 5}})),
            None,
            "outside the section"
        );
    }

    #[test]
    fn a_warm_run_without_evidence_of_a_cache_fails() {
        let hits = |hits: u64| json!({"cache": {"modules": {"hits": hits, "misses": 3}}});
        let silent = json!({"diagnostics": []});
        for cli in [CacheCli::OptIn, CacheCli::DefaultOn] {
            assert_eq!(warm_evidence(cli, false, &hits(4), 100).unwrap(), "4 cache hit(s)");
            let zero = warm_evidence(cli, false, &hits(0), 100).unwrap_err();
            assert!(zero.contains("zero cache hits"), "{zero}");
            let empty = warm_evidence(cli, false, &silent, 0).unwrap_err();
            assert!(empty.contains("store is empty"), "{empty}");
        }
        let unreported = warm_evidence(CacheCli::DefaultOn, false, &silent, 100).unwrap_err();
        assert!(unreported.contains("no cache hit counter"), "{unreported}");
        assert!(warm_evidence(CacheCli::OptIn, true, &silent, 100).is_err());
        let written = warm_evidence(CacheCli::OptIn, false, &silent, 100).unwrap();
        assert!(written.contains("wrote 100 byte(s)"), "{written}");
    }

    #[test]
    fn an_uncached_run_that_counts_hits_is_not_uncached() {
        let mut uncached = report(&[], 0);
        assert_eq!(uncached_used_the_cache(&uncached), None);
        uncached.as_mut().unwrap().0["cache"] = json!({"hits": 0, "misses": 0});
        assert_eq!(uncached_used_the_cache(&uncached), None);
        uncached.as_mut().unwrap().0["cache"] = json!({"hits": 2});
        assert!(uncached_used_the_cache(&uncached).unwrap().contains("2 cache hit(s)"));
        assert_eq!(uncached_used_the_cache(&Err("timed out".to_owned())), None);
    }

    #[test]
    fn the_environment_control_passes_no_flag_and_no_inherited_setting() {
        let store = Path::new("/s");
        let value = |text: &str| Some(OsString::from(text));
        assert_eq!(
            CacheCli::DefaultOn.settings(Control::Env, Some(store)),
            Settings {
                args: Vec::new(),
                env: vec![
                    (ENV_ENABLED, value("1")),
                    (ENV_LOCATION, value("/s")),
                    (ENV_MAX_SIZE, None)
                ],
            }
        );
        assert_eq!(
            CacheCli::DefaultOn.settings(Control::Env, None),
            Settings {
                args: Vec::new(),
                env: vec![(ENV_ENABLED, value("0")), (ENV_LOCATION, None), (ENV_MAX_SIZE, None)],
            }
        );
        let flags = CacheCli::DefaultOn.settings(Control::Flags, None);
        assert_eq!(flags.args, [OsString::from("--no-cache")]);
        assert!(flags.env.iter().all(|(_, value)| value.is_none()), "{flags:?}");
        assert_eq!(flags.env.len(), 3);
    }

    #[test]
    fn each_mode_passes_its_own_flags() {
        let store = Path::new("/s");
        let strings = |args: Vec<OsString>| args.into_iter().map(|a| a.into_string().unwrap()).collect::<Vec<_>>();
        assert_eq!(strings(CacheCli::OptIn.uncached_args()), [] as [String; 0]);
        assert_eq!(
            strings(CacheCli::OptIn.cached_args(store)),
            ["--cache", "--cache-location", "/s"]
        );
        assert_eq!(strings(CacheCli::DefaultOn.uncached_args()), ["--no-cache"]);
        assert_eq!(
            strings(CacheCli::DefaultOn.cached_args(store)),
            ["--cache-location", "/s"]
        );
    }
}
