use std::path::Path;

use skif_core::{Category, Fixability, Location, RuleMeta, Severity, diagnostic, slash};
use skif_lint::{Emitter, LintContext, OptionsError, Rule};
use skif_workspace::Package;

use super::allow_list::{self, AllowList};

/// Metadata of `nested-workspace-root`.
pub static META: RuleMeta = RuleMeta::new(
    "nested-workspace-root",
    Category::Workspace,
    Some(Severity::Warning),
    Fixability::None,
    "A workspace member holds a `pnpm-workspace.yaml` of its own, so installs in that directory leave the workspace.",
);

const PNPM_WORKSPACE: &str = "pnpm-workspace.yaml";

/// Reports a `pnpm-workspace.yaml` in a directory the workspace patterns select.
///
/// pnpm takes the nearest `pnpm-workspace.yaml` above the working directory, so an install in such a member uses the
/// member's file and silently leaves the root workspace; skif likewise lints the directory as a root of its own. The
/// finding is reported by the enclosing workspace also when a recursive run lints the member as its own root, and is
/// skipped for a member `[workspace] ignore-packages` excludes. A nested workspace outside the patterns is an
/// independent root and is not reported. There is no fix: the file may hold settings that belong in the root file.
///
/// Options (`[rules.nested-workspace-root] allow`): paths or globs, relative to the workspace root, of
/// `pnpm-workspace.yaml` files kept on purpose, each a string or `{ name, reason }`. An entry that matches no such file
/// is reported as stale.
#[derive(Debug, Clone, Copy, Default)]
pub struct NestedWorkspaceRoot;

/// The `pnpm-workspace.yaml` files below the root, with the member whose directory holds each.
fn nested<'w>(cx: &LintContext<'w>) -> Vec<(&'w Path, Option<&'w Package>)> {
    let workspace = cx.workspace;
    let ignore = &cx.config.workspace.ignore_packages;
    workspace
        .facts
        .package_manager_files
        .iter()
        .filter(|path| path.file_name().is_some_and(|name| name == PNPM_WORKSPACE))
        .filter_map(|path| {
            let dir = path.parent().filter(|dir| !dir.as_os_str().is_empty())?;
            let package = workspace.packages().iter().find(|package| package.dir == dir);
            if package.is_some_and(|package| package.is_selected_by(ignore)) {
                return None;
            }
            Some((path.as_path(), package))
        })
        .collect()
}

impl Rule for NestedWorkspaceRoot {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn options_schema(&self) -> Option<serde_json::Value> {
        Some(allow_list::schema())
    }

    fn validate_options(&self, options: &toml::Table) -> Result<(), OptionsError> {
        allow_list::validate(options)
    }

    fn check(&self, cx: &LintContext<'_>, out: &mut Emitter<'_>) {
        let allow = AllowList::of(cx, &META);
        let found = nested(cx);
        for (path, package) in found.iter().filter(|(path, _)| !allow.allows(path)) {
            let dir = path.parent().map(slash).unwrap_or_default();
            let mut builder = diagnostic(
                &META,
                format!(
                    "`{dir}` is a workspace member but declares a workspace of its own in `{}`",
                    slash(path)
                ),
                Location::new(path.to_path_buf(), None),
            )
            .help(
                "pnpm installs from the nearest `pnpm-workspace.yaml`, so installs in this directory leave the root \
                 workspace; move its settings to the root file and delete it, or exclude the directory from the \
                 workspace patterns",
            );
            // A member another root lints is ignored in this workspace, and the emitter drops findings tied to an
            // ignored package; the file still belongs to this workspace's layout, so such a finding names no package.
            if let Some(package) = package.filter(|package| !cx.workspace.is_ignored(package.id)) {
                builder = builder.package(package.display_name()).package_id(package.id);
            }
            out.emit(builder.build());
        }
        let paths: Vec<&Path> = found.iter().map(|(path, _)| *path).collect();
        allow.report_stale(cx, &META, &paths, "nested `pnpm-workspace.yaml`", out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{config, lint, root};

    const NESTED: &str = "packages:\n  - 'p/*'\n";

    fn paths(diagnostics: &[skif_core::Diagnostic]) -> Vec<String> {
        diagnostics
            .iter()
            .map(|diagnostic| slash(&diagnostic.location.path))
            .collect()
    }

    fn files(root_json: &str) -> Vec<(&str, &str)> {
        vec![
            ("package.json", root_json),
            ("pnpm-workspace.yaml", "packages:\n  - 'packages/*'\n"),
            ("pnpm-lock.yaml", ""),
            ("packages/a/package.json", r#"{"name":"a"}"#),
            ("packages/a/pnpm-workspace.yaml", NESTED),
            ("packages/b/package.json", r#"{"name":"b"}"#),
            ("tools/x/package.json", r#"{"name":"x"}"#),
            ("tools/x/pnpm-workspace.yaml", NESTED),
        ]
    }

    #[test]
    fn a_pnpm_workspace_in_a_member_is_reported_and_the_roots_is_not() {
        let root_json = root("");
        let files = files(&root_json);
        let diagnostics = lint(META.id, &files, &config(""));
        assert_eq!(paths(&diagnostics), ["packages/a/pnpm-workspace.yaml"]);
        assert_eq!(
            diagnostics[0].message,
            "`packages/a` is a workspace member but declares a workspace of its own in `packages/a/pnpm-workspace.yaml`"
        );
        assert_eq!(diagnostics[0].package.as_deref(), Some("a"));
        assert_eq!(diagnostics[0].fixes, []);
    }

    #[test]
    fn a_nested_pnpm_workspace_is_reported_under_any_package_manager() {
        let root_json = r#"{"name":"root","private":true,"packageManager":"npm@10.0.0","workspaces":["packages/*"]}"#;
        let files = [
            ("package.json", root_json),
            ("package-lock.json", "{}"),
            ("packages/a/package.json", r#"{"name":"a"}"#),
            ("packages/a/pnpm-workspace.yaml", NESTED),
        ];
        assert_eq!(
            paths(&lint(META.id, &files, &config(""))),
            ["packages/a/pnpm-workspace.yaml"]
        );
    }

    #[test]
    fn a_member_excluded_by_ignore_packages_is_not_reported() {
        let root_json = root("");
        let files = files(&root_json);
        let ignoring = config("[workspace]\nignore-packages = [\"a\"]\n");
        assert_eq!(lint(META.id, &files, &ignoring), []);
    }

    #[test]
    fn a_member_another_root_lints_is_still_reported() {
        let root_json = root("");
        let files = files(&root_json);
        let mut workspace = crate::testing::workspace(&files, &config(""));
        workspace.ignore_dirs(&std::collections::BTreeSet::from([
            Path::new("/ws/packages/a").to_path_buf()
        ]));
        let registry = skif_lint::Registry::new(crate::providers()).unwrap();
        let diagnostics: Vec<_> =
            skif_lint::run_unsorted(&registry, &workspace, &config(""), &skif_lint::RunOptions::default())
                .unwrap()
                .into_iter()
                .filter(|diagnostic| diagnostic.rule.as_str() == META.id)
                .collect();
        assert_eq!(paths(&diagnostics), ["packages/a/pnpm-workspace.yaml"]);
    }

    #[test]
    fn allowed_and_stale_entries() {
        let root_json = root("");
        let files = files(&root_json);
        let allowing = config(
            "[rules.nested-workspace-root]\nallow = [\n  { name = \"packages/a/pnpm-workspace.yaml\", reason = \"installed on its own\" },\n  \"tools/x/pnpm-workspace.yaml\",\n]\n",
        );
        let diagnostics = lint(META.id, &files, &allowing);
        let messages: Vec<&str> = diagnostics.iter().map(|d| d.message.as_str()).collect();
        assert_eq!(
            messages,
            [
                "`tools/x/pnpm-workspace.yaml` in `rules.nested-workspace-root.allow` names no nested `pnpm-workspace.yaml`"
            ]
        );
        assert_eq!(paths(&diagnostics), ["skif.toml"]);
    }
}
