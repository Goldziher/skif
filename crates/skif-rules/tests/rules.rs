//! Crate-level checks over the public API: the rule catalogue and a snapshot of a representative workspace.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashSet;
use std::path::Path;

use skif_config::Config;
use skif_core::{Category, Fixability, RuleId, Severity};
use skif_lint::{Registry, RuleProvider, RunOptions, run};
use skif_rules::{manifest_provider, providers};
use skif_workspace::Workspace;

const EXPECTED: [&str; 11] = [
    "empty-dependencies",
    "root-package-private-field",
    "root-package-manager-field",
    "root-package-dependencies",
    "types-in-dependencies",
    "member-toolchain-fields",
    "incompatible-peer-dependencies",
    "unsorted-dependencies",
    "unmatched-workspace-patterns",
    "missing-package-manifest",
    "local-path-dependency",
];

fn registry() -> Registry {
    Registry::new(providers()).unwrap()
}

fn fixture(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

fn lint_fixture(name: &str) -> skif_core::Report {
    let config = Config::default();
    let workspace = Workspace::discover(&fixture(name), &config).unwrap();
    run(&registry(), &workspace, &config, &RunOptions::default()).unwrap()
}

#[test]
fn provider_ids_and_rule_set_are_stable() {
    assert_eq!(manifest_provider().id(), "manifest");
    let all = providers();
    assert_eq!(
        all.iter().map(|provider| provider.id()).collect::<Vec<_>>(),
        ["manifest"]
    );
    let ids: Vec<&str> = all
        .iter()
        .flat_map(|provider| provider.rules())
        .map(|meta| meta.id)
        .collect();
    assert_eq!(ids, EXPECTED);
}

#[test]
fn rule_ids_are_unique_kebab_case_and_documented() {
    let metas: Vec<_> = registry().rules().collect();
    let unique: HashSet<&str> = metas.iter().map(|meta| meta.id).collect();
    assert_eq!(unique.len(), metas.len(), "duplicate rule id");
    for meta in &metas {
        assert!(RuleId::new(meta.id).is_ok(), "{} is not kebab-case", meta.id);
        assert!(meta.summary.len() > 20, "{} has no real summary", meta.id);
        assert!(meta.summary.ends_with('.'), "{} summary must be a sentence", meta.id);
        assert!(meta.default.is_some(), "{} must be on by default", meta.id);
    }
    let summaries: HashSet<&str> = metas.iter().map(|meta| meta.summary).collect();
    assert_eq!(summaries.len(), metas.len(), "two rules share a summary");
}

#[test]
fn categories_and_fixability_follow_the_adr() {
    let by_id = |id: &str| registry().get(id).unwrap();
    for id in ["unmatched-workspace-patterns", "missing-package-manifest"] {
        assert_eq!(by_id(id).category, Category::Workspace, "{id}");
    }
    for id in EXPECTED
        .iter()
        .filter(|id| !id.contains("workspace-patterns") && !id.contains("package-manifest"))
    {
        assert_eq!(by_id(id).category, Category::Manifest, "{id}");
    }
    for id in ["root-package-manager-field", "incompatible-peer-dependencies"] {
        assert_eq!(by_id(id).fixable, Fixability::None, "{id}");
    }
    assert_eq!(by_id("local-path-dependency").fixable, Fixability::Sometimes);
    assert_eq!(by_id("empty-dependencies").default, Some(Severity::Error));
    assert_eq!(by_id("member-toolchain-fields").default, Some(Severity::Warning));
}

#[test]
fn a_clean_workspace_reports_nothing() {
    let report = lint_fixture("clean");
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
}

#[test]
fn every_rule_fires_on_the_dirty_workspace() {
    let report = lint_fixture("dirty");
    let fired: HashSet<&str> = report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.rule.as_str())
        .collect();
    for id in EXPECTED {
        assert!(fired.contains(id), "{id} did not fire on the dirty fixture");
    }
}

#[test]
fn dirty_workspace_snapshot() {
    let report = lint_fixture("dirty");
    insta::assert_snapshot!(serde_json::to_string_pretty(&report).unwrap());
}

#[test]
fn rules_can_be_turned_off_and_ignored_through_configuration() {
    let config = Config::from_toml_str(
        "[rules]\nunsorted-dependencies = \"off\"\n\n[[ignore]]\nrules = [\"types-in-dependencies\"]\npackages = [\"@dirty/a\"]\n",
    )
    .unwrap();
    let workspace = Workspace::discover(&fixture("dirty"), &config).unwrap();
    let report = run(&registry(), &workspace, &config, &RunOptions::default()).unwrap();
    assert!(
        report
            .diagnostics
            .iter()
            .all(|d| !matches!(d.rule.as_str(), "unsorted-dependencies" | "types-in-dependencies"))
    );
    assert_ne!(report.diagnostics, [] as [skif_core::Diagnostic; 0]);
}
