//! Conformance of the catalog, workspace-protocol and dependency-family rules with `fixtures/policy`.
//!
//! The harness is in `common` and follows `fixtures/policy/README.md`; this file selects the groups `catalog`,
//! `workspace-protocol` and `dependency-families` and the rules of the catalog provider.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{CATALOG_GROUPS, discover_cases, rule_ids, run_groups};

#[test]
fn catalog_workspace_protocol_and_family_fixtures_conform() {
    run_groups(CATALOG_GROUPS, &rule_ids(&skif_rules::catalog_provider()));
}

#[test]
fn the_catalog_provider_provides_the_rules_of_its_groups() {
    let rules = rule_ids(&skif_rules::catalog_provider());
    for case in discover_cases()
        .into_iter()
        .filter(|case| CATALOG_GROUPS.contains(&case.group.as_str()))
    {
        let expected: Vec<serde_json::Value> =
            serde_json::from_str(&std::fs::read_to_string(case.dir.join("expected.json")).unwrap()).unwrap();
        if !case.is_partial() {
            for row in &expected {
                let rule = row["rule"].as_str().unwrap();
                assert!(rules.contains(rule), "{}: {rule} is not a catalog rule", case.id());
            }
        }
    }
}
