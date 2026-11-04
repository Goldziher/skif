//! Helpers shared by the catalog, workspace-protocol and family rules: package selection, locations and fix ops.

use serde_json::Value;
use skif_core::{FixOp, JsonAction, Location, PathSeg};
use skif_workspace::{DependencyEntry, Package, Workspace};

pub(crate) fn key(name: &str) -> PathSeg {
    PathSeg::Key(name.to_owned())
}

/// Every package the workspace does not ignore, the root first.
pub(crate) fn active_packages(workspace: &Workspace) -> impl Iterator<Item = &Package> {
    workspace
        .iter()
        .filter(move |(id, _)| !workspace.is_ignored(*id))
        .map(|(_, package)| package)
}

/// The specifier string of a dependency entry.
pub(crate) fn value_location(package: &Package, entry: &DependencyEntry) -> Location {
    Location::new(
        package.manifest.path.clone(),
        Some(entry.value_span),
    )
}

/// Replaces the specifier of `entry` with `specifier` in its own manifest.
pub(crate) fn set_specifier(package: &Package, entry: &DependencyEntry, specifier: &str) -> FixOp {
    FixOp::json(
        package.manifest.path.clone(),
        vec![key(entry.kind.field()), key(&entry.name)],
        JsonAction::Set(Value::String(specifier.to_owned())),
    )
}
