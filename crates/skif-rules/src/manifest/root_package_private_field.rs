use serde_json::Value;
use skif_core::{Category, Fix, FixSafety, Fixability, InsertPosition, JsonAction, RuleMeta, Severity, diagnostic};
use skif_lint::{Emitter, LintContext, Rule};

use super::support::{document_location, field_location, json_op, key};

/// Metadata of `root-package-private-field`.
pub static META: RuleMeta = RuleMeta::new(
    "root-package-private-field",
    Category::Manifest,
    Some(Severity::Error),
    Fixability::Safe,
    "The root `package.json` must set `\"private\": true` so it cannot be published by accident.",
);

/// Reports a root manifest whose `private` field is missing or not `true`.
#[derive(Debug, Clone, Copy, Default)]
pub struct RootPackagePrivateField;

impl Rule for RootPackagePrivateField {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn check(&self, cx: &LintContext<'_>, out: &mut Emitter<'_>) {
        // A single package is not a workspace root; the monorepo root conventions don't apply.
        if cx.workspace.facts.source == skif_workspace::WorkspaceSource::SinglePackage {
            return;
        }
        let root = cx.workspace.root_package();
        if cx.workspace.is_ignored(root.id) {
            return;
        }
        let current = root.manifest.value().get("private");
        if current == Some(&Value::Bool(true)) {
            return;
        }
        let (message, location) = match current {
            None => (
                "the root package is not marked private".to_owned(),
                document_location(root),
            ),
            Some(_) => (
                "`private` of the root package is not `true`".to_owned(),
                field_location(root, &[key("private")]),
            ),
        };
        out.emit(
            diagnostic(&META, message, location)
                .package(root.display_name())
                .package_id(root.id)
                .help("set `\"private\": true` in the root package.json to prevent publishing it to a registry")
                .fix(Fix::new(
                    "Set `private` to `true`".to_owned(),
                    FixSafety::Safe,
                    vec![json_op(
                        root,
                        Vec::new(),
                        JsonAction::insert("private".to_owned(), Value::Bool(true), InsertPosition::End),
                    )],
                ))
                .build(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{self, assert_fixes_clear, config, lint, located};

    fn root_json(private: &str) -> String {
        format!(r#"{{"name":"root"{private},"packageManager":"pnpm@9.0.0","workspaces":["packages/*"]}}"#)
    }

    const MEMBER: &str = r#"{"name":"a"}"#;

    #[test]
    fn reports_missing_and_non_true_private() {
        let cases = [
            ("true", r#","private":true"#, false),
            ("missing", "", true),
            ("false", r#","private":false"#, true),
            ("string", r#","private":"true""#, true),
            ("null", r#","private":null"#, true),
        ];
        for (name, private, expected) in cases {
            let json = root_json(private);
            let files = [("package.json", json.as_str()), ("packages/a/package.json", MEMBER)];
            assert_eq!(!lint(META.id, &files, &config("")).is_empty(), expected, "{name}");
        }
    }

    #[test]
    fn members_are_never_checked() {
        let json = root_json(r#","private":true"#);
        let files = [("package.json", json.as_str()), ("packages/a/package.json", MEMBER)];
        assert_eq!(lint(META.id, &files, &config("")), [] as [skif_core::Diagnostic; 0]);
    }

    #[test]
    fn missing_field_is_anchored_at_the_opening_brace() {
        let json = root_json("");
        let files = [("package.json", json.as_str()), ("packages/a/package.json", MEMBER)];
        let diagnostics = lint(META.id, &files, &config(""));
        assert_eq!(located(&files, &diagnostics[0]), "{");
        assert_eq!(diagnostics[0].package.as_deref(), Some("root"));
        assert_eq!(diagnostics[0].location.path.to_str(), Some("package.json"));
    }

    #[test]
    fn wrong_value_is_anchored_at_the_field() {
        let json = root_json(r#","private":false"#);
        let files = [("package.json", json.as_str()), ("packages/a/package.json", MEMBER)];
        let diagnostics = lint(META.id, &files, &config(""));
        assert_eq!(located(&files, &diagnostics[0]), r#""private":false"#);
    }

    #[test]
    fn fix_adds_or_replaces_the_field_in_the_root_manifest() {
        for private in ["", r#","private":false"#, r#","private":"yes""#] {
            let json = root_json(private);
            let files = [("package.json", json.as_str()), ("packages/a/package.json", MEMBER)];
            assert_fixes_clear(META.id, &files, &config(""));
            let fixed = testing::fixed_json(META.id, &files, &config(""), "package.json");
            assert_eq!(fixed["private"], serde_json::json!(true), "{private}");
            assert_eq!(fixed["name"], serde_json::json!("root"));
        }
    }

    #[test]
    fn fix_targets_the_root_manifest_not_a_working_directory_file() {
        let json = root_json("");
        let files = [("package.json", json.as_str()), ("packages/a/package.json", MEMBER)];
        let diagnostics = lint(META.id, &files, &config(""));
        for op in &diagnostics[0].fixes[0].ops {
            let skif_core::FixOp::Json { path, .. } = op else {
                panic!("unexpected op {op:?}")
            };
            assert_eq!(path.to_str(), Some("package.json"));
        }
    }
}
