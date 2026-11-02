//! Which packages and binaries the code and scripts use, against what each manifest lists.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;
use skif_core::FileSystem;
use skif_js_syntax::ImportKind;
use skif_workspace::{PackageId, Workspace};

use crate::graph::{ModuleGraph, ModuleId};
use crate::setup::Setup;
use crate::specifier::{RUNTIME_PACKAGES, package_name, typed_package, types_package};

/// Packages that are used by tooling rather than imports and never reported as unused.
const ALWAYS_USED: &[&str] = &["typescript", "knip", "skif"];

/// Packages whose `@types` twin does not need the package itself.
const UNPAIRED_TYPES: &[&str] = &["node", "bun", "jest"];

/// Binaries whose package has a different name.
const BINARY_PACKAGES: &[(&str, &str)] = &[
    ("tsc", "typescript"),
    ("tsserver", "typescript"),
    ("ng", "@angular/cli"),
    ("changeset", "@changesets/cli"),
    ("commitlint", "@commitlint/cli"),
    ("graphql-codegen", "@graphql-codegen/cli"),
    ("biome", "@biomejs/biome"),
    ("webpack", "webpack-cli"),
];

/// An import of a package the manifest does not list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnlistedImport {
    /// The package whose file imports it.
    pub package: PackageId,
    /// The imported package.
    pub name: String,
    /// The importing module.
    pub module: ModuleId,
    /// Index into the importing module's imports.
    pub import: usize,
}

/// A script binary that no dependency provides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnlistedBinary {
    /// The package whose script runs it.
    pub package: PackageId,
    /// The binary name.
    pub name: String,
    /// The script that runs it.
    pub script: String,
}

struct Installed {
    bins: Vec<String>,
    peers: Vec<String>,
}

fn unscoped(name: &str) -> &str {
    name.rsplit('/').next().unwrap_or(name)
}

fn read_installed(
    fs: &dyn FileSystem,
    start: &Path,
    root: &Path,
    name: &str,
    cache: &mut HashMap<PathBuf, Option<Arc<Installed>>>,
) -> Option<Arc<Installed>> {
    let mut dir = start;
    loop {
        let manifest = dir.join("node_modules").join(name).join("package.json");
        let entry = cache.entry(manifest.clone()).or_insert_with(|| {
            fs.read_to_string(&manifest)
                .ok()
                .and_then(|text| parse_installed(&text, name))
        });
        if entry.is_some() {
            return entry.clone();
        }
        if dir == root {
            return None;
        }
        dir = dir.parent()?;
        if !dir.starts_with(root) {
            return None;
        }
    }
}

fn parse_installed(text: &str, name: &str) -> Option<Arc<Installed>> {
    let value: Value = serde_json::from_str(text).ok()?;
    let bins = match value.get("bin") {
        Some(Value::String(_)) => vec![unscoped(name).to_owned()],
        Some(Value::Object(map)) => map.keys().cloned().collect(),
        _ => Vec::new(),
    };
    let peers = value
        .get("peerDependencies")
        .and_then(Value::as_object)
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default();
    Some(Arc::new(Installed { bins, peers }))
}

/// Dependency and binary usage of every package.
#[derive(Debug, Clone, Default)]
pub struct DependencyUsage {
    referenced: Vec<HashSet<String>>,
    listed: Vec<HashSet<String>>,
    hosts: Vec<HashMap<String, Vec<String>>>,
    bins: Vec<HashMap<String, Vec<String>>>,
    member_bins: BTreeMap<String, Vec<String>>,
    members: HashSet<String>,
    /// Imports of packages that are not listed, in module order.
    pub unlisted: Vec<UnlistedImport>,
    /// Script binaries that nothing provides, in package order.
    pub unlisted_binaries: Vec<UnlistedBinary>,
}

impl DependencyUsage {
    pub(crate) fn compute(workspace: &Workspace, setup: &Setup, graph: &ModuleGraph, fs: &dyn FileSystem) -> Self {
        let count = workspace.package_ids().count();
        let mut usage = Self {
            referenced: vec![HashSet::new(); count],
            listed: vec![HashSet::new(); count],
            hosts: vec![HashMap::new(); count],
            bins: vec![HashMap::new(); count],
            members: workspace.all_packages().filter_map(|p| p.name.clone()).collect(),
            ..Self::default()
        };
        usage.read_manifests(workspace, fs);
        usage.collect_member_bins(workspace);
        usage.collect_imports(workspace, setup, graph);
        usage.collect_scripts(workspace, setup);
        usage
    }

    fn read_manifests(&mut self, workspace: &Workspace, fs: &dyn FileSystem) {
        let mut cache = HashMap::new();
        for (id, package) in workspace.iter() {
            let dir = workspace.root.join(&package.dir);
            for entry in &package.dependencies {
                self.listed[id.index()].insert(entry.name.clone());
            }
            let names: Vec<String> = self.listed[id.index()].iter().cloned().collect();
            for name in names {
                let Some(installed) = read_installed(fs, &dir, &workspace.root, &name, &mut cache) else {
                    continue;
                };
                for bin in &installed.bins {
                    self.bins[id.index()].entry(bin.clone()).or_default().push(name.clone());
                }
                for peer in &installed.peers {
                    self.hosts[id.index()]
                        .entry(peer.clone())
                        .or_default()
                        .push(name.clone());
                }
            }
        }
    }

    fn collect_member_bins(&mut self, workspace: &Workspace) {
        for package in workspace.all_packages() {
            let Some(name) = package.name.as_deref() else { continue };
            let bins: Vec<String> = match package.manifest.value.get("bin") {
                Some(Value::String(_)) => vec![unscoped(name).to_owned()],
                Some(Value::Object(map)) => map.keys().cloned().collect(),
                _ => continue,
            };
            for bin in bins {
                self.member_bins.entry(bin).or_default().push(name.to_owned());
            }
        }
    }

    /// The closest manifest that lists `name`: the package itself, then the root.
    fn find_listed(&self, id: PackageId, name: &str) -> Option<PackageId> {
        if self.listed[id.index()].contains(name) {
            Some(id)
        } else if id != PackageId::ROOT && self.listed[PackageId::ROOT.index()].contains(name) {
            Some(PackageId::ROOT)
        } else {
            None
        }
    }

    fn reference(&mut self, id: PackageId, name: &str) {
        self.referenced[id.index()].insert(name.to_owned());
    }

    fn collect_imports(&mut self, workspace: &Workspace, setup: &Setup, graph: &ModuleGraph) {
        for module in graph.modules() {
            if setup.package(module.owner).is_none() {
                continue;
            }
            for resolved in &module.imports {
                if let Some(name) = &resolved.package {
                    let kind = module.info.imports.get(resolved.import).map(|import| import.kind);
                    let type_only = resolved.type_only && kind != Some(ImportKind::ExportFrom);
                    self.handle_import(workspace, module.owner, name, type_only, module.id, resolved.import);
                }
            }
            if let Some(source) = module.info.jsx_import_source.as_deref().and_then(package_name) {
                self.handle_import(workspace, module.owner, &source, false, module.id, 0);
            }
        }
    }

    fn handle_import(
        &mut self,
        workspace: &Workspace,
        owner: PackageId,
        name: &str,
        type_only: bool,
        module: ModuleId,
        import: usize,
    ) {
        if RUNTIME_PACKAGES.contains(&name) {
            return;
        }
        let Some(package) = workspace.package(owner) else {
            return;
        };
        if package.name.as_deref() == Some(name) {
            return;
        }
        let types = types_package(name).and_then(|types| self.find_listed(owner, &types).map(|found| (found, types)));
        if let Some((found, types)) = &types {
            self.reference(*found, types);
        }
        if let Some(found) = self.find_listed(owner, name) {
            self.reference(found, name);
            return;
        }
        if type_only && types.is_some() {
            return;
        }
        if package
            .manifest
            .value
            .get("engines")
            .and_then(|engines| engines.get(name))
            .is_some()
        {
            return;
        }
        if package.private && self.members.contains(name) {
            self.reference(owner, name);
            return;
        }
        self.unlisted.push(UnlistedImport {
            package: owner,
            name: name.to_owned(),
            module,
            import,
        });
    }

    fn collect_scripts(&mut self, workspace: &Workspace, setup: &Setup) {
        for (id, _) in workspace.iter() {
            let Some(package_setup) = setup.package(id) else {
                continue;
            };
            for name in &package_setup.referenced_dependencies {
                if let Some(found) = self.find_listed(id, name) {
                    self.reference(found, name);
                }
            }
            for bin in &package_setup.referenced_binaries {
                self.provide_binary(id, bin);
            }
            for binary in &package_setup.scripts.binaries {
                if !self.provide_binary(id, &binary.name) && !setup.ignores_binary(id, &binary.name) {
                    self.unlisted_binaries.push(UnlistedBinary {
                        package: id,
                        name: binary.name.clone(),
                        script: binary.script.clone(),
                    });
                }
            }
        }
    }

    /// Marks the dependencies that provide binary `bin` as used by `id`. False when none does.
    fn provide_binary(&mut self, id: PackageId, bin: &str) -> bool {
        let mut provided: Vec<(PackageId, String)> = Vec::new();
        let scopes = [id, PackageId::ROOT];
        for scope in scopes.into_iter().take(if id == PackageId::ROOT { 1 } else { 2 }) {
            if let Some(deps) = self.bins[scope.index()].get(bin) {
                provided.extend(deps.iter().map(|dep| (scope, dep.clone())));
            }
            provided.extend(
                self.listed[scope.index()]
                    .iter()
                    .filter(|dep| {
                        unscoped(dep) == bin || BINARY_PACKAGES.iter().any(|(b, package)| *b == bin && package == dep)
                    })
                    .map(|dep| (scope, dep.clone())),
            );
        }
        if let Some(members) = self.member_bins.get(bin).cloned() {
            for member in members {
                if let Some(found) = self.find_listed(id, &member) {
                    provided.push((found, member));
                } else {
                    provided.push((id, String::new()));
                }
            }
        }
        let found = !provided.is_empty();
        for (scope, dep) in provided {
            if !dep.is_empty() {
                self.reference(scope, &dep);
            }
        }
        found
    }

    /// Whether package `id` uses dependency `name`, directly, through its `@types` twin, or as a peer of a used host.
    #[must_use]
    pub fn is_referenced(&self, id: PackageId, name: &str) -> bool {
        if ALWAYS_USED.contains(&name) {
            return true;
        }
        self.is_referenced_inner(id, name, &mut HashSet::new())
    }

    fn is_referenced_inner(&self, id: PackageId, name: &str, visited: &mut HashSet<String>) -> bool {
        let referenced = &self.referenced[id.index()];
        if referenced.contains(name) {
            return true;
        }
        if !visited.insert(name.to_owned()) {
            return false;
        }
        let typed = typed_package(name);
        if let Some(typed) = &typed
            && (UNPAIRED_TYPES.contains(&typed.as_str()) || referenced.contains(typed))
        {
            return true;
        }
        let hosts = &self.hosts[id.index()];
        std::iter::once(name)
            .chain(typed.as_deref())
            .filter_map(|candidate| hosts.get(candidate))
            .flatten()
            .any(|host| self.is_referenced_inner(id, host, visited))
    }

    /// Whether anything used `name` through package `id`'s manifest, regardless of the `@types` and peer rules.
    #[must_use]
    pub fn was_imported(&self, id: PackageId, name: &str) -> bool {
        self.referenced[id.index()].contains(name)
    }

    /// Whether the manifest of `id` lists `name` in any section.
    #[must_use]
    pub fn is_listed(&self, id: PackageId, name: &str) -> bool {
        self.listed[id.index()].contains(name)
    }
}
