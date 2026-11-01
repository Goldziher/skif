use serde_json::Value;
use skif_core::{Category, Fixability, RuleMeta, Severity, diagnostic};
use skif_lint::{Emitter, LintContext, Rule};

use super::support::document_location;

/// Metadata of `root-package-manager-field`.
pub static META: RuleMeta = RuleMeta {
    id: "root-package-manager-field",
    category: Category::Manifest,
    default: Some(Severity::Error),
    fixable: Fixability::None,
    summary: "The root `package.json` must declare the package manager in `packageManager` or `devEngines.packageManager`.",
};

/// Reports a root manifest that declares neither `packageManager` nor `devEngines.packageManager`.
#[derive(Debug, Clone, Copy, Default)]
pub struct RootPackageManagerField;

fn declares(manifest: &Value) -> bool {
    let present = |value: Option<&Value>| value.is_some_and(|value| !value.is_null());
    present(manifest.get("packageManager")) || present(manifest.pointer("/devEngines/packageManager"))
}

impl Rule for RootPackageManagerField {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn check(&self, cx: &LintContext<'_>, out: &mut Emitter<'_>) {
        let root = &cx.workspace.root_package;
        if cx.workspace.is_ignored(root.id) || declares(&root.manifest.value) {
            return;
        }
        out.emit(
            diagnostic(
                &META,
                "the root package does not declare its package manager",
                document_location(root),
            )
            .package(root.display_name())
            .help(
                "add `\"packageManager\": \"<name>@<version>\"` (or `devEngines.packageManager`) to the root package.json; tools such as corepack read it",
            )
            .build(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{config, lint, located};

    const MEMBER: &str = r#"{"name":"a"}"#;

    fn root_json(extra: &str) -> String {
        format!(r#"{{"name":"root","private":true,"workspaces":["packages/*"]{extra}}}"#)
    }

    #[test]
    fn declaration_table() {
        let cases = [
            ("none", "", true),
            ("packageManager", r#","packageManager":"pnpm@9.0.0""#, false),
            ("devEngines object", r#","devEngines":{"packageManager":{"name":"pnpm","version":"9"}}"#, false),
            ("devEngines array", r#","devEngines":{"packageManager":[{"name":"npm"},{"name":"pnpm"}]}"#, false),
            ("devEngines without manager", r#","devEngines":{"runtime":{"name":"node"}}"#, true),
            ("devEngines empty", r#","devEngines":{}"#, true),
            ("null field", r#","packageManager":null"#, true),
            ("unknown manager is still a declaration", r#","packageManager":"deno@2""#, false),
        ];
        for (name, extra, expected) in cases {
            let json = root_json(extra);
            let files = [("package.json", json.as_str()), ("packages/a/package.json", MEMBER)];
            assert_eq!(!lint(META.id, &files, &config("")).is_empty(), expected, "{name}");
        }
    }

    #[test]
    fn members_may_omit_it_and_the_report_is_anchored_at_the_root() {
        let json = root_json("");
        let files = [("package.json", json.as_str()), ("packages/a/package.json", MEMBER)];
        let diagnostics = lint(META.id, &files, &config(""));
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(located(&files, &diagnostics[0]), "{");
        assert!(diagnostics[0].fixes.is_empty());
        assert!(diagnostics[0].help.is_some());
    }

    #[test]
    fn root_can_be_ignored_by_name() {
        let json = root_json("");
        let files = [("package.json", json.as_str()), ("packages/a/package.json", MEMBER)];
        let ignored = config("[workspace]\nignore-packages = [\"root\"]\n");
        assert!(lint(META.id, &files, &ignored).is_empty());
    }
}
