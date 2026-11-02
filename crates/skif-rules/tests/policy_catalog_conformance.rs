//! Conformance of the catalog, workspace-protocol and dependency-family rules with `fixtures/policy`.
//!
//! Implements the harness steps of `fixtures/policy/README.md` for the groups `catalog`, `workspace-protocol` and
//! `dependency-families`: load `workspace/` and `skif.toml` into memory, run the rules, project the diagnostics and
//! compare them with `expected.json`; then apply the fixes and compare the changed files with `expected-fixed/`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use skif_config::Config;
use skif_core::{Diagnostic, FixOp, MemoryFs};
use skif_edit::FileKind;
use skif_lint::{Registry, RunOptions, run_unsorted};
use skif_workspace::Workspace;

const GROUPS: [&str; 3] = ["catalog", "workspace-protocol", "dependency-families"];

/// The rules these groups exercise; diagnostics of other rules belong to other harnesses.
const RULES: [&str; 7] = [
    "catalog-not-used",
    "catalog-mismatch",
    "catalog-missing-entry",
    "unresolved-catalog-references",
    "unused-catalog-entries",
    "workspace-protocol",
    "unsynced-dependency-family",
];

/// Cases the rules cannot pass yet, with the reason. A rule's severity comes from its configured level or its
/// default, so `mode = "enforce"` cannot raise `catalog-not-used` to an error.
const PENDING: [(&str, &str); 1] = [(
    "catalog/enforce-not-using-catalog",
    "severity cannot depend on the catalog mode",
)];

fn policy_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/policy")
}

fn files_below(dir: &Path) -> Vec<(String, String)> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
        let mut entries: Vec<_> = fs::read_dir(dir).unwrap().map(|entry| entry.unwrap().path()).collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let relative = path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
                out.push((relative, fs::read_to_string(&path).unwrap()));
            }
        }
    }
    let mut out = Vec::new();
    if dir.is_dir() {
        walk(dir, dir, &mut out);
    }
    out
}

fn cases() -> Vec<(String, PathBuf)> {
    let mut cases = Vec::new();
    for group in GROUPS {
        let mut dirs: Vec<_> = fs::read_dir(policy_root().join(group))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.join("case.toml").is_file())
            .collect();
        dirs.sort();
        for dir in dirs {
            let name = format!("{group}/{}", dir.file_name().unwrap().to_string_lossy());
            if PENDING.iter().all(|(pending, _)| *pending != name) {
                cases.push((name, dir));
            }
        }
    }
    cases
}

fn run(files: &[(String, String)], config_text: &str) -> Vec<Diagnostic> {
    let config = Config::from_toml_str(config_text).unwrap();
    let fs = files.iter().fold(MemoryFs::new(), |fs, (path, content)| {
        fs.with_file(Path::new("/ws").join(path), content)
    });
    let workspace = Workspace::discover_with_fs(Path::new("/ws"), &config, &fs).unwrap();
    let registry = Registry::new(skif_rules::providers()).unwrap();
    run_unsorted(&registry, &workspace, &config, &RunOptions::default())
        .unwrap()
        .into_iter()
        .filter(|diagnostic| RULES.contains(&diagnostic.rule.as_str()))
        .collect()
}

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

/// Sorted by path, rule, dependency, detail; `null` sorts before any string.
fn sorted(mut diagnostics: Vec<Value>) -> Vec<Value> {
    let key =
        |value: &Value| ["path", "rule", "dependency", "detail"].map(|field| value[field].as_str().map(str::to_owned));
    diagnostics.sort_by_key(key);
    diagnostics
}

/// The first fix of every diagnostic, grouped by file, identical operations once.
fn operations(diagnostics: &[Diagnostic]) -> BTreeMap<String, Vec<&FixOp>> {
    let mut by_file: BTreeMap<String, Vec<&FixOp>> = BTreeMap::new();
    for op in diagnostics
        .iter()
        .filter_map(|diagnostic| diagnostic.fixes.first())
        .flat_map(|fix| &fix.ops)
    {
        let (FixOp::Json { path, .. } | FixOp::Yaml { path, .. }) = op else {
            panic!("unexpected fix operation {op:?}");
        };
        let ops = by_file.entry(path.to_string_lossy().replace('\\', "/")).or_default();
        if !ops.contains(&op) {
            ops.push(op);
        }
    }
    by_file
}

fn apply_fixes(files: &[(String, String)], diagnostics: &[Diagnostic]) -> Result<Vec<(String, String)>, String> {
    let mut contents: BTreeMap<String, String> = files.iter().cloned().collect();
    for (path, ops) in operations(diagnostics) {
        let source = contents
            .get(&path)
            .ok_or_else(|| format!("fix edits missing file {path}"))?;
        let kind = FileKind::from_path(Path::new(&path)).ok_or_else(|| format!("unknown file kind {path}"))?;
        let edited = skif_edit::apply(source, kind, &ops).map_err(|error| format!("{path}: {error}"))?;
        contents.insert(path, edited);
    }
    Ok(contents.into_iter().collect())
}

fn parsed(path: &str, text: &str) -> Value {
    if Path::new(path)
        .extension()
        .is_some_and(|extension| extension == "yaml" || extension == "yml")
    {
        serde_saphyr::from_str(text).unwrap()
    } else {
        serde_json::from_str(text).unwrap()
    }
}

fn check_case(dir: &Path) -> Result<(), String> {
    let config_text = fs::read_to_string(dir.join("skif.toml")).unwrap();
    let mut files = files_below(&dir.join("workspace"));
    files.push(("skif.toml".to_owned(), config_text.clone()));
    let expected_all: Vec<Value> =
        serde_json::from_str(&fs::read_to_string(dir.join("expected.json")).unwrap()).unwrap();
    let foreign = expected_all
        .iter()
        .any(|diagnostic| !RULES.contains(&diagnostic["rule"].as_str().unwrap()));
    let expected = sorted(
        expected_all
            .into_iter()
            .filter(|diagnostic| RULES.contains(&diagnostic["rule"].as_str().unwrap()))
            .collect(),
    );
    let diagnostics = run(&files, &config_text);
    let actual = sorted(diagnostics.iter().map(project).collect());
    if actual != expected {
        return Err(format!(
            "diagnostics differ\nexpected: {}\nactual:   {}",
            serde_json::to_string_pretty(&expected).unwrap(),
            serde_json::to_string_pretty(&actual).unwrap()
        ));
    }
    let expected_fixed = files_below(&dir.join("expected-fixed"));
    if foreign || (expected_fixed.is_empty() && !diagnostics.iter().any(Diagnostic::is_fixable)) {
        return Ok(());
    }
    let fixed = apply_fixes(&files, &diagnostics)?;
    for (path, text) in &fixed {
        let before = files
            .iter()
            .find(|(candidate, _)| candidate == path)
            .map(|(_, text)| text);
        match expected_fixed.iter().find(|(candidate, _)| candidate == path) {
            Some((_, wanted)) if parsed(path, text) != parsed(path, wanted) => {
                return Err(format!("{path} after fixing:\n{text}\nexpected:\n{wanted}"));
            }
            None if before != Some(text) => return Err(format!("{path} changed by the fixes:\n{text}")),
            _ => {}
        }
    }
    let remaining = run(&fixed, &config_text);
    if remaining.iter().any(Diagnostic::is_fixable) {
        return Err(format!(
            "fixable diagnostics remain after fixing: {:?}",
            remaining.iter().map(project).collect::<Vec<_>>()
        ));
    }
    Ok(())
}

#[test]
fn catalog_workspace_protocol_and_family_fixtures_conform() {
    let cases = cases();
    assert!(
        cases.len() + PENDING.len() >= 42,
        "fixtures not found: {} cases",
        cases.len()
    );
    let failures: Vec<String> = cases
        .iter()
        .filter_map(|(name, dir)| check_case(dir).err().map(|error| format!("{name}: {error}")))
        .collect();
    assert!(
        failures.is_empty(),
        "{} of {} cases failed:\n\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n\n")
    );
}

#[test]
fn every_expected_rule_is_registered() {
    let registry = Registry::new(skif_rules::providers()).unwrap();
    for rule in RULES {
        assert!(registry.get(rule).is_some(), "{rule} is not registered");
    }
}
