//! ADR 0007: released rule IDs are permanent, retired IDs stay reserved.
//!
//! `rule-ids.txt` lists every released ID and `retired-rule-ids.txt` the IDs that were renamed. A rule that
//! disappears from the registry, or a new rule missing from the list, fails here.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeSet;

use skif_lint::Registry;

fn listed(text: &'static str) -> Vec<&'static str> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect()
}

fn registered() -> BTreeSet<&'static str> {
    Registry::new(skif_rules::providers())
        .unwrap()
        .rules()
        .map(|meta| meta.id)
        .collect()
}

#[test]
fn every_released_rule_id_is_still_registered() {
    let registered = registered();
    let missing: Vec<&str> = listed(include_str!("rule-ids.txt"))
        .into_iter()
        .filter(|id| !registered.contains(id))
        .collect();
    assert!(missing.is_empty(), "released rule IDs disappeared: {missing:?}");
}

#[test]
fn every_registered_rule_id_is_listed_as_released() {
    let released: BTreeSet<&str> = listed(include_str!("rule-ids.txt")).into_iter().collect();
    let unlisted: Vec<&str> = registered().into_iter().filter(|id| !released.contains(id)).collect();
    assert!(
        unlisted.is_empty(),
        "add the new rule IDs to tests/rule-ids.txt: {unlisted:?}"
    );
}

#[test]
fn rule_ids_are_listed_once() {
    let ids = listed(include_str!("rule-ids.txt"));
    let unique: BTreeSet<&str> = ids.iter().copied().collect();
    assert_eq!(unique.len(), ids.len(), "duplicate entry in rule-ids.txt");
}

#[test]
fn retired_ids_are_gone_and_point_at_a_registered_replacement() {
    let registered = registered();
    for line in listed(include_str!("retired-rule-ids.txt")) {
        let (retired, replacement) = line.split_once(' ').expect("`<retired> <replacement>`");
        assert!(!registered.contains(retired), "{retired} is retired but registered");
        assert!(
            registered.contains(replacement),
            "{retired} points at {replacement}, which is not registered"
        );
    }
}
