//! Package-manager detection: the single source of truth for every rule and fix.

use std::path::Path;

use serde_json::Value;
use skif_config::{Config, PackageManager};

use skif_core::FileSystem;

fn from_name(name: &str) -> Option<PackageManager> {
    match name {
        "npm" => Some(PackageManager::Npm),
        "pnpm" => Some(PackageManager::Pnpm),
        "yarn" => Some(PackageManager::Yarn),
        "bun" => Some(PackageManager::Bun),
        _ => None,
    }
}

fn from_dev_engine(entry: &Value) -> Option<PackageManager> {
    from_name(entry.get("name")?.as_str()?)
}

/// The package manager a root `package.json` declares: the `packageManager` field (`pnpm@9.12.0`), then
/// `devEngines.packageManager` (an object or a list; the first known entry wins).
#[must_use]
pub fn package_manager_from_manifest(manifest: &Value) -> Option<PackageManager> {
    if let Some(field) = manifest.get("packageManager").and_then(Value::as_str) {
        let name = field.split('@').next().unwrap_or(field);
        if let Some(package_manager) = from_name(name) {
            return Some(package_manager);
        }
    }
    match manifest.get("devEngines")?.get("packageManager")? {
        Value::Array(entries) => entries.iter().find_map(from_dev_engine),
        entry => from_dev_engine(entry),
    }
}

fn from_lockfile(fs: &dyn FileSystem, root: &Path) -> Option<PackageManager> {
    let exists = |name: &str| fs.is_file(&root.join(name));
    if exists("pnpm-lock.yaml") {
        Some(PackageManager::Pnpm)
    } else if exists("bun.lock") || exists("bun.lockb") {
        Some(PackageManager::Bun)
    } else if exists("yarn.lock") {
        Some(PackageManager::Yarn)
    } else if exists("package-lock.json") {
        Some(PackageManager::Npm)
    } else {
        None
    }
}

/// Detects the package manager, first match wins: `[workspace] package-manager` in the config, the manifest
/// declaration, the one `rush.json` names, a lockfile (`pnpm-lock.yaml`, `bun.lock`/`bun.lockb`, `yarn.lock`, `package-lock.json`), then the
/// presence of `pnpm-workspace.yaml`. `None` when nothing indicates a manager.
pub(crate) fn detect(
    fs: &dyn FileSystem,
    root: &Path,
    config: &Config,
    manifest: &Value,
    from_rush: Option<PackageManager>,
    has_pnpm_workspace: bool,
) -> Option<PackageManager> {
    config
        .workspace
        .package_manager
        .or_else(|| package_manager_from_manifest(manifest))
        .or(from_rush)
        .or_else(|| from_lockfile(fs, root))
        .or_else(|| has_pnpm_workspace.then_some(PackageManager::Pnpm))
}

/// Whether the workspace uses Yarn Classic (1.x), which has no `workspace:` protocol.
///
/// The `packageManager` field decides when it names Yarn; without it the first lines of `yarn.lock` do
/// (`# yarn lockfile v1`).
pub(crate) fn is_yarn_classic(fs: &dyn FileSystem, root: &Path, manifest: &Value) -> bool {
    match manifest
        .get("packageManager")
        .and_then(Value::as_str)
        .filter(|field| field.starts_with("yarn@"))
    {
        Some(field) => field.starts_with("yarn@1."),
        None => fs
            .read_to_string(&root.join("yarn.lock"))
            .is_ok_and(|lock| lock.lines().take(8).any(|line| line.trim() == "# yarn lockfile v1")),
    }
}
