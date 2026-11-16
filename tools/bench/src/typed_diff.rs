//! The correctness gate of the type-aware redesign: skif's JSON output on every `bench` repository from a reference
//! binary against the same output from the binary under test, with `--type-aware` and without.
//!
//! The two binaries must agree exactly, except where an allow list names the difference (a change an ADR sanctions).
//! A difference is reported as the top-level sections that differ, the findings only one side has (by rule and file)
//! and the packages whose type-aware state changed.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::corpus::{self, Repo};
use crate::proc::capture;
use crate::type_aware::TYPE_AWARE_FLAG;

/// The suite whose repositories are compared.
const SUITE: &str = "bench";
/// Findings or package changes listed per comparison before the rest is summarised as a count.
const LIST_CAP: usize = 20;
/// Top-level keys that are not output of the analysis: the version stamp of the binary.
const IGNORED_SECTIONS: &[&str] = &["skif"];

/// The command line of `skif-bench typed-diff`.
pub struct Options {
    pub reference: PathBuf,
    pub skif: PathBuf,
    pub corpus_file: PathBuf,
    pub corpus_dir: PathBuf,
    pub only: Vec<String>,
    pub allow: Option<PathBuf>,
    pub out: Option<PathBuf>,
    pub scratch: PathBuf,
    pub timeout: Duration,
}

/// The two runs compared for each repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Typed,
    Untyped,
}

impl Mode {
    const ALL: [Self; 2] = [Self::Typed, Self::Untyped];

    const fn name(self) -> &'static str {
        match self {
            Self::Typed => "typed",
            Self::Untyped => "untyped",
        }
    }
}

/// A finding of one side's `diagnostics`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub rule: String,
    pub file: String,
    pub line: Option<u64>,
    pub message: String,
}

impl Finding {
    fn label(&self) -> String {
        let place = self
            .line
            .map_or_else(|| self.file.clone(), |line| format!("{}:{line}", self.file));
        format!("{} {place} {}", self.rule, self.message)
    }
}

/// A package whose entry of `type-aware.packages` differs; `None` when the package is absent on that side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageChange {
    pub dir: String,
    pub reference: Option<String>,
    pub skif: Option<String>,
}

impl PackageChange {
    fn label(&self) -> String {
        let side = |state: &Option<String>| state.clone().unwrap_or_else(|| "absent".to_owned());
        format!("{}: {} -> {}", self.dir, side(&self.reference), side(&self.skif))
    }
}

/// How two JSON documents differ.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Diff {
    /// The top-level keys whose values differ.
    pub sections: BTreeSet<String>,
    /// Findings the binary under test reports and the reference does not.
    pub added: Vec<Finding>,
    /// Findings the reference reports and the binary under test does not.
    pub removed: Vec<Finding>,
    pub packages: Vec<PackageChange>,
    /// The `type-aware` object differs only in its package list.
    packages_only: bool,
}

impl Diff {
    pub fn is_empty(&self) -> bool {
        self.sections.is_empty()
    }
}

fn describe_package(package: &Value) -> String {
    let field = |key: &str| package.get(key).and_then(Value::as_str);
    let mut text = field("state").unwrap_or("?").to_owned();
    if let Some(reason) = field("reason") {
        let _ = write!(text, " ({reason})");
    }
    if let Some(untyped) = package.get("untyped-files").and_then(Value::as_u64) {
        let _ = write!(text, " untyped-files={untyped}");
    }
    if let Some(name) = field("name") {
        let _ = write!(text, " [{name}]");
    }
    text
}

fn finding(diagnostic: &Value) -> Finding {
    let text = |pointer: &str| {
        diagnostic
            .pointer(pointer)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    Finding {
        rule: text("/rule"),
        file: text("/location/path"),
        line: diagnostic.pointer("/location/span/start-line").and_then(Value::as_u64),
        message: text("/message"),
    }
}

/// The diagnostics as a multiset keyed by their whole JSON, so a change to any field (severity, fixes, detail) is a
/// removed finding and an added one.
fn diagnostic_counts(document: &Value) -> BTreeMap<String, (usize, Finding)> {
    let mut counts = BTreeMap::new();
    for diagnostic in document
        .get("diagnostics")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        counts
            .entry(diagnostic.to_string())
            .or_insert_with(|| (0, finding(diagnostic)))
            .0 += 1;
    }
    counts
}

fn surplus(of: &BTreeMap<String, (usize, Finding)>, against: &BTreeMap<String, (usize, Finding)>) -> Vec<Finding> {
    let mut found = Vec::new();
    for (key, (count, finding)) in of {
        let other = against.get(key).map_or(0, |entry| entry.0);
        found.extend(std::iter::repeat_n(finding.clone(), count.saturating_sub(other)));
    }
    found.sort_by(|a, b| (&a.rule, &a.file, a.line, &a.message).cmp(&(&b.rule, &b.file, b.line, &b.message)));
    found
}

fn packages_by_dir(document: &Value) -> BTreeMap<String, &Value> {
    document
        .pointer("/type-aware/packages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|package| {
            let dir = package.get("dir").and_then(Value::as_str).unwrap_or_default();
            (dir.to_owned(), package)
        })
        .collect()
}

fn without_packages(type_aware: Option<&Value>) -> Option<Value> {
    let mut value = type_aware.cloned()?;
    if let Some(object) = value.as_object_mut() {
        object.remove("packages");
    }
    Some(value)
}

/// Compares the JSON reports of the reference and of the binary under test.
pub fn diff_documents(reference: &Value, skif: &Value) -> Diff {
    let mut diff = Diff::default();
    let keys: BTreeSet<&String> = reference
        .as_object()
        .into_iter()
        .chain(skif.as_object())
        .flat_map(serde_json::Map::keys)
        .filter(|key| !IGNORED_SECTIONS.contains(&key.as_str()))
        .collect();
    for key in keys {
        if reference.get(key) != skif.get(key) {
            diff.sections.insert(key.clone());
        }
    }
    let (before, after) = (diagnostic_counts(reference), diagnostic_counts(skif));
    diff.added = surplus(&after, &before);
    diff.removed = surplus(&before, &after);
    let (old, new) = (packages_by_dir(reference), packages_by_dir(skif));
    let dirs: BTreeSet<&String> = old.keys().chain(new.keys()).collect();
    for dir in dirs {
        let (a, b) = (old.get(dir), new.get(dir));
        if a != b {
            diff.packages.push(PackageChange {
                dir: dir.clone(),
                reference: a.map(|package| describe_package(package)),
                skif: b.map(|package| describe_package(package)),
            });
        }
    }
    diff.packages_only = !diff.packages.is_empty()
        && without_packages(reference.get("type-aware")) == without_packages(skif.get("type-aware"));
    diff
}

/// What an allow-list entry covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A finding only the binary under test reports.
    Added,
    /// A finding only the reference reports.
    Removed,
    /// A package whose type-aware state changed.
    Package,
    /// A whole top-level section.
    Section,
}

/// One expected difference. Fields left out match anything, except `kind` and `reason`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Allow {
    pub kind: Kind,
    /// Why the difference is expected: the ADR or change that sanctions it.
    pub reason: String,
    pub repo: Option<String>,
    pub mode: Option<Mode>,
    /// The rule id of an `added` or `removed` finding.
    pub rule: Option<String>,
    /// The file of an `added` or `removed` finding.
    pub file: Option<String>,
    /// The package directory of a `package` change.
    pub package: Option<String>,
    /// The top-level key of a `section`.
    pub section: Option<String>,
}

impl Allow {
    fn applies(&self, repo: &str, mode: Mode) -> bool {
        self.repo.as_deref().is_none_or(|r| r == repo) && self.mode.is_none_or(|m| m == mode)
    }

    fn covers_finding(&self, kind: Kind, finding: &Finding) -> bool {
        self.kind == kind
            && self.rule.as_deref().is_none_or(|r| r == finding.rule)
            && self.file.as_deref().is_none_or(|f| f == finding.file)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AllowFile {
    allow: Vec<Allow>,
}

/// Reads an allow list.
///
/// # Errors
/// Fails when the file cannot be read, is not the expected JSON, or an entry lacks the field its kind needs.
pub fn load_allow(path: &Path) -> Result<Vec<Allow>> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let file: AllowFile = serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    for entry in &file.allow {
        if entry.kind == Kind::Section && entry.section.is_none() {
            bail!("{}: a `section` entry needs `section`", path.display());
        }
        if entry.reason.trim().is_empty() {
            bail!("{}: every entry needs a `reason`", path.display());
        }
    }
    Ok(file.allow)
}

/// A comparison after the allow list is applied: what it leaves unexplained and how many differences it covered.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Assessment {
    pub residual: Diff,
    pub allowed: usize,
}

/// Removes from `diff` what `allow` covers for this repository and mode. `diagnostics` and `summary` count as covered
/// when every finding difference is; `type-aware` when every package change is and nothing else of it differs.
pub fn apply_allow(diff: &Diff, allow: &[Allow], repo: &str, mode: Mode) -> Assessment {
    let applicable: Vec<&Allow> = allow.iter().filter(|entry| entry.applies(repo, mode)).collect();
    let mut residual = Diff {
        sections: diff.sections.clone(),
        ..Diff::default()
    };
    let mut allowed = 0;
    for (kind, findings, kept) in [
        (Kind::Added, &diff.added, &mut residual.added),
        (Kind::Removed, &diff.removed, &mut residual.removed),
    ] {
        for finding in findings {
            if applicable.iter().any(|entry| entry.covers_finding(kind, finding)) {
                allowed += 1;
            } else {
                kept.push(finding.clone());
            }
        }
    }
    for change in &diff.packages {
        let covered = applicable
            .iter()
            .any(|entry| entry.kind == Kind::Package && entry.package.as_deref().is_none_or(|dir| dir == change.dir));
        if covered {
            allowed += 1;
        } else {
            residual.packages.push(change.clone());
        }
    }
    for entry in applicable.iter().filter(|entry| entry.kind == Kind::Section) {
        if let Some(section) = &entry.section
            && residual.sections.remove(section)
        {
            allowed += 1;
        }
    }
    let findings_covered = (!diff.added.is_empty() || !diff.removed.is_empty())
        && residual.added.is_empty()
        && residual.removed.is_empty();
    if findings_covered {
        residual.sections.remove("diagnostics");
        residual.sections.remove("summary");
    }
    if diff.packages_only && residual.packages.is_empty() {
        residual.sections.remove("type-aware");
    }
    Assessment { residual, allowed }
}

/// The outcome of comparing one repository in one mode.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Identical,
    /// Every difference is on the allow list.
    Allowed(Assessment),
    Different(Assessment),
    /// A run produced no comparable report.
    Failed(String),
}

impl Outcome {
    const fn passes(&self) -> bool {
        matches!(self, Self::Identical | Self::Allowed(_))
    }
}

/// Classifies a comparison of two reports.
pub fn compare(reference: &Value, skif: &Value, allow: &[Allow], repo: &str, mode: Mode) -> Outcome {
    let diff = diff_documents(reference, skif);
    if diff.is_empty() {
        return Outcome::Identical;
    }
    let assessment = apply_allow(&diff, allow, repo, mode);
    if assessment.residual.is_empty() {
        Outcome::Allowed(assessment)
    } else {
        Outcome::Different(assessment)
    }
}

fn list<T>(out: &mut String, prefix: &str, items: &[T], label: impl Fn(&T) -> String) {
    for item in items.iter().take(LIST_CAP) {
        let _ = writeln!(out, "    {prefix} {}", label(item));
    }
    if items.len() > LIST_CAP {
        let _ = writeln!(out, "    {prefix} ... and {} more", items.len() - LIST_CAP);
    }
}

fn render_diff(out: &mut String, diff: &Diff) {
    let sections: Vec<&str> = diff.sections.iter().map(String::as_str).collect();
    if !sections.is_empty() {
        let _ = writeln!(out, "    sections: {}", sections.join(", "));
    }
    list(out, "+", &diff.added, Finding::label);
    list(out, "-", &diff.removed, Finding::label);
    list(out, "~", &diff.packages, PackageChange::label);
}

fn diff_json(diff: &Diff) -> Value {
    let findings = |list: &[Finding]| -> Vec<Value> {
        list.iter()
            .map(|f| json!({"rule": f.rule, "file": f.file, "line": f.line, "message": f.message}))
            .collect()
    };
    json!({
        "sections": diff.sections,
        "added": findings(&diff.added),
        "removed": findings(&diff.removed),
        "packages": diff.packages.iter().map(|p| json!({
            "dir": p.dir, "reference": p.reference, "skif": p.skif,
        })).collect::<Vec<_>>(),
    })
}

/// One repository and mode with its outcome.
pub struct Row {
    pub repo: String,
    pub mode: Mode,
    pub outcome: Outcome,
}

/// The report text: one line per comparison, the differences under it, and a total.
pub fn render(rows: &[Row]) -> String {
    let mut out = String::new();
    for row in rows {
        let status = match &row.outcome {
            Outcome::Identical => "identical".to_owned(),
            Outcome::Allowed(assessment) => format!("identical but for {} allowed difference(s)", assessment.allowed),
            Outcome::Different(_) => "DIFFERENT".to_owned(),
            Outcome::Failed(reason) => format!("FAILED: {reason}"),
        };
        let _ = writeln!(out, "{:<20} {:<8} {status}", row.repo, row.mode.name());
        match &row.outcome {
            Outcome::Different(assessment) => {
                render_diff(&mut out, &assessment.residual);
                if assessment.allowed > 0 {
                    let _ = writeln!(out, "    ({} more difference(s) allowed)", assessment.allowed);
                }
            }
            Outcome::Identical | Outcome::Allowed(_) | Outcome::Failed(_) => {}
        }
    }
    let count = |keep: fn(&Outcome) -> bool| rows.iter().filter(|row| keep(&row.outcome)).count();
    let _ = writeln!(
        out,
        "{} comparison(s): {} identical, {} allowed, {} different, {} failed",
        rows.len(),
        count(|o| matches!(o, Outcome::Identical)),
        count(|o| matches!(o, Outcome::Allowed(_))),
        count(|o| matches!(o, Outcome::Different(_))),
        count(|o| matches!(o, Outcome::Failed(_))),
    );
    out
}

fn report_json(rows: &[Row]) -> Value {
    let rows: Vec<Value> = rows
        .iter()
        .map(|row| {
            let (status, detail) = match &row.outcome {
                Outcome::Identical => ("identical", Value::Null),
                Outcome::Allowed(a) => ("allowed", diff_json(&a.residual)),
                Outcome::Different(a) => ("different", diff_json(&a.residual)),
                Outcome::Failed(reason) => ("failed", json!(reason)),
            };
            json!({"repo": row.repo, "mode": row.mode.name(), "status": status, "detail": detail})
        })
        .collect();
    json!({"rows": rows})
}

/// Runs `binary` on a checkout and reads its JSON report and exit code.
fn run_binary(
    binary: &Path,
    (repo, root): (&Repo, &Path),
    mode: Mode,
    scratch: &Path,
    timeout: Duration,
    label: &str,
) -> Result<Result<(Value, Option<i32>), String>> {
    let mut command = Command::new(binary);
    command
        .args([".", "--format", "json", "--no-install"])
        .current_dir(root);
    if repo.recursive {
        command.arg("--recursive");
    }
    if mode == Mode::Typed {
        command.arg(TYPE_AWARE_FLAG);
    }
    let run = capture(&mut command, scratch, label, timeout)?;
    if run.timed_out {
        return Ok(Err("timed out".to_owned()));
    }
    let doc: Value = match serde_json::from_slice(&run.stdout) {
        Ok(doc) => doc,
        Err(error) => return Ok(Err(format!("output is not JSON ({error}); {}", run.stderr_summary()))),
    };
    if doc.get("diagnostics").is_none() {
        let message = doc["error"]["message"]
            .as_str()
            .unwrap_or("no diagnostics in the output");
        return Ok(Err(format!("failed: {message}")));
    }
    Ok(Ok((doc, run.code)))
}

fn compare_runs(
    reference: Result<(Value, Option<i32>), String>,
    skif: Result<(Value, Option<i32>), String>,
    allow: &[Allow],
    repo: &str,
    mode: Mode,
) -> Outcome {
    match (reference, skif) {
        (Ok((mut a, code_a)), Ok((mut b, code_b))) => {
            a["exit-code"] = json!(code_a);
            b["exit-code"] = json!(code_b);
            compare(&a, &b, allow, repo, mode)
        }
        (Err(reason), Ok(_)) => Outcome::Failed(format!("reference {reason}")),
        (Ok(_), Err(reason)) => Outcome::Failed(format!("skif {reason}")),
        (Err(a), Err(b)) => Outcome::Failed(format!("reference {a}; skif {b}")),
    }
}

/// Runs the gate.
///
/// # Errors
/// Fails when a checkout cannot be fetched or a binary cannot be started, and when any comparison differs beyond the
/// allow list or has no report.
pub fn run(options: &Options) -> Result<()> {
    let absolute = |binary: &Path| {
        binary
            .canonicalize()
            .with_context(|| format!("{}: not a skif binary", binary.display()))
    };
    let (reference, skif) = (absolute(&options.reference)?, absolute(&options.skif)?);
    let scratch = std::path::absolute(&options.scratch)?;
    let allow = options
        .allow
        .as_deref()
        .map(load_allow)
        .transpose()?
        .unwrap_or_default();
    let suite = corpus::fetch_suite(&options.corpus_file, SUITE, &options.only, &options.corpus_dir)?;
    let mut rows = Vec::new();
    for (repo, root) in &suite.repos {
        for mode in Mode::ALL {
            eprintln!("typed-diff: {} ({})", repo.name, mode.name());
            let reference = run_binary(
                &reference,
                (repo, root),
                mode,
                &scratch,
                options.timeout,
                &format!("{}-{}-reference", repo.name, mode.name()),
            )?;
            let skif = run_binary(
                &skif,
                (repo, root),
                mode,
                &scratch,
                options.timeout,
                &format!("{}-{}-skif", repo.name, mode.name()),
            )?;
            rows.push(Row {
                repo: repo.name.clone(),
                mode,
                outcome: compare_runs(reference, skif, &allow, &repo.name, mode),
            });
        }
    }
    print!("{}", render(&rows));
    if let Some(out) = &options.out {
        fs::write(out, serde_json::to_string_pretty(&report_json(&rows))?)
            .with_context(|| format!("writing {}", out.display()))?;
    }
    if rows.is_empty() {
        bail!("no repository was compared");
    }
    let bad = rows.iter().filter(|row| !row.outcome.passes()).count();
    if bad > 0 {
        bail!("{bad} comparison(s) differ between the reference and the binary under test");
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn diagnostic(rule: &str, file: &str, line: u64) -> Value {
        json!({
            "rule": rule, "severity": "warning", "message": format!("{rule} in {file}"),
            "location": {"path": file, "span": {"start-line": line}}, "fixes": [],
        })
    }

    fn package(dir: &str, state: &str) -> Value {
        json!({"dir": dir, "name": dir, "state": state, "untyped-files": 0})
    }

    fn document(diagnostics: &[Value], packages: &[Value]) -> Value {
        json!({
            "version": 1, "skif": "0.6.0",
            "summary": {"warnings": diagnostics.len()},
            "diagnostics": diagnostics,
            "type-aware": {"enabled": true, "engine": "tsrs", "packages": packages},
        })
    }

    fn base() -> Value {
        document(&[diagnostic("unused-file", "a.ts", 1)], &[package(".", "typed")])
    }

    fn allow(entries: &Value) -> Vec<Allow> {
        serde_json::from_value(entries.clone()).unwrap()
    }

    #[test]
    fn identical_documents_are_identical_whatever_the_version_stamp() {
        let mut other = base();
        other["skif"] = json!("0.7.0");
        assert_eq!(compare(&base(), &other, &[], "r", Mode::Typed), Outcome::Identical);
    }

    #[test]
    fn an_added_finding_is_reported_by_rule_and_file() {
        let new = document(
            &[
                diagnostic("unused-file", "a.ts", 1),
                diagnostic("unused-export", "b.ts", 4),
            ],
            &[package(".", "typed")],
        );
        let Outcome::Different(found) = compare(&base(), &new, &[], "r", Mode::Typed) else {
            panic!("not different");
        };
        assert_eq!(found.residual.added.len(), 1);
        assert_eq!(found.residual.added[0].rule, "unused-export");
        assert_eq!(found.residual.added[0].file, "b.ts");
        assert_eq!(found.residual.removed.len(), 0);
        assert!(found.residual.sections.contains("diagnostics") && found.residual.sections.contains("summary"));
        assert!(!found.residual.sections.contains("type-aware"));
    }

    #[test]
    fn a_removed_finding_is_reported() {
        let new = document(&[], &[package(".", "typed")]);
        let Outcome::Different(found) = compare(&base(), &new, &[], "r", Mode::Untyped) else {
            panic!("not different");
        };
        assert_eq!(found.residual.removed.len(), 1);
        assert_eq!(found.residual.added.len(), 0);
    }

    #[test]
    fn a_changed_field_of_a_finding_is_a_removal_and_an_addition() {
        let mut changed = diagnostic("unused-file", "a.ts", 1);
        changed["severity"] = json!("error");
        let diff = diff_documents(&base(), &document(&[changed], &[package(".", "typed")]));
        assert_eq!((diff.added.len(), diff.removed.len()), (1, 1));
    }

    #[test]
    fn a_changed_package_state_is_reported_and_leaves_other_sections_alone() {
        let new = document(&[diagnostic("unused-file", "a.ts", 1)], &[package(".", "partial")]);
        let Outcome::Different(found) = compare(&base(), &new, &[], "r", Mode::Typed) else {
            panic!("not different");
        };
        assert_eq!(found.residual.packages.len(), 1);
        assert_eq!(found.residual.packages[0].dir, ".");
        assert!(
            found.residual.packages[0]
                .reference
                .as_deref()
                .unwrap()
                .starts_with("typed")
        );
        assert!(
            found.residual.packages[0]
                .skif
                .as_deref()
                .unwrap()
                .starts_with("partial")
        );
        assert_eq!(found.residual.sections, BTreeSet::from(["type-aware".to_owned()]));
    }

    #[test]
    fn an_allow_list_covering_every_difference_passes() {
        let new = document(
            &[
                diagnostic("unused-file", "a.ts", 1),
                diagnostic("unused-export", "b.ts", 4),
            ],
            &[package(".", "partial")],
        );
        let covering = allow(&json!([
            {"kind": "added", "rule": "unused-export", "file": "b.ts", "repo": "r", "reason": "ADR"},
            {"kind": "package", "package": ".", "mode": "typed", "reason": "ADR"},
        ]));
        let outcome = compare(&base(), &new, &covering, "r", Mode::Typed);
        let Outcome::Allowed(assessment) = outcome else {
            panic!("not allowed: {outcome:?}");
        };
        assert_eq!(assessment.allowed, 2);
    }

    #[test]
    fn an_allow_list_for_another_repository_mode_or_rule_does_not_cover() {
        let new = document(&[], &[package(".", "typed")]);
        for entry in [
            json!({"kind": "removed", "repo": "other", "reason": "x"}),
            json!({"kind": "removed", "mode": "typed", "reason": "x"}),
            json!({"kind": "removed", "rule": "unused-export", "reason": "x"}),
            json!({"kind": "added", "reason": "x"}),
        ] {
            let outcome = compare(&base(), &new, &allow(&json!([entry])), "r", Mode::Untyped);
            assert!(matches!(outcome, Outcome::Different(_)), "{entry}");
        }
    }

    #[test]
    fn a_partly_covered_difference_still_fails() {
        let new = document(
            &[
                diagnostic("unused-file", "a.ts", 1),
                diagnostic("unused-export", "b.ts", 4),
            ],
            &[package(".", "typed")],
        );
        let partial = allow(&json!([{"kind": "added", "rule": "unused-export", "file": "c.ts", "reason": "x"}]));
        let Outcome::Different(found) = compare(&base(), &new, &partial, "r", Mode::Typed) else {
            panic!("not different");
        };
        assert_eq!(found.residual.added.len(), 1);
    }

    #[test]
    fn an_unrelated_section_difference_is_not_covered_by_finding_entries() {
        let mut new = base();
        new["incomplete-packages"] = json!(["x"]);
        let covering = allow(&json!([{"kind": "added", "reason": "x"}, {"kind": "removed", "reason": "x"}]));
        assert!(matches!(
            compare(&base(), &new, &covering, "r", Mode::Typed),
            Outcome::Different(_)
        ));
        let by_section = allow(&json!([{"kind": "section", "section": "incomplete-packages", "reason": "x"}]));
        assert!(matches!(
            compare(&base(), &new, &by_section, "r", Mode::Typed),
            Outcome::Allowed(_)
        ));
    }

    #[test]
    fn the_report_lists_differences_and_totals() {
        let new = document(&[], &[package(".", "partial")]);
        let rows = vec![
            Row {
                repo: "a".into(),
                mode: Mode::Typed,
                outcome: Outcome::Identical,
            },
            Row {
                repo: "b".into(),
                mode: Mode::Typed,
                outcome: compare(&base(), &new, &[], "b", Mode::Typed),
            },
        ];
        let text = render(&rows);
        assert!(text.contains("DIFFERENT"));
        assert!(text.contains("- unused-file a.ts:1"));
        assert!(text.contains("~ .: typed"));
        assert!(text.contains("2 comparison(s): 1 identical, 0 allowed, 1 different, 0 failed"));
    }

    #[test]
    fn a_long_list_is_capped() {
        let many: Vec<Value> = (0..30)
            .map(|i| diagnostic("unused-file", &format!("f{i}.ts"), 1))
            .collect();
        let rows = vec![Row {
            repo: "a".into(),
            mode: Mode::Typed,
            outcome: compare(&document(&[], &[]), &document(&many, &[]), &[], "a", Mode::Typed),
        }];
        assert!(render(&rows).contains("... and 10 more"));
    }

    #[test]
    fn allow_entries_need_a_reason_and_known_fields() {
        let dir = std::env::temp_dir().join(format!("skif-typed-diff-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("allow.json");
        for (text, ok) in [
            (r#"{"allow": [{"kind": "added", "reason": "ADR"}]}"#, true),
            (r#"{"allow": [{"kind": "added", "reason": " "}]}"#, false),
            (r#"{"allow": [{"kind": "section", "reason": "ADR"}]}"#, false),
            (r#"{"allow": [{"kind": "added", "reason": "ADR", "typo": 1}]}"#, false),
        ] {
            fs::write(&path, text).unwrap();
            assert_eq!(load_allow(&path).is_ok(), ok, "{text}");
        }
        fs::remove_dir_all(&dir).unwrap();
    }
}
