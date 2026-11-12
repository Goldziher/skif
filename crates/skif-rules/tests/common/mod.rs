//! The fixture harness shared by the policy and catalog conformance tests.
//!
//! It implements the steps of `fixtures/policy/README.md`: load `workspace/` and `skif.toml` into memory, run every
//! built-in provider, project the diagnostics of the rules a group exercises and compare them with `expected.json`,
//! check the spans and the package ids, plan the fixes the way the facade does (the first fix of each diagnostic,
//! in report order, skipped when it overlaps an earlier one) and compare the changed files with `expected-fixed/`,
//! then lint the fixed workspace again.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use skif_config::Config;
use skif_core::{Diagnostic, FixOp, FixSafety, MemoryFs, TextEdit, apply_text_edits};
use skif_edit::{EditPlanError, FileKind, plan_edits};
use skif_lint::{Registry, RuleProvider, RunOptions, run};
use skif_workspace::Workspace;

/// The number of fixture cases below `fixtures/policy`.
pub const TOTAL_CASES: usize = 193;

/// The groups whose rules are `policy_provider()`'s.
pub const POLICY_GROUPS: &[&str] = &[
    "version-mismatch",
    "pinned",
    "banned",
    "ignored",
    "same-range",
    "same-minor",
    "snapped-to",
    "semver-groups",
    "semver-range-only",
    "dependency-types",
    "selectors",
    "invalid-local-version",
    "policy-conflict",
];

/// The groups whose rules are `catalog_provider()`'s.
pub const CATALOG_GROUPS: &[&str] = &["catalog", "workspace-protocol", "dependency-families"];

/// The groups that run the rules of both providers together: the catalog rules and the version groups must leave
/// each instance to exactly one of them.
pub const JOINT_GROUPS: &[&str] = &["catalog-groups"];

/// Cases the rules cannot pass yet and that are therefore not run, with the reason.
pub const PENDING: &[(&str, &str)] = &[];

/// Cases whose `expected.json` also lists diagnostics of the other family of rules. Only the rules of the case's
/// group are compared, and the fixes of those rules; the rest waits for catalog definitions to take part in version
/// and semver groups, and for the harness to compare the rules of both families together.
pub const PARTIAL: &[(&str, &str)] = &[
    (
        "catalog/catalog-definition-semver-group",
        "semver groups do not yet cover catalog definitions",
    ),
    (
        "catalog/off-catalog-definition-joins-alignment",
        "catalog definitions do not yet take part in version alignment",
    ),
    (
        "workspace-protocol/version-mismatch-and-protocol-both-reported",
        "the version-mismatch half is a policy rule",
    ),
];

const ROOT: &str = "/ws";

pub struct Case {
    pub group: String,
    pub name: String,
    pub dir: PathBuf,
    meta: toml::Table,
}

impl Case {
    pub fn id(&self) -> String {
        format!("{}/{}", self.group, self.name)
    }

    /// Whether `expected-fixed/` is compared byte for byte instead of as parsed data.
    pub fn compares_bytes(&self) -> bool {
        self.meta.get("compare").and_then(toml::Value::as_str) == Some("bytes")
    }

    pub fn is_pending(&self) -> bool {
        PENDING.iter().any(|(id, _)| *id == self.id())
    }

    pub fn is_partial(&self) -> bool {
        PARTIAL.iter().any(|(id, _)| *id == self.id())
    }
}

pub fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/policy")
}

fn sorted_dirs(dir: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    dirs
}

/// Every case below `fixtures/policy`, in group and name order.
pub fn discover_cases() -> Vec<Case> {
    let mut cases = Vec::new();
    for group_dir in sorted_dirs(&fixtures_root()) {
        let group = group_dir.file_name().unwrap().to_string_lossy().into_owned();
        for dir in sorted_dirs(&group_dir) {
            let Ok(meta) = std::fs::read_to_string(dir.join("case.toml")) else {
                continue;
            };
            cases.push(Case {
                group: group.clone(),
                name: dir.file_name().unwrap().to_string_lossy().into_owned(),
                dir,
                meta: toml::from_str(&meta).unwrap(),
            });
        }
    }
    cases
}

/// Every file below `dir`, keyed by its `/`-separated path relative to `dir`.
pub fn read_tree(dir: &Path) -> BTreeMap<String, String> {
    fn walk(base: &Path, dir: &Path, files: &mut BTreeMap<String, String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(base, &path, files);
            } else {
                let relative = path.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/");
                files.insert(relative, std::fs::read_to_string(&path).unwrap());
            }
        }
    }
    let mut files = BTreeMap::new();
    if dir.is_dir() {
        walk(dir, dir, &mut files);
    }
    files
}

/// The IDs of the rules `provider` provides.
pub fn rule_ids(provider: &dyn RuleProvider) -> BTreeSet<&'static str> {
    provider.rules().into_iter().map(|meta| meta.id).collect()
}

/// Runs every built-in provider over `files` and keeps the diagnostics of `rules`.
pub fn lint(
    files: &BTreeMap<String, String>,
    config: &Config,
    rules: &BTreeSet<&'static str>,
) -> Result<Vec<Diagnostic>, String> {
    let mut fs = MemoryFs::new();
    for (path, content) in files {
        fs.insert(Path::new(ROOT).join(path), content);
    }
    let workspace =
        Workspace::discover_with_fs(Path::new(ROOT), config, &fs).map_err(|error| format!("workspace: {error}"))?;
    let registry = Registry::new(skif_rules::providers()).map_err(|error| error.to_string())?;
    let report =
        run(&registry, &workspace, config, &RunOptions::default()).map_err(|error| format!("lint: {error}"))?;
    Ok(report
        .diagnostics
        .into_iter()
        .filter(|diagnostic| rules.contains(diagnostic.rule.as_str()))
        .collect())
}

/// The comparable projection of a diagnostic: everything `expected.json` pins.
pub fn project(diagnostic: &Diagnostic) -> Value {
    let mut row = json!({
        "rule": diagnostic.rule.as_str(),
        "severity": serde_json::to_value(diagnostic.severity).unwrap(),
        "package": diagnostic.package,
        "dependency": diagnostic.dependency,
        "detail": diagnostic.detail,
        "path": diagnostic.location.path.to_string_lossy().replace('\\', "/"),
        "fixable": diagnostic.is_fixable(),
    });
    // ADR 0009: a fix `--fix` applies is marked; the key is absent for an unsafe fix and for no fix.
    if diagnostic
        .fixes
        .first()
        .is_some_and(|fix| fix.safety == FixSafety::Safe)
    {
        row["safe"] = json!(true);
    }
    row
}

/// The order the fixtures are written in: path, rule, dependency, detail (byte order, `null` first).
pub fn sort_rows(rows: &mut [Value]) {
    let key =
        |row: &Value| ["path", "rule", "dependency", "detail"].map(|field| row[field].as_str().map(str::to_owned));
    rows.sort_by_key(key);
}

pub fn describe(row: &Value) -> String {
    let field = |name: &str| row[name].as_str().unwrap_or("-").to_owned();
    format!(
        "{} {} {} {} {}: {}{}",
        field("rule"),
        field("severity"),
        field("package"),
        field("dependency"),
        field("detail"),
        field("path"),
        match (row["fixable"] == json!(true), row["safe"] == json!(true)) {
            (true, true) => " (fixable, safe)",
            (true, false) => " (fixable)",
            _ => "",
        },
    )
}

/// What is missing from `actual` and what it has in excess, as printable lines.
pub fn diff_rows(expected: &[Value], actual: &[Value]) -> Vec<String> {
    let mut unmatched: Vec<&Value> = actual.iter().collect();
    let mut lines = Vec::new();
    for row in expected {
        match unmatched.iter().position(|candidate| *candidate == row) {
            Some(index) => {
                unmatched.remove(index);
            }
            None => lines.push(format!("  missing:    {}", describe(row))),
        }
    }
    lines.extend(unmatched.iter().map(|row| format!("  unexpected: {}", describe(row))));
    lines
}

/// The 1-based line and scalar-value column of byte `offset` in `text`.
pub fn line_column(text: &str, offset: usize) -> (u32, u32) {
    let before = &text[..offset];
    let line = before.matches('\n').count() + 1;
    let column = before.rsplit('\n').next().map_or(0, |last| last.chars().count()) + 1;
    (u32::try_from(line).unwrap(), u32::try_from(column).unwrap())
}

/// Checks what the fixtures cannot pin: the span of each diagnostic starts where it says it does, and, when
/// `entry_spans` (the rules locate the whole dependency entry), covers the dependency it names; a diagnostic about a
/// package carries the package id suppression needs.
const CATALOG_RULES: [&str; 5] = [
    "catalog-mismatch",
    "catalog-not-used",
    "catalog-missing-entry",
    "unused-catalog-entries",
    "unresolved-catalog-references",
];

pub fn check_diagnostics(
    files: &BTreeMap<String, String>,
    diagnostics: &[Diagnostic],
    entry_spans: bool,
) -> Result<(), String> {
    let mut problems = Vec::new();
    for diagnostic in diagnostics {
        let label = || describe(&project(diagnostic));
        if diagnostic.package.is_some() && diagnostic.package_id.is_none() {
            problems.push(format!("{}: no package id", label()));
        }
        let path = diagnostic.location.path.to_string_lossy().replace('\\', "/");
        let Some(span) = diagnostic.location.span else {
            if path != "skif.toml" {
                problems.push(format!("{}: no span", label()));
            }
            continue;
        };
        let Some(text) = files.get(&path) else {
            problems.push(format!("{}: located in {path}, which is not in the workspace", label()));
            continue;
        };
        let (start, end) = (span.start as usize, span.end as usize);
        let Some(covered) = text.get(start..end) else {
            problems.push(format!("{}: span {start}..{end} is not a range of {path}", label()));
            continue;
        };
        if (span.start_line, span.start_col) != line_column(text, start) {
            problems.push(format!(
                "{}: span starts at {}:{} but offset {start} is {:?}",
                label(),
                span.start_line,
                span.start_col,
                line_column(text, start)
            ));
        }
        // A package's own version is located at its `version` member, which does not name the package.
        // Catalog rules locate the specifier, not the entry, also when they run together with the policy rules.
        let locates_entry = entry_spans && !CATALOG_RULES.contains(&diagnostic.rule.as_str());
        if let Some(dependency) = diagnostic.dependency.as_ref().filter(|_| locates_entry)
            && !covered.starts_with("\"version\"")
            && !covered.contains(dependency.as_str())
        {
            problems.push(format!("{}: the span {covered:?} does not cover {dependency}", label()));
        }
        // The catalog, workspace-protocol and family rules locate the specifier itself; catalog entries are located
        // at their key (or key and value).
        if let (Some(dependency), Some(specifier)) = (&diagnostic.dependency, &diagnostic.specifier)
            && !entry_spans
            && !covered.contains(dependency.as_str())
            && covered.trim_matches('"') != specifier.as_str()
        {
            problems.push(format!(
                "{}: the span {covered:?} is neither {dependency} nor its specifier {specifier:?}",
                label()
            ));
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("\n"))
    }
}

/// The files the first fix of each diagnostic changes, planned like the facade does.
pub struct Planned {
    /// Changed and created files; deleted files are absent.
    pub changed: BTreeMap<String, String>,
    pub deleted: BTreeSet<String>,
    /// Diagnostics whose fix overlapped an earlier one and was left out.
    pub skipped: Vec<String>,
}

enum Failure {
    Conflict(String),
    Other(String),
}

fn normalized(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn edit_file(source: &str, path: &str, ops: &[FixOp]) -> Result<String, Failure> {
    let (texts, structured): (Vec<&FixOp>, Vec<&FixOp>) = ops.iter().partition(|op| matches!(op, FixOp::Text { .. }));
    let mut edits: Vec<TextEdit> = texts
        .iter()
        .filter_map(|op| match op {
            FixOp::Text { edit, .. } => Some(edit.clone()),
            _ => None,
        })
        .collect();
    if !structured.is_empty() {
        let kind =
            FileKind::from_path(Path::new(path)).ok_or_else(|| Failure::Other(format!("no editor for {path}")))?;
        edits.extend(plan_edits(source, kind, &structured).map_err(|error| match error {
            EditPlanError::Conflict { .. } => Failure::Conflict(error.to_string()),
            _ => Failure::Other(format!("{path}: {error}")),
        })?);
    }
    edits.sort_by_key(|edit| (edit.start, edit.end));
    apply_text_edits(source, &edits).map_err(|error| Failure::Other(format!("{path}: {error}")))
}

/// Plans the first fix of every diagnostic, in order, at the `Unsafe` ceiling; which of them are safe is compared
/// through the `safe` key of `expected.json` (ADR 0009). Identical operations of different fixes count once, as several fixes may share the catalog entry
/// they add.
pub fn plan_fixes(files: &BTreeMap<String, String>, diagnostics: &[Diagnostic]) -> Result<Planned, String> {
    let mut edits: BTreeMap<String, Vec<FixOp>> = BTreeMap::new();
    let mut created: BTreeMap<String, String> = BTreeMap::new();
    let mut deleted: BTreeSet<String> = BTreeSet::new();
    let mut skipped = Vec::new();
    for diagnostic in diagnostics {
        let Some(fix) = diagnostic.fixes.first() else {
            continue;
        };
        let mut next = edits.clone();
        let (mut next_created, mut next_deleted) = (created.clone(), deleted.clone());
        let mut touched = BTreeSet::new();
        let mut clash = false;
        for op in &fix.ops {
            match op {
                FixOp::Json { path, .. } | FixOp::Yaml { path, .. } | FixOp::Text { path, .. } => {
                    let name = normalized(path);
                    clash |= next_created.contains_key(&name) || next_deleted.contains(&name);
                    let list = next.entry(name.clone()).or_default();
                    if !list.contains(op) {
                        list.push(op.clone());
                    }
                    touched.insert(name);
                }
                FixOp::CreateFile { path, contents, .. } => {
                    let name = normalized(path);
                    clash |= next.contains_key(&name);
                    next_created.insert(name, contents.clone());
                }
                FixOp::DeleteFile { path, .. } => {
                    let name = normalized(path);
                    clash |= next.contains_key(&name) || next_created.contains_key(&name);
                    next_deleted.insert(name);
                }
                other => return Err(format!("unsupported fix operation {other:?}")),
            }
        }
        if !clash {
            for name in &touched {
                let source = files
                    .get(name)
                    .ok_or_else(|| format!("a fix edits {name}, which is not in the workspace"))?;
                match edit_file(source, name, &next[name]) {
                    Ok(_) => {}
                    Err(Failure::Conflict(_)) => {
                        clash = true;
                        break;
                    }
                    Err(Failure::Other(message)) => return Err(message),
                }
            }
        }
        if clash {
            skipped.push(describe(&project(diagnostic)));
        } else {
            (edits, created, deleted) = (next, next_created, next_deleted);
        }
    }
    let mut changed = created;
    for (name, ops) in &edits {
        let edited = edit_file(&files[name], name, ops).map_err(|failure| match failure {
            Failure::Conflict(message) | Failure::Other(message) => message,
        })?;
        changed.insert(name.clone(), edited);
    }
    Ok(Planned {
        changed,
        deleted,
        skipped,
    })
}

fn parse_data(path: &str, content: &str) -> Result<Value, String> {
    let parsed = match FileKind::from_path(Path::new(path)) {
        Some(FileKind::Json) => serde_json::from_str(content).map_err(|error| error.to_string()),
        Some(FileKind::Yaml) => serde_saphyr::from_str(content).map_err(|error| error.to_string()),
        _ => return Ok(Value::String(content.to_owned())),
    };
    parsed.map_err(|error| format!("{path} does not parse: {error}"))
}

/// How the fixes of the diagnostics of `family` are planned for the files of a case.
pub type Planner = dyn Fn(&BTreeMap<String, String>, &Config, &BTreeSet<&'static str>) -> Result<Planned, String>;

/// Plans the first fix of every diagnostic jointly, the way [`plan_fixes`] does.
pub fn plan_jointly(
    files: &BTreeMap<String, String>,
    config: &Config,
    family: &BTreeSet<&'static str>,
) -> Result<Planned, String> {
    plan_fixes(files, &lint(files, config, family)?)
}

/// Runs one case: `family` selects the rules the group exercises.
pub fn check_case(case: &Case, family: &BTreeSet<&'static str>, plan: &Planner) -> Result<(), String> {
    let config_source = std::fs::read_to_string(case.dir.join("skif.toml")).map_err(|error| error.to_string())?;
    let config = Config::from_toml_str(&config_source).map_err(|error| format!("skif.toml: {error}"))?;
    let mut files = read_tree(&case.dir.join("workspace"));
    files.insert("skif.toml".to_owned(), config_source);
    let diagnostics = lint(&files, &config, family)?;

    let expected_source = std::fs::read_to_string(case.dir.join("expected.json")).map_err(|error| error.to_string())?;
    let all_expected: Vec<Value> = serde_json::from_str(&expected_source).map_err(|error| error.to_string())?;
    let (mut expected, foreign): (Vec<Value>, Vec<Value>) = all_expected
        .into_iter()
        .partition(|row| row["rule"].as_str().is_some_and(|rule| family.contains(rule)));
    if foreign.is_empty() == case.is_partial() {
        return Err(format!(
            "PARTIAL is out of step with the fixture: it {} rows of the other family of rules",
            if foreign.is_empty() { "has no" } else { "has" }
        ));
    }
    let mut actual: Vec<Value> = diagnostics.iter().map(project).collect();
    sort_rows(&mut expected);
    sort_rows(&mut actual);
    if expected != actual {
        return Err(format!(
            "diagnostics differ:\n{}",
            diff_rows(&expected, &actual).join("\n")
        ));
    }
    check_diagnostics(&files, &diagnostics, family.contains("version-mismatch"))?;

    let planned = plan(&files, &config, family)?;
    if !planned.skipped.is_empty() {
        return Err(format!(
            "fixes skipped as conflicting:\n  {}",
            planned.skipped.join("\n  ")
        ));
    }
    if case.is_partial() {
        // The expected files also carry the fixes of the other family of rules, which are not planned here.
        return Ok(());
    }
    let expected_fixed = read_tree(&case.dir.join("expected-fixed"));
    if expected_fixed.is_empty() && !planned.changed.is_empty() {
        return Err(format!(
            "no expected-fixed, but the fixes change {}",
            planned.changed.keys().cloned().collect::<Vec<_>>().join(", ")
        ));
    }
    if let Some(path) = planned.changed.keys().find(|path| !expected_fixed.contains_key(*path)) {
        return Err(format!("the fixes change {path}, which expected-fixed does not list"));
    }
    let mut after = files.clone();
    after.extend(planned.changed);
    for path in &planned.deleted {
        after.remove(path);
    }
    for (path, wanted) in &expected_fixed {
        let got = after
            .get(path)
            .ok_or_else(|| format!("{path} is missing after fixing"))?;
        if case.compares_bytes() {
            if got != wanted {
                return Err(format!(
                    "{path} differs byte for byte after fixing:\n  expected: {wanted:?}\n  actual:   {got:?}"
                ));
            }
        } else {
            let (want, got) = (parse_data(path, wanted)?, parse_data(path, got)?);
            if want != got {
                return Err(format!(
                    "{path} differs after fixing:\n  expected: {want}\n  actual:   {got}"
                ));
            }
        }
    }

    // Every fix is idempotent: nothing fixable is left, and the run reports nothing it did not report before.
    let remaining = lint(&after, &config, family)?;
    let mut allowed: Vec<Value> = diagnostics
        .iter()
        .filter(|diagnostic| !diagnostic.is_fixable())
        .map(project)
        .collect();
    let mut again: Vec<Value> = remaining.iter().map(project).collect();
    sort_rows(&mut allowed);
    sort_rows(&mut again);
    let new_rows = diff_rows(&allowed, &again)
        .into_iter()
        .filter(|line| line.starts_with("  unexpected:"))
        .collect::<Vec<_>>();
    if !new_rows.is_empty() {
        return Err(format!(
            "the fixed workspace reports more than before:\n{}",
            new_rows.join("\n")
        ));
    }
    Ok(())
}

/// Runs every case of `groups` that is not pending and reports all failures at once, planning the fixes jointly.
pub fn run_groups(groups: &[&str], family: &BTreeSet<&'static str>) {
    run_groups_with(groups, family, &plan_jointly);
}

/// As [`run_groups`], planning the fixes with `plan`.
pub fn run_groups_with(groups: &[&str], family: &BTreeSet<&'static str>, plan: &Planner) {
    let cases: Vec<Case> = discover_cases()
        .into_iter()
        .filter(|case| groups.contains(&case.group.as_str()) && !case.is_pending())
        .collect();
    assert!(
        !cases.is_empty(),
        "no fixtures found under {}",
        fixtures_root().display()
    );
    let mut report = String::new();
    let mut failed = 0;
    for case in &cases {
        if let Err(message) = check_case(case, family, plan) {
            failed += 1;
            writeln!(report, "\n{}\n{message}", case.id()).unwrap();
        }
    }
    assert!(failed == 0, "{failed} of {} cases fail:\n{report}", cases.len());
}
