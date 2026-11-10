//! Set comparison of findings, and the committed baselines the comparisons are checked against.
//!
//! Baselines hold counts only: the keys of a finding name files and symbols of the compared repositories, so they
//! stay in the full report under the output directory and never reach the repository.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Finding keys by category.
pub type Keys = BTreeMap<String, BTreeSet<String>>;

pub const BASELINE_SCHEMA: u32 = 1;
/// Sample keys kept per category and side in the full report.
pub const SAMPLE_LIMIT: usize = 5;

/// How the findings of skif and a comparison tool overlap in one category.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    pub skif_only: usize,
    pub tool_only: usize,
    pub both: usize,
}

/// A few keys from each side, for the full report only.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Samples {
    pub skif_only: Vec<String>,
    pub tool_only: Vec<String>,
}

/// The per-case summary of a fixture comparison.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaseSummary {
    pub compared: usize,
    pub identical: usize,
    pub skipped: usize,
    /// Cases whose findings differ, by name.
    pub differing: Vec<String>,
}

/// The comparison of one repository or fixture set.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    /// `compared` or `skipped`.
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default)]
    pub categories: BTreeMap<String, Counts>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cases: Option<CaseSummary>,
    /// One-sided findings that belong to a named, intended difference, by deviation id. They are not part of
    /// `categories` and the drift check ignores them.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub known_deviations: Known,
    /// Configuration files the comparison tool could not load while it still produced a report (knip's `Error loading`
    /// lines). A count, since the file names belong to the compared repository; the drift check ignores it.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub tool_errors: usize,
    /// Why the comparison tool's report is not a valid comparison here, when it is not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invalid: Option<String>,
    /// Whether this repository's dependencies were installed, read from the checkout rather than from a flag. `None`
    /// in entries that do not record it, which the [`Baseline`]'s own `installed` then stands for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installed: Option<bool>,
    /// Whether skif ran with `--recursive` here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recursive: Option<bool>,
    /// The skif source commit the entry was recorded at (`-dirty` with local changes); informative, never compared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skif_rev: Option<String>,
}

#[allow(clippy::trivially_copy_pass_by_ref)] // serde passes the field by reference.
const fn is_zero(count: &usize) -> bool {
    *count == 0
}

impl Entry {
    pub fn skipped(reason: impl Into<String>) -> Self {
        Self {
            status: SKIPPED.to_owned(),
            reason: Some(reason.into()),
            categories: BTreeMap::new(),
            cases: None,
            known_deviations: Known::new(),
            tool_errors: 0,
            invalid: None,
            installed: None,
            recursive: None,
            skif_rev: None,
        }
    }

    pub fn compared(categories: BTreeMap<String, Counts>) -> Self {
        Self {
            status: COMPARED.to_owned(),
            reason: None,
            categories,
            cases: None,
            known_deviations: Known::new(),
            tool_errors: 0,
            invalid: None,
            installed: None,
            recursive: None,
            skif_rev: None,
        }
    }

    #[must_use]
    pub fn with_known(mut self, known: Known) -> Self {
        self.known_deviations = known;
        self
    }

    #[must_use]
    pub const fn with_tool_errors(mut self, count: usize) -> Self {
        self.tool_errors = count;
        self
    }

    #[must_use]
    pub fn with_invalid(mut self, reason: Option<String>) -> Self {
        self.invalid = reason;
        self
    }

    #[must_use]
    pub const fn with_installed(mut self, installed: bool) -> Self {
        self.installed = Some(installed);
        self
    }

    #[must_use]
    pub fn with_run(mut self, recursive: bool, skif_rev: impl Into<String>) -> Self {
        self.recursive = Some(recursive);
        self.skif_rev = Some(skif_rev.into());
        self
    }
}

pub const COMPARED: &str = "compared";
pub const SKIPPED: &str = "skipped";

/// The committed form of a comparison run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Baseline {
    pub schema: u32,
    /// The comparison tool and its pinned version, e.g. `knip@6.44.1`.
    pub tool: String,
    /// Whether dependencies were installed in the compared repositories; counts differ a lot between the two. When the
    /// entries record their own `installed`, this is whether every one of them was installed.
    pub installed: bool,
    /// What the harness compares and sets apart; counts recorded under another set are not comparable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category_set: Option<CategorySet>,
    pub entries: BTreeMap<String, Entry>,
}

/// The categories a comparison counts and the known deviations it sets apart from them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CategorySet {
    pub compared: Vec<String>,
    pub known_deviations: Vec<String>,
}

impl CategorySet {
    /// What `self` has that `other` lacks, and the reverse, as `+name` / `-name` items.
    fn changes_from(&self, other: &Self) -> Vec<String> {
        let mine: BTreeSet<&String> = self.compared.iter().chain(&self.known_deviations).collect();
        let theirs: BTreeSet<&String> = other.compared.iter().chain(&other.known_deviations).collect();
        mine.difference(&theirs)
            .map(|name| format!("+{name}"))
            .chain(theirs.difference(&mine).map(|name| format!("-{name}")))
            .collect()
    }
}

/// Why `current` cannot be checked against or merged into `baseline` at all, if it cannot: another tool version, or
/// another set of categories and known deviations.
pub fn incomparable(baseline: &Baseline, current: &Baseline) -> Option<String> {
    if baseline.tool != current.tool {
        return Some(format!(
            "the baseline was recorded with {} and this run uses {}; counts of another version are not comparable, so re-record the baseline with --write-baseline",
            baseline.tool, current.tool
        ));
    }
    match (&baseline.category_set, &current.category_set) {
        (None, Some(_)) => Some(
            "the baseline records no category set (an older harness wrote it), so its counts are not comparable; re-record it with --write-baseline".to_owned(),
        ),
        (Some(recorded), Some(now)) if recorded != now => Some(format!(
            "the baseline was recorded with another set of categories and known deviations ({} in this run); re-record it with --write-baseline",
            now.changes_from(recorded).join(", ")
        )),
        _ => None,
    }
}

/// How far a count may move from its baseline: the larger of `absolute` and `percent` % of the baseline value.
#[derive(Debug, Clone, Copy)]
pub struct Tolerance {
    pub percent: usize,
    pub absolute: usize,
}

impl Tolerance {
    fn allowed(self, baseline: usize) -> usize {
        self.absolute.max((baseline * self.percent).div_ceil(100))
    }
}

/// A named class of intended difference between skif and a comparison tool.
#[derive(Debug, Clone, Copy)]
pub struct Deviation {
    pub id: &'static str,
    pub reason: &'static str,
}

/// Which tool a one-sided finding belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Skif,
    Tool,
}

/// How many one-sided findings a deviation explains.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnownCounts {
    pub skif_only: usize,
    pub tool_only: usize,
}

/// Known-deviation counts by deviation id.
pub type Known = BTreeMap<String, KnownCounts>;

/// The findings left after the known deviations are taken out, and what was taken out.
#[derive(Debug, Default)]
pub struct Split {
    pub skif: Keys,
    pub tool: Keys,
    pub known: Known,
    /// A few `<category>|<key>` per deviation id, for the full report only.
    pub samples: BTreeMap<String, Vec<String>>,
}

/// Takes the one-sided findings that `classify` assigns to a deviation (given the side, category and key) out of
/// both sets. Findings present on both sides are never classified.
pub fn split_known(skif: &Keys, tool: &Keys, classify: &dyn Fn(Side, &str, &str) -> Option<&'static str>) -> Split {
    let empty = BTreeSet::new();
    let mut split = Split::default();
    for category in skif.keys().chain(tool.keys()).collect::<BTreeSet<_>>() {
        let a = skif.get(category).unwrap_or(&empty);
        let b = tool.get(category).unwrap_or(&empty);
        for (side, own, other) in [(Side::Skif, a, b), (Side::Tool, b, a)] {
            for key in own {
                let target = if side == Side::Skif {
                    &mut split.skif
                } else {
                    &mut split.tool
                };
                match (!other.contains(key)).then(|| classify(side, category, key)).flatten() {
                    Some(id) => {
                        let counts = split.known.entry(id.to_owned()).or_default();
                        if side == Side::Skif {
                            counts.skif_only += 1;
                        } else {
                            counts.tool_only += 1;
                        }
                        let samples = split.samples.entry(id.to_owned()).or_default();
                        if samples.len() < SAMPLE_LIMIT {
                            samples.push(format!("{category}|{key}"));
                        }
                    }
                    None => {
                        target.entry(category.clone()).or_default().insert(key.clone());
                    }
                }
            }
        }
    }
    split
}

/// Total findings explained by known deviations.
pub fn known_total(known: &Known) -> usize {
    known.values().map(|c| c.skif_only + c.tool_only).sum()
}

/// A markdown section that lists, per target, how many findings each known deviation explains, followed by the
/// deviation reasons. Empty when no target has one.
pub fn known_section(tool: &str, deviations: &[Deviation], entries: &BTreeMap<String, Entry>) -> String {
    use std::fmt::Write as _;
    if entries.values().all(|e| e.known_deviations.is_empty()) {
        return String::new();
    }
    let mut out = format!(
        "\n## Known deviations\n\nIntended differences, reported apart from the counts above and ignored by `--check`. Each cell is skif only / {tool} only.\n\n| Target |"
    );
    for deviation in deviations {
        let _ = write!(out, " {} |", deviation.id);
    }
    out.push_str("\n| --- |");
    for _ in deviations {
        out.push_str(" ---: |");
    }
    out.push('\n');
    for (name, entry) in entries.iter().filter(|(_, e)| !e.known_deviations.is_empty()) {
        let _ = write!(out, "| {name} |");
        for deviation in deviations {
            let cell = entry
                .known_deviations
                .get(deviation.id)
                .map_or_else(|| "-".to_owned(), |c| format!("{} / {}", c.skif_only, c.tool_only));
            let _ = write!(out, " {cell} |");
        }
        out.push('\n');
    }
    out.push('\n');
    for deviation in deviations {
        let _ = writeln!(out, "* `{}`: {}", deviation.id, deviation.reason);
    }
    out
}

/// Splits the findings of skif and a tool into only-skif, only-tool and both, per category.
pub fn compare(skif: &Keys, tool: &Keys) -> (BTreeMap<String, Counts>, BTreeMap<String, Samples>) {
    let empty = BTreeSet::new();
    let mut counts = BTreeMap::new();
    let mut samples = BTreeMap::new();
    for category in skif.keys().chain(tool.keys()).collect::<BTreeSet<_>>() {
        let a = skif.get(category).unwrap_or(&empty);
        let b = tool.get(category).unwrap_or(&empty);
        let found = Counts {
            skif_only: a.difference(b).count(),
            tool_only: b.difference(a).count(),
            both: a.intersection(b).count(),
        };
        if found == Counts::default() {
            continue;
        }
        samples.insert(
            category.clone(),
            Samples {
                skif_only: a.difference(b).take(SAMPLE_LIMIT).cloned().collect(),
                tool_only: b.difference(a).take(SAMPLE_LIMIT).cloned().collect(),
            },
        );
        counts.insert(category.clone(), found);
    }
    (counts, samples)
}

/// Reads a baseline file.
///
/// # Errors
/// Fails when the file cannot be read or is not a baseline of the current schema.
pub fn read_baseline(path: &Path) -> Result<Baseline> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let baseline: Baseline = serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    if baseline.schema != BASELINE_SCHEMA {
        anyhow::bail!(
            "{}: schema {} is not {BASELINE_SCHEMA}",
            path.display(),
            baseline.schema
        );
    }
    Ok(baseline)
}

/// Writes a baseline file, creating its directory.
///
/// # Errors
/// Fails when the file cannot be written.
pub fn write_baseline(path: &Path, baseline: &Baseline) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    fs::write(path, serde_json::to_string_pretty(baseline)? + "\n")
        .with_context(|| format!("writing {}", path.display()))
}

/// Lists how `current` drifts from `baseline` beyond `tolerance`; empty when it does not. Only the entries of
/// `current` are examined, so a run narrowed to a few repositories is checked against their baselines alone.
pub fn check(baseline: &Baseline, current: &Baseline, tolerance: Tolerance) -> Vec<String> {
    if let Some(reason) = incomparable(baseline, current) {
        return vec![reason];
    }
    let per_entry = records_installed(current);
    if !per_entry && baseline.installed != current.installed {
        return vec![format!(
            "baseline was recorded with {} (installed: {}), this run used {} (installed: {}); refresh it with --write-baseline",
            baseline.tool, baseline.installed, current.tool, current.installed
        )];
    }
    let mut problems = Vec::new();
    for (name, now) in &current.entries {
        let Some(was) = baseline.entries.get(name) else {
            problems.push(format!("{name}: not in the baseline"));
            continue;
        };
        if let Some(installed) = now.installed {
            let recorded = was.installed.unwrap_or(baseline.installed);
            if recorded != installed {
                problems.push(format!(
                    "{name}: the baseline was recorded with installed: {recorded}, this checkout has installed: {installed}; install it (--install) or re-record it with --write-baseline"
                ));
                continue;
            }
        }
        if now.recursive.is_some() && was.recursive != now.recursive {
            problems.push(format!(
                "{name}: the baseline was recorded with recursive: {}, this run used recursive: {}; re-record it with --write-baseline",
                was.recursive.map_or_else(|| "unknown".to_owned(), |recorded| recorded.to_string()),
                now.recursive.unwrap_or_default()
            ));
            continue;
        }
        if was.status != now.status {
            problems.push(format!(
                "{name}: was {}, is now {}{}",
                was.status,
                now.status,
                now.reason.as_deref().map_or_else(String::new, |r| format!(" ({r})"))
            ));
            continue;
        }
        let zero = Counts::default();
        for category in was
            .categories
            .keys()
            .chain(now.categories.keys())
            .collect::<BTreeSet<_>>()
        {
            let before = was.categories.get(category).unwrap_or(&zero);
            let after = now.categories.get(category).unwrap_or(&zero);
            for (side, b, a) in [
                ("skif-only", before.skif_only, after.skif_only),
                ("tool-only", before.tool_only, after.tool_only),
                ("both", before.both, after.both),
            ] {
                if b.abs_diff(a) > tolerance.allowed(b) {
                    problems.push(format!("{name}: {category} {side} moved from {b} to {a}"));
                }
            }
        }
        if let (Some(was_cases), Some(now_cases)) = (&was.cases, &now.cases) {
            let known: BTreeSet<_> = was_cases.differing.iter().collect();
            for case in now_cases.differing.iter().filter(|case| !known.contains(case)) {
                problems.push(format!("{name}: case {case} now differs"));
            }
            if now_cases.skipped != was_cases.skipped {
                problems.push(format!(
                    "{name}: skipped cases moved from {} to {}",
                    was_cases.skipped, now_cases.skipped
                ));
            }
        }
    }
    problems
}

/// Whether the entries of a run record their own install state.
fn records_installed(baseline: &Baseline) -> bool {
    baseline.entries.values().any(|entry| entry.installed.is_some())
}

/// Where a comparison run records or checks its baseline.
#[derive(Debug, Clone)]
pub struct Gate {
    pub baseline: std::path::PathBuf,
    /// Record this run as the baseline (merging into an existing file recorded the same way).
    pub write: bool,
    /// Fail when this run drifts from the baseline.
    pub check: bool,
    pub tolerance: Tolerance,
}

/// Applies the gate to a finished run.
///
/// # Errors
/// Fails when the baseline cannot be read or written, or `check` found drift.
pub fn conclude(gate: &Gate, current: &Baseline) -> Result<()> {
    if gate.write {
        let mut merged = current.clone();
        let per_entry = records_installed(current);
        if let Ok(old) = read_baseline(&gate.baseline) {
            if let Some(reason) = incomparable(&old, current) {
                eprintln!("baseline: dropping its {} earlier entries: {reason}", old.entries.len());
            } else if per_entry || old.installed == current.installed {
                merged.entries = old.entries;
                merged.entries.extend(current.entries.clone());
            }
        }
        if per_entry {
            merged.installed = merged.entries.values().all(|entry| entry.installed != Some(false));
        }
        write_baseline(&gate.baseline, &merged)?;
        eprintln!("baseline: wrote {}", gate.baseline.display());
    }
    if gate.check {
        let problems = check(&read_baseline(&gate.baseline)?, current, gate.tolerance);
        if !problems.is_empty() {
            for problem in &problems {
                eprintln!("drift: {problem}");
            }
            anyhow::bail!("{} drift(s) from {}", problems.len(), gate.baseline.display());
        }
        eprintln!("baseline: no drift from {}", gate.baseline.display());
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn keys(items: &[(&str, &[&str])]) -> Keys {
        items
            .iter()
            .map(|(category, names)| ((*category).to_owned(), names.iter().map(|n| (*n).to_owned()).collect()))
            .collect()
    }

    fn baseline(entries: &[(&str, Entry)]) -> Baseline {
        Baseline {
            schema: BASELINE_SCHEMA,
            tool: "knip@1".to_owned(),
            installed: false,
            category_set: None,
            entries: entries.iter().map(|(n, e)| ((*n).to_owned(), e.clone())).collect(),
        }
    }

    fn counts(skif_only: usize, tool_only: usize, both: usize) -> BTreeMap<String, Counts> {
        BTreeMap::from([(
            "files".to_owned(),
            Counts {
                skif_only,
                tool_only,
                both,
            },
        )])
    }

    const TOLERANCE: Tolerance = Tolerance {
        percent: 10,
        absolute: 2,
    };

    #[test]
    fn compare_splits_each_category() {
        let skif = keys(&[("files", &["a", "b"]), ("exports", &["x"])]);
        let tool = keys(&[("files", &["b", "c"]), ("types", &[])]);
        let (found, samples) = compare(&skif, &tool);
        assert_eq!(
            found["files"],
            Counts {
                skif_only: 1,
                tool_only: 1,
                both: 1
            }
        );
        assert_eq!(
            found["exports"],
            Counts {
                skif_only: 1,
                tool_only: 0,
                both: 0
            }
        );
        assert!(!found.contains_key("types"));
        assert_eq!(samples["files"].skif_only, ["a"]);
        assert_eq!(samples["files"].tool_only, ["c"]);
    }

    #[test]
    fn check_accepts_small_drift_and_rejects_large() {
        let base = baseline(&[("r", Entry::compared(counts(0, 100, 1000)))]);
        let near = baseline(&[("r", Entry::compared(counts(2, 108, 1090)))]);
        assert_eq!(check(&base, &near, TOLERANCE), [] as [std::string::String; 0]);
        let far = baseline(&[("r", Entry::compared(counts(3, 100, 1000)))]);
        assert_eq!(check(&base, &far, TOLERANCE).len(), 1);
    }

    #[test]
    fn check_reports_a_repository_that_stopped_running() {
        let base = baseline(&[("r", Entry::compared(counts(1, 1, 1)))]);
        let now = baseline(&[("r", Entry::skipped("knip failed"))]);
        let problems = check(&base, &now, TOLERANCE);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("knip failed"));
    }

    #[test]
    fn check_rejects_a_new_entry_and_an_installed_mismatch() {
        let base = baseline(&[]);
        let now = baseline(&[("r", Entry::skipped("x"))]);
        assert_eq!(check(&base, &now, TOLERANCE), ["r: not in the baseline"]);
        let mut installed = now.clone();
        installed.installed = true;
        assert!(check(&base, &installed, TOLERANCE)[0].contains("--write-baseline"));
    }

    #[test]
    fn check_compares_the_install_state_of_each_entry_not_the_run_flag() {
        let mut base = baseline(&[
            ("a", Entry::compared(counts(1, 1, 1)).with_installed(true)),
            ("b", Entry::compared(counts(1, 1, 1)).with_installed(false)),
            ("old", Entry::compared(counts(1, 1, 1))),
        ]);
        base.installed = true;
        let mut now = baseline(&[
            ("a", Entry::compared(counts(1, 1, 1)).with_installed(true)),
            ("b", Entry::compared(counts(1, 1, 1)).with_installed(false)),
            ("old", Entry::compared(counts(1, 1, 1)).with_installed(true)),
        ]);
        now.installed = false;
        assert_eq!(
            check(&base, &now, TOLERANCE),
            [] as [String; 0],
            "the flag differs, the checkouts match"
        );
        let flipped = baseline(&[("a", Entry::compared(counts(1, 1, 1)).with_installed(false))]);
        let problems = check(&base, &flipped, TOLERANCE);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(
            problems[0].starts_with("a: the baseline was recorded with installed: true"),
            "{problems:?}"
        );
    }

    fn category_set(compared: &[&str], known: &[&str]) -> CategorySet {
        CategorySet {
            compared: compared.iter().map(|c| (*c).to_owned()).collect(),
            known_deviations: known.iter().map(|k| (*k).to_owned()).collect(),
        }
    }

    #[test]
    fn check_refuses_another_tool_version_or_category_set_instead_of_reporting_drift() {
        let mut base = baseline(&[("r", Entry::compared(counts(1, 1, 1)))]);
        let mut now = baseline(&[("r", Entry::compared(counts(900, 1, 1)))]);
        now.category_set = Some(category_set(&["files"], &["x", "new"]));
        let problems = check(&base, &now, TOLERANCE);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("records no category set"), "{problems:?}");

        base.category_set = Some(category_set(&["files"], &["x", "gone"]));
        let problems = check(&base, &now, TOLERANCE);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("(+new, -gone in this run)"), "{problems:?}");

        base.category_set.clone_from(&now.category_set);
        now.tool = "knip@2".to_owned();
        let problems = check(&base, &now, TOLERANCE);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(
            problems[0].starts_with("the baseline was recorded with knip@1 and this run uses knip@2"),
            "{problems:?}"
        );

        now.tool = "knip@1".to_owned();
        assert_eq!(check(&base, &now, TOLERANCE), ["r: files skif-only moved from 1 to 900"]);
    }

    #[test]
    fn check_refuses_an_entry_recorded_with_another_recursive_mode() {
        let base = baseline(&[("r", Entry::compared(counts(1, 1, 1)).with_run(false, "aaa"))]);
        let same = baseline(&[("r", Entry::compared(counts(1, 1, 1)).with_run(false, "bbb"))]);
        assert_eq!(check(&base, &same, TOLERANCE), [] as [String; 0], "the rev is not compared");
        let recursive = baseline(&[("r", Entry::compared(counts(1, 1, 1)).with_run(true, "bbb"))]);
        assert_eq!(
            check(&base, &recursive, TOLERANCE),
            ["r: the baseline was recorded with recursive: false, this run used recursive: true; re-record it with --write-baseline"]
        );
    }

    #[test]
    fn write_baseline_keeps_earlier_entries_only_when_comparable() {
        let dir = std::env::temp_dir().join(format!("skif-bench-baseline-merge-{}", std::process::id()));
        let file = dir.join("b.json");
        let gate = Gate {
            baseline: file.clone(),
            write: true,
            check: false,
            tolerance: TOLERANCE,
        };
        let set = Some(category_set(&["files"], &["x"]));
        let mut old = baseline(&[("old", Entry::compared(counts(1, 1, 1)).with_installed(true))]);
        old.category_set.clone_from(&set);
        write_baseline(&file, &old).unwrap();
        let mut now = baseline(&[("new", Entry::compared(counts(2, 2, 2)).with_installed(false))]);
        now.category_set.clone_from(&set);
        conclude(&gate, &now).unwrap();
        let merged = read_baseline(&file).unwrap();
        assert_eq!(merged.entries.keys().collect::<Vec<_>>(), ["new", "old"]);
        assert!(!merged.installed, "not every entry is installed");

        now.category_set = Some(category_set(&["files"], &["x", "y"]));
        conclude(&gate, &now).unwrap();
        assert_eq!(read_baseline(&file).unwrap().entries.keys().collect::<Vec<_>>(), ["new"]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn check_flags_newly_differing_cases() {
        let case = |differing: &[&str]| Entry {
            cases: Some(CaseSummary {
                compared: 3,
                identical: 3 - differing.len(),
                skipped: 0,
                differing: differing.iter().map(|d| (*d).to_owned()).collect(),
            }),
            ..Entry::compared(BTreeMap::new())
        };
        let base = baseline(&[("fixtures", case(&["a/one"]))]);
        let now = baseline(&[("fixtures", case(&["a/one", "b/two"]))]);
        assert_eq!(check(&base, &now, TOLERANCE), ["fixtures: case b/two now differs"]);
    }

    #[test]
    fn split_known_takes_only_classified_one_sided_findings() {
        let skif = keys(&[("a", &["both", "s-known", "s-real"])]);
        let tool = keys(&[("a", &["both", "t-known", "t-real"]), ("b", &["t-known"])]);
        let classify = |side: Side, category: &str, key: &str| {
            (key.ends_with("known") && category != "b").then_some(if side == Side::Skif { "x" } else { "y" })
        };
        let split = split_known(&skif, &tool, &classify);
        assert_eq!(split.skif, keys(&[("a", &["both", "s-real"])]));
        assert_eq!(split.tool, keys(&[("a", &["both", "t-real"]), ("b", &["t-known"])]));
        assert_eq!(
            split.known["x"],
            KnownCounts {
                skif_only: 1,
                tool_only: 0
            }
        );
        assert_eq!(
            split.known["y"],
            KnownCounts {
                skif_only: 0,
                tool_only: 1
            }
        );
        assert_eq!(known_total(&split.known), 2);
        assert_eq!(split.samples["x"], ["a|s-known"]);
        let (counts, _) = compare(&split.skif, &split.tool);
        assert_eq!(
            counts["a"],
            Counts {
                skif_only: 1,
                tool_only: 1,
                both: 1
            }
        );
    }

    #[test]
    fn check_ignores_known_deviations() {
        let known = |n| {
            BTreeMap::from([(
                "x".to_owned(),
                KnownCounts {
                    skif_only: 0,
                    tool_only: n,
                },
            )])
        };
        let base = baseline(&[("r", Entry::compared(counts(1, 1, 1)).with_known(known(5)))]);
        let now = baseline(&[("r", Entry::compared(counts(1, 1, 1)).with_known(known(500)))]);
        assert_eq!(check(&base, &now, TOLERANCE), [] as [String; 0]);
    }

    #[test]
    fn known_section_lists_reasons_only_when_something_is_known() {
        let deviation = [Deviation { id: "x", reason: "why" }];
        let plain = BTreeMap::from([("r".to_owned(), Entry::compared(BTreeMap::new()))]);
        assert_eq!(known_section("t", &deviation, &plain), "");
        let known = BTreeMap::from([(
            "x".to_owned(),
            KnownCounts {
                skif_only: 1,
                tool_only: 2,
            },
        )]);
        let with = BTreeMap::from([("r".to_owned(), Entry::compared(BTreeMap::new()).with_known(known))]);
        let section = known_section("t", &deviation, &with);
        assert!(section.contains("| r | 1 / 2 |"));
        assert!(section.contains("* `x`: why"));
    }
}
