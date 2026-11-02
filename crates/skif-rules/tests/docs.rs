//! Keeps `docs/rules.md` in sync with the built-in rule metadata.
//!
//! Run with `SKIF_UPDATE_DOCS=1` to regenerate the file instead of failing.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fmt::Write as _;
use std::path::PathBuf;

use serde::Serialize;
use skif_core::RuleMeta;

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
