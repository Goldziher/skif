use skif_core::{Category, DependencyKind, Fix, FixSafety, Fixability, RuleMeta, Severity, diagnostic};
use skif_lint::{Emitter, LintContext, Rule};

use super::support::{entry_location, field_location, move_to_dev_dependencies, section};

/// Metadata of `root-package-dependencies`.
pub static META: RuleMeta = RuleMeta::new(
    "root-package-dependencies",
    Category::Manifest,
    Some(Severity::Warning),
    Fixability::Unsafe,
    "The root package should only have `devDependencies`; runtime dependencies belong to the packages that use them.",
);

/// Reports a non-empty `dependencies` section in the root manifest.
#[derive(Debug, Clone, Copy, Default)]
pub struct RootPackageDependencies;

impl Rule for RootPackageDependencies {
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
        let Some(dependencies) = root.dependency_map(DependencyKind::Dependencies) else {
            return;
        };
        if dependencies.is_empty() {
            return;
        }
        let names: Vec<&str> = dependencies.keys().map(String::as_str).collect();
        let mut builder = diagnostic(
            &META,
            "the root package has `dependencies`",
            field_location(root, &section(DependencyKind::Dependencies)),
        )
        .package(root.display_name())
        .package_id(root.id)
        .help("declare dependencies in the packages that use them, or move them to `devDependencies`");
        for entry in root
            .dependencies
            .iter()
            .filter(|entry| entry.kind == DependencyKind::Dependencies)
        {
            builder = builder.related(skif_core::Related::new(
                format!("`{}` is a runtime dependency of the root", entry.name),
                entry_location(root, entry),
            ));
        }
        if let Some(ops) = move_to_dev_dependencies(root, &names) {
            builder = builder.fix(Fix::new(
                "Move the root `dependencies` to `devDependencies`".to_owned(),
                FixSafety::Unsafe,
                ops,
            ));
        }
        out.emit(builder.build());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{self, assert_fixes_clear, config, lint, located, root};

    const MEMBER: &str = r#"{"name":"a"}"#;

    #[test]
    fn reports_non_empty_dependencies_only() {
        let cases = [
            ("none", "", 0),
            ("empty (owned by empty-dependencies)", r#","dependencies":{}"#, 0),
            ("dev only", r#","devDependencies":{"x":"1"}"#, 0),
            ("one", r#","dependencies":{"x":"1"}"#, 1),
            ("two stay one diagnostic", r#","dependencies":{"x":"1","y":"2"}"#, 1),
        ];
        for (name, extra, expected) in cases {
            let json = root(extra);
            let files = [("package.json", json.as_str()), ("packages/a/package.json", MEMBER)];
            assert_eq!(lint(META.id, &files, &config("")).len(), expected, "{name}");
        }
    }

    #[test]
    fn members_may_have_dependencies() {
        let json = root("");
        let member = r#"{"name":"a","dependencies":{"x":"1"}}"#;
        let files = [("package.json", json.as_str()), ("packages/a/package.json", member)];
        assert_eq!(lint(META.id, &files, &config("")), [] as [skif_core::Diagnostic; 0]);
    }

    #[test]
    fn span_covers_the_section_and_related_the_entries() {
        let json = root(r#","dependencies":{"x":"1","y":"2"}"#);
        let files = [("package.json", json.as_str()), ("packages/a/package.json", MEMBER)];
        let diagnostics = lint(META.id, &files, &config(""));
        assert_eq!(located(&files, &diagnostics[0]), r#""dependencies":{"x":"1","y":"2"}"#);
        let related: Vec<_> = diagnostics[0]
            .related
            .iter()
            .map(|related| testing::slice(&files, "package.json", related.location.span.unwrap()))
            .collect();
        assert_eq!(related, [r#""x":"1""#, r#""y":"2""#]);
    }

    #[test]
    fn fix_renames_the_section_when_there_is_no_dev_section() {
        let json = root(r#","dependencies":{"x":"^1","y":"catalog:"}"#);
        let files = [("package.json", json.as_str()), ("packages/a/package.json", MEMBER)];
        assert_fixes_clear(META.id, &files, &config(""));
        let fixed = testing::fixed_json(META.id, &files, &config(""), "package.json");
        assert_eq!(
            fixed["devDependencies"],
            serde_json::json!({"x": "^1", "y": "catalog:"})
        );
        assert!(fixed.get("dependencies").is_none());
    }

    #[test]
    fn fix_merges_into_an_existing_dev_section_and_keeps_specifiers() {
        let json = root(r#","dependencies":{"b":"1.2","a":"workspace:*"},"devDependencies":{"c":"3"}"#);
        let files = [("package.json", json.as_str()), ("packages/a/package.json", MEMBER)];
        assert_fixes_clear(META.id, &files, &config(""));
        let fixed = testing::fixed_json(META.id, &files, &config(""), "package.json");
        assert_eq!(
            fixed["devDependencies"],
            serde_json::json!({"a": "workspace:*", "b": "1.2", "c": "3"})
        );
        assert!(fixed.get("dependencies").is_none());
    }

    #[test]
    fn fix_fills_an_empty_dev_section() {
        let json = root(r#","dependencies":{"x":"1"},"devDependencies":{}"#);
        let files = [("package.json", json.as_str()), ("packages/a/package.json", MEMBER)];
        let fixed = testing::fixed_json(META.id, &files, &config(""), "package.json");
        assert_eq!(fixed["devDependencies"], serde_json::json!({"x": "1"}));
        assert!(fixed.get("dependencies").is_none());
    }

    #[test]
    fn fix_is_unsafe_and_withheld_when_it_would_drop_a_specifier() {
        let same = root(r#","dependencies":{"x":"1"},"devDependencies":{"x":"1"}"#);
        let files = [("package.json", same.as_str()), ("packages/a/package.json", MEMBER)];
        let diagnostics = lint(META.id, &files, &config(""));
        assert_eq!(diagnostics[0].fixes[0].safety, skif_core::FixSafety::Unsafe);
        assert_fixes_clear(META.id, &files, &config(""));

        let conflict = root(r#","dependencies":{"x":"1"},"devDependencies":{"x":"2"}"#);
        let files = [("package.json", conflict.as_str()), ("packages/a/package.json", MEMBER)];
        let diagnostics = lint(META.id, &files, &config(""));
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].fixes, [] as [skif_core::Fix; 0]);
    }
}
