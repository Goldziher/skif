//! Where catalog entries live and the operations that add them.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use skif_core::{FixOp, InsertPosition, JsonAction, PathSeg, YamlAction};
use skif_workspace::{PackageManager, Workspace};

use super::names::CatalogName;
use super::support::key;

/// A file that can hold catalogs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CatalogFile {
    /// `pnpm-workspace.yaml`.
    Yaml(PathBuf),
    /// The root `package.json`, for bun.
    RootJson(PathBuf),
}

impl CatalogFile {
    fn from_source(workspace: &Workspace, source: &Path) -> Option<Self> {
        let extension = source.extension()?.to_str()?;
        match extension {
            "yaml" | "yml" => Some(Self::Yaml(source.to_path_buf())),
            "json" if source == workspace.root_package.manifest.path.as_path() => {
                Some(Self::RootJson(source.to_path_buf()))
            }
            _ => None,
        }
    }
}

/// The file the workspace's catalogs were read from.
pub(crate) fn existing_file(workspace: &Workspace) -> Option<CatalogFile> {
    CatalogFile::from_source(workspace, workspace.catalogs.source()?)
}

/// The file a first catalog would be created in: `pnpm-workspace.yaml` for pnpm, the root `package.json` for bun.
/// npm has no catalogs and yarn's live in a file skif does not edit.
pub(crate) fn creatable_file(workspace: &Workspace) -> Option<CatalogFile> {
    match workspace.package_manager? {
        PackageManager::Pnpm => workspace
            .pnpm_workspace
            .as_ref()
            .map(|yaml| CatalogFile::Yaml(yaml.path.clone())),
        PackageManager::Bun => Some(CatalogFile::RootJson(workspace.root_package.manifest.path.clone())),
        _ => None,
    }
}

fn route(catalog: &CatalogName) -> Vec<PathSeg> {
    match catalog {
        CatalogName::Default => vec![key("catalog")],
        CatalogName::Named(name) => vec![key("catalogs"), key(name)],
    }
}

fn lookup<'v>(container: &'v Value, catalog: &CatalogName) -> Option<&'v Value> {
    match catalog {
        CatalogName::Default => container.get("catalog"),
        CatalogName::Named(name) => container.get("catalogs")?.get(name),
    }
}

/// Bun accepts catalogs at the top level of the root `package.json` and inside its `workspaces` object; an existing
/// catalog is edited where it is declared.
fn json_pointer(workspace: &Workspace, catalog: &CatalogName) -> Vec<PathSeg> {
    let root = &workspace.root_package.manifest.value;
    let top_level = route(catalog);
    let declared_at_top = lookup(root, catalog).is_some_and(Value::is_object);
    let declared_nested = root
        .get("workspaces")
        .and_then(|workspaces| lookup(workspaces, catalog))
        .is_some_and(Value::is_object);
    if declared_at_top || !declared_nested {
        top_level
    } else {
        std::iter::once(key("workspaces")).chain(top_level).collect()
    }
}

/// Adds `dependency: version` to the existing `catalog` in `file`.
pub(crate) fn add_entry(
    workspace: &Workspace,
    file: &CatalogFile,
    catalog: &CatalogName,
    dependency: &str,
    version: &str,
) -> FixOp {
    match file {
        CatalogFile::Yaml(path) => {
            let mut entry_route = route(catalog);
            entry_route.push(key(dependency));
            FixOp::Yaml {
                path: path.clone(),
                route: entry_route,
                action: YamlAction::Set(Value::String(version.to_owned())),
            }
        }
        CatalogFile::RootJson(path) => FixOp::Json {
            path: path.clone(),
            pointer: json_pointer(workspace, catalog),
            action: JsonAction::Insert {
                key: dependency.to_owned(),
                value: Value::String(version.to_owned()),
                position: InsertPosition::Sorted,
            },
        },
    }
}

/// Creates the default catalog in `file` holding all of `entries`.
///
/// Every diagnostic that needs the new catalog carries this same operation with the complete entry list, so applying
/// it once or several times gives the same file; hosts that merge fixes drop the repeats.
pub(crate) fn create_default_catalog(file: &CatalogFile, entries: Map<String, Value>) -> FixOp {
    match file {
        CatalogFile::Yaml(path) => FixOp::Yaml {
            path: path.clone(),
            route: vec![key("catalog")],
            action: YamlAction::Set(Value::Object(entries)),
        },
        CatalogFile::RootJson(path) => FixOp::Json {
            path: path.clone(),
            pointer: Vec::new(),
            action: JsonAction::Insert {
                key: "catalog".to_owned(),
                value: Value::Object(entries),
                position: InsertPosition::End,
            },
        },
    }
}
