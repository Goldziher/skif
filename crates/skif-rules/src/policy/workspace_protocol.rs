//! `workspace-protocol`: dependencies on workspace packages must use the `workspace:` protocol.

use skif_config::PackageManager;
use skif_core::{Category, Fixability, RuleMeta, Severity, diagnostic};
use skif_lint::{Emitter, LintContext, Rule};
use skif_workspace::Package;

use super::catalog::support::{active_packages, value_location};

/// Metadata of `workspace-protocol`.
pub static META: RuleMeta = RuleMeta {
    id: "workspace-protocol",
    category: Category::Policy,
    default: Some(Severity::Error),
    fixable: Fixability::None,
    summary: "A dependency on a workspace package does not use the `workspace:` protocol.",
};

/// Reports dependencies on other workspace packages that are written as plain versions, `catalog:` references or
/// paths, when `[workspace-protocol] internal` is set.
///
/// npm does not implement the protocol, so the rule stays silent for npm workspaces.
#[derive(Debug, Clone, Copy, Default)]
pub struct WorkspaceProtocol;

/// The workspace-relative directory a `link:` or `file:` specifier of a dependency in `package` points at.
fn path_target(package: &Package, specifier: &str) -> Option<String> {
    let path = ["link:", "file:"]
        .iter()
        .find_map(|prefix| specifier.strip_prefix(prefix))?;
    if path.starts_with('/') {
        return None;
    }
    let base = package.dir.to_string_lossy().replace('\\', "/");
    let mut parts: Vec<&str> = base.split('/').filter(|part| !part.is_empty()).collect();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }
    Some(parts.join("/"))
}

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
        let npm = workspace.package_manager == Some(PackageManager::Npm);
        if !settings.internal || npm || !cx.enabled.contains(META.id) {
            return;
        }
        for package in active_packages(workspace) {
            for entry in &package.dependencies {
                let Some(target) = workspace
                    .package_by_name(&entry.name)
                    .filter(|target| target.id != package.id)
                else {
                    continue;
                };
                if entry.parsed.is_workspace_protocol() {
                    continue;
                }
                let target_dir = target.dir.to_string_lossy().replace('\\', "/");
                let same_directory = path_target(package, &entry.specifier).is_some_and(|dir| dir == target_dir);
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
