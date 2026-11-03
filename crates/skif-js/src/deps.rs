//! Which packages and binaries the code and scripts use, against what each manifest lists.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;
use skif_core::{FileSystem, Span};
use skif_js_syntax::{ConfigValue, ImportKind, extract_config_object};
use skif_workspace::{PackageId, Workspace};

use crate::graph::{ModuleGraph, ModuleId};
use crate::lockfile;
use crate::resolve::join_normalized;
use crate::setup::Setup;
use crate::specifier::{RUNTIME_PACKAGES, package_name, typed_package, types_package};

/// Packages that are used by tooling rather than imports and never reported as unused.
const ALWAYS_USED: &[&str] = &["typescript", "knip", "skif"];

/// Packages whose `@types` twin does not need the package itself.
const UNPAIRED_TYPES: &[&str] = &["node", "bun", "jest"];

/// `@types` packages that only declare globals (no module to import), so a `tsconfig.json` that includes every
/// visible `@types/*` package is their only use. Other `@types` packages stay paired with their runtime package.
const GLOBAL_TYPES: &[&str] = &[
    "@types/chrome",
    "@types/dom-chromium-ai",
    "@types/dom-mediacapture-record",
    "@types/dom-speech-recognition",
    "@types/dom-webcodecs",
    "@types/firefox-webext-browser",
    "@types/ga-gtag",
    "@types/gapi",
    "@types/gapi.auth2",
    "@types/gapi.client",
    "@types/google.accounts",
    "@types/google.analytics",
    "@types/google.maps",
    "@types/google.picker",
    "@types/google-apps-script",
    "@types/gtag.js",
    "@types/jasmine",
    "@types/mocha",
    "@types/office-js",
    "@types/office-js-preview",
    "@types/office-runtime",
    "@types/offscreencanvas",
    "@types/serviceworker",
    "@types/sharedworker",
    "@types/trusted-types",
    "@types/w3c-web-hid",
    "@types/w3c-web-serial",
    "@types/w3c-web-usb",
    "@types/web",
    "@types/web-bluetooth",
    "@types/webextension-polyfill",
    "@types/webpack-env",
    "@types/wicg-file-system-access",
];

/// Binaries whose package has a different name. Used when the package is not installed, so its `bin` cannot be read.
const BINARY_PACKAGES: &[(&str, &str)] = &[
    ("tsc", "typescript"),
    ("tsserver", "typescript"),
    ("ng", "@angular/cli"),
    ("changeset", "@changesets/cli"),
    ("commitlint", "@commitlint/cli"),
    ("graphql-codegen", "@graphql-codegen/cli"),
    ("biome", "@biomejs/biome"),
    ("webpack", "webpack-cli"),
    ("babel", "@babel/cli"),
    ("nest", "@nestjs/cli"),
    ("svelte-kit", "@sveltejs/kit"),
    ("swc", "@swc/cli"),
    ("docusaurus", "@docusaurus/core"),
    ("sentry-cli", "@sentry/cli"),
    ("vue-cli-service", "@vue/cli-service"),
    ("lhci", "@lhci/cli"),
    ("redocly", "@redocly/cli"),
    ("tsgo", "@typescript/native-preview"),
    ("sb", "storybook"),
    ("cz", "commitizen"),
    ("firebase", "firebase-tools"),
    ("eas", "eas-cli"),
    ("netlify", "netlify-cli"),
    ("playwright", "@playwright/test"),
    ("gql-gen", "@graphql-codegen/cli"),
    ("nuxi", "nuxt"),
    ("cdk", "aws-cdk"),
    ("rspack", "@rspack/cli"),
    ("rsbuild", "@rsbuild/core"),
    ("rslib", "@rslib/core"),
    ("remix", "@remix-run/dev"),
    ("react-router", "@react-router/dev"),
    ("tailwindcss", "@tailwindcss/cli"),
    ("tauri", "@tauri-apps/cli"),
    ("cap", "@capacitor/cli"),
    ("capacitor", "@capacitor/cli"),
    ("ionic", "@ionic/cli"),
    ("ember", "ember-cli"),
    ("quasar", "@quasar/cli"),
    ("mikro-orm", "@mikro-orm/cli"),
    ("sequelize", "sequelize-cli"),
    ("sls", "serverless"),
    ("vsce", "@vscode/vsce"),
    ("ncu", "npm-check-updates"),
    ("dotenv", "dotenv-cli"),
    ("markdownlint", "markdownlint-cli"),
    ("babel-node", "@babel/node"),
    ("tspc", "ts-patch"),
    ("api-extractor", "@microsoft/api-extractor"),
    ("rush", "@microsoft/rush"),
    ("manypkg", "@manypkg/cli"),
    ("graphql-inspector", "@graphql-inspector/cli"),
    ("graphql-codegen-esm", "@graphql-codegen/cli"),
    ("buf", "@bufbuild/buf"),
    ("protoc-gen-es", "@bufbuild/protoc-gen-es"),
    ("pbjs", "protobufjs-cli"),
    ("pbts", "protobufjs-cli"),
    ("wdio", "@wdio/cli"),
    ("percy", "@percy/cli"),
    ("electron-forge", "@electron-forge/cli"),
    ("start-storybook", "@storybook/cli"),
    ("build-storybook", "@storybook/cli"),
    ("start-test", "start-server-and-test"),
    ("server-test", "start-server-and-test"),
    ("jest-cli", "jest"),
    ("ts-node-esm", "ts-node"),
    ("ts-node-cwd", "ts-node"),
    ("prettier-eslint", "prettier-eslint-cli"),
    ("openapi-generator-cli", "@openapitools/openapi-generator-cli"),
    ("pm2-runtime", "pm2"),
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
    /// Where the import is written: the import statement, or the `@jsxImportSource` pragma.
    pub span: Option<Span>,
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

/// The binaries and peer dependencies a package declares.
pub(crate) struct Installed {
    pub(crate) bins: Vec<String>,
    pub(crate) peers: Vec<String>,
}

/// The binary names a manifest declares: the package's own name for a `bin` string, the keys of a `bin` map.
pub(crate) fn bins_of(manifest: &Value, name: &str) -> Vec<String> {
    match manifest.get("bin") {
        Some(Value::String(_)) => vec![unscoped(name).to_owned()],
        Some(Value::Object(map)) => map.keys().cloned().collect(),
        _ => Vec::new(),
    }
}

/// The names a manifest lists under `peerDependencies`.
pub(crate) fn peers_of(manifest: &Value) -> Vec<String> {
    manifest
        .get("peerDependencies")
        .and_then(Value::as_object)
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default()
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
    Some(Arc::new(Installed {
        bins: bins_of(&value, name),
        peers: peers_of(&value),
    }))
}

/// Whether `dir` is where the repository starts: it holds the `.git` entry.
fn is_repository_root(fs: &dyn FileSystem, dir: &Path) -> bool {
    let git = dir.join(".git");
    fs.is_dir(&git) || fs.is_file(&git)
}

pub(crate) fn parse_tsconfig(fs: &dyn FileSystem, path: &Path) -> Option<ConfigValue> {
    let text = fs.read_to_string(path).ok()?;
    extract_config_object(&format!("module.exports = {text}"), Path::new("tsconfig.js"))
}

/// Whether the `tsconfig.json` at `path` makes TypeScript include every installed `@types/*` package: it exists, no
/// `compilerOptions.types` is set along its `extends` chain, and `typeRoots`, when set, keeps `node_modules/@types`.
fn includes_all_types(fs: &dyn FileSystem, path: &Path, depth: usize) -> bool {
    const MAX_DEPTH: usize = 8;
    let Some(config) = parse_tsconfig(fs, path) else {
        return false;
    };
    if let Some(options) = config.get("compilerOptions") {
        if options.get("types").is_some() {
            return false;
        }
        if let Some(ConfigValue::Array(roots)) = options.get("typeRoots") {
            let keeps_default = roots.iter().filter_map(ConfigValue::as_str).any(|root| {
                let root = root.trim_end_matches('/');
                root.ends_with("node_modules/@types") || root == "@types"
            });
            if !keeps_default {
                return false;
            }
        }
    }
    let Some(extends) = config.get("extends") else {
        return true;
    };
    let parents: Vec<&str> = match extends {
        ConfigValue::String(one) => vec![one.as_str()],
        ConfigValue::Array(many) => many.iter().filter_map(ConfigValue::as_str).collect(),
        _ => Vec::new(),
    };
    let dir = path.parent().unwrap_or(path);
    parents.iter().filter(|parent| parent.starts_with('.')).all(|parent| {
        let mut target = join_normalized(dir, parent);
        if target.extension().is_none_or(|extension| extension != "json") {
            target.as_mut_os_string().push(".json");
        }
        depth >= MAX_DEPTH || includes_all_types(fs, &target, depth + 1)
    })
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
    /// Binaries that `node_modules/.bin` directories of ancestor projects put on the `PATH`.
    ancestor_bins: HashSet<String>,
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
        usage.collect_ambient_types(workspace, fs);
        usage.collect_ancestor_bins(workspace, fs);
        usage.collect_member_bins(workspace);
        usage.collect_imports(workspace, setup, graph);
        usage.collect_scripts(workspace, setup);
        usage
    }

    fn read_manifests(&mut self, workspace: &Workspace, fs: &dyn FileSystem) {
        let mut cache = HashMap::new();
        let locked = lockfile::read(fs, &workspace.root);
        for (id, package) in workspace.iter() {
            let dir = workspace.root.join(&package.dir);
            for entry in &package.dependencies {
                self.listed[id.index()].insert(entry.name.clone());
            }
            let names: Vec<String> = self.listed[id.index()].iter().cloned().collect();
            for name in names {
                let Some(installed) =
                    read_installed(fs, &dir, &workspace.root, &name, &mut cache).or_else(|| locked.get(&name).cloned())
                else {
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

    /// TypeScript includes every visible `@types/*` package unless `compilerOptions.types` narrows them, so the
    /// globals-only ones a package (and the root, which hoists them) lists are used when the package has a
    /// `tsconfig.json` that does not narrow them.
    fn collect_ambient_types(&mut self, workspace: &Workspace, fs: &dyn FileSystem) {
        for (id, package) in workspace.iter() {
            if !includes_all_types(fs, &workspace.root.join(&package.dir).join("tsconfig.json"), 0) {
                continue;
            }
            for scope in [id, PackageId::ROOT] {
                let types: Vec<String> = self.listed[scope.index()]
                    .iter()
                    .filter(|name| GLOBAL_TYPES.contains(&name.as_str()))
                    .cloned()
                    .collect();
                for name in types {
                    self.reference(scope, &name);
                }
            }
        }
    }

    /// Binaries of the dependencies listed by the directories above the workspace, up to the repository root.
    fn collect_ancestor_bins(&mut self, workspace: &Workspace, fs: &dyn FileSystem) {
        let mut cache = HashMap::new();
        let mut dir = workspace.root.as_path();
        while !is_repository_root(fs, dir) {
            let Some(parent) = dir.parent() else { break };
            dir = parent;
            let Some(manifest) = fs
                .read_to_string(&dir.join("package.json"))
                .ok()
                .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            else {
                continue;
            };
            for section in [
                "dependencies",
                "devDependencies",
                "optionalDependencies",
                "peerDependencies",
            ] {
                let Some(listed) = manifest.get(section).and_then(Value::as_object) else {
                    continue;
                };
                for name in listed.keys() {
                    self.ancestor_bins.insert(unscoped(name).to_owned());
                    self.ancestor_bins.extend(
                        BINARY_PACKAGES
                            .iter()
                            .filter(|(_, package)| *package == name.as_str())
                            .map(|(bin, _)| (*bin).to_owned()),
                    );
                    if let Some(installed) = read_installed(fs, dir, dir, name, &mut cache) {
                        self.ancestor_bins.extend(installed.bins.iter().cloned());
                    }
                }
            }
        }
    }

    fn collect_member_bins(&mut self, workspace: &Workspace) {
        for package in workspace.all_packages() {
            let Some(name) = package.name.as_deref() else { continue };
            for bin in bins_of(&package.manifest.value, name) {
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
                    let span = module.info.imports.get(resolved.import).map(|import| import.span);
                    self.handle_import(workspace, module.owner, name, type_only, module.id, span);
                }
            }
            if let Some(source) = module.info.jsx_import_source.as_deref().and_then(package_name) {
                let span = module.info.jsx_import_source_span;
                self.handle_import(workspace, module.owner, &source, false, module.id, span);
            } else if module.info.has_jsx
                && let Some(found) = self.find_listed(module.owner, "react")
            {
                // The automatic JSX runtime imports `react/jsx-runtime` on behalf of the file.
                self.reference(found, "react");
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
        span: Option<Span>,
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
            span,
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
            for binary in &package_setup.scripts.ephemeral_binaries {
                self.provide_binary(id, &binary.name);
            }
            for manager in &package_setup.scripts.package_managers {
                if let Some(found) = self.find_listed(id, manager) {
                    self.reference(found, manager);
                }
            }
            for binary in &package_setup.scripts.binaries {
                if !self.provide_binary(id, &binary.name)
                    && !self.ancestor_bins.contains(&binary.name)
                    && !setup.ignores_binary(id, &binary.name)
                {
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
