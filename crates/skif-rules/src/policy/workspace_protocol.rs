//! `workspace-protocol`: dependencies on workspace packages must use the `workspace:` protocol.

use skif_core::{Category, DependencyKind, Fixability, RuleMeta, Severity, diagnostic};
use skif_lint::{Emitter, LintContext, Rule};

use super::catalog::support::{active_packages, value_location};
use crate::manifest::local_path_dependency::{self, is_local_path, path_target, relative_dir};

/// Metadata of `workspace-protocol`.
pub static META: RuleMeta = RuleMeta::new(
    "workspace-protocol",
    Category::Policy,
    Some(Severity::Error),
    Fixability::None,
    "A dependency on a workspace package does not use the `workspace:` protocol.",
);

/// Reports dependencies on other workspace packages that are written as plain versions, `catalog:` references or
/// paths, when `[workspace-protocol] internal` is set.
///
/// npm and Yarn Classic do not implement the protocol, so the rule stays silent for those workspaces. It leaves
/// `peerDependencies` alone (a published library names its peers by range) and, while `local-path-dependency` is on,
/// the path specifiers that rule already reports.
#[derive(Debug, Clone, Copy, Default)]
pub struct WorkspaceProtocol;

/// The rules of this module.
pub(crate) fn rules() -> Vec<Box<dyn Rule>> {
    vec![Box::new(WorkspaceProtocol)]
}

impl Rule for WorkspaceProtocol {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn check(&self, cx: &LintContext<'_>, out: &mut Emitter<'_>) {
        let settings = &cx.config.workspace_protocol;
        let workspace = cx.workspace;
        if !settings.internal || !workspace.supports_workspace_protocol() || !cx.enabled.contains(META.id) {
            return;
        }
        let paths_reported_elsewhere = cx.enabled.contains(local_path_dependency::META.id);
        for package in active_packages(workspace) {
            for entry in package
                .dependencies
                .iter()
                .filter(|entry| entry.kind != DependencyKind::PeerDependencies)
            {
                let Some(target) = workspace
                    .package_by_name(&entry.name)
                    .filter(|target| target.id != package.id)
                else {
                    continue;
                };
                if entry.parsed.is_workspace_protocol() || (paths_reported_elsewhere && is_local_path(&entry.specifier))
                {
                    continue;
                }
                // `allow-local-paths` accepts the `link:` and `file:` forms only.
                let same_directory = ["link:", "file:"]
                    .iter()
                    .any(|protocol| entry.specifier.starts_with(protocol))
                    && path_target(package, &entry.specifier).is_some_and(|dir| dir == relative_dir(target));
                if settings.allow_local_paths && same_directory {
                    continue;
                }
                out.emit(
                    diagnostic(
                        &META,
                        format!(
                            "`{}` is a workspace package but is declared as `{}`",
                            entry.name, entry.specifier
                        ),
                        value_location(package, entry),
                    )
                    .package(package.display_name())
                    .package_id(package.id)
                    .dependency(entry.name.clone())
                    .specifier(entry.specifier.clone())
                    .help("use `workspace:*` (or `workspace:^`) so the dependency always resolves to the local package")
                    .build(),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::{config, lint, subjects};

    use super::META;

    const SETTINGS: &str = "[workspace-protocol]\ninternal = true\n";

    fn root(package_manager: Option<&str>) -> String {
        let field = package_manager.map_or_else(String::new, |manager| format!(r#""packageManager":"{manager}","#));
        format!(r#"{{"name":"root","private":true,{field}"workspaces":["packages/*"]}}"#)
    }

    fn consumer(section: &str, specifier: &str) -> String {
        format!(r#"{{"name":"@scope/a","version":"1.0.0","{section}":{{"@scope/b":"{specifier}"}}}}"#)
    }

    const B: &str = r#"{"name":"@scope/b","version":"1.0.0"}"#;

    fn reports(package_manager: Option<&str>, section: &str, specifier: &str, settings: &str) -> usize {
        let root_json = root(package_manager);
        let a = consumer(section, specifier);
        let files = [
            ("package.json", root_json.as_str()),
            ("packages/a/package.json", a.as_str()),
            ("packages/b/package.json", B),
        ];
        lint(META.id, &files, &config(settings)).len()
    }

    #[test]
    fn a_plain_range_on_a_workspace_package_is_reported_where_the_manager_has_the_protocol() {
        for manager in ["pnpm@9.0.0", "yarn@4.1.0", "bun@1.2.0"] {
            assert_eq!(
                reports(Some(manager), "dependencies", "^1.0.0", SETTINGS),
                1,
                "{manager}"
            );
        }
        assert_eq!(reports(None, "dependencies", "^1.0.0", SETTINGS), 1);
    }

    #[test]
    fn npm_and_yarn_classic_cannot_install_the_protocol_so_nothing_is_demanded() {
        for manager in ["npm@10.0.0", "yarn@1.22.19"] {
            assert_eq!(
                reports(Some(manager), "dependencies", "^1.0.0", SETTINGS),
                0,
                "{manager}"
            );
        }
    }

    #[test]
    fn yarn_classic_is_recognised_from_the_lockfile_header_too() {
        let a = consumer("dependencies", "^1.0.0");
        let files = [
            (
                "package.json",
                r#"{"name":"root","private":true,"workspaces":["packages/*"]}"#,
            ),
            ("yarn.lock", "# yarn lockfile v1\n"),
            ("packages/a/package.json", a.as_str()),
            ("packages/b/package.json", B),
        ];
        assert_eq!(lint(META.id, &files, &config(SETTINGS)).len(), 0);
    }

    #[test]
    fn peer_dependencies_are_not_reported() {
        assert_eq!(reports(Some("pnpm@9.0.0"), "peerDependencies", "^1.0.0", SETTINGS), 0);
        assert_eq!(reports(Some("pnpm@9.0.0"), "devDependencies", "^1.0.0", SETTINGS), 1);
    }

    #[test]
    fn paths_are_left_to_local_path_dependency_while_it_is_enabled() {
        let both = reports(Some("pnpm@9.0.0"), "dependencies", "link:../b", SETTINGS);
        assert_eq!(both, 0);
        let alone = reports(
            Some("pnpm@9.0.0"),
            "dependencies",
            "link:../b",
            "[workspace-protocol]\ninternal = true\n[rules]\nlocal-path-dependency = \"off\"\n",
        );
        assert_eq!(alone, 1);
    }

    #[test]
    fn the_reported_dependency_is_named() {
        let root_json = root(Some("pnpm@9.0.0"));
        let a = consumer("dependencies", "^1.0.0");
        let files = [
            ("package.json", root_json.as_str()),
            ("packages/a/package.json", a.as_str()),
            ("packages/b/package.json", B),
        ];
        let found = lint(META.id, &files, &config(SETTINGS));
        assert_eq!(subjects(&found), [("@scope/a".to_owned(), Some("@scope/b".to_owned()))]);
    }
}
