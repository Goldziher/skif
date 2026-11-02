use skif_core::{Category, DependencyKind, Fix, FixSafety, Fixability, Related, RuleMeta, Severity, diagnostic};
use skif_lint::{Emitter, LintContext, Rule};

use super::support::{active_members, entry_location, move_to_dev_dependencies};

/// Metadata of `types-in-dependencies`.
pub static META: RuleMeta = RuleMeta {
    id: "types-in-dependencies",
    category: Category::Manifest,
    default: Some(Severity::Error),
    fixable: Fixability::Unsafe,
    summary: "A private package lists `@types/*` packages in `dependencies`; type packages belong in `devDependencies`.",
};

/// Reports `@types/*` entries in the `dependencies` of private member packages.
#[derive(Debug, Clone, Copy, Default)]
pub struct TypesInDependencies;

impl Rule for TypesInDependencies {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn check(&self, cx: &LintContext<'_>, out: &mut Emitter<'_>) {
        for package in active_members(cx.workspace).filter(|package| package.private) {
            let entries: Vec<_> = package
                .dependencies
                .iter()
                .filter(|entry| entry.kind == DependencyKind::Dependencies && entry.name.starts_with("@types/"))
                .collect();
            let Some((first, rest)) = entries.split_first() else {
                continue;
            };
            let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
            let mut builder = diagnostic(
                &META,
                format!(
                    "private package has {} in `dependencies`",
                    names
                        .iter()
                        .map(|name| format!("`{name}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                entry_location(package, first),
            )
            .package(package.display_name())
            .package_id(package.id)
            .dependency(first.name.clone())
            .help(
                "move `@types/*` packages to `devDependencies`; a private package is never installed as a dependency",
            );
            for entry in rest {
                builder = builder.related(Related {
                    message: format!("`{}` is also a type package in `dependencies`", entry.name),
                    location: entry_location(package, entry),
                });
            }
            if let Some(ops) = move_to_dev_dependencies(package, &names) {
                builder = builder.fix(Fix {
                    title: "Move the `@types/*` packages to `devDependencies`".to_owned(),
                    safety: FixSafety::Unsafe,
                    ops,
                });
            }
            out.emit(builder.build());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{self, assert_fixes_clear, config, lint, located, root, subjects};

    fn files_with<'a>(root_json: &'a str, member: &'a str) -> [(&'a str, &'a str); 2] {
        [("package.json", root_json), ("packages/a/package.json", member)]
    }

    #[test]
    fn detection_table() {
        let root_json = root("");
        let cases = [
            (
                "private with types",
                r#"{"name":"a","private":true,"dependencies":{"@types/node":"1"}}"#,
                1,
            ),
            (
                "public with types",
                r#"{"name":"a","dependencies":{"@types/node":"1"}}"#,
                0,
            ),
            (
                "private in dev",
                r#"{"name":"a","private":true,"devDependencies":{"@types/node":"1"}}"#,
                0,
            ),
            (
                "private in peer",
                r#"{"name":"a","private":true,"peerDependencies":{"@types/node":"1"}}"#,
                0,
            ),
            (
                "private without types",
                r#"{"name":"a","private":true,"dependencies":{"x":"1"}}"#,
                0,
            ),
            (
                "lookalike scope",
                r#"{"name":"a","private":true,"dependencies":{"@typesafe/x":"1","types":"1"}}"#,
                0,
            ),
            (
                "private false",
                r#"{"name":"a","private":false,"dependencies":{"@types/node":"1"}}"#,
                0,
            ),
            (
                "several stay one diagnostic",
                r#"{"name":"a","private":true,"dependencies":{"@types/a":"1","@types/b":"2"}}"#,
                1,
            ),
            ("empty", r#"{"name":"a","private":true,"dependencies":{}}"#, 0),
        ];
        for (name, member, expected) in cases {
            let files = files_with(&root_json, member);
            assert_eq!(lint(META.id, &files, &config("")).len(), expected, "{name}");
        }
    }

    #[test]
    fn the_root_is_not_checked_by_this_rule() {
        let root_json = root(r#","dependencies":{"@types/node":"1"}"#);
        let files = files_with(&root_json, r#"{"name":"a"}"#);
        assert_eq!(lint(META.id, &files, &config("")), [] as [skif_core::Diagnostic; 0]);
    }

    #[test]
    fn locates_the_first_entry_and_relates_the_others() {
        let root_json = root("");
        let member = r#"{"name":"a","private":true,"dependencies":{"@types/a":"1","x":"1","@types/b":"2"}}"#;
        let files = files_with(&root_json, member);
        let diagnostics = lint(META.id, &files, &config(""));
        assert_eq!(subjects(&diagnostics), [("a".to_owned(), Some("@types/a".to_owned()))]);
        assert_eq!(located(&files, &diagnostics[0]), r#""@types/a":"1""#);
        let related = &diagnostics[0].related;
        assert_eq!(related.len(), 1);
        assert_eq!(
            testing::slice(&files, "packages/a/package.json", related[0].location.span.unwrap()),
            r#""@types/b":"2""#
        );
    }

    #[test]
    fn unnamed_members_are_reported_by_directory() {
        let root_json = root("");
        let member = r#"{"private":true,"dependencies":{"@types/a":"1"}}"#;
        let files = files_with(&root_json, member);
        let diagnostics = lint(META.id, &files, &config(""));
        assert_eq!(diagnostics[0].package.as_deref(), Some("packages/a"));
    }

    #[test]
    fn ignored_packages_are_skipped() {
        let root_json = root("");
        let member = r#"{"name":"a","private":true,"dependencies":{"@types/a":"1"}}"#;
        let files = files_with(&root_json, member);
        assert_eq!(
            lint(
                META.id,
                &files,
                &config("[workspace]\nignore-packages = [\"packages/*\"]\n")
            ),
            [] as [skif_core::Diagnostic; 0]
        );
        let by_rule = config("[[ignore]]\nrules = [\"types-in-dependencies\"]\npackages = [\"a\"]\n");
        assert_eq!(lint(META.id, &files, &by_rule), [] as [skif_core::Diagnostic; 0]);
    }

    #[test]
    fn the_fix_is_unsafe_because_it_changes_what_gets_installed() {
        let root_json = root("");
        let member = r#"{"name":"a","private":true,"dependencies":{"@types/node":"1"}}"#;
        let files = files_with(&root_json, member);
        let diagnostics = lint(META.id, &files, &config(""));
        assert_eq!(diagnostics[0].fixes[0].safety, FixSafety::Unsafe);
    }

    #[test]
    fn fix_moves_only_the_type_packages_and_keeps_specifiers() {
        let root_json = root("");
        let cases = [
            (
                "no dev section, subset",
                r#"{"name":"a","private":true,"dependencies":{"@types/node":"~1.2","x":"1"}}"#,
                serde_json::json!({"name":"a","private":true,"dependencies":{"x":"1"},"devDependencies":{"@types/node":"~1.2"}}),
            ),
            (
                "no dev section, all",
                r#"{"name":"a","private":true,"dependencies":{"@types/node":"1.2"}}"#,
                serde_json::json!({"name":"a","private":true,"devDependencies":{"@types/node":"1.2"}}),
            ),
            (
                "existing dev section",
                r#"{"name":"a","private":true,"dependencies":{"@types/node":"catalog:","x":"1"},"devDependencies":{"a":"1","z":"1"}}"#,
                serde_json::json!({"name":"a","private":true,"dependencies":{"x":"1"},"devDependencies":{"a":"1","@types/node":"catalog:","z":"1"}}),
            ),
        ];
        for (name, member, expected) in cases {
            let files = files_with(&root_json, member);
            assert_fixes_clear(META.id, &files, &config(""));
            let fixed = testing::fixed_json(META.id, &files, &config(""), "packages/a/package.json");
            assert_eq!(fixed, expected, "{name}");
        }
    }
}
