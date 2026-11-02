use skif_core::{Category, DependencyKind, Fix, FixSafety, Fixability, JsonAction, RuleMeta, Severity, diagnostic};
use skif_lint::{Emitter, LintContext, Rule};

use super::support::{active_packages, field_location, json_op, key, section};

/// Metadata of `unsorted-dependencies`.
pub static META: RuleMeta = RuleMeta {
    id: "unsorted-dependencies",
    category: Category::Manifest,
    default: Some(Severity::Error),
    fixable: Fixability::Safe,
    summary: "The entries of a dependency section are not in alphabetical (byte) order.",
};

/// Reports dependency sections whose keys are not sorted.
#[derive(Debug, Clone, Copy, Default)]
pub struct UnsortedDependencies;

impl Rule for UnsortedDependencies {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn check(&self, cx: &LintContext<'_>, out: &mut Emitter<'_>) {
        for package in active_packages(cx.workspace) {
            for kind in DependencyKind::ALL {
                let Some(map) = package.dependency_map(kind) else {
                    continue;
                };
                let keys: Vec<&str> = map.keys().map(String::as_str).collect();
                let mut sorted = keys.clone();
                sorted.sort_unstable();
                let Some(first) = keys
                    .iter()
                    .zip(&sorted)
                    .position(|(actual, expected)| actual != expected)
                else {
                    continue;
                };
                let field = kind.field();
                let location = field_location(package, &[key(field), key(keys[first])]);
                out.emit(
                    diagnostic(&META, format!("`{field}` is not sorted alphabetically"), location)
                        .package(package.display_name())
                        .package_id(package.id)
                        .dependency(keys[first])
                        .help(format!(
                            "sort the keys of `{field}`; `{}` should come before `{}`",
                            sorted[first], keys[first]
                        ))
                        .fix(Fix {
                            title: format!("Sort `{field}`"),
                            safety: FixSafety::Safe,
                            ops: vec![json_op(package, section(kind), JsonAction::SortKeys)],
                        })
                        .build(),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{self, assert_fixes_clear, config, lint, located, root, subjects};

    #[test]
    fn ordering_table() {
        let root_json = root("");
        let cases = [
            ("sorted", r#"{"name":"a","dependencies":{"a":"1","b":"1","c":"1"}}"#, 0),
            ("unsorted", r#"{"name":"a","dependencies":{"b":"1","a":"1"}}"#, 1),
            ("empty", r#"{"name":"a","dependencies":{}}"#, 0),
            ("single", r#"{"name":"a","dependencies":{"a":"1"}}"#, 0),
            (
                "scoped sorts before plain",
                r#"{"name":"a","dependencies":{"@b/x":"1","a":"1"}}"#,
                0,
            ),
            (
                "plain before scoped",
                r#"{"name":"a","dependencies":{"a":"1","@b/x":"1"}}"#,
                1,
            ),
            (
                "uppercase before lowercase",
                r#"{"name":"a","dependencies":{"B":"1","a":"1"}}"#,
                0,
            ),
            (
                "lowercase before uppercase",
                r#"{"name":"a","dependencies":{"a":"1","B":"1"}}"#,
                1,
            ),
            ("unicode", r#"{"name":"a","dependencies":{"é":"1","z":"1"}}"#, 1),
            (
                "two sections",
                r#"{"name":"a","dependencies":{"b":"1","a":"1"},"devDependencies":{"d":"1","c":"1"}}"#,
                2,
            ),
            ("not an object", r#"{"name":"a","dependencies":["b","a"]}"#, 0),
        ];
        for (name, member, expected) in cases {
            let files = [
                ("package.json", root_json.as_str()),
                ("packages/a/package.json", member),
            ];
            assert_eq!(lint(META.id, &files, &config("")).len(), expected, "{name}");
        }
    }

    #[test]
    fn reports_the_first_misplaced_entry() {
        let root_json = root("");
        let member = r#"{"name":"a","dependencies":{"a":"1","c":"1","b":"1"}}"#;
        let files = [
            ("package.json", root_json.as_str()),
            ("packages/a/package.json", member),
        ];
        let diagnostics = lint(META.id, &files, &config(""));
        assert_eq!(subjects(&diagnostics), [("a".to_owned(), Some("c".to_owned()))]);
        assert_eq!(located(&files, &diagnostics[0]), r#""c":"1""#);
    }

    #[test]
    fn the_root_is_checked() {
        let root_json = root(r#","devDependencies":{"b":"1","a":"1"}"#);
        let files = [
            ("package.json", root_json.as_str()),
            ("packages/a/package.json", r#"{"name":"a"}"#),
        ];
        let diagnostics = lint(META.id, &files, &config(""));
        assert_eq!(diagnostics[0].package.as_deref(), Some("root"));
        assert_eq!(diagnostics[0].location.path.to_str(), Some("package.json"));
    }

    #[test]
    fn fix_is_a_single_sort_keys_operation() {
        let root_json = root("");
        let member = r#"{"name":"a","dependencies":{"b":"^1","a":"catalog:","c":"workspace:*"}}"#;
        let files = [
            ("package.json", root_json.as_str()),
            ("packages/a/package.json", member),
        ];
        let diagnostics = lint(META.id, &files, &config(""));
        let ops = &diagnostics[0].fixes[0].ops;
        assert_eq!(ops.len(), 1);
        assert!(matches!(
            &ops[0],
            skif_core::FixOp::Json { action: JsonAction::SortKeys, pointer, .. } if pointer == &[key("dependencies")]
        ));
        assert_fixes_clear(META.id, &files, &config(""));
        let fixed = testing::apply_fixes(&files, &diagnostics);
        let (_, text) = fixed
            .iter()
            .find(|(path, _)| path == "packages/a/package.json")
            .unwrap();
        assert_eq!(
            text,
            r#"{"name":"a","dependencies":{"a":"catalog:","b":"^1","c":"workspace:*"}}"#
        );
    }

    #[test]
    fn fixes_every_section_of_every_package() {
        let root_json = root(r#","devDependencies":{"z":"1","y":"1"}"#);
        let member = r#"{"name":"a","dependencies":{"b":"1","a":"1"},"optionalDependencies":{"q":"1","p":"1"}}"#;
        let files = [
            ("package.json", root_json.as_str()),
            ("packages/a/package.json", member),
        ];
        assert_fixes_clear(META.id, &files, &config(""));
    }

    /// Rules plan their fixes independently, so a host that applies two overlapping fixes in one run skips the later
    /// one. Sorting is reported again by the next run, which is what makes the sequence converge.
    #[test]
    fn a_section_edited_by_another_rule_is_sorted_by_the_next_run() {
        let root_json = root("");
        let member = r#"{"name":"a","devDependencies":{"z":"1","@scope/b":"link:../b"}}"#;
        let b = r#"{"name":"@scope/b","version":"1.0.0"}"#;
        let files = [
            ("package.json", root_json.as_str()),
            ("packages/a/package.json", member),
            ("packages/b/package.json", b),
        ];
        let linked = lint("local-path-dependency", &files, &config(""));
        let first_run = testing::apply_fixes(&files, &linked);
        let after_first_run = testing::borrowed(&first_run);
        let unsorted = lint(META.id, &after_first_run, &config(""));
        assert_eq!(unsorted.len(), 1, "the section is still unsorted after the other rule's fix");
        let second_run = testing::apply_fixes(&after_first_run, &unsorted);
        assert_eq!(lint(META.id, &testing::borrowed(&second_run), &config("")), [] as [skif_core::Diagnostic; 0]);
    }

    #[test]
    fn ignored_packages_are_skipped() {
        let root_json = root("");
        let member = r#"{"name":"a","dependencies":{"b":"1","a":"1"}}"#;
        let files = [
            ("package.json", root_json.as_str()),
            ("packages/a/package.json", member),
        ];
        assert_eq!(
            lint(META.id, &files, &config("[workspace]\nignore-packages = [\"a\"]\n")),
            [] as [skif_core::Diagnostic; 0]
        );
    }
}
