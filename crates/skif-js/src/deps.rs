//! Which packages and binaries the code and scripts use, against what each manifest lists.

use std::collections::{BTreeMap, BTreeSet};

use skif_core::{HashMap, HashSet};
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use serde_json::Value;
use skif_core::{DependencyKind, FileSystem, Span};
use skif_js_plugins::scripts::{ScriptContext, is_ignored_binary, parse_script};
use skif_js_syntax::{ConfigValue, ImportKind, extract_config_object, is_declaration_file, place_file_dir};
use skif_specifier::Specifier;
use skif_workspace::{Package, PackageId, PackageManager, Workspace, map_ordered, npmrc_entries};

use crate::ancestors::{external_manifests, is_repository_root, listed_names, read_manifest};
use crate::discover::{is_ignored_in_repository, is_package_manifest, relative};
use crate::fixture::in_fixture_directory;
use crate::graph::{ModuleGraph, ModuleId, Target};
use crate::hoisting::PublicHoisting;
use crate::input::ConfigDependency;
use crate::lockfile;
use crate::resolve::is_installed;
use crate::scripts::{ScriptBinary, script_context};
use crate::setup::{Setup, package_dirs, package_in_dir, placed_dir};
use crate::specifier::{RUNTIME_PACKAGES, bin_link, package_name, typed_package, types_package};
use crate::tsconfig_files::{reference, references_of};
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
/// `svelte` compiles every `.svelte` component, though no file need import it.
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
    ("svelte", &["svelte"]),
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

/// The file that makes a repository a Rush repository.
const RUSH_JSON: &str = "rush.json";

/// The commands Rush itself provides to the scripts of a Rush repository.
const RUSH_COMMANDS: &[&str] = &["rush", "rushx", "rush-pnpm"];

/// Whether the binaries of a workspace member are on the `PATH` of every package: npm, Yarn Classic and the hoisted
/// linkers of pnpm ([`pnpm_hoisted`]) and Bun hoist the members into the root `node_modules` and link their binaries
/// into its `.bin`, while pnpm's isolated linker, npm's linked strategy ([`npm_linked`]), Yarn Berry, Rush and Bun's
/// isolated linker ([`bun_isolated`]) link a member's binaries only into the packages that depend on it. An unknown
/// package manager counts as hoisting, so it reports nothing new, and so does a Yarn whose version the repository does
/// not tell ([`yarn_berry`]).
/// `bun_config_version` is the `configVersion` of `bun.lock`.
fn hoists_member_bins(workspace: &Workspace, rush: bool, bun_config_version: Option<u64>, fs: &dyn FileSystem) -> bool {
    if rush {
        return false;
    }
    match workspace.package_manager {
        Some(PackageManager::Npm) => !npm_linked(workspace, fs),
        Some(PackageManager::Pnpm) => pnpm_hoisted(workspace, fs),
        Some(PackageManager::Yarn) => !yarn_berry(workspace, fs),
        Some(PackageManager::Bun) => !bun_isolated(fs, &workspace.root, bun_config_version),
        _ => true,
    }
}

/// Whether a Yarn workspace positively uses Yarn Berry (2 and later): the root's `packageManager` names a later Yarn
/// than 1, or, without one, the root has a `.yarnrc.yml` or a Berry `yarn.lock` (one with `__metadata`).
fn yarn_berry(workspace: &Workspace, fs: &dyn FileSystem) -> bool {
    if let Some(version) = declared_version(workspace, "yarn") {
        return !version.starts_with("1.");
    }
    fs.is_file(&workspace.root.join(".yarnrc.yml"))
        || fs
            .read_to_string(&workspace.root.join("yarn.lock"))
            .is_ok_and(|text| lockfile::is_yarn_berry(&text))
}

/// The version of `name` that the root's `packageManager` names (`10.12.1` of `pnpm@10.12.1`), when it names `name`.
fn declared_version<'a>(workspace: &'a Workspace, name: &str) -> Option<&'a str> {
    workspace
        .root_package()
        .manifest
        .value()
        .get("packageManager")
        .and_then(Value::as_str)?
        .strip_prefix(name)?
        .strip_prefix('@')
}

/// The first pnpm major that reads no `node_modules` layout setting from `.npmrc`: pnpm 11 reads only auth and
/// registry settings there, and the linker only from `pnpm-workspace.yaml`.
const PNPM_WITHOUT_NPMRC_SETTINGS: u64 = 11;

/// Whether pnpm installs the workspace with its hoisted linker. `nodeLinker` in `pnpm-workspace.yaml` decides when it
/// is set; skif's choice is to take it over the root `.npmrc`. Without one, the last top-level `node-linker` of the
/// root `.npmrc` decides, but only before pnpm 11, which ignores `node-linker` in `.npmrc`: when the root's
/// `packageManager` names pnpm 11 or later, `.npmrc` is not read. Without a pnpm version in `packageManager` it is
/// read, since skif does not tell the pnpm version from anything else. Without either setting, pnpm uses its isolated
/// linker. A member's `.npmrc`, the user's `.npmrc`, the environment and `pnpm install --node-linker` are not read.
fn pnpm_hoisted(workspace: &Workspace, fs: &dyn FileSystem) -> bool {
    let reads_npmrc = pnpm_reads_npmrc(workspace);
    let linker = workspace
        .pnpm_workspace
        .as_ref()
        .and_then(|yaml| yaml.node_linker.clone())
        .or_else(|| {
            reads_npmrc
                .then(|| npmrc_setting(fs, &workspace.root, "node-linker"))
                .flatten()
        });
    linker.as_deref() == Some("hoisted")
}

/// Whether pnpm reads `node_modules` layout settings from the root `.npmrc`: the root's `packageManager` names a pnpm
/// before 11, or no pnpm version, since skif does not tell the pnpm version from anything else.
fn pnpm_reads_npmrc(workspace: &Workspace) -> bool {
    declared_version(workspace, "pnpm")
        .and_then(major_of)
        .is_none_or(|major| major < PNPM_WITHOUT_NPMRC_SETTINGS)
}

/// Whether npm installs the workspace with its linked strategy, which installs isolated, as pnpm does: the last
/// top-level `install-strategy` of the root `.npmrc` is `linked`. Without one, npm hoists. A member's `.npmrc`, the
/// user's `.npmrc`, the environment and `npm install --install-strategy` are not read.
fn npm_linked(workspace: &Workspace, fs: &dyn FileSystem) -> bool {
    npmrc_setting(fs, &workspace.root, "install-strategy").as_deref() == Some("linked")
}

/// The value of the last top-level entry `key` of the `.npmrc` in `root`, as npm's `ini` parser reads it.
fn npmrc_setting(fs: &dyn FileSystem, root: &Path, key: &str) -> Option<String> {
    let npmrc = fs.read_to_string(&root.join(".npmrc")).ok()?;
    npmrc_entries(&npmrc)
        .filter(|(entry, _)| *entry == key)
        .last()
        .map(|(_, value)| value.to_owned())
}

/// Whether Bun installs the workspace at `root` with its isolated linker. `linker` in the `[install]` table of the
/// root `bunfig.toml` decides; without one, `config_version`, the `configVersion` of `bun.lock`, does: from 1 (written
/// by Bun 1.3.2 and later for a new project) a workspace is isolated, before it hoisted. Without either, Bun's choice
/// cannot be read from the repository, so it counts as hoisted. The global `~/.bunfig.toml` and `bun install --linker` are not read.
fn bun_isolated(fs: &dyn FileSystem, root: &Path, config_version: Option<u64>) -> bool {
    let linker = fs
        .read_to_string(&root.join("bunfig.toml"))
        .ok()
        .and_then(|text| text.parse::<toml::Table>().ok())
        .and_then(|table| Some(table.get("install")?.get("linker")?.as_str()?.to_owned()));
    match linker.as_deref() {
        Some("isolated") => true,
        Some("hoisted") => false,
        _ => config_version.is_some_and(|version| version >= 1),
    }
}

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
    /// Where the command runs the binary after `cd`, when that is another workspace package or a project of its own,
    /// which does not provide it either.
    pub runs_in: Option<RunsIn>,
}

/// The workspace package or project of its own that a command runs an unlisted binary in after `cd`; see
/// [`UnlistedBinary::runs_in`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunsIn {
    /// Its directory, relative to the workspace root with `/` separators; `.` for the root.
    pub dir: String,
    /// The package's display name, or the project's `name`, else its directory.
    pub name: String,
}

/// A package that a tool's config file names and no manifest lists; see [`DependencyUsage::unlisted_config`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnlistedConfigDependency {
    /// The package that must list it: the one that owns the config file (the innermost workspace package whose
    /// directory holds it), or the package reading the config when the reference names that package's install
    /// ([`crate::ConfigDependency::reader_lists`]).
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

/// The binaries, peer dependencies and version a package declares.
pub(crate) struct Installed {
    pub(crate) bins: Vec<String>,
    pub(crate) peers: Vec<String>,
    /// The peers of `peers` that are optional: the package manager does not install them for the package.
    pub(crate) optional_peers: Vec<String>,
    /// The `version` of its manifest; `None` when it has none, and for a package known only from the lockfile, whose
    /// packages are read without one.
    pub(crate) version: Option<String>,
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

/// The names `peerDependenciesMeta` marks `optional`.
pub(crate) fn optional_peers_of(manifest: &Value) -> Vec<String> {
    manifest
        .get("peerDependenciesMeta")
        .and_then(Value::as_object)
        .map(|meta| {
            meta.iter()
                .filter(|(_, entry)| entry.get("optional").and_then(Value::as_bool) == Some(true))
                .map(|(name, _)| name.clone())
                .collect()
        })
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

/// The binaries that dependency `name` of the manifest in `dir`, a project other than the workspace packages,
/// provides: one named like it, those [`BINARY_PACKAGES`] gives it and those of its manifest installed in `dir`.
fn listed_bins(
    fs: &dyn FileSystem,
    dir: &Path,
    name: &str,
    cache: &mut HashMap<PathBuf, Option<Arc<Installed>>>,
) -> Vec<String> {
    let mut bins = vec![unscoped(name).to_owned()];
    bins.extend(
        BINARY_PACKAGES
            .iter()
            .filter(|(_, package)| *package == name)
            .map(|(bin, _)| (*bin).to_owned()),
    );
    if let Some(installed) = read_installed(fs, dir, dir, name, cache) {
        bins.extend(installed.bins.iter().cloned());
    }
    bins
}

/// The innermost project of its own that holds `target` below the workspace package directory `package` (both relative
/// to `root`, `/`-separated): a directory, `target` or one above it, with a real `package.json`
/// ([`is_package_manifest`]), such as a nested root of a recursive run. Its absolute directory and manifest; `None`
/// without one and below a `node_modules` directory, whose packages are installed code.
fn project_between(fs: &dyn FileSystem, root: &Path, package: &str, target: &str) -> Option<(PathBuf, Value)> {
    let package = root.join(package);
    let target = root.join(target);
    let between: Vec<&Path> = target.ancestors().take_while(|dir| *dir != package).collect();
    if !target.starts_with(&package) || between.iter().any(|dir| dir.ends_with("node_modules")) {
        return None;
    }
    let dir = between
        .into_iter()
        .find(|dir| fs.is_file(&dir.join("package.json")) && is_package_manifest(fs, dir))?;
    Some((dir.to_path_buf(), read_manifest(fs, dir).unwrap_or(Value::Null)))
}

/// Where a command that runs a binary is written.
#[derive(Clone, Copy)]
enum Origin<'a> {
    /// The `package.json` script of that name.
    Script(&'a str),
    /// A file, relative to the workspace root: a workflow, a tool config or a source file.
    File(&'a Path),
}

/// A binary that a command of a package runs.
#[derive(Clone, Copy)]
struct CommandBinary<'a> {
    name: &'a str,
    /// The directory it runs in after `cd`, relative to the package directory; `None` where the command starts.
    dir: Option<&'a str>,
    /// The command runs it in another workspace (`pnpm -C dir exec`), so the package itself does not.
    filtered: bool,
    /// A binary nothing provides is unlisted; otherwise it is only a credit.
    judged: bool,
    origin: Origin<'a>,
}

/// The workspace package or the project of its own that a command of another package runs a binary in after `cd`.
struct Host {
    /// The innermost workspace package whose directory holds the directory.
    package: PackageId,
    /// The project of its own below that package that holds the directory ([`project_between`]): its absolute
    /// directory and manifest.
    project: Option<(PathBuf, Value)>,
    runs_in: RunsIn,
}

impl Host {
    /// Whether the package or project has a script `name`, which a package manager run there runs instead of a
    /// binary (`cd web && pnpm bundle`).
    fn has_script(&self, workspace: &Workspace, name: &str) -> bool {
        let manifest = match &self.project {
            Some((_, manifest)) => Some(manifest),
            None => workspace.package(self.package).map(|package| package.manifest.value()),
        };
        manifest
            .and_then(|manifest| manifest.get("scripts"))
            .and_then(|scripts| scripts.get(name))
            .is_some()
    }
}

fn read_installed(
    fs: &dyn FileSystem,
    start: &Path,
    root: &Path,
    name: &str,
    cache: &mut HashMap<PathBuf, Option<Arc<Installed>>>,
) -> Option<Arc<Installed>> {
    find_installed(start, root, |dir| {
        let package = dir.join("node_modules").join(name);
        let manifest = package.join("package.json");
        cache
            .entry(manifest.clone())
            .or_insert_with(|| read_installed_at(fs, &package, &manifest, name))
            .clone()
    })
}

/// The first installed package `lookup` finds in a directory from `start` up to `root`, as Node.js looks up
/// `node_modules`.
fn find_installed(
    start: &Path,
    root: &Path,
    mut lookup: impl FnMut(&Path) -> Option<Arc<Installed>>,
) -> Option<Arc<Installed>> {
    let mut dir = start;
    loop {
        if let Some(found) = lookup(dir) {
            return Some(found);
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

/// The installed package `name` in `package`, whose manifest is `manifest`.
fn read_installed_at(fs: &dyn FileSystem, package: &Path, manifest: &Path, name: &str) -> Option<Arc<Installed>> {
    fs.read_to_string(manifest)
        .ok()
        .and_then(|text| parse_installed(fs, package, &text, name))
}

/// The installed manifests the packages of a run look up side by side, each read once for all of them, and which
/// directories hold no `node_modules`, so that the packages listed nowhere in them are not probed one by one.
#[derive(Default)]
struct SharedInstalled {
    manifests: Mutex<HashMap<PathBuf, Option<Arc<Installed>>>>,
    module_dirs: Mutex<HashMap<PathBuf, bool>>,
}

impl SharedInstalled {
    /// As [`read_installed`], through the shared cache.
    fn read(&self, fs: &dyn FileSystem, start: &Path, root: &Path, name: &str) -> Option<Arc<Installed>> {
        find_installed(start, root, |dir| {
            if !self.has_node_modules(fs, dir) {
                return None;
            }
            let package = dir.join("node_modules").join(name);
            let manifest = package.join("package.json");
            if let Some(known) = self
                .manifests
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(&manifest)
            {
                return known.clone();
            }
            // Read outside the lock: two packages may read the same manifest at once, to the same result.
            let read = read_installed_at(fs, &package, &manifest, name);
            self.manifests
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .entry(manifest)
                .or_insert(read)
                .clone()
        })
    }

    /// Whether `dir` holds a `node_modules` directory: without one, no package is installed there.
    fn has_node_modules(&self, fs: &dyn FileSystem, dir: &Path) -> bool {
        if let Some(&known) = self.module_dirs.lock().unwrap_or_else(PoisonError::into_inner).get(dir) {
            return known;
        }
        let found = fs.is_dir(&dir.join("node_modules"));
        *self
            .module_dirs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(dir.to_path_buf())
            .or_insert(found)
    }
}

/// The `version` of a manifest.
fn version_of(manifest: &Value) -> Option<String> {
    manifest.get("version").and_then(Value::as_str).map(str::to_owned)
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
        optional_peers: optional_peers_of(&value),
        version: version_of(&value),
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
            parsed: Mutex::new(HashMap::default()),
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

/// What the `compilerOptions.types` of a `tsconfig.json` and its `extends` chain leave of the installed `@types/*`
/// packages, ordered from fewest to most.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum AmbientTypes {
    /// None: the config is missing, its `types` lists no `*`, or its `typeRoots` drop `node_modules/@types`.
    Narrowed,
    /// TypeScript's default, no `types` along the chain: every package before version 6, none from version 6.
    Default,
    /// Every package, on every TypeScript version: `types: ["*"]`.
    Every,
}

/// What the `tsconfig.json` at `path` makes TypeScript include of the installed `@types/*` packages, from its
/// effective `types` and `typeRoots`: each the value the config sets itself, else the one it inherits
/// ([`inherited_option`]), as TypeScript replaces an inherited compiler option rather than merging it.
fn ambient_types(memo: &TsconfigMemo<'_>, members: &[(String, PathBuf)], path: &Path) -> AmbientTypes {
    if memo.get(path).is_none() {
        return AmbientTypes::Narrowed;
    }
    let keeps_default_root = |options: &ConfigValue| match options.get("typeRoots") {
        Some(ConfigValue::Array(roots)) => Some(roots.iter().filter_map(ConfigValue::as_str).any(|root| {
            let root = root.trim_end_matches('/');
            root.ends_with("node_modules/@types") || root == "@types"
        })),
        _ => None,
    };
    if inherited_option(memo, members, path, 0, &keeps_default_root) == Some(false) {
        return AmbientTypes::Narrowed;
    }
    let lists_every = |options: &ConfigValue| options.get("types").map(lists_every_type);
    match inherited_option(memo, members, path, 0, &lists_every) {
        Some(true) => AmbientTypes::Every,
        Some(false) => AmbientTypes::Narrowed,
        None => AmbientTypes::Default,
    }
}

/// What the `tsconfig.json` at `path` and the configs its `references` name inside its directory (a solution config,
/// such as Vite's `files: []` beside `tsconfig.app.json` and `tsconfig.node.json`) make TypeScript include of the
/// installed `@types/*` packages: every package when one of them includes every package, else the default when one
/// of them leaves it.
fn project_ambient_types(memo: &TsconfigMemo<'_>, members: &[(String, PathBuf)], path: &Path) -> AmbientTypes {
    let own = ambient_types(memo, members, path);
    let Some(config) = memo.get(path) else {
        return own;
    };
    let dir = path.parent().unwrap_or(path);
    let referenced = references_of(&config)
        .into_iter()
        .filter_map(|target| reference(memo.fs, dir, target))
        .filter(|target| target.starts_with(dir) && target != path)
        .map(|target| ambient_types(memo, members, &target));
    referenced.fold(own, Ord::max)
}

/// The major version of TypeScript that `package` compiles with: that of the `typescript` installed in the nearest
/// `node_modules` from its directory up to the workspace root, else the lowest version the `typescript` range it lists
/// admits, else `root_declared`, the one the root manifest lists. `None` when none of them names a version.
/// `installed` holds the `typescript` manifests read so far, by path.
fn typescript_major(
    fs: &dyn FileSystem,
    workspace: &Workspace,
    package: &Package,
    installed: &mut HashMap<PathBuf, Option<Arc<Installed>>>,
    root_declared: Option<u64>,
) -> Option<u64> {
    let dir = workspace.root.join(&package.dir);
    read_installed(fs, &dir, &workspace.root, "typescript", installed)
        .and_then(|typescript| typescript.version.as_deref().and_then(major_of))
        .or_else(|| declared_typescript_major(workspace, package))
        .or(root_declared)
}

/// The major version of the lowest version the `typescript` ranges of `package` admit, a `catalog:` reference read
/// from the workspace catalogs. The entries under `dependencies`, `devDependencies` or `optionalDependencies` decide
/// when there are any, even when none of them names a version; a `peerDependencies` range, which describes the
/// consumers, only when there are none.
fn declared_typescript_major(workspace: &Workspace, package: &Package) -> Option<u64> {
    let typescript = move |peer: bool| {
        package.dependencies.iter().filter(move |entry| {
            &*entry.name == "typescript" && (entry.kind == DependencyKind::PeerDependencies) == peer
        })
    };
    let peer = typescript(false).next().is_none();
    typescript(peer)
        .filter_map(
            |entry| match workspace.catalogs.resolve(&entry.name, &entry.specifier) {
                Some(version) => lowest_major(&Specifier::new(version)),
                None => lowest_major(&entry.parsed),
            },
        )
        .min()
}

/// The major number of the lowest version `specifier` admits: `^6.0.0`, `>=6 <8` and `6.1.0-dev.1` are 6, `^5 || ^6`
/// is 5, and an upper bound alone (`<6`, `<=5.9.3`) admits `0.0.0`. `None` for a specifier without a range (a tag, a
/// path, a `catalog:` reference) and for `*` and `latest`, which follow the newest release.
fn lowest_major(specifier: &Specifier) -> Option<u64> {
    let range = match specifier {
        Specifier::Latest(_) | Specifier::None => return None,
        // npm reads a range the specifier model leaves unclassified, such as `<6`.
        Specifier::Unsupported(raw) => Specifier::new_node_range(raw)?,
        _ => specifier.get_node_range()?,
    };
    range.min_version().map(|version| version.major())
}

/// The major number of a version such as `6.0.2`, `6` or `6.0.0-beta`.
fn major_of(version: &str) -> Option<u64> {
    version.trim().split(['.', '-', '+']).next()?.parse().ok()
}

/// Whether a `compilerOptions.types` value holds `*`, which TypeScript 6 reads as every package of the type roots.
fn lists_every_type(types: &ConfigValue) -> bool {
    match types {
        ConfigValue::Array(items) => items
            .iter()
            .any(|item| item.as_str().is_some_and(|name| name.trim() == "*")),
        _ => false,
    }
}

/// The `extends` entries of a parsed `tsconfig.json`, in order.
fn extends_of(config: &ConfigValue) -> Vec<&str> {
    match config.get("extends") {
        Some(ConfigValue::String(one)) => vec![one.as_str()],
        Some(ConfigValue::Array(many)) => many.iter().filter_map(ConfigValue::as_str).collect(),
        _ => Vec::new(),
    }
}

/// The `compilerOptions.jsxImportSource` the `tsconfig.json` at `path` sets itself or inherits.
fn jsx_import_source(memo: &TsconfigMemo<'_>, members: &[(String, PathBuf)], path: &Path) -> Option<String> {
    inherited_option(memo, members, path, 0, &|options: &ConfigValue| {
        options
            .get("jsxImportSource")
            .and_then(ConfigValue::as_str)
            .map(str::to_owned)
    })
}

/// What `read` makes of the `compilerOptions` of the nearest config that sets the option it reads: the
/// `tsconfig.json` at `path` itself, else the configs it extends, a later `extends` entry before an earlier one, each
/// with its own chain. `None` when no config of the chain sets it, or the config at `path` is missing.
fn inherited_option<T>(
    memo: &TsconfigMemo<'_>,
    members: &[(String, PathBuf)],
    path: &Path,
    depth: usize,
    read: &impl Fn(&ConfigValue) -> Option<T>,
) -> Option<T> {
    const MAX_DEPTH: usize = 8;
    let config = memo.get(path)?;
    if let Some(value) = config.get("compilerOptions").and_then(read) {
        return Some(value);
    }
    if depth >= MAX_DEPTH {
        return None;
    }
    let dir = path.parent().unwrap_or(path);
    extends_of(&config)
        .iter()
        .rev()
        .filter_map(|parent| resolve_extends(memo.fs, members, dir, parent))
        .find_map(|target| inherited_option(memo, members, &target, depth + 1, read))
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
    // The packages other than the root by directory, so each package looks up its ancestor directories, innermost
    // first, instead of testing every other package.
    let mut by_dir: HashMap<&Path, Vec<PackageId>> = HashMap::default();
    for (id, package) in workspace.iter().filter(|(_, package)| !package.is_root) {
        by_dir.entry(package.dir.as_path()).or_default().push(id);
    }
    let mut all = vec![Vec::new(); workspace.package_ids().count()];
    for (id, package) in workspace.iter() {
        all[id.index()] = package
            .dir
            .ancestors()
            .skip(1)
            .flat_map(|dir| by_dir.get(dir).into_iter().flatten().copied())
            .chain(root.filter(|root| *root != id))
            .collect();
    }
    all
}

/// What counted a dependency as used: the evidence `skif explain-usage --dependency` lists, recorded with every use
/// so the verdict and the evidence come from the same facts.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Use {
    /// A module imports it, re-exports it, declares or augments it as a module, or imports the package it types.
    Import(ModuleId),
    /// A test module mocks it.
    Mock(ModuleId),
    /// A module of a file type it compiles (`.mdx`, `.marko`).
    Compiler(ModuleId),
    /// A module with JSX, which the automatic runtime compiles to an import of it.
    JsxRuntime(ModuleId),
    /// A command a `node:child_process` call of this file (relative to the root) runs.
    Command(PathBuf),
    /// It provides this binary, which a script, a command or a plugin runs.
    Binary(String),
    /// A script of `package.json` or a plugin of the package names it.
    #[default]
    Tooling,
    /// This tool configuration file (relative to the root) names it.
    Config(PathBuf),
    /// A `tsconfig.json` includes every visible `@types` package, and this one declares only globals.
    AmbientTypes,
}

/// Why a package counts a dependency as used ([`DependencyUsage::credit`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Credit<'a> {
    /// A tool that is used without an import (`typescript`).
    AlwaysUsed,
    /// The uses recorded against the package's manifest.
    Uses(&'a BTreeSet<Use>),
    /// The types of a runtime that needs no package of its own (`@types/node`).
    UnpairedTypes,
    /// The `@types` package of this used package.
    TypesOf(String),
    /// A peer dependency of this used package.
    PeerOf(String),
}

/// Dependency and binary usage of every package.
#[derive(Debug, Clone, Default)]
pub struct DependencyUsage {
    /// For every package, the dependencies its manifest is credited with, each with the uses that credited it.
    referenced: Vec<HashMap<String, BTreeSet<Use>>>,
    /// The use being collected, recorded with every reference.
    via: Use,
    listed: Vec<HashSet<Arc<str>>>,
    /// For every package, the names its manifest lists in `dependencies` and `peerDependencies`: what production
    /// code may import in strict mode.
    production_listed: Vec<HashSet<Arc<str>>>,
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
    /// The packages files outside the workspace root import, with the directory of the manifest that lists each (see
    /// [`Self::collect_outside_imports`]), and those providing a binary that a command runs after `cd` into a project
    /// of its own below the root, with that project's directory (see [`Self::provide_project_binary`]).
    outside_used: BTreeSet<(PathBuf, String)>,
    /// For every package, the peer dependencies of its listed dependencies, each with the dependencies declaring it.
    hosts: Vec<HashMap<String, Vec<String>>>,
    /// For every package, the peers of `hosts` that at least one of its listed dependencies does not mark optional:
    /// whoever installs that dependency installs them too.
    required_peers: Vec<HashSet<String>>,
    bins: Vec<HashMap<String, Vec<String>>>,
    /// The workspace members that declare each binary, with their id and name.
    member_bins: BTreeMap<String, Vec<(PackageId, String)>>,
    /// Whether a member's binaries are on the `PATH` of every package of the workspace; see [`hoists_member_bins`].
    member_bins_hoisted: bool,
    /// Whether the workspace is a Rush repository (a `rush.json` at its root), whose scripts run Rush's own commands.
    rush: bool,
    /// What the install hoists publicly, with `[js] use-public-hoisting` outside strict mode.
    public_hoisting: Option<PublicHoisting>,
    /// Every package with its workspace-relative directory, for the commands that change directory.
    dirs: Vec<(PackageId, String)>,
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
    unlisted_config_index: UnlistedConfigIndex,
}

/// Where each entry of [`DependencyUsage::unlisted_config`] is, by package, name and file, and which readers each
/// entry has, so recording one more reference costs no scan of the entries or of their readers.
#[derive(Debug, Clone, Default)]
struct UnlistedConfigIndex {
    positions: HashMap<(PackageId, String, PathBuf), usize>,
    readers: HashSet<(usize, PackageId)>,
}

impl UnlistedConfigIndex {
    /// Records that config `file` of `package` names the unlisted `name`, read by `reader`: a new entry at the end of
    /// `list` the first time, after which `reader` joins the readers of that entry once, in the order first seen.
    fn record(
        &mut self,
        list: &mut Vec<UnlistedConfigDependency>,
        package: PackageId,
        name: &str,
        file: PathBuf,
        reader: Option<PackageId>,
    ) {
        let position = *self
            .positions
            .entry((package, name.to_owned(), file))
            .or_insert_with_key(|(package, name, file)| {
                list.push(UnlistedConfigDependency {
                    package: *package,
                    name: name.clone(),
                    file: file.clone(),
                    readers: Vec::new(),
                });
                list.len() - 1
            });
        if let Some(reader) = reader
            && self.readers.insert((position, reader))
        {
            list[position].readers.push(reader);
        }
    }
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
            referenced: vec![HashMap::default(); count],
            listed: vec![HashSet::default(); count],
            production_listed: vec![HashSet::default(); count],
            strict: setup.isolate_workspaces,
            known_bins: vec![HashSet::default(); count],
            other_jsx_runtime: vec![false; count],
            ancestors: package_ancestors(workspace, setup.isolate_workspaces),
            hosts: vec![HashMap::default(); count],
            required_peers: vec![HashSet::default(); count],
            bins: vec![HashMap::default(); count],
            members: workspace.all_packages().filter_map(|p| p.name.clone()).collect(),
            rush: fs.is_file(&workspace.root.join(RUSH_JSON)),
            dirs: package_dirs(workspace),
            ..Self::default()
        };
        let locked = lockfile::read(fs, &workspace.root);
        usage.member_bins_hoisted = hoists_member_bins(workspace, usage.rush, locked.bun_config_version, fs);
        if setup.use_public_hoisting && !setup.isolate_workspaces {
            let pnpm = (matches!(workspace.package_manager, Some(PackageManager::Pnpm)) && !usage.rush)
                .then(|| pnpm_reads_npmrc(workspace));
            usage.public_hoisting = Some(PublicHoisting::read(workspace, pnpm, setup.git_root.as_deref(), fs));
        }
        usage.read_manifests(workspace, &locked.packages, fs, sequential);
        let tsconfigs = TsconfigMemo::new(fs);
        usage.collect_ambient_types(workspace, setup, &tsconfigs);
        usage.collect_jsx_runtimes(workspace, setup, &tsconfigs);
        usage.collect_ancestor_bins(workspace, fs);
        usage.collect_member_bins(workspace);
        usage.collect_imports(workspace, setup, graph, fs);
        usage.collect_outside_imports(workspace, graph, fs);
        usage.collect_scripts(workspace, setup, fs);
        usage.collect_config_dependencies(workspace, setup, fs);
        usage
    }

    /// Reads what each listed dependency provides from its installed manifest, else a workspace package's own, else
    /// `locked`, the packages of the lockfile.
    fn read_manifests(
        &mut self,
        workspace: &Workspace,
        locked: &HashMap<String, Arc<Installed>>,
        fs: &dyn FileSystem,
        sequential: bool,
    ) {
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
                    optional_peers: optional_peers_of(manifest),
                    version: version_of(manifest),
                };
                Some((name, Arc::new(installed)))
            })
            .collect();
        let members: Vec<(PackageId, &Package)> = workspace.iter().collect();
        for &(id, package) in &members {
            for entry in &package.dependencies {
                self.listed[id.index()].insert(Arc::clone(&entry.name));
                if matches!(
                    entry.kind,
                    DependencyKind::Dependencies | DependencyKind::PeerDependencies
                ) {
                    self.production_listed[id.index()].insert(Arc::clone(&entry.name));
                }
            }
        }
        // The installed manifests are looked up for every package on its own thread: each lookup walks up the
        // `node_modules` directories and is bound by file system latency.
        let listed = &self.listed;
        let shared = SharedInstalled::default();
        let installed: Vec<Vec<(String, Arc<Installed>)>> = map_ordered(&members, sequential, |&(id, package)| {
            let dir = workspace.root.join(&package.dir);
            listed[id.index()]
                .iter()
                .filter_map(|name| {
                    let found = shared
                        .read(fs, &dir, &workspace.root, name)
                        .or_else(|| linked.get(&**name).cloned())
                        .or_else(|| locked.get(&**name).cloned())?;
                    Some((name.to_string(), found))
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
                    if !installed.optional_peers.contains(peer) {
                        self.required_peers[id.index()].insert(peer.clone());
                    }
                }
            }
        }
    }

    /// TypeScript includes every visible `@types/*` package when `compilerOptions.types` lists `*`, and before version
    /// 6 also when it is absent, so the globals-only ones a package (and the root, which hoists them) lists are used
    /// when the package has a `tsconfig.json` that includes them all. From TypeScript 6 an absent `types` includes
    /// none; a package whose TypeScript version is unknown keeps the earlier default.
    fn collect_ambient_types(&mut self, workspace: &Workspace, setup: &Setup, tsconfigs: &TsconfigMemo<'_>) {
        const TYPES_DEFAULT_EMPTY_SINCE: u64 = 6;
        let members = member_dirs(workspace);
        let mut installed = HashMap::default();
        let root_declared = workspace
            .package(PackageId::ROOT)
            .and_then(|root| declared_typescript_major(workspace, root));
        for (id, package) in workspace.iter() {
            let tsconfig = setup.tsconfig_of(workspace, package);
            let includes_all = match project_ambient_types(tsconfigs, &members, &tsconfig) {
                AmbientTypes::Narrowed => false,
                AmbientTypes::Every => true,
                AmbientTypes::Default => {
                    typescript_major(tsconfigs.fs, workspace, package, &mut installed, root_declared)
                        .is_none_or(|major| major < TYPES_DEFAULT_EMPTY_SINCE)
                }
            };
            if !includes_all {
                continue;
            }
            for scope in [id, PackageId::ROOT] {
                let types: Vec<String> = self.listed[scope.index()]
                    .iter()
                    .filter(|name| GLOBAL_TYPES.contains(&&***name))
                    .map(ToString::to_string)
                    .collect();
                for name in types {
                    self.with_via(Use::AmbientTypes, |usage| usage.reference_in(scope, &name));
                }
            }
        }
    }

    fn collect_jsx_runtimes(&mut self, workspace: &Workspace, setup: &Setup, tsconfigs: &TsconfigMemo<'_>) {
        let members = member_dirs(workspace);
        for (id, package) in workspace.iter() {
            let config = setup.tsconfig_of(workspace, package);
            self.other_jsx_runtime[id.index()] = jsx_import_source(tsconfigs, &members, &config)
                .is_some_and(|source| package_name(&source).is_some_and(|name| name != "react"));
        }
    }

    /// The dependencies of the directories above the workspace, up to the repository root: their binaries are on the
    /// `PATH`, and the packages below may import what they list.
    fn collect_ancestor_bins(&mut self, workspace: &Workspace, fs: &dyn FileSystem) {
        let mut cache = HashMap::default();
        for (dir, manifest) in external_manifests(fs, &workspace.root) {
            for name in listed_names(&manifest) {
                self.external_listed
                    .entry(name.to_owned())
                    .or_insert_with(|| dir.clone());
                for bin in listed_bins(fs, &dir, name, &mut cache) {
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
        let mut listed_in: HashMap<PathBuf, HashSet<String>> = HashMap::default();
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
        for (id, package) in workspace.iter() {
            let Some(name) = package.name.as_deref() else { continue };
            for bin in bins_of(package.manifest.value(), name) {
                self.member_bins.entry(bin).or_default().push((id, name.to_owned()));
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
                if !self.external_used.contains(name) {
                    self.external_used.insert(name.to_owned());
                }
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
        // Most uses repeat a name and a use already recorded, which then cost no allocation.
        let referenced = &mut self.referenced[id.index()];
        if !referenced.contains_key(name) {
            referenced.insert(name.to_owned(), BTreeSet::new());
        }
        if let Some(uses) = referenced.get_mut(name)
            && !uses.contains(&self.via)
        {
            uses.insert(self.via.clone());
        }
    }

    /// Runs `collect` with `via` as the use it records, then restores the use before it.
    fn with_via<R>(&mut self, via: Use, collect: impl FnOnce(&mut Self) -> R) -> R {
        let outer = std::mem::replace(&mut self.via, via);
        let result = collect(self);
        self.via = outer;
        result
    }

    fn collect_imports(&mut self, workspace: &Workspace, setup: &Setup, graph: &ModuleGraph, fs: &dyn FileSystem) {
        for module in graph.modules() {
            // A file of another project uses that project's dependencies, not those of the package that reaches it.
            if module.foreign || setup.package(module.owner).is_none() {
                continue;
            }
            let declaration = module.abs.to_str().is_some_and(is_declaration_file);
            self.via = Use::Import(module.id);
            for resolved in &module.imports {
                if let Some(name) = &resolved.package {
                    let kind = module.info.imports.get(resolved.import).map(|import| import.kind);
                    if kind == Some(ImportKind::Mock) {
                        // A test that mocks a package does not need it installed: a listed package is used, an
                        // unlisted one is not an unlisted import.
                        if let Some(found) = self.find_listed(module.owner, name) {
                            self.with_via(Use::Mock(module.id), |usage| usage.reference(found, name));
                        }
                        continue;
                    }
                    if self.strict && resolved.type_only {
                        // knip's strict mode skips type-only imports of packages: they neither use a dependency
                        // nor need one listed.
                        continue;
                    }
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
                } else if let Some(bin) = module
                    .info
                    .imports
                    .get(resolved.import)
                    .and_then(|import| bin_link(&import.specifier))
                {
                    // `require.resolve('.bin/tsc')` finds the binary's link to spawn it: a use of the dependency that
                    // provides the binary, as a script that runs it is, and an unlisted binary when none does.
                    if !self.provide_binary_or_ancestor(module.owner, bin)
                        && !module.ignored
                        && !setup.ignores_binary(module.owner, bin)
                    {
                        self.push_unlisted_binary(module.owner, bin, &module.path);
                    }
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
                        self.with_via(Use::Compiler(module.id), |usage| usage.reference(found, compiler));
                    }
                }
            }
            if !module.info.commands.is_empty() && !module.ignored && !in_fixture_directory(&module.path) {
                let context = workspace
                    .package(module.owner)
                    .map_or_else(ScriptContext::new, |package| script_context(package.manifest.value()));
                self.with_via(Use::Command(module.path.clone()), |usage| {
                    usage.collect_commands(
                        (workspace, setup, fs),
                        module.owner,
                        &module.path,
                        &module.info.commands,
                        &context,
                    );
                });
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
                    declaration,
                };
                self.handle_import(workspace, &source, &site);
            } else if module.info.has_jsx
                && !self.other_jsx_runtime[module.owner.index()]
                && let Some(found) = self.find_listed(module.owner, "react")
            {
                // The automatic JSX runtime imports `react/jsx-runtime` on behalf of the file.
                self.with_via(Use::JsxRuntime(module.id), |usage| usage.reference(found, "react"));
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
    /// dependency of a listed package, a package the install hoists publicly at or above `owner`
    /// ([`PublicHoisting`]) or the runtime of an `engines` entry.
    fn is_provided(&self, workspace: &Workspace, owner: PackageId, name: &str) -> bool {
        if let Some(hoisting) = &self.public_hoisting
            && let Some(package) = workspace.package(owner)
            && hoisting.hoists(&workspace.root.join(&package.dir), name)
        {
            return true;
        }
        // A peer dependency of a listed package is provided by whoever installs that package, unless it is optional,
        // which the package manager leaves out; strict mode, like knip's, does not count on that.
        let scopes = std::iter::once(owner).chain(self.ancestors[owner.index()].iter().copied());
        if !self.strict
            && scopes
                .into_iter()
                .any(|scope| self.required_peers[scope.index()].contains(name))
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
                // A reference to the reader's own install is the reader's, whichever package holds the file.
                let owner = if dependency.reader_lists {
                    reader
                } else {
                    owner.unwrap_or(reader)
                };
                let may_list = dependency.reader_may_list.then_some(reader);
                self.with_via(Use::Config(PathBuf::from(&file)), |usage| {
                    usage.config_dependency(workspace, (owner, may_list), dependency, &file);
                });
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
        self.unlisted_config_index
            .record(&mut self.unlisted_config, owner, name, PathBuf::from(file), reader);
    }

    fn collect_scripts(&mut self, workspace: &Workspace, setup: &Setup, fs: &dyn FileSystem) {
        self.via = Use::Tooling;
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
                        runs_in: None,
                    });
                }
            }
            for relocated in &package_setup.scripts.relocated_binaries {
                let binary = CommandBinary {
                    name: &relocated.binary.name,
                    dir: Some(relocated.dir.as_str()),
                    filtered: relocated.filtered,
                    judged: true,
                    origin: Origin::Script(&relocated.binary.script),
                };
                self.command_binary((workspace, setup, fs), id, binary);
            }
            for run in &package_setup.run_binaries {
                let file = relative(&run.file, &workspace.root).map_or_else(|| run.file.clone(), PathBuf::from);
                let binary = CommandBinary {
                    name: &run.name,
                    dir: run.dir.as_deref(),
                    filtered: run.filtered,
                    judged: run.judged,
                    origin: Origin::File(&file),
                };
                self.command_binary((workspace, setup, fs), id, binary);
            }
        }
    }

    /// The binaries and packages the `node:child_process` commands of the source file `file` (relative to the root)
    /// run and load, owned by package `id` whose scripts `context` holds: a binary fetched on demand credits the
    /// dependency that provides it, any other is a use of one or unlisted at the file ([`Self::command_binary`]), and
    /// a package is a use of the package where the command loads it.
    fn collect_commands(
        &mut self,
        cx: (&Workspace, &Setup, &dyn FileSystem),
        id: PackageId,
        file: &Path,
        commands: &[String],
        context: &ScriptContext,
    ) {
        // A call whose `cwd` is computed from the file's location runs in the file's directory.
        let path = self.path_in_package(id, file);
        for command in commands {
            let command = path
                .as_deref()
                .map_or_else(|| command.clone(), |path| place_file_dir(command, path));
            let info = parse_script(&command, context);
            for dependency in &info.dependencies {
                // A package loaded after `cd` into another workspace package is needed by that package.
                let owner = dependency
                    .dir
                    .as_deref()
                    .map_or(id, |dir| package_in_dir(&self.dirs, id, dir));
                if let Some(found) = self.find_listed(owner, &dependency.name) {
                    self.reference(found, &dependency.name);
                }
            }
            // As for the scripts of `package.json`: the system binaries count only with `ignore-global-binaries` off.
            let global_binaries = if cx.1.ignore_global_binaries {
                Vec::new()
            } else {
                info.global_binaries
            };
            let binaries = info
                .binaries
                .into_iter()
                .filter(|binary| !is_ignored_binary(&binary.name));
            for binary in binaries.chain(global_binaries) {
                let run = CommandBinary {
                    name: &binary.name,
                    dir: binary.relocation_dir(),
                    filtered: binary.filtered,
                    judged: !binary.via.is_ephemeral(),
                    origin: Origin::File(file),
                };
                self.command_binary(cx, id, run);
            }
        }
    }

    /// `file`, relative to the root, as a path relative to the directory of package `id`, `/`-separated; `None` when
    /// the file is not below it.
    fn path_in_package(&self, id: PackageId, file: &Path) -> Option<String> {
        let base = self.dirs.iter().find(|(package, _)| *package == id)?.1.as_str();
        let path = file.to_string_lossy().replace('\\', "/");
        if base.is_empty() {
            return Some(path);
        }
        path.strip_prefix(base)?.strip_prefix('/').map(str::to_owned)
    }

    /// The other workspace package, or the project of its own, whose directory holds `dir`, where a command of package
    /// `id` runs a binary after `cd` (relative to the package directory); `None` when it is in `id` itself, outside
    /// the workspace root, or in no project of its own below `id`.
    fn host(&self, workspace: &Workspace, fs: &dyn FileSystem, id: PackageId, dir: &str) -> Option<Host> {
        let (package, target) = placed_dir(&self.dirs, id, dir)?;
        let package_dir = self
            .dirs
            .iter()
            .find(|(candidate, _)| *candidate == package)
            .map_or("", |(_, dir)| dir.as_str());
        let project = project_between(fs, &workspace.root, package_dir, &target);
        if package == id && project.is_none() {
            return None;
        }
        let shown = |dir: &str| if dir.is_empty() { ".".to_owned() } else { dir.to_owned() };
        let runs_in = match &project {
            Some((abs, manifest)) => {
                let dir = abs
                    .strip_prefix(&workspace.root)
                    .map_or_else(|_| abs.to_string_lossy(), |rel| rel.to_string_lossy())
                    .replace('\\', "/");
                let name = manifest
                    .get("name")
                    .and_then(Value::as_str)
                    .map_or_else(|| dir.clone(), str::to_owned);
                RunsIn { dir, name }
            }
            None => RunsIn {
                dir: shown(package_dir),
                name: workspace
                    .package(package)
                    .map_or_else(|| shown(package_dir), |found| found.display_name().to_owned()),
            },
        };
        Some(Host {
            package,
            project,
            runs_in,
        })
    }

    /// A binary that a command of package `id` runs. Where the command starts, or after `cd` into another directory of
    /// `id`, it is a use of a dependency of `id` that provides it, else unlisted at the command, unless the command
    /// runs it in another workspace (`filtered`). After `cd` into another workspace package or a project of its own
    /// ([`Self::host`]) it is a use of a dependency there that provides it, which for a project its multi-root host
    /// credits to the root that owns it, or of the package holding the project; else it is unlisted at the command,
    /// naming where it runs, unless a package manager runs a script of that name there.
    fn command_binary(
        &mut self,
        (workspace, setup, fs): (&Workspace, &Setup, &dyn FileSystem),
        id: PackageId,
        binary: CommandBinary<'_>,
    ) {
        let name = binary.name;
        let Some(host) = binary.dir.and_then(|dir| self.host(workspace, fs, id, dir)) else {
            if binary.filtered {
                return;
            }
            match binary.origin {
                _ if !binary.judged => {
                    self.provide_binary_or_ancestor(id, name);
                }
                Origin::File(file) => self.run_binary(setup, id, name, file),
                Origin::Script(script) => {
                    if !self.provide_binary_or_ancestor(id, name) && !setup.ignores_binary(id, name) {
                        self.push_unlisted(UnlistedBinary {
                            package: id,
                            name: name.to_owned(),
                            script: script.to_owned(),
                            file: None,
                            runs_in: None,
                        });
                    }
                }
            }
            return;
        };
        let provided = host
            .project
            .as_ref()
            .is_some_and(|(dir, manifest)| self.provide_project_binary(fs, dir, manifest, name))
            || self.provide_binary_or_ancestor(host.package, name);
        if provided
            || !binary.judged
            || setup.ignores_binary(id, name)
            || setup.ignores_binary(host.package, name)
            || host.has_script(workspace, name)
        {
            return;
        }
        let (script, file) = match binary.origin {
            Origin::Script(script) => (script.to_owned(), None),
            Origin::File(file) => (String::new(), Some(file.to_path_buf())),
        };
        self.push_unlisted(UnlistedBinary {
            package: id,
            name: name.to_owned(),
            script,
            file,
            runs_in: Some(host.runs_in),
        });
    }

    /// Marks the dependencies that `manifest`, the manifest of the project of its own in `dir`, lists and that provide
    /// binary `bin` as used through that manifest ([`Self::ancestor_uses`]). False when none does.
    fn provide_project_binary(&mut self, fs: &dyn FileSystem, dir: &Path, manifest: &Value, bin: &str) -> bool {
        let mut cache = HashMap::default();
        let providers: Vec<String> = listed_names(manifest)
            .filter(|name| {
                listed_bins(fs, dir, name, &mut cache)
                    .iter()
                    .any(|provided| provided == bin)
            })
            .map(str::to_owned)
            .collect();
        for provider in &providers {
            self.outside_used.insert((dir.to_path_buf(), provider.clone()));
        }
        !providers.is_empty()
    }

    /// A binary that a command read from `file` (relative to the root) runs in package `id`: a use of the dependency
    /// that provides it, else an unlisted binary reported at the file.
    fn run_binary(&mut self, setup: &Setup, id: PackageId, name: &str, file: &Path) {
        if self.provide_binary(id, name) || self.ancestor_bins.contains_key(name) || setup.ignores_binary(id, name) {
            return;
        }
        self.push_unlisted_binary(id, name, file);
    }

    /// Reports binary `name`, which `file` (relative to the root) runs in package `id`, as unlisted, once.
    fn push_unlisted_binary(&mut self, id: PackageId, name: &str, file: &Path) {
        self.push_unlisted(UnlistedBinary {
            package: id,
            name: name.to_owned(),
            script: String::new(),
            file: Some(file.to_path_buf()),
            runs_in: None,
        });
    }

    /// Reports `binary` as unlisted, once.
    fn push_unlisted(&mut self, binary: UnlistedBinary) {
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
                        (unscoped(dep) == bin && !self.known_bins[scope.index()].contains(&***dep))
                            || BINARY_PACKAGES
                                .iter()
                                .any(|(b, package)| *b == bin && *package == &***dep)
                    })
                    .map(|dep| (Listed::Package(scope), dep.to_string())),
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
                        .filter(|dep| !self.known_bins[scope.index()].contains(&***dep) && resembles(dep, bin))
                        .map(|dep| (Listed::Package(scope), dep.to_string())),
                );
            }
        }
        if let Some(members) = self.member_bins.get(bin).cloned() {
            for (member_id, member) in members {
                if let Some(found) = self.find_listed(id, &member) {
                    provided.push((found, member));
                } else if self.member_bins_hoisted || member_id == id {
                    // A member that no manifest lists provides its binary only where the package manager links it
                    // for every package, or to its own scripts.
                    provided.push((Listed::Package(id), String::new()));
                }
            }
        }
        // Rush runs its own commands (`rush`, `rushx`, `rush-pnpm`) from its install, whatever a manifest lists.
        let found = !provided.is_empty() || (self.rush && RUSH_COMMANDS.contains(&bin));
        self.with_via(Use::Binary(bin.to_owned()), |usage| {
            for (scope, dep) in provided {
                if !dep.is_empty() {
                    usage.reference(scope, &dep);
                }
            }
        });
        found
    }

    /// Whether package `id` uses dependency `name`, directly, through its `@types` twin, or as a peer of a used host.
    #[must_use]
    pub fn is_referenced(&self, id: PackageId, name: &str) -> bool {
        self.credit(id, name).is_some()
    }

    /// Why package `id` uses dependency `name`; `None` when it does not. [`Self::is_referenced`] is this check.
    pub(crate) fn credit(&self, id: PackageId, name: &str) -> Option<Credit<'_>> {
        if ALWAYS_USED.contains(&name) {
            return Some(Credit::AlwaysUsed);
        }
        self.credit_inner(id, name, &mut HashSet::default())
    }

    fn credit_inner(&self, id: PackageId, name: &str, visited: &mut HashSet<String>) -> Option<Credit<'_>> {
        let referenced = &self.referenced[id.index()];
        if let Some(uses) = referenced.get(name) {
            return Some(Credit::Uses(uses));
        }
        if !visited.insert(name.to_owned()) {
            return None;
        }
        let typed = typed_package(name);
        if let Some(typed) = &typed {
            if UNPAIRED_TYPES.contains(&typed.as_str()) {
                return Some(Credit::UnpairedTypes);
            }
            if referenced.contains_key(typed) {
                return Some(Credit::TypesOf(typed.clone()));
            }
        }
        let hosts = &self.hosts[id.index()];
        std::iter::once(name)
            .chain(typed.as_deref())
            .filter_map(|candidate| hosts.get(candidate))
            .flatten()
            .find(|host| self.credit_inner(id, host, visited).is_some())
            .map(|host| Credit::PeerOf(host.clone()))
    }

    /// The uses recorded against the manifest of package `id` for dependency `name`, without the `@types` and peer
    /// rules of [`Self::credit`].
    pub(crate) fn uses(&self, id: PackageId, name: &str) -> Option<&BTreeSet<Use>> {
        self.referenced[id.index()].get(name)
    }

    /// Whether anything used `name` through package `id`'s manifest, regardless of the `@types` and peer rules.
    #[must_use]
    pub fn was_imported(&self, id: PackageId, name: &str) -> bool {
        self.referenced[id.index()].contains_key(name)
    }

    /// Whether the manifest of `id` lists `name` in any section.
    #[must_use]
    pub fn is_listed(&self, id: PackageId, name: &str) -> bool {
        self.listed[id.index()].contains(name)
    }

    /// The dependencies this workspace uses through the `package.json` of another project, each with the directory of
    /// that manifest: those its own code uses that only a manifest above its root lists (the innermost one), those
    /// the files outside its root that the graph reached import (the innermost manifest at or above the file that
    /// lists them), and those that provide a binary a command runs after `cd` into a project of its own below the root,
    /// with that project's manifest. Node.js resolves them from that project's `node_modules`, so they are used
    /// dependencies of that project, which cannot see this code.
    #[must_use]
    pub fn ancestor_uses(&self) -> Vec<(PathBuf, String)> {
        self.external_used
            .iter()
            .filter_map(|name| Some((self.external_listed.get(name)?.clone(), name.clone())))
            .chain(self.outside_used.iter().cloned())
            .collect()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::path::{Path, PathBuf};

    use skif_config::Config;
    use skif_core::MemoryFs;
    use skif_workspace::{PackageId, Workspace};

    use super::{SharedInstalled, UnlistedConfigDependency, UnlistedConfigIndex, package_ancestors};

    #[test]
    fn shared_installed_manifests_are_read_once_and_only_where_node_modules_is() {
        let fs = MemoryFs::new()
            .with_file(
                "/ws/node_modules/a/package.json",
                r#"{"name":"a","bin":"cli.js","version":"1.0.0"}"#,
            )
            .with_file(
                "/ws/packages/q/node_modules/a/package.json",
                r#"{"name":"a","version":"2.0.0"}"#,
            );
        let shared = SharedInstalled::default();
        let root = Path::new("/ws");
        let from_p = shared.read(&fs, Path::new("/ws/packages/p"), root, "a").unwrap();
        let again = shared.read(&fs, Path::new("/ws/packages/p"), root, "a").unwrap();
        let from_q = shared.read(&fs, Path::new("/ws/packages/q"), root, "a").unwrap();
        assert_eq!(from_p.bins, ["a"]);
        assert_eq!(from_p.version.as_deref(), Some("1.0.0"));
        assert!(std::sync::Arc::ptr_eq(&from_p, &again));
        assert_eq!(from_q.version.as_deref(), Some("2.0.0"));
        assert!(shared.read(&fs, Path::new("/ws/packages/p"), root, "missing").is_none());
        let manifests = shared.manifests.lock().unwrap();
        let mut read: Vec<&Path> = manifests.keys().map(PathBuf::as_path).collect();
        read.sort_unstable();
        assert_eq!(
            read,
            [
                Path::new("/ws/node_modules/a/package.json"),
                Path::new("/ws/node_modules/missing/package.json"),
                Path::new("/ws/packages/q/node_modules/a/package.json"),
            ]
        );
    }

    #[test]
    fn ancestors_are_the_containing_packages_innermost_first_then_the_root() {
        let fs = MemoryFs::new()
            .with_file(
                "/ws/package.json",
                r#"{"name":"r","workspaces":["packages/a","packages/a/sub","packages/a/sub/deep","packages/ab"]}"#,
            )
            .with_file("/ws/packages/a/package.json", r#"{"name":"a"}"#)
            .with_file("/ws/packages/a/sub/package.json", r#"{"name":"sub"}"#)
            .with_file("/ws/packages/a/sub/deep/package.json", r#"{"name":"deep"}"#)
            .with_file("/ws/packages/ab/package.json", r#"{"name":"ab"}"#);
        let config = Config::from_toml_str("").unwrap();
        let workspace = Workspace::discover_with_fs(Path::new("/ws"), &config, &fs).unwrap();
        let ancestors = package_ancestors(&workspace, false);
        let name = |id: PackageId| workspace.package(id).and_then(|package| package.name.clone()).unwrap();
        let of = |wanted: &str| {
            let (id, _) = workspace
                .iter()
                .find(|(_, package)| package.name.as_deref() == Some(wanted))
                .unwrap();
            ancestors[id.index()]
                .iter()
                .map(|&other| name(other))
                .collect::<Vec<_>>()
        };
        assert_eq!(of("deep"), ["sub", "a", "r"]);
        assert_eq!(of("sub"), ["a", "r"]);
        assert_eq!(of("a"), ["r"]);
        // `packages/ab` starts with the text of `packages/a` but is not below it.
        assert_eq!(of("ab"), ["r"]);
        assert_eq!(of("r"), [] as [std::string::String; 0]);
        assert!(package_ancestors(&workspace, true).iter().all(Vec::is_empty));
    }

    #[test]
    fn unlisted_config_entries_are_kept_once_per_package_name_and_file_with_each_reader_once() {
        let (a, b, c) = (
            PackageId::from_index(1),
            PackageId::from_index(2),
            PackageId::from_index(3),
        );
        let mut list = Vec::new();
        let mut index = UnlistedConfigIndex::default();
        let mut record = |package, name, file: &str, reader| {
            index.record(&mut list, package, name, PathBuf::from(file), reader);
        };
        record(a, "x", "tsconfig.base.json", Some(b));
        record(a, "y", "tsconfig.base.json", None);
        record(a, "x", "tsconfig.base.json", Some(c));
        record(a, "x", "tsconfig.base.json", Some(b));
        record(a, "x", "other.json", None);
        record(b, "x", "tsconfig.base.json", None);
        record(a, "x", "tsconfig.base.json", None);
        let entry = |package, name: &str, file: &str, readers: Vec<PackageId>| UnlistedConfigDependency {
            package,
            name: name.to_owned(),
            file: PathBuf::from(file),
            readers,
        };
        assert_eq!(
            list,
            [
                entry(a, "x", "tsconfig.base.json", vec![b, c]),
                entry(a, "y", "tsconfig.base.json", vec![]),
                entry(a, "x", "other.json", vec![]),
                entry(b, "x", "tsconfig.base.json", vec![]),
            ]
        );
    }
}
