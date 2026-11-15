//! The untyped pass of the parity harness: skif's findings with `--type-aware` (the run compared with knip) against
//! the same run with `--no-type-aware`, per rule (ADR 0021).
//!
//! A finding only one mode reports is set apart from the knip comparison as the `type-resolved` deviation
//! ([`type_resolved`]). The counts per rule are checked against their own baseline, whose comparison tool is skif
//! without types: an entry's `skif_only` counts the findings only the typed run reports, `tool_only` those only the
//! untyped run reports and `both` those of both runs.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use serde_json::{Value, json};

use crate::diff::{BASELINE_SCHEMA, Baseline, COMPARED, Counts, Entry, Keys, Samples, Side, TYPE_AWARE};
use crate::parity::normalize;
use crate::run::table_cell;

/// The skif flag of the typed run.
pub const TYPE_AWARE_FLAG: &str = "--type-aware";
/// The typed run, as the bench rows name it.
pub const TYPED_TOOL: &str = "skif --type-aware";
/// The skif flag of the untyped run.
pub const NO_TYPE_AWARE_FLAG: &str = "--no-type-aware";
/// The comparison tool of the type-aware baseline.
pub const TOOL: &str = "skif --no-type-aware";

/// The findings of a skif JSON document by rule, each keyed `<file>:<line>|<message>` (`<file>|<message>` without a
/// span), so findings of every rule are keyed alike in both modes.
pub fn rule_keys(document: &Value, root: &Path) -> Keys {
    let mut keys = Keys::new();
    let diagnostics = document
        .get("diagnostics")
        .and_then(Value::as_array)
        .into_iter()
        .flatten();
    for diagnostic in diagnostics {
        let Some(rule) = diagnostic.get("rule").and_then(Value::as_str) else {
            continue;
        };
        let path = normalize(
            diagnostic
                .pointer("/location/path")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            root,
        );
        let message = diagnostic.get("message").and_then(Value::as_str).unwrap_or_default();
        let key = match diagnostic.pointer("/location/span/start-line").and_then(Value::as_u64) {
            Some(line) => format!("{path}:{line}|{message}"),
            None => format!("{path}|{message}"),
        };
        keys.entry(rule.to_owned()).or_default().insert(key);
    }
    keys
}

/// Whether a finding of the knip comparison that only one side reports is one the untyped run settles the other
/// way: a finding of the typed run (`Side::Skif`) that the untyped run does not report, or a finding of knip's
/// (`Side::Tool`) that the untyped run reports and the typed run does not. `untyped` holds the untyped run's findings
/// by knip category, aligned with knip's as the typed run's are.
pub fn type_resolved(side: Side, category: &str, key: &str, untyped: &Keys) -> bool {
    let untyped_reports = untyped.get(category).is_some_and(|keys| keys.contains(key));
    match side {
        Side::Skif => !untyped_reports,
        Side::Tool => untyped_reports,
    }
}

/// The per-rule detail of one repository for the full report: each mode's count, the findings only one mode has and a
/// few of their keys.
pub fn rule_detail(counts: &BTreeMap<String, Counts>, samples: &BTreeMap<String, Samples>) -> Value {
    Value::Object(
        counts
            .iter()
            .map(|(rule, found)| {
                (
                    rule.clone(),
                    json!({
                        "typed": found.skif_only + found.both,
                        "untyped": found.tool_only + found.both,
                        "typed_only": found.skif_only,
                        "untyped_only": found.tool_only,
                        "samples": samples.get(rule).map(|s| json!({ "typed_only": s.skif_only, "untyped_only": s.tool_only })),
                    }),
                )
            })
            .collect(),
    )
}

/// A markdown section with each repository's typed and untyped counts per rule; empty without entries (the untyped
/// pass was skipped).
pub fn markdown(entries: &BTreeMap<String, Entry>) -> String {
    if entries.is_empty() {
        return String::new();
    }
    let mut out = format!(
        "\n## Typed versus untyped\n\nskif with `{TYPE_AWARE_FLAG}` (the run compared above) and with `{NO_TYPE_AWARE_FLAG}`, per rule. Findings only one mode reports that knip does not share are counted under `type-resolved` above; `--check` checks these counts against `type-aware.json`.\n\n| Repository | Rule | Typed | Untyped | Typed only | Untyped only |\n| --- | --- | ---: | ---: | ---: | ---: |\n"
    );
    for (name, entry) in entries {
        if entry.status != COMPARED {
            let _ = writeln!(
                out,
                "| {} | skipped: {} | - | - | - | - |",
                table_cell(name),
                table_cell(entry.reason.as_deref().unwrap_or(""))
            );
            continue;
        }
        if entry.categories.is_empty() {
            let _ = writeln!(out, "| {} | no findings | 0 | 0 | 0 | 0 |", table_cell(name));
        }
        for (rule, found) in &entry.categories {
            let _ = writeln!(
                out,
                "| {} | {} | {} | {} | {} | {} |",
                table_cell(name),
                table_cell(rule),
                found.skif_only + found.both,
                found.tool_only + found.both,
                found.skif_only,
                found.tool_only
            );
        }
    }
    out
}

/// The type-aware baseline of a run from its entries.
pub fn baseline(entries: BTreeMap<String, Entry>) -> Baseline {
    Baseline {
        schema: BASELINE_SCHEMA,
        tool: TOOL.to_owned(),
        mode: Some(TYPE_AWARE.to_owned()),
        installed: entries.values().all(|entry| entry.installed != Some(false)),
        category_set: None,
        entries,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::diff;

    fn keys(items: &[(&str, &[&str])]) -> Keys {
        items
            .iter()
            .map(|(category, names)| ((*category).to_owned(), names.iter().map(|n| (*n).to_owned()).collect()))
            .collect()
    }

    #[test]
    fn rule_keys_key_every_rule_by_file_line_and_message() {
        let doc = json!({ "diagnostics": [
            { "rule": "unused-class-members", "message": "`m` of `C` is never used",
              "location": { "path": "/r/src/c.ts", "span": { "start": 10, "end": 11, "start-line": 3, "start-col": 2, "end-line": 3, "end-col": 3 } } },
            { "rule": "unused-class-members", "message": "`n` of `C` is never used",
              "location": { "path": "src/c.ts", "span": { "start": 20, "end": 21, "start-line": 4, "start-col": 2, "end-line": 4, "end-col": 3 } } },
            { "rule": "unused-dependencies", "message": "`left-pad` is never used", "location": { "path": "package.json" } },
            { "message": "no rule", "location": { "path": "x.ts" } },
        ]});
        assert_eq!(
            rule_keys(&doc, Path::new("/r")),
            keys(&[
                (
                    "unused-class-members",
                    &[
                        "src/c.ts:3|`m` of `C` is never used",
                        "src/c.ts:4|`n` of `C` is never used"
                    ]
                ),
                ("unused-dependencies", &["package.json|`left-pad` is never used"]),
            ])
        );
        assert_eq!(rule_keys(&json!({ "error": {} }), Path::new("/r")), Keys::new());
    }

    #[test]
    fn a_one_sided_finding_is_type_resolved_only_when_the_untyped_run_settles_it_the_other_way() {
        let untyped = keys(&[("enumMembers", &["e.ts|E.A", "e.ts|E.B"])]);
        let cases = [
            (Side::Skif, "e.ts|E.C", true, "only the typed run reports it"),
            (
                Side::Skif,
                "e.ts|E.A",
                false,
                "both runs report it: knip disagrees with either",
            ),
            (Side::Tool, "e.ts|E.B", true, "the untyped run agrees with knip"),
            (Side::Tool, "e.ts|E.D", false, "neither run reports it"),
        ];
        for (side, key, expected, why) in cases {
            assert_eq!(
                type_resolved(side, "enumMembers", key, &untyped),
                expected,
                "{key}: {why}"
            );
        }
        assert!(
            !type_resolved(Side::Tool, "exports", "e.ts|E.A", &untyped),
            "another category is another finding"
        );
    }

    #[test]
    fn the_rule_detail_gives_each_mode_its_count() {
        let typed = keys(&[("unused-class-members", &["a", "b", "c"]), ("unused-files", &["f"])]);
        let untyped = keys(&[("unused-class-members", &["a", "d"]), ("unused-files", &["f"])]);
        let (counts, samples) = diff::compare(&typed, &untyped);
        assert_eq!(
            rule_detail(&counts, &samples),
            json!({
                "unused-class-members": {
                    "typed": 3, "untyped": 2, "typed_only": 2, "untyped_only": 1,
                    "samples": { "typed_only": ["b", "c"], "untyped_only": ["d"] },
                },
                "unused-files": {
                    "typed": 1, "untyped": 1, "typed_only": 0, "untyped_only": 0,
                    "samples": { "typed_only": [], "untyped_only": [] },
                },
            })
        );
    }

    #[test]
    fn the_markdown_section_lists_both_counts_per_rule_and_is_empty_without_the_pass() {
        assert_eq!(markdown(&BTreeMap::new()), "");
        let counts = BTreeMap::from([(
            "unused-enum-members".to_owned(),
            Counts {
                skif_only: 1,
                tool_only: 4,
                both: 10,
            },
        )]);
        let entries = BTreeMap::from([
            ("a".to_owned(), Entry::compared(counts)),
            ("b".to_owned(), Entry::compared(BTreeMap::new())),
            ("c".to_owned(), Entry::skipped("skif --no-type-aware timed out")),
        ]);
        let section = markdown(&entries);
        assert!(section.starts_with("\n## Typed versus untyped\n"), "{section}");
        assert!(
            section.contains("| a | unused-enum-members | 11 | 14 | 1 | 4 |\n"),
            "{section}"
        );
        assert!(section.contains("| b | no findings | 0 | 0 | 0 | 0 |\n"), "{section}");
        assert!(
            section.contains("| c | skipped: skif --no-type-aware timed out | - | - | - | - |\n"),
            "{section}"
        );
    }

    #[test]
    fn the_baseline_names_the_untyped_run_as_its_tool_and_records_the_mode() {
        let entries = BTreeMap::from([
            ("a".to_owned(), Entry::compared(BTreeMap::new()).with_installed(true)),
            ("b".to_owned(), Entry::compared(BTreeMap::new()).with_installed(false)),
        ]);
        let recorded = baseline(entries);
        assert_eq!(recorded.tool, "skif --no-type-aware");
        assert_eq!(recorded.mode.as_deref(), Some("type-aware"));
        assert!(!recorded.installed, "not every entry is installed");
        assert!(recorded.category_set.is_none());
    }
}
