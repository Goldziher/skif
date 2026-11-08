//! Keeps `docs/rules.md` in sync with the metadata of every built-in rule (manifest, policy, catalog and JS), the
//! sherif names of the renamed rules (`crates/skif-rules/tests/retired-rule-ids.txt`) and the policy states skif does
//! not emit yet.
//!
//! Run with `SKIF_UPDATE_DOCS=1` to regenerate the file instead of failing.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fmt::Write as _;
use std::path::PathBuf;

use skif::{Config, Linter, RuleMeta};

/// Policy states of ADR 0009 that no rule emits yet, with the reason.
const UNIMPLEMENTED_STATES: [(&str, &str); 3] = [
    (
        "DiffersToHighestOrLowestSemverMinor",
        "a `same-minor` group cannot name a direction, see ADR 0009",
    ),
    (
        "DiffersToNpmRegistry",
        "reported by `skif update`, never by a lint rule, see ADR 0058",
    ),
    (
        "RefuseToCatalogLocal",
        "a workspace package is never moved into a catalog",
    ),
];

fn repo_file(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").join(relative)
}

/// `(sherif ID, skif ID)` of every renamed rule, from `retired-rule-ids.txt`.
fn renamed() -> Vec<(String, String)> {
    let text = std::fs::read_to_string(repo_file("crates/skif-rules/tests/retired-rule-ids.txt")).unwrap();
    text.lines()
        .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
        .filter_map(|line| line.split_once(' '))
        .map(|(sherif, skif)| (sherif.to_owned(), skif.to_owned()))
        .collect()
}

fn text(value: serde_json::Result<serde_json::Value>) -> String {
    value.unwrap().as_str().unwrap_or_default().to_owned()
}

fn category(meta: &RuleMeta) -> String {
    text(serde_json::to_value(meta.category))
}

fn fixable(meta: &RuleMeta) -> String {
    text(serde_json::to_value(meta.fixable))
}

fn severity(meta: &RuleMeta) -> String {
    meta.default
        .map_or_else(|| "off".to_owned(), |s| text(serde_json::to_value(s)))
}

fn metas(linter: &Linter) -> Vec<&'static RuleMeta> {
    let mut metas: Vec<_> = linter.rules().collect();
    metas.sort_by_key(|meta| (category(meta), meta.id));
    metas
}

fn render(metas: &[&RuleMeta]) -> String {
    let mut out = String::from(
        "<!-- Generated from the rule metadata; run `SKIF_UPDATE_DOCS=1 cargo test -p skif --features js-analysis --test docs` to update. -->\n\n# Rules\n\n| Rule | Category | Default severity | Fixable |\n| --- | --- | --- | --- |\n",
    );
    for meta in metas {
        let _ = writeln!(
            out,
            "| [`{id}`](#{id}) | {} | {} | {} |",
            category(meta),
            severity(meta),
            fixable(meta),
            id = meta.id,
        );
    }
    let renamed = renamed();
    for meta in metas {
        let _ = write!(
            out,
            "\n## {id}\n\n{}\n\n- Category: {}\n- Default severity: {}\n- Fixable: {}\n",
            meta.summary,
            category(meta),
            severity(meta),
            fixable(meta),
            id = meta.id,
        );
        if let Some((sherif, _)) = renamed.iter().find(|(_, skif)| skif == meta.id) {
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
    let path = repo_file("docs/rules.md");
    let linter = Linter::new(&Config::default()).unwrap();
    let expected = render(&metas(&linter));
    if std::env::var_os("SKIF_UPDATE_DOCS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, expected).unwrap();
        return;
    }
    let actual = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        actual == expected,
        "docs/rules.md is stale or missing; regenerate with SKIF_UPDATE_DOCS=1 cargo test -p skif --features js-analysis --test docs"
    );
}

#[test]
fn every_documented_rule_is_listed_in_rule_ids() {
    let linter = Linter::new(&Config::default()).unwrap();
    let mut registered: Vec<&str> = linter.rules().map(|meta| meta.id).collect();
    registered.sort_unstable();
    assert!(
        registered.len() > 30,
        "the registry holds only {} rules",
        registered.len()
    );
    let recorded = std::fs::read_to_string(repo_file("rule-ids.txt")).unwrap();
    let mut recorded: Vec<&str> = recorded.lines().filter(|line| !line.trim().is_empty()).collect();
    recorded.sort_unstable();
    assert_eq!(registered, recorded, "rule-ids.txt must list every registered rule");
}
