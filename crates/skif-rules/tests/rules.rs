//! Crate-level checks over the public API: the rule catalogue and a snapshot of a representative workspace.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use skif_core::HashSet;
use std::path::Path;

use skif_config::Config;
use skif_core::{Category, Fixability, RuleId, Severity};
use skif_lint::{Registry, RuleProvider, RunOptions, run};
use skif_rules::{manifest_provider, policy_provider, providers};
use skif_workspace::Workspace;

const EXPECTED: [&str; 18] = [
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
    "stray-lockfile",
    "nested-workspace-root",
    "local-path-dependency",
    "package-json-format",
    "stale-patched-dependencies",
    "invalid-workspace-file",
    "stale-lerna-config",
    "lerna-packages-mismatch",
];

/// The manifest rules only: the snapshot fixtures are not meant to exercise the policy rules.
fn registry() -> Registry {
    Registry::new(vec![Box::new(manifest_provider())]).unwrap()
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
    assert_eq!(policy_provider().id(), "policy");
    let all = providers();
    assert_eq!(
        all.iter().map(|provider| provider.id()).collect::<Vec<_>>(),
        ["manifest", "policy", "catalog", "toolchain"]
    );
    let ids: Vec<&str> = all
        .iter()
        .flat_map(|provider| provider.rules())
        .map(|meta| meta.id)
        .collect();
    let (manifest, policy) = ids.split_at(EXPECTED.len());
    assert_eq!(manifest, EXPECTED);
    assert_eq!(
        policy,
        [
            "version-mismatch",
            "banned-dependency",
            "semver-range-mismatch",
            "policy-conflict",
            "invalid-local-version",
            "catalog-not-used",
            "catalog-mismatch",
            "catalog-missing-entry",
            "catalog-version-divergence",
            "unresolved-catalog-references",
            "unused-catalog-entries",
            "workspace-protocol",
            "unsynced-dependency-family",
            "toolchain-pin-drift",
            "foreign-package-manager-command",
        ]
    );
    Registry::new(all).unwrap();
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
        // `package-json-format` is opt-in: it reformats manifests no other rule touches (ADR 0055).
        assert!(
            meta.default.is_some() || meta.id == "package-json-format",
            "{} must be on by default",
            meta.id
        );
    }
    let summaries: HashSet<&str> = metas.iter().map(|meta| meta.summary).collect();
    assert_eq!(summaries.len(), metas.len(), "two rules share a summary");
}

#[test]
fn categories_and_fixability_follow_the_adr() {
    let by_id = |id: &str| registry().get(id).unwrap();
    let workspace_rules = [
        "unmatched-workspace-patterns",
        "missing-package-manifest",
        "stray-lockfile",
        "nested-workspace-root",
        "stale-patched-dependencies",
        "invalid-workspace-file",
        "stale-lerna-config",
        "lerna-packages-mismatch",
    ];
    for id in workspace_rules {
        assert_eq!(by_id(id).category, Category::Workspace, "{id}");
    }
    for id in EXPECTED.iter().filter(|id| !workspace_rules.contains(id)) {
        assert_eq!(by_id(id).category, Category::Manifest, "{id}");
    }
    for id in [
        "root-package-manager-field",
        "incompatible-peer-dependencies",
        "nested-workspace-root",
        "invalid-workspace-file",
        "stale-lerna-config",
        "lerna-packages-mismatch",
    ] {
        assert_eq!(by_id(id).fixable, Fixability::None, "{id}");
    }
    assert_eq!(by_id("local-path-dependency").fixable, Fixability::Sometimes);
    assert_eq!(by_id("stray-lockfile").fixable, Fixability::Unsafe);
    assert_eq!(by_id("empty-dependencies").default, Some(Severity::Error));
    assert_eq!(by_id("invalid-workspace-file").default, Some(Severity::Error));
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
    // An opt-in rule is off under the default configuration; its own tests turn it on.
    for id in EXPECTED
        .into_iter()
        .filter(|id| registry().get(id).unwrap().default.is_some())
    {
        assert!(fired.contains(id), "{id} did not fire on the dirty fixture");
    }
}

#[test]
fn dirty_workspace_snapshot() {
    let report = lint_fixture("dirty");
    insta::assert_snapshot!(serde_json::to_string_pretty(&report).unwrap());
}

#[test]
fn every_diagnostic_naming_a_dependency_instance_carries_its_specifier() {
    let report = lint_fixture("dirty");
    let named: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.dependency.is_some())
        .collect();
    assert_ne!(named, [] as [&skif_core::Diagnostic; 0]);
    for diagnostic in named {
        assert!(
            diagnostic.specifier.is_some(),
            "{} names {:?} without a specifier",
            diagnostic.rule,
            diagnostic.dependency
        );
    }
}

#[test]
fn ignore_entries_match_dependencies_by_name_and_specifier() {
    let ignored = |dependency: &str| {
        let toml = format!("[[ignore]]\nrules = [\"unsorted-dependencies\"]\ndependencies = [\"{dependency}\"]\n");
        let config = Config::from_toml_str(&toml).unwrap();
        let workspace = Workspace::discover(&fixture("dirty"), &config).unwrap();
        let report = run(&registry(), &workspace, &config, &RunOptions::default()).unwrap();
        report
            .diagnostics
            .iter()
            .filter(|diagnostic| {
                diagnostic.rule.as_str() == "unsorted-dependencies" && diagnostic.dependency.as_deref() == Some("react")
            })
            .count()
    };
    assert_eq!(ignored("react"), 0);
    assert_eq!(ignored("react@^18.0.0"), 0);
    assert_eq!(ignored("react@^17.0.0"), 1);
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

#[test]
fn only_the_rules_about_a_version_are_version_scoped() {
    let mut scoped: Vec<&str> = Registry::new(providers())
        .unwrap()
        .rules()
        .filter(|meta| meta.version_scoped)
        .map(|meta| meta.id)
        .collect();
    scoped.sort_unstable();
    assert_eq!(
        scoped,
        [
            "catalog-mismatch",
            "catalog-version-divergence",
            "incompatible-peer-dependencies",
            "policy-conflict",
            "semver-range-mismatch",
            "unsynced-dependency-family",
            "version-mismatch",
        ]
    );
}
