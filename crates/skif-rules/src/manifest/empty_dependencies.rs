use serde_json::Map;
use skif_core::{Category, DependencyKind, Fix, FixSafety, Fixability, JsonAction, RuleMeta, Severity, diagnostic};
use skif_lint::{Emitter, LintContext, Rule};

use super::support::{active_packages, field_location, json_op, section};

/// Metadata of `empty-dependencies`.
pub static META: RuleMeta = RuleMeta {
    id: "empty-dependencies",
    category: Category::Manifest,
    default: Some(Severity::Error),
    fixable: Fixability::Safe,
    summary: "A dependency section (`dependencies`, `devDependencies`, `peerDependencies`, `optionalDependencies`) is an empty object.",
};

/// Reports dependency sections that are present but empty, in the root and in every member.
#[derive(Debug, Clone, Copy, Default)]
pub struct EmptyDependencies;

impl Rule for EmptyDependencies {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn check(&self, cx: &LintContext<'_>, out: &mut Emitter<'_>) {
        for package in active_packages(cx.workspace) {
            for kind in DependencyKind::ALL {
                if !package.dependency_map(kind).is_some_and(Map::is_empty) {
                    continue;
                }
                let field = kind.field();
                out.emit(
                    diagnostic(&META, format!("`{field}` is empty"), field_location(package, &section(kind)))
                        .package(package.display_name())
                        .help(format!("remove the empty `{field}` field"))
                        .fix(Fix {
                            title: format!("Remove the empty `{field}` field"),
                            safety: FixSafety::Safe,
                            ops: vec![json_op(package, section(kind), JsonAction::Remove)],
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

    const A_EMPTY: &str = r#"{"name":"a","dependencies":{},"devDependencies":{},"peerDependencies":{}, "optionalDependencies":{}}"#;
    const A_FILLED: &str = r#"{"name":"a","dependencies":{"x":"1"},"devDependencies":{"y":"1"}}"#;
    const A_ABSENT: &str = r#"{"name":"a"}"#;
    const A_NON_OBJECT: &str = r#"{"name":"a","dependencies":[],"devDependencies":null}"#;

    #[test]
    fn reports_empty_sections_per_package() {
        let root_filled = root("");
        let root_empty = root(r#","dependencies":{}"#);
        let cases: [(&str, &str, &str, usize); 6] = [
            ("all four empty", root_filled.as_str(), A_EMPTY, 4),
            ("filled", root_filled.as_str(), A_FILLED, 0),
            ("absent", root_filled.as_str(), A_ABSENT, 0),
            ("not objects", root_filled.as_str(), A_NON_OBJECT, 0),
            ("empty in the root", root_empty.as_str(), A_ABSENT, 1),
            ("empty in both", root_empty.as_str(), A_EMPTY, 5),
        ];
        for (name, root_json, member, expected) in cases {
            let files = [("package.json", root_json), ("packages/a/package.json", member)];
            assert_eq!(lint(META.id, &files, &config("")).len(), expected, "{name}");
        }
    }

    #[test]
    fn locates_the_entry_and_names_the_package() {
        let root_json = root(r#","devDependencies":{}"#);
        let files = [("package.json", root_json.as_str()), ("packages/a/package.json", A_ABSENT)];
        let diagnostics = lint(META.id, &files, &config(""));
        assert_eq!(subjects(&diagnostics), [("root".to_owned(), None)]);
        assert_eq!(located(&files, &diagnostics[0]), r#""devDependencies":{}"#);
        assert_eq!(diagnostics[0].location.path.to_str(), Some("package.json"));
    }

    #[test]
    fn member_spans_use_the_member_manifest() {
        let root_json = root("");
        let files = [("package.json", root_json.as_str()), ("packages/a/package.json", A_EMPTY)];
        let diagnostics = lint(META.id, &files, &config(""));
        assert!(diagnostics.iter().all(|d| d.location.path.to_str() == Some("packages/a/package.json")));
        assert_eq!(located(&files, &diagnostics[0]), r#""dependencies":{}"#);
    }

    #[test]
    fn unicode_package_names_are_reported_verbatim() {
        let root_json = root("");
        let member = r#"{"name":"ünï-é","dependencies":{}}"#;
        let files = [("package.json", root_json.as_str()), ("packages/a/package.json", member)];
        let diagnostics = lint(META.id, &files, &config(""));
        assert_eq!(diagnostics[0].package.as_deref(), Some("ünï-é"));
        assert_eq!(located(&files, &diagnostics[0]), r#""dependencies":{}"#);
    }

    #[test]
    fn ignored_packages_are_not_reported() {
        let root_json = root("");
        let files = [("package.json", root_json.as_str()), ("packages/a/package.json", A_EMPTY)];
        let ignored = config("[workspace]\nignore-packages = [\"a\"]\n");
        assert!(lint(META.id, &files, &ignored).is_empty());
    }

    #[test]
    fn fix_removes_the_sections_and_clears_the_rule() {
        let root_json = root(r#","dependencies":{}"#);
        let files = [("package.json", root_json.as_str()), ("packages/a/package.json", A_EMPTY)];
        assert_fixes_clear(META.id, &files, &config(""));
        let member = testing::fixed_json(META.id, &files, &config(""), "packages/a/package.json");
        assert_eq!(member, serde_json::json!({"name": "a"}));
        let root_fixed = testing::fixed_json(META.id, &files, &config(""), "package.json");
        assert!(root_fixed.get("dependencies").is_none());
    }
}
