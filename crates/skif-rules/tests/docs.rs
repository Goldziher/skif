//! Keeps `docs/rules.md` in sync with the built-in rule metadata, the sherif names of the renamed rules
//! (`retired-rule-ids.txt`) and the policy states skif does not emit yet.
//!
//! Run with `SKIF_UPDATE_DOCS=1` to regenerate the file instead of failing.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fmt::Write as _;
use std::path::PathBuf;

use serde::Serialize;
use skif_core::RuleMeta;

/// Policy states of ADR 0009 that no rule emits yet, with the reason.
const UNIMPLEMENTED_STATES: [(&str, &str); 3] = [
    (
        "DiffersToHighestOrLowestSemverMinor",
        "a `same-minor` group cannot name a direction, see ADR 0009",
    ),
    (
        "DiffersToNpmRegistry",
        "registry updates are out of scope, see ADR 0001",
    ),
    (
        "RefuseToCatalogLocal",
        "a workspace package is never moved into a catalog",
    ),
];

/// `(sherif ID, skif ID)` of every renamed rule, from `retired-rule-ids.txt`.
fn renamed() -> Vec<(&'static str, &'static str)> {
    include_str!("retired-rule-ids.txt")
        .lines()
        .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
        .filter_map(|line| line.split_once(' '))
        .collect()
}

fn name(value: &impl Serialize) -> String {
    serde_json::to_value(value)
        .unwrap()
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

fn severity(meta: &RuleMeta) -> String {
    meta.default.map_or_else(|| "off".to_owned(), |s| name(&s))
}

fn metas() -> Vec<&'static RuleMeta> {
    let mut metas: Vec<_> = skif_rules::providers().iter().flat_map(|p| p.rules()).collect();
    metas.sort_by_key(|meta| (name(&meta.category), meta.id));
    metas
}

fn render(metas: &[&RuleMeta]) -> String {
    let mut out = String::from(
        "<!-- Generated from the rule metadata; run `SKIF_UPDATE_DOCS=1 cargo test -p skif-rules --test docs` to update. -->\n\n# Rules\n\n| Rule | Category | Default severity | Fixable |\n| --- | --- | --- | --- |\n",
    );
    for meta in metas {
        let _ = writeln!(
            out,
            "| [`{id}`](#{id}) | {} | {} | {} |",
            name(&meta.category),
            severity(meta),
            name(&meta.fixable),
            id = meta.id,
        );
    }
    let renamed = renamed();
    for meta in metas {
        let _ = write!(
            out,
            "\n## {id}\n\n{}\n\n- Category: {}\n- Default severity: {}\n- Fixable: {}\n",
            meta.summary,
            name(&meta.category),
            severity(meta),
            name(&meta.fixable),
            id = meta.id,
        );
        if let Some((sherif, _)) = renamed.iter().find(|(_, skif)| *skif == meta.id) {
            let _ = writeln!(out, "- sherif name: `{sherif}`");
        }
    }
    out.push_str("\n## Unimplemented states\n\nThese policy states of ADR 0009 are not emitted yet.\n\n");
    for (state, reason) in UNIMPLEMENTED_STATES {
        let _ = writeln!(out, "- `{state}`: {reason}");
    }
    out
}

#[test]
fn rules_reference_is_up_to_date() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/rules.md");
    let expected = render(&metas());
    if std::env::var_os("SKIF_UPDATE_DOCS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, expected).unwrap();
        return;
    }
    let actual = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        actual == expected,
        "docs/rules.md is stale or missing; regenerate with SKIF_UPDATE_DOCS=1 cargo test -p skif-rules --test docs"
    );
}
