//! Where catalog entries live and the operations that add and remove them.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use skif_core::{FixOp, InsertPosition, JsonAction, PathSeg, YamlAction};
use skif_workspace::{PackageManager, Workspace};

use super::names::CatalogName;
use super::support::key;

/// A file that can hold catalogs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CatalogFile {
    /// `pnpm-workspace.yaml` or `.yarnrc.yml`.
    Yaml(PathBuf),
    /// The root `package.json`, for bun.
    RootJson(PathBuf),
}

impl CatalogFile {
    fn from_source(workspace: &Workspace, source: &Path) -> Option<Self> {
        let extension = source.extension()?.to_str()?;
        match extension {
            "yaml" | "yml" => Some(Self::Yaml(source.to_path_buf())),
            "json" if source == workspace.root_package().manifest.path.as_path() => {
                Some(Self::RootJson(source.to_path_buf()))
            }
            _ => None,
        }
    }
}

/// The file that declares `catalog`, where its new entries belong.
pub(crate) fn existing_file(workspace: &Workspace, catalog: &CatalogName) -> Option<CatalogFile> {
    let name = match catalog {
        CatalogName::Default => None,
        CatalogName::Named(name) => Some(name.as_str()),
    };
    CatalogFile::from_source(workspace, workspace.catalogs.source_of(name)?)
}

/// The file a first catalog would be created in: `pnpm-workspace.yaml` for pnpm, the root `package.json` for bun.
/// npm has no catalogs, and yarn's `.yarnrc.yml` is only edited once it declares one.
pub(crate) fn creatable_file(workspace: &Workspace) -> Option<CatalogFile> {
    match workspace.package_manager? {
        PackageManager::Pnpm => workspace
            .pnpm_workspace
            .as_ref()
            .map(|yaml| CatalogFile::Yaml(yaml.path.clone())),
        PackageManager::Bun => Some(CatalogFile::RootJson(workspace.root_package().manifest.path.clone())),
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
    let root = workspace.root_package().manifest.value();
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
            // A default catalog declared as `catalogs.default` stays there: pnpm rejects it declared both ways.
            let mut entry_route =
                if matches!(catalog, CatalogName::Default) && workspace.catalogs.default_declared_as_named() {
                    vec![key("catalogs"), key("default")]
                } else {
                    route(catalog)
                };
            entry_route.push(key(dependency));
            FixOp::yaml(
                path.clone(),
                entry_route,
                YamlAction::Set(Value::String(version.to_owned())),
            )
        }
        CatalogFile::RootJson(path) => FixOp::json(
            path.clone(),
            json_pointer(workspace, catalog),
            JsonAction::insert(
                dependency.to_owned(),
                Value::String(version.to_owned()),
                InsertPosition::Sorted,
            ),
        ),
    }
}

/// Removes the entry of `dependency` from `catalog` in `file`.
pub(crate) fn remove_entry(
    workspace: &Workspace,
    file: &CatalogFile,
    catalog: &CatalogName,
    dependency: &str,
) -> FixOp {
    match file {
        CatalogFile::Yaml(path) => {
            let mut entry_route =
                if matches!(catalog, CatalogName::Default) && workspace.catalogs.default_declared_as_named() {
                    vec![key("catalogs"), key("default")]
                } else {
                    route(catalog)
                };
            entry_route.push(key(dependency));
            FixOp::yaml(path.clone(), entry_route, YamlAction::Remove)
        }
        CatalogFile::RootJson(path) => {
            let mut pointer = json_pointer(workspace, catalog);
            pointer.push(key(dependency));
            FixOp::json(path.clone(), pointer, JsonAction::Remove)
        }
    }
}

/// Creates the default catalog in `file` holding all of `entries`.
///
/// Every diagnostic that needs the new catalog carries this same operation with the complete entry list, so applying
/// it once or several times gives the same file; hosts that merge fixes drop the repeats.
pub(crate) fn create_default_catalog(file: &CatalogFile, entries: Map<String, Value>) -> FixOp {
    match file {
        CatalogFile::Yaml(path) => FixOp::yaml(
            path.clone(),
            vec![key("catalog")],
            YamlAction::Set(Value::Object(entries)),
        ),
        CatalogFile::RootJson(path) => FixOp::json(
            path.clone(),
            Vec::new(),
            JsonAction::insert("catalog".to_owned(), Value::Object(entries), InsertPosition::End),
        ),
    }
}
