//! Which packages and binaries the code and scripts use, against what each manifest lists.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use serde_json::Value;
use skif_core::{DependencyKind, FileSystem, Span};
use skif_js_plugins::scripts::{ScriptContext, is_ignored_binary, parse_script};
use skif_js_syntax::{ConfigValue, ImportKind, extract_config_object};
use skif_workspace::{Package, PackageId, Workspace, map_ordered};

use crate::ancestors::{external_manifests, is_repository_root, listed_names, read_manifest};
use crate::discover::{is_ignored_in_repository, relative};
use crate::fixture::in_fixture_directory;
use crate::graph::{ModuleGraph, ModuleId, Target};
use crate::input::ConfigDependency;
use crate::lockfile;
use crate::resolve::is_installed;
use crate::scripts::{ScriptBinary, script_context};
use crate::setup::Setup;
use crate::specifier::{RUNTIME_PACKAGES, package_name, typed_package, types_package};
use crate::tsconfig_paths::resolve_extends;

/// Where a dependency an import names is listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Listed {
    /// In the manifest of this package (the importer or a package that contains it, or the root).
    Package(PackageId),
    /// In a manifest above the workspace root, which this run does not judge; the use is recorded for the root that
    /// owns that manifest ([`DependencyUsage::ancestor_uses`]).
    External,
}

/// Packages that are used by tooling rather than imports and never reported as unused.
const ALWAYS_USED: &[&str] = &["typescript", "knip", "skif"];

/// The packages that compile a file type skif extracts imports from without running a compiler (ADR 0001): a file
/// of that type in the graph uses those of them its package lists, the way knip's built-in compilers mark them.
const COMPILER_DEPENDENCIES: &[(&str, &[&str])] = &[
    ("marko", &["marko"]),
    (
        "mdx",
        &[
            "@mdx-js/esbuild",
            "@mdx-js/loader",
            "@mdx-js/mdx",
            "@mdx-js/node-loader",
            "@mdx-js/preact",
            "@mdx-js/react",
            "@mdx-js/rollup",
            "@mdx-js/vue",
            "remark-mdx",
        ],
    ),
    ("styl", &["stylus"]),
    (
        "tsrx",
        &[
            "@tsrx/core",
            "@tsrx/react",
            "@tsrx/preact",
            "@tsrx/vue",
            "@tsrx/solid",
            "@tsrx/ripple",
        ],
    ),
    ("stylus", &["stylus"]),
];

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

/// Where a package import is written, as [`DependencyUsage::handle_import`] judges it.
struct ImportSite<'s> {
    /// The package whose file imports it.
    owner: PackageId,
    /// The specifier as written.
    specifier: &'s str,
    /// What the specifier resolved to; `None` for a `@jsxImportSource` pragma, which is not resolved.
    target: Option<&'s Target>,
    module: ModuleId,
    span: Option<Span>,
    /// It brings in types only and is not a re-export, so an `@types` package can stand in for the package.
    type_only: bool,
    /// The importing file is a declaration file (`.d.ts`, `.d.mts`, `.d.cts`).
    declaration: bool,
}

/// Whether `path` is a TypeScript declaration file.
fn is_declaration_file(path: &Path) -> bool {
    path.file_name().and_then(|name| name.to_str()).is_some_and(|name| {
        [".d.ts", ".d.mts", ".d.cts"]
            .iter()
            .any(|suffix| name.ends_with(suffix))
    })
}

/// An import of a package the manifest does not list.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct UnlistedImport {
    /// The package whose file imports it.
    pub package: PackageId,
    /// The imported package.
    pub name: String,
    /// The importing module.
    pub module: ModuleId,
    /// Where the import is written: the import statement, or the `@jsxImportSource` pragma.
    pub span: Option<Span>,
    /// The specifier as written, such as `name/subpath`.
    pub specifier: String,
    /// What the specifier resolved to: a module of the graph, a package by name (installed or not) or nothing. `None`
    /// for a `@jsxImportSource` pragma, which is not resolved.
    pub target: Option<Target>,
    /// The `paths` pattern of another package's TypeScript configuration that maps the specifier, when the
    /// configuration the importing file follows maps it not.
    pub foreign_paths: Option<ForeignMapping>,
}

/// A `paths` pattern of a TypeScript configuration the importing file does not follow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignMapping {
    /// The configuration file that declares the pattern (the one found for a package, or a configuration it
    /// extends), relative to the workspace root with `/` separators.
    pub config: String,
    /// The pattern, as written.
    pub pattern: String,
}

/// A binary that a script or a command read from a file runs and no dependency provides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnlistedBinary {
    /// The package whose script or file runs it.
    pub package: PackageId,
    /// The binary name.
    pub name: String,
    /// The `package.json` script that runs it; empty when `file` is set.
    pub script: String,
    /// The file whose command runs it, relative to the workspace root with `/` separators, when that is not a
    /// `package.json` script: a workflow, hook or tool config a plugin reads, or a source file that runs it through
    /// `node:child_process`, `execa` or `zx`.
    pub file: Option<PathBuf>,
}

/// A package that a tool's config file names and no manifest lists; see [`DependencyUsage::unlisted_config`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnlistedConfigDependency {
    /// The package that owns the config file: the innermost workspace package whose directory holds it.
    pub package: PackageId,
    /// The package the config names.
    pub name: String,
    /// The config file, relative to the workspace root with `/` separators.
    pub file: PathBuf,
    /// The packages reading the config that could have listed the package in place of `package` and do not: those a
    /// compiler option of a shared base `tsconfig` applies to. Empty for any other reference, which only `package`
    /// can satisfy.
    pub readers: Vec<PackageId>,
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

/// Words too common in package names to tie a binary to a package.
const GENERIC_WORDS: &[&str] = &[
    "cli", "core", "dev", "kit", "tool", "tools", "js", "node", "bin", "run", "start",
];

fn words(name: &str) -> impl Iterator<Item = &str> {
    name.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| word.len() > 1 && !GENERIC_WORDS.contains(word))
}

/// Whether the package `dependency` is named like the binary `bin`: they share a word, or one name starts with the
/// other.
fn resembles(dependency: &str, bin: &str) -> bool {
    let short = unscoped(dependency);
    (bin.len() > 2 && short.starts_with(bin))
        || (short.len() > 2 && bin.starts_with(short))
        || words(dependency).any(|word| words(bin).any(|other| other == word))
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
        let package = dir.join("node_modules").join(name);
        let manifest = package.join("package.json");
        let entry = cache.entry(manifest.clone()).or_insert_with(|| {
            fs.read_to_string(&manifest)
                .ok()
                .and_then(|text| parse_installed(fs, &package, &text, name))
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

fn parse_installed(fs: &dyn FileSystem, dir: &Path, text: &str, name: &str) -> Option<Arc<Installed>> {
    let value: Value = serde_json::from_str(text).ok()?;
    let bins = if value.get("bin").is_some() {
        bins_of(&value, name)
    } else {
        directory_bins(fs, dir, &value)
    };
    Some(Arc::new(Installed {
        bins,
        peers: peers_of(&value),
    }))
}

/// The binaries the `directories.bin` of the installed package at `dir` declares, for a manifest without `bin`: every
/// file below that directory, named by its file name, as npm links them. As in npm, the directory is resolved as if
/// the package were the file system root, so `..` cannot leave the package, and a file or directory whose name starts
/// with `.` is skipped.
fn directory_bins(fs: &dyn FileSystem, dir: &Path, manifest: &Value) -> Vec<String> {
    let Some(bin_dir) = manifest
        .get("directories")
        .and_then(|directories| directories.get("bin"))
        .and_then(Value::as_str)
    else {
        return Vec::new();
    };
    let mut pending = vec![dir.join(clamped(Path::new(bin_dir)))];
    let mut names = BTreeSet::new();
    while let Some(current) = pending.pop() {
        let Ok(entries) = fs.read_dir(&current) else {
            continue;
        };
        for entry in entries {
            let Some(name) = entry.path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if name.starts_with('.') {
                continue;
            }
            if entry.is_dir && !entry.is_symlink {
                pending.push(entry.path);
            } else if entry.is_file {
                names.insert(name.to_owned());
            }
        }
    }
    names.into_iter().collect()
}

/// `path` joined onto a root it cannot climb out of: `..` drops the previous component, or nothing at the top, and a
/// root or prefix is dropped, as npm's `path.join("/", path)` does for `directories.bin`.
fn clamped(path: &Path) -> PathBuf {
    let mut parts: Vec<&OsStr> = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => parts.push(part),
            Component::ParentDir => {
                parts.pop();
            }
            Component::CurDir | Component::RootDir | Component::Prefix(_) => {}
        }
    }
    parts.into_iter().collect()
}

pub(crate) fn parse_tsconfig(fs: &dyn FileSystem, path: &Path) -> Option<ConfigValue> {
    let text = fs.read_to_string(path).ok()?;
    extract_config_object(&format!("module.exports = {text}"), Path::new("tsconfig.js"))
}

/// The `tsconfig` files parsed so far in a run: a base config that every package extends is parsed once.
struct TsconfigMemo<'a> {
    fs: &'a dyn FileSystem,
    parsed: Mutex<HashMap<PathBuf, Option<Arc<ConfigValue>>>>,
}

impl<'a> TsconfigMemo<'a> {
    fn new(fs: &'a dyn FileSystem) -> Self {
        Self {
            fs,
            parsed: Mutex::new(HashMap::new()),
        }
    }

    fn get(&self, path: &Path) -> Option<Arc<ConfigValue>> {
        if let Some(known) = self.parsed.lock().unwrap_or_else(PoisonError::into_inner).get(path) {
            return known.clone();
        }
        let config = parse_tsconfig(self.fs, path).map(Arc::new);
        self.parsed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(path.to_path_buf(), config.clone());
        config
    }
}

/// Whether the `tsconfig.json` at `path` makes TypeScript include every installed `@types/*` package: it exists, no
/// `compilerOptions.types` is set along its `extends` chain, and `typeRoots`, when set, keeps `node_modules/@types`.
fn includes_all_types(memo: &TsconfigMemo<'_>, members: &[(String, PathBuf)], path: &Path, depth: usize) -> bool {
    const MAX_DEPTH: usize = 8;
    let Some(config) = memo.get(path) else {
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
    let dir = path.parent().unwrap_or(path);
    extends_of(&config)
        .iter()
        .filter_map(|parent| resolve_extends(memo.fs, members, dir, parent))
        .all(|target| depth >= MAX_DEPTH || includes_all_types(memo, members, &target, depth + 1))
}

/// The `extends` entries of a parsed `tsconfig.json`, in order.
fn extends_of(config: &ConfigValue) -> Vec<&str> {
    match config.get("extends") {
        Some(ConfigValue::String(one)) => vec![one.as_str()],
        Some(ConfigValue::Array(many)) => many.iter().filter_map(ConfigValue::as_str).collect(),
        _ => Vec::new(),
    }
}

/// The `compilerOptions.jsxImportSource` the `tsconfig.json` at `path` sets itself or inherits; a later `extends`
/// entry overrides an earlier one.
fn jsx_import_source(
    memo: &TsconfigMemo<'_>,
    members: &[(String, PathBuf)],
    path: &Path,
    depth: usize,
) -> Option<String> {
    const MAX_DEPTH: usize = 8;
    let config = memo.get(path)?;
    if let Some(source) = config
        .get("compilerOptions")
        .and_then(|options| options.get("jsxImportSource"))
        .and_then(ConfigValue::as_str)
    {
        return Some(source.to_owned());
    }
    if depth >= MAX_DEPTH {
        return None;
    }
    let dir = path.parent().unwrap_or(path);
    extends_of(&config)
        .iter()
        .rev()
        .filter_map(|parent| resolve_extends(memo.fs, members, dir, parent))
        .find_map(|target| jsx_import_source(memo, members, &target, depth + 1))
}

/// The workspace members that have a name, with their absolute directory.
pub(crate) fn member_dirs(workspace: &Workspace) -> Vec<(String, PathBuf)> {
    workspace
        .all_packages()
        .filter_map(|package| Some((package.name.clone()?, workspace.root.join(&package.dir))))
        .collect()
}

/// The innermost workspace package whose directory holds `file` (absolute); `None` for a file outside every package.
fn config_owner(workspace: &Workspace, setup: &Setup, file: &Path) -> Option<PackageId> {
    workspace
        .iter()
        .filter_map(|(id, _)| setup.package(id).map(|package_setup| (id, &package_setup.dir)))
        .filter(|(_, dir)| file.starts_with(dir))
        .max_by_key(|(_, dir)| dir.components().count())
        .map(|(id, _)| id)
}

/// The owner ([`config_owner`]) of the config `file` (absolute) and its path relative to the root, when the packages
/// it names are judged: not for a config outside the root, in `node_modules`, that git ignores (generated, such as
/// `.nuxt/tsconfig.json`, unless `--no-gitignore`), in a fixture directory (unless `[js] include-fixtures`) or that an
/// `ignore` pattern of the root or of its owner matches.
fn judged_config(
    workspace: &Workspace,
    setup: &Setup,
    fs: &dyn FileSystem,
    file: &Path,
) -> Option<(Option<PackageId>, String)> {
    let rel = relative(file, &workspace.root)?;
    let in_node_modules = rel.split('/').any(|part| part == "node_modules");
    let in_fixture = !setup.include_fixtures && in_fixture_directory(Path::new(&rel));
    if in_node_modules || in_fixture || setup.global_ignore.matches_any(&rel) {
        return None;
    }
    if setup.gitignore && is_ignored_in_repository(fs, &workspace.root, file) {
        return None;
    }
    let owner = config_owner(workspace, setup, file);
    let ignored = owner.and_then(|owner| setup.package(owner)).is_some_and(|owner_setup| {
        relative(file, &owner_setup.dir).is_some_and(|rel| owner_setup.ignore.matches_any(&rel))
    });
    (!ignored).then_some((owner, rel))
}

/// For every package, the packages whose directory contains it, innermost first, and the root last. With `isolate`
/// (`--strict`) none: each package uses only what its own manifest lists.
fn package_ancestors(workspace: &Workspace, isolate: bool) -> Vec<Vec<PackageId>> {
    if isolate {
        return vec![Vec::new(); workspace.package_ids().count()];
    }
    let root = workspace.iter().find(|(_, package)| package.is_root).map(|(id, _)| id);
    let mut all = vec![Vec::new(); workspace.package_ids().count()];
    for (id, package) in workspace.iter() {
        let mut containing: Vec<(PackageId, usize)> = workspace
            .iter()
            .filter(|(other_id, other)| {
                *other_id != id && !other.is_root && package.dir != other.dir && package.dir.starts_with(&other.dir)
            })
            .map(|(other_id, other)| (other_id, other.dir.components().count()))
            .collect();
        containing.sort_by_key(|(_, depth)| std::cmp::Reverse(*depth));
        all[id.index()] = containing
            .into_iter()
            .map(|(other, _)| other)
            .chain(root.filter(|root| *root != id))
            .collect();
    }
    all
}

/// Dependency and binary usage of every package.
#[derive(Debug, Clone, Default)]
pub struct DependencyUsage {
    referenced: Vec<HashSet<String>>,
    listed: Vec<HashSet<String>>,
    /// For every package, the names its manifest lists in `dependencies` and `peerDependencies`: what production
    /// code may import in strict mode.
    production_listed: Vec<HashSet<String>>,
    /// knip's strict mode (`skif --strict`): type-only imports use nothing, and production code uses only the
    /// `dependencies` and `peerDependencies` of its own package.
    strict: bool,
    /// For every package, the packages whose directory contains it, innermost first and the root last.
    ancestors: Vec<Vec<PackageId>>,
    /// For every package, the listed dependencies whose binaries are known: they are installed or in the lockfile.
    known_bins: Vec<HashSet<String>>,
    /// For every package, whether its `tsconfig.json` sets a `jsxImportSource` other than `react`, so JSX files do not
    /// use React.
    other_jsx_runtime: Vec<bool>,
    /// Names listed by the manifests above the workspace root, each with the directory of the innermost manifest that
    /// lists it: Node.js resolves it from there.
    external_listed: HashMap<String, PathBuf>,
    /// The names of `external_listed` that this workspace uses.
    external_used: BTreeSet<String>,
    /// The packages files outside the workspace root import, with the directory of the manifest that lists each; see
    /// [`Self::collect_outside_imports`].
    outside_used: BTreeSet<(PathBuf, String)>,
    hosts: Vec<HashMap<String, Vec<String>>>,
    bins: Vec<HashMap<String, Vec<String>>>,
    member_bins: BTreeMap<String, Vec<String>>,
    members: HashSet<String>,
    /// Binaries that `node_modules/.bin` directories of ancestor projects put on the `PATH`, each with the dependencies
    /// of those projects that provide it.
    ancestor_bins: HashMap<String, Vec<String>>,
    /// Imports of packages that are not listed, in module order.
    pub unlisted: Vec<UnlistedImport>,
    /// Script binaries that nothing provides, in package order.
    pub unlisted_binaries: Vec<UnlistedBinary>,
    /// Packages that tool configs name and no manifest lists, in package order.
    pub unlisted_config: Vec<UnlistedConfigDependency>,
}

impl DependencyUsage {
    pub(crate) fn compute(
        workspace: &Workspace,
        setup: &Setup,
        graph: &ModuleGraph,
        fs: &dyn FileSystem,
        sequential: bool,
    ) -> Self {
        let count = workspace.package_ids().count();
        let mut usage = Self {
            referenced: vec![HashSet::new(); count],
            listed: vec![HashSet::new(); count],
            production_listed: vec![HashSet::new(); count],
            strict: setup.isolate_workspaces,
            known_bins: vec![HashSet::new(); count],
            other_jsx_runtime: vec![false; count],
            ancestors: package_ancestors(workspace, setup.isolate_workspaces),
            hosts: vec![HashMap::new(); count],
            bins: vec![HashMap::new(); count],
            members: workspace.all_packages().filter_map(|p| p.name.clone()).collect(),
            ..Self::default()
        };
        usage.read_manifests(workspace, fs, sequential);
        let tsconfigs = TsconfigMemo::new(fs);
        usage.collect_ambient_types(workspace, setup, &tsconfigs);
        usage.collect_jsx_runtimes(workspace, setup, &tsconfigs);
        usage.collect_ancestor_bins(workspace, fs);
        usage.collect_member_bins(workspace);
        usage.collect_imports(workspace, setup, graph);
        usage.collect_outside_imports(workspace, graph, fs);
        usage.collect_scripts(workspace, setup);
        usage.collect_config_dependencies(workspace, setup, fs);
        usage
    }

    fn read_manifests(&mut self, workspace: &Workspace, fs: &dyn FileSystem, sequential: bool) {
        let locked = lockfile::read(fs, &workspace.root);
        // A workspace package is linked into `node_modules` on install; without an install its own manifest says the
        // same.
        let linked: HashMap<String, Arc<Installed>> = workspace
            .all_packages()
            .filter_map(|package| {
                let name = package.name.clone()?;
                let manifest = package.manifest.value();
                let installed = Installed {
                    bins: bins_of(manifest, &name),
                    peers: peers_of(manifest),
                };
                Some((name, Arc::new(installed)))
            })
            .collect();
        let members: Vec<(PackageId, &Package)> = workspace.iter().collect();
        for &(id, package) in &members {
            for entry in &package.dependencies {
                self.listed[id.index()].insert(entry.name.clone());
                if matches!(
                    entry.kind,
                    DependencyKind::Dependencies | DependencyKind::PeerDependencies
                ) {
                    self.production_listed[id.index()].insert(entry.name.clone());
                }
            }
        }
        // The installed manifests are looked up for every package on its own thread: each lookup walks up the
        // `node_modules` directories and is bound by file system latency.
        let listed = &self.listed;
        let installed: Vec<Vec<(String, Arc<Installed>)>> = map_ordered(&members, sequential, |&(id, package)| {
            let dir = workspace.root.join(&package.dir);
            let mut cache = HashMap::new();
            listed[id.index()]
                .iter()
                .filter_map(|name| {
                    let found = read_installed(fs, &dir, &workspace.root, name, &mut cache)
                        .or_else(|| linked.get(name).cloned())
                        .or_else(|| locked.get(name).cloned())?;
                    Some((name.clone(), found))
                })
                .collect()
        });
        for (&(id, _), found) in members.iter().zip(installed) {
            for (name, installed) in found {
                self.known_bins[id.index()].insert(name.clone());
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
    fn collect_ambient_types(&mut self, workspace: &Workspace, setup: &Setup, tsconfigs: &TsconfigMemo<'_>) {
        let members = member_dirs(workspace);
        for (id, package) in workspace.iter() {
            if !includes_all_types(tsconfigs, &members, &setup.tsconfig_of(workspace, package), 0) {
                continue;
            }
            for scope in [id, PackageId::ROOT] {
                let types: Vec<String> = self.listed[scope.index()]
                    .iter()
                    .filter(|name| GLOBAL_TYPES.contains(&name.as_str()))
                    .cloned()
                    .collect();
                for name in types {
                    self.reference_in(scope, &name);
                }
            }
        }
    }

    fn collect_jsx_runtimes(&mut self, workspace: &Workspace, setup: &Setup, tsconfigs: &TsconfigMemo<'_>) {
        let members = member_dirs(workspace);
        for (id, package) in workspace.iter() {
            let config = setup.tsconfig_of(workspace, package);
            self.other_jsx_runtime[id.index()] = jsx_import_source(tsconfigs, &members, &config, 0)
                .is_some_and(|source| package_name(&source).is_some_and(|name| name != "react"));
        }
    }

    /// The dependencies of the directories above the workspace, up to the repository root: their binaries are on the
    /// `PATH`, and the packages below may import what they list.
    fn collect_ancestor_bins(&mut self, workspace: &Workspace, fs: &dyn FileSystem) {
        let mut cache = HashMap::new();
        for (dir, manifest) in external_manifests(fs, &workspace.root) {
            for name in listed_names(&manifest) {
                self.external_listed
                    .entry(name.to_owned())
                    .or_insert_with(|| dir.clone());
                let mut bins = vec![unscoped(name).to_owned()];
                bins.extend(
                    BINARY_PACKAGES
                        .iter()
                        .filter(|(_, package)| *package == name)
                        .map(|(bin, _)| (*bin).to_owned()),
                );
                if let Some(installed) = read_installed(fs, &dir, &dir, name, &mut cache) {
                    bins.extend(installed.bins.iter().cloned());
                }
                for bin in bins {
                    let providers = self.ancestor_bins.entry(bin).or_default();
                    if !providers.iter().any(|provider| provider == name) {
                        providers.push(name.to_owned());
                    }
                }
            }
        }
    }

    /// The packages that the files outside the workspace root which the graph reached import (a sibling root's
    /// `lib.js`, a file of an enclosing root), each with the directory of the innermost manifest at or above the file,
    /// up to the repository root, that lists it: Node.js resolves it from there, so it is a used dependency of the
    /// project that owns that manifest, which does not see this code. Installed code is left out.
    fn collect_outside_imports(&mut self, workspace: &Workspace, graph: &ModuleGraph, fs: &dyn FileSystem) {
        let mut listed_in: HashMap<PathBuf, HashSet<String>> = HashMap::new();
        let outside = graph
            .modules()
            .iter()
            .filter(|module| !module.abs.starts_with(&workspace.root) && !is_installed(&module.abs));
        for module in outside {
            for name in module.imports.iter().filter_map(|import| import.package.as_deref()) {
                let mut dir = module.abs.parent();
                while let Some(current) = dir {
                    let names = listed_in.entry(current.to_path_buf()).or_insert_with(|| {
                        read_manifest(fs, current)
                            .map(|manifest| listed_names(&manifest).map(str::to_owned).collect())
                            .unwrap_or_default()
                    });
                    if names.contains(name) {
                        self.outside_used.insert((current.to_path_buf(), name.to_owned()));
                        break;
                    }
                    if is_repository_root(fs, current) {
                        break;
                    }
                    dir = current.parent();
                }
            }
        }
    }

    fn collect_member_bins(&mut self, workspace: &Workspace) {
        for package in workspace.all_packages() {
            let Some(name) = package.name.as_deref() else { continue };
            for bin in bins_of(package.manifest.value(), name) {
                self.member_bins.entry(bin).or_default().push(name.to_owned());
            }
        }
    }

    /// The closest manifest that lists `name`: the package itself, the packages whose directory contains it, then the
    /// manifests above the workspace root.
    fn find_listed(&self, id: PackageId, name: &str) -> Option<Listed> {
        std::iter::once(id)
            .chain(self.ancestors[id.index()].iter().copied())
            .find(|scope| self.listed[scope.index()].contains(name))
            .map(Listed::Package)
            .or_else(|| self.external_listed.contains_key(name).then_some(Listed::External))
    }

    /// Whether `name` is a workspace package or a manifest of the workspace, or above it, lists it: an unlisted import
    /// of it wants a `package.json` entry, whatever `paths` of another package also map it.
    pub(crate) fn known_package(&self, name: &str) -> bool {
        self.members.contains(name)
            || self.external_listed.contains_key(name)
            || self.listed.iter().any(|listed| listed.contains(name))
    }

    fn reference(&mut self, listed: Listed, name: &str) {
        match listed {
            Listed::Package(id) => self.reference_in(id, name),
            Listed::External => {
                self.external_used.insert(name.to_owned());
            }
        }
    }

    /// Counts `specifier` as a use of the package it names, when `id` or a package above it lists one.
    fn reference_listed(&mut self, id: PackageId, specifier: &str) {
        if let Some(name) = package_name(specifier)
            && let Some(found) = self.find_listed(id, &name)
        {
            self.reference(found, &name);
        }
    }

    fn reference_in(&mut self, id: PackageId, name: &str) {
        self.referenced[id.index()].insert(name.to_owned());
    }

    fn collect_imports(&mut self, workspace: &Workspace, setup: &Setup, graph: &ModuleGraph) {
        for module in graph.modules() {
            // A file of another project uses that project's dependencies, not those of the package that reaches it.
            if module.foreign || setup.package(module.owner).is_none() {
                continue;
            }
            for resolved in &module.imports {
                if let Some(name) = &resolved.package {
                    let kind = module.info.imports.get(resolved.import).map(|import| import.kind);
                    if kind == Some(ImportKind::Mock) {
                        // A test that mocks a package does not need it installed: a listed package is used, an
                        // unlisted one is not an unlisted import.
                        if let Some(found) = self.find_listed(module.owner, name) {
                            self.reference(found, name);
                        }
                        continue;
                    }
                    if self.strict && resolved.type_only {
                        // knip's strict mode skips type-only imports of packages: they neither use a dependency
                        // nor need one listed.
                        continue;
                    }
                    let declaration = is_declaration_file(&module.abs);
                    // knip's strict mode counts every import of a declaration file as type-only, so a listed `@types`
                    // package stands in for the package.
                    let type_only =
                        (resolved.type_only && kind != Some(ImportKind::ExportFrom)) || (self.strict && declaration);
                    let import = module.info.imports.get(resolved.import);
                    let site = ImportSite {
                        owner: module.owner,
                        specifier: import.map_or(name.as_str(), |import| import.specifier.as_str()),
                        target: Some(&resolved.target),
                        module: module.id,
                        span: import.map(|import| import.span),
                        type_only,
                        declaration,
                    };
                    self.handle_import(workspace, name, &site);
                } else if let Some(import) = module.info.imports.get(resolved.import) {
                    // A path alias can stand in for a package the manifest lists: the alias serves development,
                    // the listed dependency is what consumers install. A bare builtin name (`events`, `buffer`) uses
                    // a listed package of that name, the polyfill a bundler resolves it to for the browser; a
                    // `node:` specifier names no package.
                    self.reference_listed(module.owner, &import.specifier);
                }
            }
            let extension = module
                .abs
                .extension()
                .and_then(|extension| extension.to_str())
                .map(str::to_ascii_lowercase);
            if let Some((_, compilers)) = COMPILER_DEPENDENCIES
                .iter()
                .find(|(wanted, _)| extension.as_deref() == Some(*wanted))
            {
                for compiler in *compilers {
                    if let Some(found) = self.find_listed(module.owner, compiler) {
                        self.reference(found, compiler);
                    }
                }
            }
            if !module.info.commands.is_empty() && !module.ignored && !in_fixture_directory(&module.path) {
                let context = workspace
                    .package(module.owner)
                    .map_or_else(ScriptContext::new, |package| script_context(package.manifest.value()));
                self.collect_commands(setup, module.owner, &module.path, &module.info.commands, &context);
            }
            // `declare module 'pkg'` augments the package, which has to be installed for that to compile. It stays
            // out of `imports`: declaring a module the project does not list is not an unlisted import.
            for name in module.info.ambient_modules.iter().chain(&module.info.augmented_modules) {
                self.reference_listed(module.owner, name);
            }
            if let Some(pragma) = module.info.jsx_import_source.as_deref()
                && let Some(source) = package_name(pragma)
            {
                let site = ImportSite {
                    owner: module.owner,
                    specifier: pragma,
                    target: None,
                    module: module.id,
                    span: module.info.jsx_import_source_span,
                    type_only: false,
                    declaration: is_declaration_file(&module.abs),
                };
                self.handle_import(workspace, &source, &site);
            } else if module.info.has_jsx
                && !self.other_jsx_runtime[module.owner.index()]
                && let Some(found) = self.find_listed(module.owner, "react")
            {
                // The automatic JSX runtime imports `react/jsx-runtime` on behalf of the file.
                self.reference(found, "react");
            }
        }
    }

    /// Where an import of a package may be listed. In strict mode, production code (any file but a declaration file,
    /// whose imports knip treats as type-only) may import only what its own package lists in `dependencies` or
    /// `peerDependencies`, as in knip's `DependencyDeputy.isInDependencies`.
    fn find_listed_for(&self, site: &ImportSite<'_>, name: &str) -> Option<Listed> {
        if !self.strict || site.declaration {
            return self.find_listed(site.owner, name);
        }
        if self.production_listed[site.owner.index()].contains(name) {
            Some(Listed::Package(site.owner))
        } else {
            self.external_listed.contains_key(name).then_some(Listed::External)
        }
    }

    fn handle_import(&mut self, workspace: &Workspace, name: &str, site: &ImportSite<'_>) {
        let find = |usage: &Self, name: &str| usage.find_listed_for(site, name);
        if self.is_satisfied(workspace, site.owner, name, site.type_only, find) {
            return;
        }
        self.unlisted.push(UnlistedImport {
            package: site.owner,
            name: name.to_owned(),
            module: site.module,
            span: site.span,
            specifier: site.specifier.to_owned(),
            target: site.target.cloned(),
            foreign_paths: None,
        });
    }

    /// Whether package `name` that package `owner` uses needs no manifest entry, recording every use it finds: a
    /// runtime module, the package itself, a package that `find` finds listed (for `type_only` uses also its
    /// `DefinitelyTyped` package), one that is provided ([`Self::is_provided`]) or a workspace member a private
    /// package uses unlisted.
    fn is_satisfied(
        &mut self,
        workspace: &Workspace,
        owner: PackageId,
        name: &str,
        type_only: bool,
        find: impl Fn(&Self, &str) -> Option<Listed>,
    ) -> bool {
        if RUNTIME_PACKAGES.contains(&name) {
            return true;
        }
        let Some(package) = workspace.package(owner) else {
            return true;
        };
        if package.name.as_deref() == Some(name) {
            return true;
        }
        let types = types_package(name).and_then(|types| find(self, &types).map(|found| (found, types)));
        if let Some((found, types)) = &types {
            self.reference(*found, types);
        }
        if let Some(found) = find(self, name) {
            self.reference(found, name);
            return true;
        }
        if type_only && types.is_some() {
            return true;
        }
        self.is_provided(workspace, owner, name) || self.use_unlisted_member(package, owner, name)
    }

    /// Whether package `name`, which no manifest of package `owner` or above lists, is there all the same: a peer
    /// dependency of a listed package or the runtime of an `engines` entry.
    fn is_provided(&self, workspace: &Workspace, owner: PackageId, name: &str) -> bool {
        // A peer dependency of a listed package is provided by whoever installs that package; strict mode, like
        // knip's, does not count on that.
        let scopes = std::iter::once(owner).chain(self.ancestors[owner.index()].iter().copied());
        if !self.strict
            && scopes
                .into_iter()
                .any(|scope| self.hosts[scope.index()].contains_key(name))
        {
            return true;
        }
        // The runtime an `engines` entry names provides its module (`vscode` for an extension), also to the packages
        // inside the one that declares it, such as the client and server of an extension.
        std::iter::once(owner)
            .chain(self.ancestors[owner.index()].iter().copied())
            .filter_map(|scope| workspace.package(scope))
            .any(|scope| {
                scope
                    .manifest
                    .value()
                    .get("engines")
                    .and_then(|engines| engines.get(name))
                    .is_some()
            })
    }

    /// Whether `name` is a workspace member that the private `package` (`owner`) uses without listing it, which is
    /// then recorded as a use of it.
    fn use_unlisted_member(&mut self, package: &Package, owner: PackageId, name: &str) -> bool {
        let member = !self.strict && package.private && self.members.contains(name);
        if member {
            self.reference_in(owner, name);
        }
        member
    }

    /// The packages that the tool configs of every package name themselves (a `tsconfig` `extends`, an eslint plugin,
    /// a Prisma generator), judged against the package that owns the config file, and for a compiler option of a
    /// shared base `tsconfig` against the package reading it as well: a listed one is used, one nothing lists or
    /// provides is unlisted at the config. A config that is not judged ([`judged_config`]) is skipped, and production
    /// mode judges none: configs serve development.
    fn collect_config_dependencies(&mut self, workspace: &Workspace, setup: &Setup, fs: &dyn FileSystem) {
        if setup.production {
            return;
        }
        // Whether a config is judged depends only on its file; every package reading a shared base config and every
        // package one config names would otherwise rebuild the ignore matchers.
        let mut judged: BTreeMap<&Path, Option<(Option<PackageId>, String)>> = BTreeMap::new();
        for (reader, _) in workspace.iter() {
            let Some(package_setup) = setup.package(reader) else {
                continue;
            };
            for dependency in &package_setup.config_dependencies {
                let verdict = judged
                    .entry(dependency.file.as_path())
                    .or_insert_with(|| judged_config(workspace, setup, fs, &dependency.file));
                let Some((owner, file)) = verdict.clone() else {
                    continue;
                };
                let owner = owner.unwrap_or(reader);
                let may_list = dependency.reader_may_list.then_some(reader);
                self.config_dependency(workspace, (owner, may_list), dependency, &file);
            }
        }
    }

    /// The package `dependency` that the config `file` (relative to the root) of package `owner` names: a use of the
    /// manifest that lists it, else unlisted at the file unless it needs no entry ([`Self::is_satisfied`]). A
    /// `reader` that may list it instead is judged too, as knip's `alsoReferenceOwner` judges both packages.
    fn config_dependency(
        &mut self,
        workspace: &Workspace,
        (owner, reader): (PackageId, Option<PackageId>),
        dependency: &ConfigDependency,
        file: &str,
    ) {
        let name = dependency.name.as_str();
        let judge = |usage: &mut Self, scope: PackageId| {
            usage.is_satisfied(workspace, scope, name, dependency.type_only, |usage, name| {
                usage.find_listed(scope, name)
            })
        };
        let by_owner = judge(self, owner);
        let by_reader = reader.is_some_and(|reader| reader != owner && judge(self, reader));
        if by_owner || by_reader {
            return;
        }
        let file = PathBuf::from(file);
        let existing = self
            .unlisted_config
            .iter_mut()
            .find(|unlisted| unlisted.package == owner && unlisted.name == name && unlisted.file == file);
        match existing {
            Some(unlisted) => {
                if let Some(reader) = reader
                    && !unlisted.readers.contains(&reader)
                {
                    unlisted.readers.push(reader);
                }
            }
            None => self.unlisted_config.push(UnlistedConfigDependency {
                package: owner,
                name: name.to_owned(),
                file,
                readers: reader.into_iter().collect(),
            }),
        }
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
                self.provide_binary_or_ancestor(id, bin);
            }
            for binary in &package_setup.scripts.ephemeral_binaries {
                self.provide_binary(id, &binary.name);
            }
            for manager in &package_setup.scripts.package_managers {
                if let Some(found) = self.find_listed(id, manager) {
                    self.reference(found, manager);
                }
            }
            let global_binaries: &[ScriptBinary] = if setup.ignore_global_binaries {
                &[]
            } else {
                &package_setup.scripts.global_binaries
            };
            for binary in package_setup.scripts.binaries.iter().chain(global_binaries) {
                if !self.provide_binary_or_ancestor(id, &binary.name) && !setup.ignores_binary(id, &binary.name) {
                    self.unlisted_binaries.push(UnlistedBinary {
                        package: id,
                        name: binary.name.clone(),
                        script: binary.script.clone(),
                        file: None,
                    });
                }
            }
            for run in &package_setup.run_binaries {
                let file = relative(&run.file, &workspace.root).map_or_else(|| run.file.clone(), PathBuf::from);
                self.run_binary(setup, id, &run.name, &file);
            }
        }
    }

    /// The binaries the `node:child_process` commands of the source file `file` (relative to the root) run, owned by
    /// package `id` whose scripts `context` holds: a binary fetched on demand credits the dependency that provides it,
    /// any other is a use of one or unlisted at the file.
    fn collect_commands(
        &mut self,
        setup: &Setup,
        id: PackageId,
        file: &Path,
        commands: &[String],
        context: &ScriptContext,
    ) {
        for command in commands {
            let info = parse_script(command, context);
            for dependency in &info.dependencies {
                if let Some(found) = self.find_listed(id, &dependency.name) {
                    self.reference(found, &dependency.name);
                }
            }
            // As for the scripts of `package.json`: the system binaries count only with `ignore-global-binaries` off.
            let global_binaries = if setup.ignore_global_binaries {
                Vec::new()
            } else {
                info.global_binaries
            };
            let binaries = info
                .binaries
                .into_iter()
                .filter(|binary| !is_ignored_binary(&binary.name));
            for binary in binaries.chain(global_binaries) {
                if binary.filtered {
                    continue;
                }
                if binary.via.is_ephemeral() {
                    self.provide_binary(id, &binary.name);
                } else {
                    self.run_binary(setup, id, &binary.name, file);
                }
            }
        }
    }

    /// A binary that a command read from `file` (relative to the root) runs in package `id`: a use of the dependency
    /// that provides it, else an unlisted binary reported at the file.
    fn run_binary(&mut self, setup: &Setup, id: PackageId, name: &str, file: &Path) {
        if self.provide_binary(id, name) || self.ancestor_bins.contains_key(name) || setup.ignores_binary(id, name) {
            return;
        }
        let binary = UnlistedBinary {
            package: id,
            name: name.to_owned(),
            script: String::new(),
            file: Some(file.to_path_buf()),
        };
        if !self.unlisted_binaries.contains(&binary) {
            self.unlisted_binaries.push(binary);
        }
    }

    /// Marks the dependencies that provide binary `bin` as used by `id`, else those of the projects above the workspace
    /// root that put it on the `PATH`. False when none does.
    fn provide_binary_or_ancestor(&mut self, id: PackageId, bin: &str) -> bool {
        if self.provide_binary(id, bin) {
            return true;
        }
        let Some(providers) = self.ancestor_bins.get(bin) else {
            return false;
        };
        self.external_used.extend(providers.iter().cloned());
        true
    }

    /// Marks the dependencies that provide binary `bin` as used by `id`. False when none does.
    fn provide_binary(&mut self, id: PackageId, bin: &str) -> bool {
        let mut provided: Vec<(Listed, String)> = Vec::new();
        let scopes = std::iter::once(id).chain(self.ancestors[id.index()].iter().copied());
        for scope in scopes {
            if let Some(deps) = self.bins[scope.index()].get(bin) {
                provided.extend(deps.iter().map(|dep| (Listed::Package(scope), dep.clone())));
            }
            // A dependency named like the binary is assumed to provide it only while its manifest is unknown: an
            // installed or locked one whose manifest lacks the binary (the `stripe` SDK, not the CLI) does not.
            provided.extend(
                self.listed[scope.index()]
                    .iter()
                    .filter(|dep| {
                        (unscoped(dep) == bin && !self.known_bins[scope.index()].contains(*dep))
                            || BINARY_PACKAGES.iter().any(|(b, package)| *b == bin && package == dep)
                    })
                    .map(|dep| (Listed::Package(scope), dep.clone())),
            );
        }
        if provided.is_empty() {
            // Nothing is installed to say what a listed dependency provides, so one named like the binary is
            // assumed to (`docusaurus-start` from `docusaurus`, `nitro` from `nitropack`) instead of reporting it.
            let scopes = std::iter::once(id).chain(self.ancestors[id.index()].iter().copied());
            for scope in scopes {
                provided.extend(
                    self.listed[scope.index()]
                        .iter()
                        .filter(|dep| !self.known_bins[scope.index()].contains(*dep) && resembles(dep, bin))
                        .map(|dep| (Listed::Package(scope), dep.clone())),
                );
            }
        }
        if let Some(members) = self.member_bins.get(bin).cloned() {
            for member in members {
                if let Some(found) = self.find_listed(id, &member) {
                    provided.push((found, member));
                } else {
                    provided.push((Listed::Package(id), String::new()));
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

    /// The dependencies this workspace uses through a `package.json` outside its root, each with the directory of
    /// that manifest: those its own code uses that only a manifest above its root lists (the innermost one), and those
    /// the files outside its root that the graph reached import (the innermost manifest at or above the file that
    /// lists them). Node.js resolves them from that project's `node_modules`, so they are used dependencies of that
    /// project, which cannot see this code.
    #[must_use]
    pub fn ancestor_uses(&self) -> Vec<(PathBuf, String)> {
        self.external_used
            .iter()
            .filter_map(|name| Some((self.external_listed.get(name)?.clone(), name.clone())))
            .chain(self.outside_used.iter().cloned())
            .collect()
    }
}
