//! Conformance suite for the policy rules: runs them over `fixtures/policy/**` and compares the diagnostics, the
//! fixes and the re-run with the expectations of each case.
//!
//! The harness follows `fixtures/policy/README.md`. Cases are filtered to [`IMPLEMENTED_GROUPS`]; extend the list when
//! the rules of another group land. A failing case prints what is missing from and what is unexpected in the
//! diagnostics, or which fixed file differs.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use skif_config::Config;
use skif_core::{Diagnostic, FixOp, MemoryFs, TextEdit, apply_text_edits};
use skif_edit::FileKind;
use skif_lint::{Registry, RunOptions, run};
use skif_rules::policy_provider;
use skif_workspace::Workspace;

/// The fixture groups whose rules are implemented.
const IMPLEMENTED_GROUPS: &[&str] = &[
    "version-mismatch",
    "pinned",
    "banned",
    "ignored",
    "same-range",
    "same-minor",
    "snapped-to",
    "semver-groups",
    "selectors",
    "invalid-local-version",
    "policy-conflict",
];

/// Cases of an implemented group that depend on rules still to be implemented, with the reason.
const PENDING_CASES: &[(&str, &str)] = &[];

const ROOT: &str = "/skif-policy-fixture";

struct Case {
    group: String,
    name: String,
    dir: PathBuf,
    tags: Vec<String>,
}

impl Case {
    fn id(&self) -> String {
        format!("{}/{}", self.group, self.name)
    }

    fn has_tag(&self, tag: &str) -> bool {
        self.tags.iter().any(|candidate| candidate == tag)
    }
}

fn fixtures_root() -> PathBuf {
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

fn discover_cases() -> Vec<Case> {
    let mut cases = Vec::new();
    for group_dir in sorted_dirs(&fixtures_root()) {
        let group = group_dir.file_name().unwrap().to_string_lossy().into_owned();
        for dir in sorted_dirs(&group_dir) {
            let Ok(meta) = std::fs::read_to_string(dir.join("case.toml")) else {
                continue;
            };
            let meta: toml::Table = toml::from_str(&meta).unwrap();
            let tags = meta
                .get("tags")
                .and_then(toml::Value::as_array)
                .map(|tags| tags.iter().filter_map(|tag| tag.as_str().map(str::to_owned)).collect())
                .unwrap_or_default();
            cases.push(Case {
                group: group.clone(),
                name: dir.file_name().unwrap().to_string_lossy().into_owned(),
                dir,
                tags,
            });
        }
    }
    cases
}

/// Every file below `dir`, keyed by its `/`-separated path relative to `dir`.
fn read_tree(dir: &Path) -> BTreeMap<String, String> {
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

fn lint(files: &BTreeMap<String, String>, config: &Config) -> Result<Vec<Diagnostic>, String> {
    let mut fs = MemoryFs::new();
    for (path, content) in files {
        fs.insert(Path::new(ROOT).join(path), content);
    }
    let workspace =
        Workspace::discover_with_fs(Path::new(ROOT), config, &fs).map_err(|error| format!("workspace: {error}"))?;
    let registry = Registry::new(vec![Box::new(policy_provider())]).map_err(|error| error.to_string())?;
    run(&registry, &workspace, config, &RunOptions::default())
        .map(|report| report.diagnostics)
        .map_err(|error| format!("lint: {error}"))
}

/// The comparable projection of a diagnostic: everything the fixtures pin, nothing about spans or messages.
fn project(diagnostic: &Diagnostic) -> Value {
    json!({
        "rule": diagnostic.rule.as_str(),
        "severity": serde_json::to_value(diagnostic.severity).unwrap(),
        "package": diagnostic.package,
        "dependency": diagnostic.dependency,
        "detail": diagnostic.detail,
        "path": diagnostic.location.path.to_string_lossy().replace('\\', "/"),
        "fixable": diagnostic.is_fixable(),
    })
}

/// The order the fixtures are written in: path, rule, dependency, detail (byte order, `null` first).
fn sort_rows(rows: &mut [Value]) {
    let key = |row: &Value| {
        let text = |field: &str| row[field].as_str().map(str::to_owned);
        (text("path"), text("rule"), text("dependency"), text("detail"))
    };
    rows.sort_by_key(key);
}

fn describe(row: &Value) -> String {
    let field = |name: &str| row[name].as_str().unwrap_or("-").to_owned();
    format!(
        "{} {} {} {} {}: {}{}",
        field("rule"),
        field("severity"),
        field("package"),
        field("dependency"),
        field("detail"),
        field("path"),
        if row["fixable"] == json!(true) {
            " (fixable)"
        } else {
            ""
        },
    )
}

/// What is missing from `actual` and what it has in excess, as printable lines.
fn diff_rows(expected: &[Value], actual: &[Value]) -> Vec<String> {
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

/// The files changed by the preferred fix of every fixable diagnostic.
fn apply_fixes(
    files: &BTreeMap<String, String>,
    diagnostics: &[Diagnostic],
) -> Result<BTreeMap<String, String>, String> {
    let name = |path: &Path| path.to_string_lossy().replace('\\', "/");
    let mut structured: BTreeMap<String, Vec<&FixOp>> = BTreeMap::new();
    let mut text: BTreeMap<String, Vec<TextEdit>> = BTreeMap::new();
    let mut changed: BTreeMap<String, String> = BTreeMap::new();
    let mut deleted = Vec::new();
    for op in diagnostics
        .iter()
        .filter_map(|diagnostic| diagnostic.fixes.first())
        .flat_map(|fix| &fix.ops)
    {
        match op {
            FixOp::Json { path, .. } | FixOp::Yaml { path, .. } => structured.entry(name(path)).or_default().push(op),
            FixOp::Text { path, edit } => text.entry(name(path)).or_default().push(edit.clone()),
            FixOp::CreateFile { path, contents } => {
                changed.insert(name(path), contents.clone());
            }
            FixOp::DeleteFile { path } => deleted.push(name(path)),
            other => return Err(format!("unsupported fix operation {other:?}")),
        }
    }
    let source = |path: &str| {
        files
            .get(path)
            .map(String::as_str)
            .ok_or_else(|| format!("a fix edits {path}, which is not in the workspace"))
    };
    for (path, ops) in structured {
        let kind = FileKind::from_path(Path::new(&path)).ok_or_else(|| format!("no editor for {path}"))?;
        let edited = skif_edit::apply(source(&path)?, kind, &ops).map_err(|error| format!("{path}: {error}"))?;
        changed.insert(path, edited);
    }
    for (path, edits) in text {
        let edited = apply_text_edits(source(&path)?, &edits).map_err(|error| format!("{path}: {error}"))?;
        changed.insert(path, edited);
    }
    for path in deleted {
        changed.remove(&path);
    }
    Ok(changed)
}

fn parse_data(path: &str, content: &str) -> Result<Value, String> {
    let parsed = match FileKind::from_path(Path::new(path)) {
        Some(FileKind::Json) => serde_json::from_str(content).map_err(|error| error.to_string()),
        Some(FileKind::Yaml) => serde_saphyr::from_str(content).map_err(|error| error.to_string()),
        _ => return Ok(Value::String(content.to_owned())),
    };
    parsed.map_err(|error| format!("{path} does not parse: {error}"))
}

fn check_case(case: &Case) -> Result<(), String> {
    let config_source = std::fs::read_to_string(case.dir.join("skif.toml")).map_err(|error| error.to_string())?;
    let config = Config::from_toml_str(&config_source).map_err(|error| format!("skif.toml: {error}"))?;
    let files = read_tree(&case.dir.join("workspace"));
    let diagnostics = lint(&files, &config)?;

    let expected_source = std::fs::read_to_string(case.dir.join("expected.json")).map_err(|error| error.to_string())?;
    let mut expected: Vec<Value> = serde_json::from_str(&expected_source).map_err(|error| error.to_string())?;
    let mut actual: Vec<Value> = diagnostics.iter().map(project).collect();
    sort_rows(&mut expected);
    sort_rows(&mut actual);
    if expected != actual {
        return Err(format!(
            "diagnostics differ:\n{}",
            diff_rows(&expected, &actual).join("\n")
        ));
    }

    let changed = apply_fixes(&files, &diagnostics)?;
    let expected_fixed = read_tree(&case.dir.join("expected-fixed"));
    if expected_fixed.is_empty() && !changed.is_empty() {
        return Err(format!(
            "no expected-fixed, but the fixes change {}",
            changed.keys().cloned().collect::<Vec<_>>().join(", ")
        ));
    }
    if let Some(path) = changed.keys().find(|path| !expected_fixed.contains_key(*path)) {
        return Err(format!("the fixes change {path}, which expected-fixed does not list"));
    }
    let mut after = files.clone();
    after.extend(changed);
    for (path, content) in &expected_fixed {
        let (want, got) = (parse_data(path, content)?, parse_data(path, &after[path])?);
        if want != got {
            return Err(format!(
                "{path} differs after fixing:\n  expected: {want}\n  actual:   {got}"
            ));
        }
    }

    if !case.has_tag("fix-not-idempotent") {
        let again: Vec<String> = lint(&after, &config)?
            .iter()
            .filter(|diagnostic| diagnostic.is_fixable())
            .map(|diagnostic| describe(&project(diagnostic)))
            .collect();
        if !again.is_empty() {
            return Err(format!("still fixable after fixing:\n  {}", again.join("\n  ")));
        }
    }
    Ok(())
}

fn is_pending(case: &Case) -> bool {
    PENDING_CASES.iter().any(|(id, _)| *id == case.id())
}

#[test]
fn implemented_groups_exist() {
    let cases = discover_cases();
    for group in IMPLEMENTED_GROUPS {
        assert!(
            cases.iter().any(|case| case.group == *group),
            "no fixtures for group {group}"
        );
    }
    for (id, _) in PENDING_CASES {
        assert!(cases.iter().any(|case| case.id() == *id), "unknown pending case {id}");
    }
}

#[test]
fn policy_fixtures_conform() {
    let cases: Vec<Case> = discover_cases()
        .into_iter()
        .filter(|case| IMPLEMENTED_GROUPS.contains(&case.group.as_str()) && !is_pending(case))
        .collect();
    assert!(
        !cases.is_empty(),
        "no policy fixtures found under {}",
        fixtures_root().display()
    );
    let mut report = String::new();
    let mut failed = 0;
    for case in &cases {
        if let Err(message) = check_case(case) {
            failed += 1;
            writeln!(report, "\n{}\n{message}", case.id()).unwrap();
        }
    }
    assert!(failed == 0, "{failed} of {} policy cases fail:\n{report}", cases.len());
}

/// README step 6: every configuration loads and validates, and every `detail` is a state of the ADR 0009 table.
#[test]
fn every_fixture_is_well_formed() {
    let adr = std::fs::read_to_string(fixtures_root().join("../../adrs/0009-version-and-catalog-policies.md")).unwrap();
    let mut problems = Vec::new();
    for case in discover_cases() {
        let config = std::fs::read_to_string(case.dir.join("skif.toml")).unwrap();
        if let Err(error) = Config::from_toml_str(&config) {
            problems.push(format!("{}: skif.toml: {error}", case.id()));
        }
        let expected: Vec<Value> =
            serde_json::from_str(&std::fs::read_to_string(case.dir.join("expected.json")).unwrap()).unwrap();
        for row in &expected {
            let known = row["detail"]
                .as_str()
                .is_none_or(|detail| detail == "UnusedGroup" || adr.contains(&format!("`{detail}`")));
            if !known {
                problems.push(format!("{}: unknown detail {}", case.id(), row["detail"]));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn row_diff_reports_missing_and_unexpected_rows() {
    let row = |rule: &str, dependency: &str| {
        json!({"rule": rule, "severity": "error", "package": "app", "dependency": dependency,
               "detail": null, "path": "package.json", "fixable": false})
    };
    let expected = [row("version-mismatch", "a"), row("version-mismatch", "b")];
    let actual = [row("version-mismatch", "b"), row("banned-dependency", "c")];
    let lines = diff_rows(&expected, &actual);
    assert_eq!(lines.len(), 2);
    assert!(
        lines[0].starts_with("  missing:") && lines[0].contains(" a "),
        "{lines:?}"
    );
    assert!(
        lines[1].starts_with("  unexpected:") && lines[1].contains("banned-dependency"),
        "{lines:?}"
    );
    assert_eq!(diff_rows(&expected, &expected), [] as [std::string::String; 0]);
}
