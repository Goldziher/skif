//! Conformance suite for the version-group and semver-group rules: runs them over `fixtures/policy/**` and compares
//! the diagnostics, the spans, the fixes and the re-run with the expectations of each case.
//!
//! The harness is in `common` and follows `fixtures/policy/README.md`. A failing case prints what is missing from
//! and what is unexpected in the diagnostics, or which fixed file differs.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use serde_json::{Value, json};
use skif_config::Config;
use skif_lint::Registry;

use common::{
    CATALOG_GROUPS, JOINT_GROUPS, PARTIAL, PENDING, POLICY_GROUPS, TOTAL_CASES, diff_rows, discover_cases,
    fixtures_root, rule_ids, run_groups,
};

#[test]
fn policy_fixtures_conform() {
    run_groups(POLICY_GROUPS, &rule_ids(&skif_rules::policy_provider()));
}

/// Every case runs in one of the two harnesses, the count is the README's, and the pending and partial lists name
/// cases that exist, so nothing is skipped silently.
#[test]
fn every_case_is_accounted_for() {
    let cases = discover_cases();
    assert_eq!(cases.len(), TOTAL_CASES, "the README counts {TOTAL_CASES} cases");
    for case in &cases {
        assert!(
            [POLICY_GROUPS, CATALOG_GROUPS, JOINT_GROUPS]
                .iter()
                .any(|groups| groups.contains(&case.group.as_str())),
            "{} belongs to a group no harness runs",
            case.id()
        );
    }
    for group in POLICY_GROUPS.iter().chain(CATALOG_GROUPS).chain(JOINT_GROUPS) {
        assert!(
            cases.iter().any(|case| case.group == *group),
            "no fixtures for group {group}"
        );
    }
    for (id, reason) in PENDING.iter().chain(PARTIAL) {
        assert!(cases.iter().any(|case| case.id() == *id), "unknown pending case {id}");
        assert!(!reason.is_empty(), "{id} needs a reason");
    }
}

/// README step 6: every configuration loads and validates, every rule is registered and every `detail` is a state
/// of the ADR 0009 table.
#[test]
fn every_fixture_is_well_formed() {
    let adr = std::fs::read_to_string(fixtures_root().join("../../adrs/0009-version-and-catalog-policies.md")).unwrap();
    let registry = Registry::new(skif_rules::providers()).unwrap();
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
            if row["rule"].as_str().is_none_or(|rule| registry.get(rule).is_none()) {
                problems.push(format!("{}: unregistered rule {}", case.id(), row["rule"]));
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
    assert_eq!(diff_rows(&expected, &expected), [] as [String; 0]);
}
