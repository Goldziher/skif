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
}

impl Entry {
    pub fn skipped(reason: impl Into<String>) -> Self {
        Self {
            status: SKIPPED.to_owned(),
            reason: Some(reason.into()),
            categories: BTreeMap::new(),
            cases: None,
        }
    }

    pub fn compared(categories: BTreeMap<String, Counts>) -> Self {
        Self {
            status: COMPARED.to_owned(),
            reason: None,
            categories,
            cases: None,
        }
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
    /// Whether dependencies were installed in the compared repositories; counts differ a lot between the two.
    pub installed: bool,
    pub entries: BTreeMap<String, Entry>,
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
    if baseline.tool != current.tool || baseline.installed != current.installed {
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
        if let Ok(old) = read_baseline(&gate.baseline)
            && old.tool == current.tool
            && old.installed == current.installed
        {
            merged.entries = old.entries;
            merged.entries.extend(current.entries.clone());
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
}
