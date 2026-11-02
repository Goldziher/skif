use serde_json::Value;
use skif_core::{Category, Fix, FixSafety, Fixability, JsonAction, RuleMeta, Severity, diagnostic};
use skif_lint::{Emitter, LintContext, Rule};
use skif_workspace::{PackageManager, Workspace};

use super::support::{active_packages, json_op, key, value_location};

/// Metadata of `local-path-dependency`.
pub static META: RuleMeta = RuleMeta {
    id: "local-path-dependency",
    category: Category::Manifest,
    default: Some(Severity::Error),
    fixable: Fixability::Sometimes,
    summary: "A dependency points at a path on disk (`link:`, `file:`, `portal:` or a relative path) instead of a registry or workspace version.",
};

/// Reports dependencies that point at local paths. Those naming a workspace package can be replaced by the
/// workspace specifier of the detected package manager.
#[derive(Debug, Clone, Copy, Default)]
pub struct LocalPathDependency;

const PROTOCOLS: [&str; 3] = ["link:", "file:", "portal:"];

fn is_local_path(specifier: &str) -> bool {
    PROTOCOLS.iter().any(|protocol| specifier.starts_with(protocol))
        || specifier.starts_with("./")
        || specifier.starts_with("../")
        || specifier.starts_with('/')
}

/// Whether the root manifest pins Yarn Classic (1.x) in `packageManager`, which does not implement `workspace:`.
fn is_yarn_classic(workspace: &Workspace) -> bool {
    workspace
        .root_package
        .manifest
        .value
        .get("packageManager")
        .and_then(Value::as_str)
        .is_some_and(|pinned| pinned.starts_with("yarn@1."))
}

/// The specifier that links a workspace package: `*` for npm, `workspace:*` for the others, `None` when the
/// package manager is unknown or (Yarn Classic) has no protocol to suggest.
fn workspace_specifier(workspace: &Workspace) -> Option<&'static str> {
    match workspace.package_manager? {
        PackageManager::Npm => Some("*"),
        PackageManager::Yarn if is_yarn_classic(workspace) => None,
        PackageManager::Pnpm | PackageManager::Yarn | PackageManager::Bun => Some("workspace:*"),
        _ => None,
    }
}

impl Rule for LocalPathDependency {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn check(&self, cx: &LintContext<'_>, out: &mut Emitter<'_>) {
        let workspace = cx.workspace;
        let replacement = workspace_specifier(workspace);
        for package in active_packages(workspace) {
            for entry in package
                .dependencies
                .iter()
                .filter(|entry| is_local_path(&entry.specifier))
            {
                let target = workspace
                    .package_by_name(&entry.name)
                    .filter(|target| target.id != package.id);
                let mut builder = diagnostic(
                    &META,
                    format!("`{}` points at the local path `{}`", entry.name, entry.specifier),
                    value_location(package, entry),
                )
                .package(package.display_name())
                .package_id(package.id)
                .dependency(entry.name.clone());
                builder = match (target, replacement) {
                    (Some(_), Some(replacement)) => builder
                        .help(format!(
                            "depend on the workspace package with `{replacement}` instead of a path"
                        ))
                        .fix(Fix {
                            title: format!("Use `{replacement}` for `{}`", entry.name),
                            safety: FixSafety::Unsafe,
                            ops: vec![json_op(
                                package,
                                vec![key(entry.kind.field()), key(&entry.name)],
                                JsonAction::Set(Value::String(replacement.to_owned())),
                            )],
                        }),
                    _ => builder.help("publish the package or vendor it; paths outside the workspace packages do not install elsewhere"),
                };
                out.emit(builder.build());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{self, assert_fixes_clear, config, lint, located, subjects};

    fn root_for(manager: &str) -> String {
        format!(r#"{{"name":"root","private":true,"packageManager":"{manager}","workspaces":["packages/*"]}}"#)
    }

    const B: &str = r#"{"name":"@scope/b","version":"1.0.0"}"#;

    fn a_with(specifier: &str) -> String {
        format!(r#"{{"name":"a","dependencies":{{"@scope/b":"{specifier}"}}}}"#)
    }

    #[test]
    fn specifier_table() {
        let root_json = root_for("pnpm@9.0.0");
        let cases = [
            ("link", "link:../b", true),
            ("file", "file:../b", true),
            ("portal", "portal:../b", true),
            ("dot slash", "./b", true),
            ("dot dot slash", "../b", true),
            ("absolute", "/abs/b", true),
            ("registry range", "^1.0.0", false),
            ("workspace", "workspace:*", false),
            ("catalog", "catalog:", false),
            ("git", "github:user/repo", false),
            ("alias", "npm:other@1", false),
            ("https tarball", "https://example.com/b.tgz", false),
        ];
        for (name, specifier, expected) in cases {
            let a = a_with(specifier);
            let files = [
                ("package.json", root_json.as_str()),
                ("packages/a/package.json", a.as_str()),
                ("packages/b/package.json", B),
            ];
            assert_eq!(!lint(META.id, &files, &config("")).is_empty(), expected, "{name}");
        }
    }

    #[test]
    fn every_dependency_section_and_the_root_are_checked() {
        let root_json = r#"{"name":"root","private":true,"packageManager":"pnpm@9.0.0","workspaces":["packages/*"],"devDependencies":{"x":"link:./x"}}"#.to_string();
        let a = r#"{"name":"a","dependencies":{"p":"file:../p"},"peerDependencies":{"q":"../q"},"optionalDependencies":{"r":"link:../r"}}"#;
        let files = [("package.json", root_json.as_str()), ("packages/a/package.json", a)];
        let diagnostics = lint(META.id, &files, &config(""));
        assert_eq!(
            subjects(&diagnostics),
            [
                ("root".to_owned(), Some("x".to_owned())),
                ("a".to_owned(), Some("p".to_owned())),
                ("a".to_owned(), Some("q".to_owned())),
                ("a".to_owned(), Some("r".to_owned())),
            ]
        );
    }

    #[test]
    fn the_span_covers_the_specifier_string() {
        let root_json = root_for("pnpm@9.0.0");
        let a = a_with("link:../b");
        let files = [
            ("package.json", root_json.as_str()),
            ("packages/a/package.json", a.as_str()),
            ("packages/b/package.json", B),
        ];
        let diagnostics = lint(META.id, &files, &config(""));
        assert_eq!(located(&files, &diagnostics[0]), r#""link:../b""#);
        assert_eq!(diagnostics[0].location.path.to_str(), Some("packages/a/package.json"));
    }

    #[test]
    fn the_replacement_follows_the_package_manager() {
        let cases = [
            ("npm@10.0.0", Some("*")),
            ("pnpm@9.0.0", Some("workspace:*")),
            ("yarn@4.0.0", Some("workspace:*")),
            ("bun@1.2.0", Some("workspace:*")),
        ];
        for (manager, expected) in cases {
            let root_json = root_for(manager);
            let a = a_with("link:../b");
            let files = [
                ("package.json", root_json.as_str()),
                ("packages/a/package.json", a.as_str()),
                ("packages/b/package.json", B),
            ];
            let fixed = testing::fixed_json(META.id, &files, &config(""), "packages/a/package.json");
            assert_eq!(fixed["dependencies"]["@scope/b"].as_str(), expected, "{manager}");
        }
    }

    #[test]
    fn an_undetected_package_manager_gets_no_workspace_suggestion() {
        let root_json = r#"{"name":"root","private":true,"workspaces":["packages/*"]}"#;
        let a = a_with("link:../b");
        let files = [
            ("package.json", root_json),
            ("packages/a/package.json", a.as_str()),
            ("packages/b/package.json", B),
        ];
        let diagnostics = lint(META.id, &files, &config(""));
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].fixes, [] as [skif_core::Fix; 0]);
    }

    #[test]
    fn yarn_classic_gets_no_workspace_suggestion_but_modern_yarn_does() {
        let a = a_with("link:../b");
        for (manager, fixable) in [("yarn@1.22.19", false), ("yarn@4.1.0", true)] {
            let root_json = root_for(manager);
            let files = [
                ("package.json", root_json.as_str()),
                ("packages/a/package.json", a.as_str()),
                ("packages/b/package.json", B),
            ];
            let diagnostics = lint(META.id, &files, &config(""));
            assert_eq!(diagnostics.len(), 1, "{manager}");
            assert_eq!(diagnostics[0].is_fixable(), fixable, "{manager}");
        }
    }

    #[test]
    fn config_override_decides_the_package_manager() {
        let root_json = root_for("pnpm@9.0.0");
        let a = a_with("link:../b");
        let files = [
            ("package.json", root_json.as_str()),
            ("packages/a/package.json", a.as_str()),
            ("packages/b/package.json", B),
        ];
        let npm = config("[workspace]\npackage-manager = \"npm\"\n");
        let fixed = testing::fixed_json(META.id, &files, &npm, "packages/a/package.json");
        assert_eq!(fixed["dependencies"]["@scope/b"], serde_json::json!("*"));
    }

    #[test]
    fn only_dependencies_naming_a_workspace_package_are_fixable() {
        let root_json = root_for("pnpm@9.0.0");
        let a = r#"{"name":"a","dependencies":{"@scope/b":"link:../b","vendored":"file:../../vendor/lib.tgz"}}"#;
        let files = [
            ("package.json", root_json.as_str()),
            ("packages/a/package.json", a),
            ("packages/b/package.json", B),
        ];
        let diagnostics = lint(META.id, &files, &config(""));
        let fixable: Vec<_> = diagnostics
            .iter()
            .map(|d| (d.dependency.clone().unwrap(), d.is_fixable()))
            .collect();
        assert_eq!(fixable, [("@scope/b".to_owned(), true), ("vendored".to_owned(), false)]);
    }

    #[test]
    fn the_fix_is_unsafe_because_it_changes_what_gets_installed() {
        let root_json = root_for("pnpm@9.0.0");
        let cases = [
            ("link:../b", FixSafety::Unsafe),
            ("file:./../b", FixSafety::Unsafe),
            ("../b", FixSafety::Unsafe),
            ("link:../elsewhere", FixSafety::Unsafe),
            ("/abs/b", FixSafety::Unsafe),
        ];
        for (specifier, expected) in cases {
            let a = a_with(specifier);
            let files = [
                ("package.json", root_json.as_str()),
                ("packages/a/package.json", a.as_str()),
                ("packages/b/package.json", B),
            ];
            let diagnostics = lint(META.id, &files, &config(""));
            assert_eq!(diagnostics[0].fixes[0].safety, expected, "{specifier}");
        }
    }

    #[test]
    fn fix_keeps_neighbours_and_clears_the_rule() {
        let root_json = root_for("pnpm@9.0.0");
        let a = r#"{"name":"a","dependencies":{"@scope/b":"link:../b","c":"catalog:","d":"^1.2"}}"#;
        let files = [
            ("package.json", root_json.as_str()),
            ("packages/a/package.json", a),
            ("packages/b/package.json", B),
        ];
        assert_fixes_clear(META.id, &files, &config(""));
        let fixed = testing::fixed_json(META.id, &files, &config(""), "packages/a/package.json");
        assert_eq!(
            fixed["dependencies"],
            serde_json::json!({"@scope/b": "workspace:*", "c": "catalog:", "d": "^1.2"})
        );
    }

    #[test]
    fn a_root_dependency_is_fixed_in_the_root_manifest() {
        let root_json = r#"{"name":"root","private":true,"packageManager":"pnpm@9.0.0","workspaces":["packages/*"],"devDependencies":{"@scope/b":"link:./packages/b"}}"#.to_string();
        let files = [("package.json", root_json.as_str()), ("packages/b/package.json", B)];
        let diagnostics = lint(META.id, &files, &config(""));
        assert_eq!(diagnostics[0].fixes[0].safety, FixSafety::Unsafe);
        let fixed = testing::fixed_json(META.id, &files, &config(""), "package.json");
        assert_eq!(fixed["devDependencies"]["@scope/b"], serde_json::json!("workspace:*"));
    }

    #[test]
    fn ignored_dependencies_are_suppressed() {
        let root_json = root_for("pnpm@9.0.0");
        let a = a_with("link:../b");
        let files = [
            ("package.json", root_json.as_str()),
            ("packages/a/package.json", a.as_str()),
            ("packages/b/package.json", B),
        ];
        let ignored = config("[[ignore]]\nrules = [\"local-path-dependency\"]\ndependencies = [\"@scope/*\"]\n");
        assert_eq!(lint(META.id, &files, &ignored), [] as [skif_core::Diagnostic; 0]);
    }
}
