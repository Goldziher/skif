//! The `catalog:` references in override sections, shared by the rules that read references.

use skif_config::OverrideReading;
use skif_workspace::Workspace;

use super::super::fields;
use super::super::instances::Instance;
use super::support::active_packages;

/// The versions of every active package's override sections (and the `overrides` of `pnpm-workspace.yaml` for the
/// root) that select a catalog entry. Every reference counts, so the entries are read the way skif reads them.
pub(super) fn override_references(workspace: &Workspace) -> Vec<Instance<'_>> {
    active_packages(workspace)
        .flat_map(|package| {
            fields::collect(workspace, package, &[], OverrideReading::Skif).into_iter().map(move |found| {
                let is_local = workspace.package_by_name(&found.name).is_some();
                Instance::of_field(package, found, is_local)
            })
        })
        .filter(|instance| instance.specifier.is_catalog())
        .collect()
}
