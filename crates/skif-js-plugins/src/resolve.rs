//! The driver: decides which plugins are enabled for a workspace, finds and reads their config files and
//! collects their output.

use std::cell::{OnceCell, RefCell};
use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::LazyLock;

use skif_config::{PluginConfig, PluginOptions};
use skif_core::{HashMap, HashSet};
use skif_js_syntax::{ConfigValue, ModuleLoader, place_file_dir};

use crate::config_load::{from_json, load_config_and_call_arguments};
use crate::files::{Listing, find_files, list_files};
use crate::plugin::{
    BinaryRef, BuildDir, ConfigAst, DepRef, Enabler, IncompleteCause, IncompleteEntries, IncompleteScope, PathAlias,
    Plugin, PluginContext, PluginId, PluginInput, PluginOutput, PluginSpec,
};
use crate::plugins::nx;
use crate::registry;
use crate::scopes::{NxProjects, ScopeShared, merge, rebase, scope_dir, within_package};
use crate::script_config::{config_flags, config_paths, flag_values};
use crate::scripts::{BinaryVia, ScriptContext, is_ignored_binary, parse_script};
use crate::sources::PackageSources;
use crate::support::{is_relative, join_lexically, join_rel, looks_like_file, path_within};

const MAX_CONFIG_DEPTH: u8 = 16;

/// Keys under which a tool config lists files to leave out (`exclude`, `coverage.exclude`, `ignores`,
/// `testPathIgnorePatterns`, `testIgnore`): a file named there is not one a factory is given to run.
const EXCLUSION_KEYS: &[&str] = &[
    "exclude",
    "excludes",
    "excludeFiles",
    "watchExclude",
    "ignore",
    "ignores",
    "ignored",
    "ignorePatterns",
    "testPathIgnorePatterns",
    "coveragePathIgnorePatterns",
    "modulePathIgnorePatterns",
    "transformIgnorePatterns",
    "watchPathIgnorePatterns",
    "testIgnore",
];

/// Resolves every enabled plugin for the workspace in `ctx`, in registry order.
///
/// A plugin is enabled when an enabler matches or `[js.plugins] <id> = true` forces it; `<id> = false`
/// disables it. A table form (`{ config = [...], entry = [...], project = [...] }`) replaces the matching
/// default globs and enables the plugin unless `enabled = false`.
///
/// The Nx projects below the package are those [`NxProjects::find`] finds, with the package taken as the Nx workspace
/// root. They are not filtered by `.gitignore` files or build output directories; [`resolve_plugins_with`] takes the
/// projects from a caller that walks the files with those rules.
#[must_use]
pub fn resolve_plugins(ctx: &PluginContext<'_>) -> Vec<(PluginId, PluginOutput)> {
    resolve_plugins_in(ctx, "", || NxProjects::found_dirs(ctx))
}

/// Like [`resolve_plugins`], with the Nx projects below the package given (ADR 0032). The `nx` plugin reads the
/// `project.json` of each of [`NxProjects::dirs`], with its paths relative to the Nx workspace root
/// ([`NxProjects::path`]). Every other plugin also runs in each of [`NxProjects::scopes`] as if it were a package
/// with the dependencies of `ctx`, and what it finds there is part of its output for `ctx`.
#[must_use]
pub fn resolve_plugins_with(ctx: &PluginContext<'_>, projects: &NxProjects) -> Vec<(PluginId, PluginOutput)> {
    let detection = Detection::new(ctx);
    let nx_enabled = is_nx_enabled(ctx, &detection);
    resolve_detected(ctx, projects, &detection, nx_enabled)
}

/// [`resolve_plugins_with`] the projects [`NxProjects::from_dirs`] lists for `path` and `projects`, with the
/// dependencies of `ctx` matched against the enablers once for both: whether the `nx` plugin is enabled, which decides
/// whether `projects` is called, and every plugin's own enablers.
#[must_use]
pub fn resolve_plugins_in(
    ctx: &PluginContext<'_>,
    path: &str,
    projects: impl FnOnce() -> Vec<(String, bool)>,
) -> Vec<(PluginId, PluginOutput)> {
    let detection = Detection::new(ctx);
    let nx_enabled = is_nx_enabled(ctx, &detection);
    let projects = if nx_enabled {
        NxProjects::listed(path, projects())
    } else {
        NxProjects::default()
    };
    resolve_detected(ctx, &projects, &detection, nx_enabled)
}

fn is_nx_enabled(ctx: &PluginContext<'_>, detection: &Detection<'_>) -> bool {
    registry::plugin(nx::ID).is_some_and(|plugin| is_enabled(plugin.spec(), ctx.plugins.get(nx::ID), ctx, detection))
}

fn resolve_detected(
    ctx: &PluginContext<'_>,
    projects: &NxProjects,
    detection: &Detection<'_>,
    nx_enabled: bool,
) -> Vec<(PluginId, PluginOutput)> {
    let package_sources = PackageSources::new(ctx.dir);
    let ctx = &PluginContext {
        nx: if nx_enabled { Some(projects) } else { ctx.nx },
        sources: Some(PackageSources::of(ctx).unwrap_or(&package_sources)),
        ..*ctx
    };
    let project_files: Vec<String> = projects.dirs.iter().map(|dir| join_rel(dir, "project.json")).collect();
    let plugins = registry::plugins();
    let configs = ConfigCache::default();
    let mut outputs: Vec<Option<PluginOutput>> = plugins
        .iter()
        .map(|&plugin| {
            let spec = plugin.spec();
            let setting = ctx.plugins.get(spec.id);
            let extra: &[String] = if spec.id == nx::ID { &project_files } else { &[] };
            is_enabled(spec, setting, ctx, detection).then(|| {
                // Only a plugin with a `package.json` key reads the manifest.
                let manifest = spec.package_json_key.and_then(|_| detection.manifest(ctx));
                run_plugin(plugin, setting, ctx, manifest, extra, None, &configs)
            })
        })
        .collect();
    run_in_scopes(ctx, &projects.scopes, plugins, &mut outputs, &configs);
    plugins
        .iter()
        .zip(outputs)
        .filter_map(|(plugin, output)| output.map(|output| (plugin.spec().id, output)))
        .collect()
}

/// How a plugin can be enabled in the scopes of a package, which share the package's dependencies and switches.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ScopeEnablement {
    /// In every scope.
    Always,
    /// In a scope holding a file one of its [`Enabler::File`] globs matches.
    ByFile,
}

/// Runs each plugin other than `nx` in every scope in `scopes` (directories of the package of `ctx`) where it is
/// enabled, and merges what it finds there, rebased onto the package, into its entry of `outputs`, which follows the
/// order of `plugins`.
///
/// Each scope's files are listed once and searched for every plugin's config files and file enablers. A plugin
/// that gives no default entry, production, project or ignore globs and finds no config file in a scope is not run
/// there, its config-less and workspace conventions included, unless a target of the scope's `project.json` runs the
/// tool's Nx executor (`@nx/vite:build`), which runs it with its defaults.
fn run_in_scopes(
    ctx: &PluginContext<'_>,
    scopes: &[String],
    plugins: &[&'static dyn Plugin],
    outputs: &mut [Option<PluginOutput>],
    configs: &ConfigCache,
) {
    let Some(first) = scopes.first() else {
        return;
    };
    let shared = ScopeShared::new(ctx);
    // A scope's context differs from another's only in its directory, which only file enablers look at.
    let first_dir = scope_dir(ctx, first);
    let template = shared.context(ctx, &first_dir);
    let detection = Detection::without_manifest(&template);
    let candidates: Vec<(usize, ScopeEnablement)> = plugins
        .iter()
        .enumerate()
        .filter(|(_, plugin)| plugin.spec().id != nx::ID)
        .filter_map(|(index, plugin)| {
            let spec = plugin.spec();
            scope_enablement(spec, template.plugins.get(spec.id), &template, &detection)
                .map(|enablement| (index, enablement))
        })
        .collect();
    if candidates.is_empty() {
        return;
    }
    for scope in scopes {
        let dir = scope_dir(ctx, scope);
        // A scope's sources are also the package's: a file the package's plugins read is not read again.
        let scope_sources = ctx.sources.map_or_else(
            || PackageSources::new(&dir),
            |package| PackageSources::within(&dir, package),
        );
        let scope_members = members_within(ctx.members, scope);
        let scope_ctx = PluginContext {
            sources: Some(&scope_sources),
            members: &scope_members,
            ..shared.context(ctx, &dir)
        };
        let files = list_files(ctx.fs, &dir);
        let listing = Listing::new(&files, ctx.globs);
        let executors = nx::executor_plugins(&scope_ctx);
        for &(index, enablement) in &candidates {
            let plugin = plugins[index];
            let spec = plugin.spec();
            if enablement == ScopeEnablement::ByFile && !has_enabling_file(spec, &listing) {
                continue;
            }
            if !gives_defaults(spec) && !listing.any_matching(spec.config) && !nx::runs_tool_of(&executors, spec.id) {
                continue;
            }
            let setting = scope_ctx.plugins.get(spec.id);
            let found = rebase(
                run_plugin(plugin, setting, &scope_ctx, None, &[], Some(&listing), configs),
                scope,
            );
            merge(outputs[index].get_or_insert_with(PluginOutput::default), found);
        }
    }
}

/// The directories of `members` below `scope`, relative to it.
fn members_within(members: &[String], scope: &str) -> Vec<String> {
    members
        .iter()
        .filter_map(|member| path_within(member, scope).filter(|inside| !inside.is_empty()))
        .collect()
}

/// How the plugin of `spec` is enabled in a scope whose context is `scope_ctx`, as far as the scope's files do not
/// matter; `None` when it cannot be.
fn scope_enablement(
    spec: &PluginSpec,
    setting: Option<&PluginConfig>,
    scope_ctx: &PluginContext<'_>,
    detection: &Detection<'_>,
) -> Option<ScopeEnablement> {
    match forced(setting) {
        Some(true) => return Some(ScopeEnablement::Always),
        Some(false) => return None,
        None => {}
    }
    if spec.root_only && !scope_ctx.is_root {
        return None;
    }
    let mut by_file = false;
    for enabler in spec.enablers {
        if matches!(enabler, Enabler::File(_)) {
            by_file = true;
        } else if enabler_matches(enabler, scope_ctx, detection) {
            return Some(ScopeEnablement::Always);
        }
    }
    by_file.then_some(ScopeEnablement::ByFile)
}

/// Whether the plugin of `spec` gives entry, production, project or ignore globs without a config.
fn gives_defaults(spec: &PluginSpec) -> bool {
    [spec.entry, spec.production, spec.project, spec.ignore]
        .iter()
        .any(|globs| !globs.is_empty())
}

/// Whether `files`, a scope's listing, holds a file an [`Enabler::File`] of `spec` matches.
fn has_enabling_file(spec: &PluginSpec, files: &Listing<'_>) -> bool {
    spec.enablers.iter().any(|enabler| match *enabler {
        Enabler::File(glob) => files.any_matching(&[glob]),
        _ => false,
    })
}

/// Whether the plugin `id` is enabled for the workspace in `ctx`.
pub(crate) fn is_plugin_enabled(ctx: &PluginContext<'_>, id: PluginId) -> bool {
    registry::plugin(id).is_some_and(|plugin| is_enabled(plugin.spec(), ctx.plugins.get(id), ctx, &Detection::new(ctx)))
}

fn read_manifest(ctx: &PluginContext<'_>) -> Option<ConfigValue> {
    let source = ctx.fs.read_to_string(&ctx.dir.join("package.json")).ok()?;
    let value: serde_json::Value = serde_json::from_str(&source).ok()?;
    Some(from_json(&value))
}

/// What `[js.plugins]` forces for a plugin: `true`, `false`, or the `enabled` of a table; `None` leaves it to the
/// enablers.
fn forced(setting: Option<&PluginConfig>) -> Option<bool> {
    match setting {
        Some(PluginConfig::Enabled(enabled)) => Some(*enabled),
        Some(PluginConfig::Options(options)) => options.enabled,
        _ => None,
    }
}

fn is_enabled(
    spec: &PluginSpec,
    setting: Option<&PluginConfig>,
    ctx: &PluginContext<'_>,
    detection: &Detection<'_>,
) -> bool {
    forced(setting).unwrap_or_else(|| detected(spec, ctx, detection))
}

fn detected(spec: &PluginSpec, ctx: &PluginContext<'_>, detection: &Detection<'_>) -> bool {
    (ctx.is_root || !spec.root_only)
        && spec
            .enablers
            .iter()
            .any(|enabler| enabler_matches(enabler, ctx, detection))
}

/// Every [`Enabler::File`] glob of the table, once each, in table order.
static FILE_ENABLERS: LazyLock<Vec<String>> = LazyLock::new(|| {
    let mut globs: Vec<String> = Vec::new();
    for plugin in registry::plugins() {
        for enabler in plugin.spec().enablers {
            if let Enabler::File(glob) = *enabler {
                push_unique(&mut globs, glob.to_owned());
            }
        }
    }
    globs
});

/// What the enablers of every plugin are matched against in one context, worked out once for all of them: the
/// dependency names as a set and in sorted order, the manifest, read when an enabler or a plugin first needs it, and
/// the files that any [`Enabler::File`] glob matches, found by one walk when the first such enabler is matched.
struct Detection<'a> {
    dependencies: HashSet<&'a str>,
    sorted: Vec<&'a str>,
    manifest: OnceCell<Option<ConfigValue>>,
    enabling_files: OnceCell<Vec<String>>,
}

impl<'a> Detection<'a> {
    fn new(ctx: &PluginContext<'a>) -> Self {
        Self::with_manifest(ctx, OnceCell::new())
    }

    /// Like [`Detection::new`], for a context whose manifest no enabler reads.
    fn without_manifest(ctx: &PluginContext<'a>) -> Self {
        Self::with_manifest(ctx, OnceCell::from(None))
    }

    fn with_manifest(ctx: &PluginContext<'a>, manifest: OnceCell<Option<ConfigValue>>) -> Self {
        let mut sorted: Vec<&'a str> = ctx
            .dependencies
            .iter()
            .chain(ctx.inherited_dependencies)
            .map(String::as_str)
            .collect();
        let dependencies = sorted.iter().copied().collect();
        sorted.sort_unstable();
        Self {
            dependencies,
            sorted,
            manifest,
            enabling_files: OnceCell::new(),
        }
    }

    fn manifest(&self, ctx: &PluginContext<'_>) -> Option<&ConfigValue> {
        self.manifest.get_or_init(|| read_manifest(ctx)).as_ref()
    }

    /// Whether the context or an ancestor depends on a package whose name starts with `prefix`: the names that do
    /// follow one another in sorted order, from the first name not below `prefix`.
    fn has_dependency_prefix(&self, prefix: &str) -> bool {
        let first = self.sorted.partition_point(|name| *name < prefix);
        self.sorted.get(first).is_some_and(|name| name.starts_with(prefix))
    }

    /// Whether a file below the context's directory matches `glob`, one of [`FILE_ENABLERS`]: a walk for all of them
    /// visits every directory a walk for `glob` visits, so the files it finds that `glob` matches are those a walk for
    /// `glob` finds.
    fn has_file(&self, ctx: &PluginContext<'_>, glob: &str) -> bool {
        let files = self
            .enabling_files
            .get_or_init(|| find_files(ctx.fs, ctx.globs, ctx.dir, &FILE_ENABLERS));
        !files.is_empty() && Listing::new(files, ctx.globs).any_matching(&[glob])
    }
}

fn enabler_matches(enabler: &Enabler, ctx: &PluginContext<'_>, detection: &Detection<'_>) -> bool {
    match *enabler {
        Enabler::Always => true,
        Enabler::Dependency(name) => detection.dependencies.contains(name),
        Enabler::DependencyPrefix(prefix) => detection.has_dependency_prefix(prefix),
        Enabler::ManifestNamePrefix(prefix) => ctx.package_name.is_some_and(|n| n.starts_with(prefix)),
        Enabler::ManifestNameContains(text) => ctx.package_name.is_some_and(|n| n.contains(text)),
        Enabler::File(glob) => detection.has_file(ctx, glob),
        Enabler::Script(text) => ctx.scripts.values().any(|script| script.contains(text)),
        Enabler::ManifestKey(key) => detection.manifest(ctx).is_some_and(|manifest| {
            key.split('.')
                .try_fold(manifest, |value, part| value.get(part))
                .is_some()
        }),
    }
}

fn globs(over: &[String], default: &[&str]) -> Vec<String> {
    if over.is_empty() {
        default.iter().map(|s| (*s).to_string()).collect()
    } else {
        over.to_vec()
    }
}

fn push_unique(list: &mut Vec<String>, item: String) {
    if !list.contains(&item) {
        list.push(item);
    }
}

fn parent_of(relative: &str) -> &str {
    relative.rsplit_once('/').map_or("", |(parent, _)| parent)
}

/// A config file as the config loader read it, with the modules it imports values from.
struct LoadedConfig {
    parsed: Option<(ConfigValue, Vec<ConfigValue>)>,
    imported: Vec<(PathBuf, String)>,
}

/// The config files read for one package and its scopes, by absolute path: several plugins read the same file (a
/// `vite.config.ts` read by every Vite plugin, a `tsconfig.json` linked from many configs), and the loader's result
/// depends only on the file.
#[derive(Default)]
struct ConfigCache(RefCell<HashMap<PathBuf, Rc<LoadedConfig>>>);

impl ConfigCache {
    /// The file at `path` with the text `source`, loaded with `modules` the first time it is asked for.
    fn load(&self, path: &Path, source: &str, modules: Option<&dyn ModuleLoader>) -> Rc<LoadedConfig> {
        if let Some(hit) = self.0.borrow().get(path) {
            return Rc::clone(hit);
        }
        let recording = modules.map(|inner| Recording {
            inner,
            loaded: RefCell::new(Vec::new()),
        });
        let parsed = load_config_and_call_arguments(
            path,
            source,
            recording.as_ref().map(|recording| recording as &dyn ModuleLoader),
        );
        let loaded = Rc::new(LoadedConfig {
            parsed,
            imported: recording.map_or_default(|recording| recording.loaded.into_inner()),
        });
        self.0.borrow_mut().insert(path.to_path_buf(), Rc::clone(&loaded));
        loaded
    }
}

struct Collector<'a, 'b> {
    ctx: &'a PluginContext<'b>,
    configs: &'a ConfigCache,
    /// The script parser's view of the workspace's scripts, made when the first command is parsed.
    script_ctx: OnceCell<ScriptContext>,
    out: PluginOutput,
    queue: VecDeque<Queued>,
    /// Whether the module resolution options of a config inside another package stay with that package.
    confine_to_package: bool,
    /// The modules, with their source, that the config being absorbed imports values from.
    imported: Vec<(PathBuf, String)>,
    /// Whether `[js.plugins]` configures the plugin's entries, which then replace its default entry globs.
    entries_configured: bool,
}

/// A [`ModuleLoader`] that records the modules it loads, so a package a config names can be traced to the module
/// that names it.
struct Recording<'a> {
    inner: &'a dyn ModuleLoader,
    loaded: RefCell<Vec<(PathBuf, String)>>,
}

impl ModuleLoader for Recording<'_> {
    fn load(&self, from: &Path, specifier: &str) -> Option<(PathBuf, String)> {
        let found = self.inner.load(from, specifier)?;
        let mut loaded = self.loaded.borrow_mut();
        if !loaded.iter().any(|(path, _)| *path == found.0) {
            loaded.push(found.clone());
        }
        Some(found)
    }

    fn is_installed(&self, from: &Path, specifier: &str) -> bool {
        self.inner.is_installed(from, specifier)
    }

    fn holds_no_code(&self, from: &Path, specifier: &str) -> bool {
        self.inner.holds_no_code(from, specifier)
    }
}

/// Whether `source` holds a string literal naming the package `name` or a path inside it (`'eslint-plugin-foo'`,
/// `"@scope/pkg/sub"`).
fn names_package(source: &str, name: &str) -> bool {
    ['\'', '"', '`'].iter().any(|quote| {
        let needle = format!("{quote}{name}");
        source.match_indices(&needle).any(|(at, _)| {
            source[at + needle.len()..]
                .chars()
                .next()
                .is_some_and(|next| next == *quote || next == '/')
        })
    })
}

/// A config file waiting to be read.
struct Queued {
    /// Path relative to the workspace directory.
    path: String,
    depth: u8,
    /// The plugin that reads it; `None` for the plugin being run.
    reader: Option<PluginId>,
}

impl Collector<'_, '_> {
    /// Whether `from` is a config of another package below the workspace, whose `paths`, `baseUrl` and output
    /// directories apply to that package's files only (a `tsconfig` `references` entry points into a sub-project).
    fn belongs_to_other_package(&self, from: &Path) -> bool {
        self.confine_to_package
            && from.parent().is_some_and(|dir| {
                dir != self.ctx.dir && dir.starts_with(self.ctx.dir) && self.ctx.fs.is_file(&dir.join("package.json"))
            })
    }

    /// Whether `path`, a module a config imports values from, belongs to another package than the workspace: it lies
    /// outside the workspace directory or below a nested `package.json`.
    fn is_foreign_module(&self, path: &Path) -> bool {
        if !path.starts_with(self.ctx.dir) {
            return true;
        }
        path.ancestors()
            .skip(1)
            .take_while(|dir| *dir != self.ctx.dir)
            .any(|dir| self.ctx.fs.is_file(&dir.join("package.json")))
    }

    /// The file that names the package `name` the config `from` holds: the config itself, unless only a module of
    /// another package it imports values from names it (`extends: [base]` of a shared config package). That module
    /// then answers for the package, so its owner is judged for it, as for a config of its own.
    fn named_by(&self, from: &Path, config: Option<&ConfigAst<'_>>, name: &str) -> PathBuf {
        let Some(config) = config.filter(|_| !self.imported.is_empty()) else {
            return from.to_path_buf();
        };
        if names_package(config.source, name) {
            return from.to_path_buf();
        }
        self.imported
            .iter()
            .find(|(path, source)| self.is_foreign_module(path) && names_package(source, name))
            .map_or_else(|| from.to_path_buf(), |(path, _)| path.clone())
    }

    /// Takes in what `reader`, the plugin whose config `from` is, read from it. `config` is the config read, which
    /// bounds the entries a [`PluginInput::EntriesIncomplete`] hides; without it they may lie anywhere.
    fn absorb(
        &mut self,
        reader: PluginId,
        inputs: Vec<PluginInput>,
        (from, config): (&Path, Option<&ConfigAst<'_>>),
        depth: u8,
    ) {
        let foreign = self.belongs_to_other_package(from);
        let optional = registry::plugin(reader).is_some_and(|plugin| plugin.spec().optional_binaries);
        for input in inputs {
            if foreign
                && matches!(
                    input,
                    PluginInput::Alias { .. }
                        | PluginInput::ModuleDir(_)
                        | PluginInput::BuildDir { .. }
                        | PluginInput::Condition(_)
                )
            {
                continue;
            }
            match input {
                PluginInput::Entry(path) => push_unique(&mut self.out.entries, path),
                PluginInput::ProductionEntry(path) => {
                    push_unique(&mut self.out.entries, path.clone());
                    push_unique(&mut self.out.production_entries, path);
                }
                PluginInput::BuildSource(path) => push_unique(&mut self.out.build_sources, path),
                PluginInput::ProductionBuildSource(path) => {
                    push_unique(&mut self.out.build_sources, path.clone());
                    push_unique(&mut self.out.production_build_sources, path);
                }
                PluginInput::EntryRegex(pattern) => push_unique(&mut self.out.entry_regexes, pattern),
                PluginInput::Dependency(name) => {
                    let named_by = self.named_by(from, config, &name);
                    self.dependency(DepRef::new(name, &named_by, false, config.is_some()));
                }
                PluginInput::ReaderDependency { name, span } => self.dependency(DepRef {
                    reader_lists: true,
                    span: Some(span),
                    ..DepRef::new(name, from, false, config.is_some())
                }),
                PluginInput::OptionalDependency(name) => {
                    self.dependency(DepRef::new(name, from, true, config.is_some()));
                }
                PluginInput::CompilerDependency { name, type_only } => self.dependency(DepRef {
                    type_only,
                    reader_may_list: true,
                    ..DepRef::new(name, from, false, config.is_some())
                }),
                PluginInput::Binary(name) => push_unique(&mut self.out.referenced_binaries, name),
                PluginInput::ConfigFile(path) => {
                    if depth < MAX_CONFIG_DEPTH && self.ctx.fs.is_file(&join_lexically(self.ctx.dir, &path)) {
                        self.queue.push_back(Queued {
                            path,
                            depth: depth + 1,
                            reader: None,
                        });
                    }
                }
                PluginInput::LinkedConfig { plugin, path } => self.linked_config(plugin, &path, depth),
                PluginInput::VirtualModule(pattern) => push_unique(&mut self.out.virtual_modules, pattern),
                PluginInput::Script(text) => self.script(&text, from, optional),
                PluginInput::ScriptAt { dir, script } => self.script_at(&dir, &script, from, optional),
                PluginInput::SourceScript { file, script } => {
                    let source = self.ctx.dir.join(&file);
                    // A call whose `cwd` is computed from the file's location runs in the file's directory.
                    self.script(&place_file_dir(&script, &file), &source, optional);
                }
                PluginInput::FileDependency { file, name } => {
                    let source = self.ctx.dir.join(&file);
                    self.dependency(DepRef::new(name, &source, false, true));
                }
                PluginInput::Ignore(glob) => push_unique(&mut self.out.ignore, glob),
                PluginInput::Alias { find, target } => {
                    let alias = PathAlias { find, target };
                    if !self.out.aliases.contains(&alias) {
                        self.out.aliases.push(alias);
                    }
                }
                PluginInput::BuildDir { out_dir, root_dir } => {
                    let dir = BuildDir { out_dir, root_dir };
                    if !self.out.build_dirs.contains(&dir) {
                        self.out.build_dirs.push(dir);
                    }
                }
                PluginInput::Condition(name) => push_unique(&mut self.out.conditions, name),
                PluginInput::ModuleDir(dir) => push_unique(&mut self.out.module_dirs, dir),
                PluginInput::Extension(extension) => push_unique(&mut self.out.extensions, extension),
                PluginInput::EntriesIncomplete => {
                    let cause = IncompleteCause::ComputedEntries;
                    let scope = config.map_or(IncompleteScope::Package, |config| scope_of(reader, cause, config));
                    self.mark_incomplete(reader, from, cause, scope);
                }
                PluginInput::ReplaceDefaultEntries(entries) => self.replace_default_entries(reader, entries),
            }
        }
    }

    /// Replaces the default entry globs of `reader` with `entries`, unless `[js.plugins]` configures its entries.
    fn replace_default_entries(&mut self, reader: PluginId, entries: Vec<String>) {
        if self.entries_configured {
            return;
        }
        if let Some(plugin) = registry::plugin(reader) {
            let defaults = plugin.spec().entry;
            self.out.entries.retain(|entry| !defaults.contains(&entry.as_str()));
        }
        for entry in entries {
            push_unique(&mut self.out.entries, entry);
        }
    }

    /// Marks the entries incomplete because `plugin` could not fully read the config `file`; the entries it hides
    /// lie in `scope`.
    fn mark_incomplete(&mut self, plugin: PluginId, file: &Path, cause: IncompleteCause, scope: IncompleteScope) {
        self.out.entries_incomplete = true;
        let reason = IncompleteEntries {
            plugin,
            file: file.to_path_buf(),
            cause,
            scope,
        };
        if !self.out.incomplete_reasons.contains(&reason) {
            self.out.incomplete_reasons.push(reason);
        }
    }

    /// Queues the files `path` names for `plugin`, a glob matching none of them queueing nothing.
    fn linked_config(&mut self, plugin: PluginId, path: &str, depth: u8) {
        if depth >= MAX_CONFIG_DEPTH || registry::plugin(plugin).is_none() {
            return;
        }
        let files = if path.contains(['*', '?', '{', '[']) {
            find_files(self.ctx.fs, self.ctx.globs, self.ctx.dir, &[path.to_owned()])
        } else if self.ctx.fs.is_file(&self.ctx.dir.join(path)) {
            vec![path.to_owned()]
        } else {
            Vec::new()
        };
        for path in files {
            self.queue.push_back(Queued {
                path,
                depth: depth + 1,
                reader: Some(plugin),
            });
        }
    }

    /// Records `reference`, merged into an earlier reference to the same package from the same file.
    fn dependency(&mut self, reference: DepRef) {
        match self
            .out
            .referenced_dependencies
            .iter_mut()
            .find(|d| d.name == reference.name && d.from == reference.from && d.dir == reference.dir)
        {
            Some(existing) => existing.merge(&reference),
            None => self.out.referenced_dependencies.push(reference),
        }
    }

    /// A command read from `from`. With `optional` (see [`PluginSpec::optional_binaries`]) a binary it runs by name is
    /// only a credit.
    fn script(&mut self, text: &str, from: &Path, optional: bool) {
        self.parse_script(text, from, optional);
        // `cdk deploy -a "npx ts-node bin/app.ts"` runs the app command, which is a script of its own.
        for app in flag_values(text, &["cdk"], &["-a", "--app"]) {
            self.parse_script(&app, from, optional);
        }
    }

    fn script_context(&self) -> &ScriptContext {
        self.script_ctx.get_or_init(|| ScriptContext {
            scripts: self.ctx.scripts.clone(),
            ..ScriptContext::new()
        })
    }

    fn parse_script(&mut self, text: &str, from: &Path, optional: bool) {
        let info = parse_script(text, self.script_context());
        for binary in info.binaries {
            // As for the scripts of `package.json`: a binary fetched on demand, run in another workspace or that the
            // script parser ignores is never unlisted here.
            let named = binary.via != BinaryVia::Direct || binary.bin_path;
            let judged = (!optional || named) && !binary.via.is_ephemeral();
            // A filtered binary counts only as a credit to the package of the directory it runs in.
            let dir = binary.relocation_dir().map(str::to_owned);
            let placed = !binary.filtered || dir.is_some();
            // A binary run in another directory belongs to whatever is there, so it is no credit where the command
            // starts.
            let elsewhere = dir.is_some();
            if placed && (judged || elsewhere) && !is_ignored_binary(&binary.name) {
                let run = BinaryRef::new(binary.name.clone(), from, dir, binary.filtered);
                let run = if judged { run } else { run.credit_only() };
                if !self.out.run_binaries.contains(&run) {
                    self.out.run_binaries.push(run);
                }
            }
            if !elsewhere {
                push_unique(&mut self.out.referenced_binaries, binary.name);
            }
        }
        // A command that runs in the Nx workspace root names the package's own files through it
        // (`cd ../.. && node packages/b/run.mjs`).
        let package_path = self.ctx.nx.map_or("", |projects| projects.path.as_str());
        for file in info.files {
            push_unique(&mut self.out.entries, within_package(package_path, &file.path));
        }
        for dep in info.dependencies {
            self.dependency(DepRef {
                dir: dep.dir,
                ..DepRef::new(dep.name, from, dep.ephemeral, false)
            });
        }
    }

    /// A command read from `from` that runs in `dir`, read as if it started with `cd <dir>`: the files it runs are
    /// entries there, and the binaries and packages it names carry the directory, as after `cd`.
    fn script_at(&mut self, dir: &str, text: &str, from: &Path, optional: bool) {
        // A line of its own, so that a script that starts with a comment or a shebang still follows it.
        let cd = format!("cd '{}'\n", dir.replace('\'', r"'\''"));
        let mut texts = vec![text.to_owned()];
        texts.extend(flag_values(text, &["cdk"], &["-a", "--app"]));
        for text in texts {
            self.parse_script(&format!("{cd}{text}"), from, optional);
        }
    }
}

/// Runs `plugin` for the workspace in `ctx`, reading `extra_configs` (relative to the workspace) besides the config
/// files its globs find: in `listing`, the workspace's files as [`list_files`] lists them, or else by a walk.
fn run_plugin(
    plugin: &dyn Plugin,
    setting: Option<&PluginConfig>,
    ctx: &PluginContext<'_>,
    manifest: Option<&ConfigValue>,
    extra_configs: &[String],
    listing: Option<&Listing<'_>>,
    configs: &ConfigCache,
) -> PluginOutput {
    let spec = plugin.spec();
    let options = match setting {
        Some(PluginConfig::Options(options)) => options.clone(),
        _ => PluginOptions::default(),
    };
    let mut collector = Collector {
        ctx,
        configs,
        script_ctx: OnceCell::new(),
        out: PluginOutput {
            entries: globs(&options.entry, spec.entry),
            project: globs(&options.project, spec.project),
            production_entries: spec.production.iter().map(|s| (*s).to_string()).collect(),
            provided_binaries: spec.binaries.iter().map(|s| (*s).to_string()).collect(),
            ignore: spec.ignore.iter().map(|s| (*s).to_string()).collect(),
            custom_elements: spec.custom_elements,
            ..PluginOutput::default()
        },
        queue: VecDeque::new(),
        // The configs an Nx target links (`tsConfig`, `jestConfig`) may be another package's, as a `tsconfig`
        // reference may.
        confine_to_package: matches!(spec.id, "typescript" | "tsrs" | "nx"),
        imported: Vec::new(),
        entries_configured: !options.entry.is_empty(),
    };
    for production in spec.production {
        push_unique(&mut collector.out.entries, (*production).to_string());
    }
    let mut patterns = globs(&options.config, spec.config);
    patterns.extend(extra_configs.iter().cloned());
    let scripted = scripted_configs(spec, ctx);
    patterns.extend(scripted.config);
    for entry in scripted.entry {
        push_unique(&mut collector.out.entries, entry);
    }
    for project in scripted.project {
        push_unique(&mut collector.out.project, project);
    }
    let found = listing.map_or_else(
        || find_files(ctx.fs, ctx.globs, ctx.dir, &patterns),
        |files| files.matching(&patterns),
    );
    collector.queue.extend(found.into_iter().map(|path| Queued {
        path,
        depth: 0,
        reader: None,
    }));

    let mut visited = BTreeSet::new();
    drain(&mut collector, plugin, &mut visited);
    let found_config = !collector.out.config_files.is_empty();

    if let (Some(key), Some(manifest)) = (spec.package_json_key, manifest)
        && let Some(value) = key.split('.').try_fold(manifest, |value, part| value.get(part))
    {
        let path = ctx.dir.join("package.json");
        let ast = ConfigAst {
            path: &path,
            base: "",
            value,
            source: "",
        };
        let inputs = plugin.resolve_config(&ast, ctx);
        if !collector.out.config_files.contains(&path) {
            collector.out.config_files.push(path.clone());
        }
        collector.absorb(spec.id, inputs, (&path, Some(&ast)), 0);
        drain(&mut collector, plugin, &mut visited);
    } else if !found_config {
        let inputs = plugin.resolve_without_config(ctx);
        collector.absorb(spec.id, inputs, (&ctx.dir.join("package.json"), None), 0);
        drain(&mut collector, plugin, &mut visited);
    }
    let inputs = plugin.resolve_workspace(ctx);
    collector.absorb(spec.id, inputs, (&ctx.dir.join("package.json"), None), 0);
    drain(&mut collector, plugin, &mut visited);
    collector.out
}

/// Config patterns, and entry and project globs, for the config files and directories that the workspace's scripts
/// pass to the tool (`storybook dev -c ui/.storybook`, `vitest --config configs/vitest.ts`).
#[derive(Default)]
struct ScriptedConfigs {
    config: Vec<String>,
    entry: Vec<String>,
    project: Vec<String>,
}

fn scripted_configs(spec: &PluginSpec, ctx: &PluginContext<'_>) -> ScriptedConfigs {
    let mut found = ScriptedConfigs::default();
    if spec.binaries.is_empty() {
        return found;
    }
    let prefix = spec.config.first().and_then(|pattern| split_last_segment(pattern).0);
    // `config_paths` yields only paths inside the package (relative, without `..`), so probing one stays inside it.
    for path in config_paths(ctx.scripts, spec.binaries, &config_flags(spec.id)) {
        if looks_like_file(&path) || ctx.fs.is_file(&ctx.dir.join(&path)) {
            found.config.push(path);
            continue;
        }
        for pattern in spec.config {
            found.config.push(join_rel(&path, split_last_segment(pattern).1));
        }
        let Some(prefix) = prefix else { continue };
        let rebase = |patterns: &[&str], into: &mut Vec<String>| {
            for pattern in patterns {
                if let (Some(head), rest) = split_last_segment(pattern)
                    && head == prefix
                {
                    into.push(join_rel(&path, rest));
                }
            }
        };
        rebase(spec.entry, &mut found.entry);
        rebase(spec.project, &mut found.project);
    }
    found
}

/// Splits a glob at its last `/` that is outside braces: `(Some(".{a,b}"), "main.ts")`; the head is `None` without one.
fn split_last_segment(pattern: &str) -> (Option<&str>, &str) {
    let mut depth = 0_u32;
    for (index, c) in pattern.char_indices().rev() {
        match c {
            '}' => depth += 1,
            '{' => depth = depth.saturating_sub(1),
            '/' if depth == 0 => return (Some(&pattern[..index]), &pattern[index + 1..]),
            _ => {}
        }
    }
    (None, pattern)
}

/// Reads every queued config file, which may queue further ones.
fn drain(collector: &mut Collector<'_, '_>, plugin: &dyn Plugin, visited: &mut BTreeSet<String>) {
    let ctx = collector.ctx;
    while let Some(Queued {
        path: relative,
        depth,
        reader,
    }) = collector.queue.pop_front()
    {
        let key = reader.map_or_else(|| relative.clone(), |id| format!("{id}:{relative}"));
        if !visited.insert(key) {
            continue;
        }
        let reader: &dyn Plugin = match reader.and_then(registry::plugin) {
            Some(linked) => linked,
            None => plugin,
        };
        let path = join_lexically(ctx.dir, &relative);
        let Ok(source) = ctx.fs.read_to_string(&path) else {
            continue;
        };
        collector.out.config_files.push(path.clone());
        let loaded = collector.configs.load(&path, &source, ctx.modules);
        // A plugin that reads the source rather than the exported value (`export const config` of Stencil) still
        // sees a file whose export is not understood.
        let fallback: ConfigValue;
        let (value, call_arguments): (&ConfigValue, &[ConfigValue]) = match &loaded.parsed {
            Some((value, call_arguments)) => (value, call_arguments.as_slice()),
            None if !reader.reads_config() => {
                fallback = ConfigValue::Null;
                (&fallback, &[][..])
            }
            None => {
                let Some(value) = reader.read_script(&path, &source) else {
                    if is_structured(&path) && !source.trim().is_empty() {
                        let cause = IncompleteCause::Unparsable;
                        collector.mark_incomplete(reader.spec().id, &path, cause, IncompleteScope::Package);
                    }
                    continue;
                };
                fallback = value;
                (&fallback, &[][..])
            }
        };
        let ast = ConfigAst {
            path: &path,
            base: parent_of(&relative),
            value,
            source: &source,
        };
        if reader.reads_config()
            && let Some(cause) = unreadable(value)
        {
            let scope = reader.incomplete_scope(cause, &ast);
            collector.mark_incomplete(reader.spec().id, &path, cause, scope);
        }
        let mut inputs = reader.resolve_config(&ast, ctx);
        if reader.reads_config() {
            inputs.extend(argument_entries(ctx, ast.base, call_arguments));
        }
        collector.imported.clone_from(&loaded.imported);
        collector.absorb(reader.spec().id, inputs, (&path, Some(&ast)), depth);
        collector.imported.clear();
    }
}

/// The files named in `arguments`, the arguments of a call a config exports that the evaluator cannot follow
/// (`createConfig({ setupFiles: ['./setup.ts'] })`): a factory hands the files it is given to the tool, so each string
/// that names an existing file of the package as a path (`./setup.ts`, `test/setup.ts`), relative to the config's
/// directory `base`, is an entry. A bare name (`chart.js`) is a package however a file is named, and a string under an
/// exclusion key ([`EXCLUSION_KEYS`]) names a file to leave out. The config stays unknown, and the package's entries
/// incomplete, for what the factory adds itself.
fn argument_entries(ctx: &PluginContext<'_>, base: &str, arguments: &[ConfigValue]) -> Vec<PluginInput> {
    let mut entries: Vec<String> = Vec::new();
    for text in arguments.iter().flat_map(included_strings).map(str::trim) {
        if !(is_relative(text) || text.contains('/')) || text.starts_with('/') {
            continue;
        }
        let path = join_rel(base, text);
        if !path.starts_with("..") && ctx.fs.is_file(&ctx.dir.join(&path)) && !entries.contains(&path) {
            entries.push(path);
        }
    }
    entries.into_iter().map(PluginInput::Entry).collect()
}

/// The strings in `value`, however deeply nested, except those under an exclusion key ([`EXCLUSION_KEYS`]).
fn included_strings(value: &ConfigValue) -> Vec<&str> {
    match value {
        ConfigValue::String(text) => vec![text.as_str()],
        ConfigValue::Array(items) => items.iter().flat_map(included_strings).collect(),
        ConfigValue::Object(entries) => entries
            .iter()
            .filter(|(key, _)| !EXCLUSION_KEYS.contains(&key.as_str()))
            .flat_map(|(_, value)| included_strings(value))
            .collect(),
        _ => Vec::new(),
    }
}

/// Where the entries hidden by `config`, which plugin `reader` reads, can lie; anywhere when the plugin is unknown.
fn scope_of(reader: PluginId, cause: IncompleteCause, config: &ConfigAst<'_>) -> IncompleteScope {
    registry::plugin(reader).map_or(IncompleteScope::Package, |plugin| {
        plugin.incomplete_scope(cause, config)
    })
}

/// Whether `path` is a JavaScript, TypeScript, JSON, YAML or TOML file, whose content a plugin is expected to read.
fn is_structured(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|extension| {
        matches!(
            extension.to_ascii_lowercase().as_str(),
            "js" | "cjs"
                | "mjs"
                | "ts"
                | "cts"
                | "mts"
                | "jsx"
                | "tsx"
                | "json"
                | "jsonc"
                | "json5"
                | "yml"
                | "yaml"
                | "toml"
        )
    })
}

/// Whether a config evaluated to something unknown, or to an object with an unknown spread at the top level: what the
/// plugin would have read from the unknown part is missing.
fn unreadable(value: &ConfigValue) -> Option<IncompleteCause> {
    match value {
        ConfigValue::Object(entries) if entries.iter().any(|(key, _)| ConfigValue::is_spread_key(key)) => {
            Some(IncompleteCause::UnknownSpread)
        }
        other => other.is_unknown().then_some(IncompleteCause::UnknownValue),
    }
}

#[cfg(test)]
mod tests {

    use indexmap::IndexMap;
    use skif_core::MemoryFs;

    use super::*;
    use crate::files::GlobCache;

    fn context<'a>(fs: &'a MemoryFs, dependencies: &'a [String], inherited: &'a [String]) -> PluginContext<'a> {
        static SCRIPTS: LazyLock<IndexMap<String, String>> = LazyLock::new(IndexMap::new);
        static PLUGINS: LazyLock<IndexMap<String, PluginConfig>> = LazyLock::new(IndexMap::new);
        static GLOBS: LazyLock<GlobCache> = LazyLock::new(GlobCache::new);
        PluginContext {
            dir: Path::new("/ws"),
            is_root: true,
            package_name: None,
            dependencies,
            inherited_dependencies: inherited,
            scripts: &SCRIPTS,
            fs,
            plugins: &PLUGINS,
            modules: None,
            nx: None,
            globs: &GLOBS,
            sources: None,
            members: &[],
        }
    }

    /// A loader that serves one module and counts how often it is asked.
    struct Counting(std::cell::Cell<usize>);

    impl ModuleLoader for Counting {
        fn load(&self, _from: &Path, specifier: &str) -> Option<(PathBuf, String)> {
            self.0.set(self.0.get() + 1);
            (specifier == "./base.js").then(|| (PathBuf::from("/ws/base.js"), "export default { y: 2 };".to_owned()))
        }
    }

    /// A config read again, by another plugin, is the value read the first time, with the modules it imports, and
    /// the loader is not asked again.
    #[test]
    fn a_config_is_loaded_once_for_every_plugin_that_reads_it() {
        let loader = Counting(std::cell::Cell::new(0));
        let cache = ConfigCache::default();
        let path = Path::new("/ws/vite.config.js");
        let source = "import base from './base.js';\nexport default { ...base, x: 1 };";
        let first = cache.load(path, source, Some(&loader));
        let asked = loader.0.get();
        assert!(asked > 0, "the config imports a value");
        let again = cache.load(path, source, Some(&loader));
        assert!(Rc::ptr_eq(&first, &again));
        assert_eq!(loader.0.get(), asked);
        assert_eq!(
            again.imported,
            [(PathBuf::from("/ws/base.js"), "export default { y: 2 };".to_owned())]
        );
        let (value, _) = again.parsed.as_ref().expect("the config parses");
        assert!(value.get("x").is_some(), "{value:?}");
        let other = cache.load(Path::new("/ws/other.json"), "{\"z\": 1}", Some(&loader));
        assert_eq!(other.imported, []);
        assert!(other.parsed.as_ref().is_some_and(|(value, _)| value.get("z").is_some()));
    }

    /// The dependency set and its sorted order answer exact names and prefixes as a scan of the context's own and
    /// inherited dependencies does.
    #[test]
    fn dependencies_are_matched_by_name_and_prefix_as_a_scan_matches_them() {
        let fs = MemoryFs::new();
        let own = ["react".to_owned(), "@storybook/react".to_owned(), "vite".to_owned()];
        let inherited = ["@vue/compiler-sfc".to_owned(), "a".to_owned()];
        for (dependencies, inherited) in [(&own[..], &inherited[..]), (&[][..], &[][..]), (&own[..], &[][..])] {
            let ctx = context(&fs, dependencies, inherited);
            let detection = Detection::new(&ctx);
            for text in [
                "react",
                "vite",
                "a",
                "@vue/compiler-sfc",
                "@storybook/",
                "@vue/",
                "@angular/",
                "",
                "z",
                "re",
            ] {
                let scan = |matches: &dyn Fn(&String) -> bool| dependencies.iter().chain(inherited).any(matches);
                assert_eq!(
                    enabler_matches(&Enabler::Dependency(text), &ctx, &detection),
                    scan(&|d: &String| d == text),
                    "{text:?} in {dependencies:?} + {inherited:?}"
                );
                assert_eq!(
                    enabler_matches(&Enabler::DependencyPrefix(text), &ctx, &detection),
                    scan(&|d: &String| d.starts_with(text)),
                    "{text:?}* in {dependencies:?} + {inherited:?}"
                );
            }
        }
    }

    /// One walk for every file enabler answers each of them as a walk for that enabler alone does.
    #[test]
    fn file_enablers_are_matched_as_a_walk_for_each_matches_them() {
        let fs = MemoryFs::new()
            .with_file("/ws/.github/workflows/ci.yml", "")
            .with_file("/ws/.azure-pipelines/deep/x/build.yaml", "")
            .with_file("/ws/Taskfile.dist.yml", "")
            .with_file("/ws/.vscode/mcp.json", "")
            .with_file("/ws/pnpm-lock.yaml", "")
            .with_file("/ws/node_modules/x/.travis.yml", "")
            .with_file("/ws/src/vercel.json", "");
        let ctx = context(&fs, &[], &[]);
        let detection = Detection::new(&ctx);
        let mut found = 0;
        for glob in FILE_ENABLERS.iter() {
            let walked = !find_files(&fs, ctx.globs, ctx.dir, std::slice::from_ref(glob)).is_empty();
            found += usize::from(walked);
            assert_eq!(detection.has_file(&ctx, glob), walked, "{glob}");
        }
        assert_eq!(found, 5, "the fixture matches five enablers");
    }

    /// Computed entries in a linked config are charged to the plugin that reads that config, as an unreadable
    /// linked config is.
    #[test]
    fn computed_entries_name_the_plugin_whose_config_it_is() {
        let fs = MemoryFs::new();
        let scripts = IndexMap::new();
        let plugins = IndexMap::new();
        let ctx = PluginContext {
            dir: Path::new("/ws"),
            is_root: true,
            package_name: None,
            dependencies: &[],
            inherited_dependencies: &[],
            scripts: &scripts,
            fs: &fs,
            plugins: &plugins,
            modules: None,
            nx: None,
            globs: &GlobCache::new(),
            sources: None,
            members: &[],
        };
        let configs = ConfigCache::default();
        let mut collector = Collector {
            ctx: &ctx,
            configs: &configs,
            script_ctx: OnceCell::new(),
            out: PluginOutput::default(),
            queue: VecDeque::new(),
            confine_to_package: false,
            imported: Vec::new(),
            entries_configured: false,
        };
        collector.absorb(
            "vite",
            vec![PluginInput::EntriesIncomplete],
            (Path::new("/ws/vite.config.ts"), None),
            1,
        );
        let plugins: Vec<PluginId> = collector
            .out
            .incomplete_reasons
            .iter()
            .map(|reason| reason.plugin)
            .collect();
        assert_eq!(plugins, ["vite"]);
    }

    /// The strings in the arguments of an exported call the evaluator cannot follow that name an existing file of the
    /// package as a path, relative to the config's directory, are entries; a bare name (a package, even where a file of
    /// that name exists), a string under an exclusion key, a missing file, a directory and a path outside the package
    /// are not.
    #[test]
    fn the_files_an_unknown_factory_is_given_are_entries() {
        let fs = MemoryFs::new()
            .with_file("/ws/test/config/boot.ts", "")
            .with_file("/ws/test/config/skip.ts", "")
            .with_file("/ws/test/vitest.setup.ts", "")
            .with_file("/ws/test/chart.js", "")
            .with_file("/ws/test/fixtures/ignored.ts", "")
            .with_file("/ws/shared.ts", "");
        let scripts = IndexMap::new();
        let plugins = IndexMap::new();
        let ctx = PluginContext {
            dir: Path::new("/ws"),
            is_root: true,
            package_name: None,
            dependencies: &[],
            inherited_dependencies: &[],
            scripts: &scripts,
            fs: &fs,
            plugins: &plugins,
            modules: None,
            nx: None,
            globs: &GlobCache::new(),
            sources: None,
            members: &[],
        };
        let text = |value: &str| ConfigValue::String(value.to_owned());
        let arguments = [
            ConfigValue::Object(vec![
                (
                    "setupFiles".to_owned(),
                    ConfigValue::Array(vec![
                        text("./config/boot.ts"),
                        text("./missing.ts"),
                        text("vitest.setup.ts"),
                    ]),
                ),
                ("exclude".to_owned(), ConfigValue::Array(vec![text("./config/skip.ts")])),
                (
                    "coverage".to_owned(),
                    ConfigValue::Object(vec![(
                        "exclude".to_owned(),
                        ConfigValue::Array(vec![text("fixtures/ignored.ts")]),
                    )]),
                ),
                (
                    "testPathIgnorePatterns".to_owned(),
                    ConfigValue::Array(vec![text("./config/skip.ts")]),
                ),
                ("environment".to_owned(), text("jsdom")),
                ("chart".to_owned(), text("chart.js")),
                ("root".to_owned(), text("./config")),
                ("outside".to_owned(), text("../../elsewhere.ts")),
            ]),
            text("../shared.ts"),
            text("./config/boot.ts"),
            text("fixtures/ignored.ts"),
        ];
        assert_eq!(
            argument_entries(&ctx, "test", &arguments),
            [
                PluginInput::Entry("test/config/boot.ts".to_owned()),
                PluginInput::Entry("shared.ts".to_owned()),
                PluginInput::Entry("test/fixtures/ignored.ts".to_owned()),
            ]
        );
    }

    /// A file named under the exclusion key of any tool (Playwright's `testIgnore`, Vitest's `watchExclude`,
    /// `excludeFiles`) is left out of what a factory that cannot be followed is given.
    #[test]
    fn a_file_under_any_tools_exclusion_key_is_not_an_entry() {
        let fs = MemoryFs::new()
            .with_file("/ws/config/boot.ts", "")
            .with_file("/ws/config/skip.ts", "");
        let scripts = IndexMap::new();
        let plugins = IndexMap::new();
        let ctx = PluginContext {
            dir: Path::new("/ws"),
            is_root: true,
            package_name: None,
            dependencies: &[],
            inherited_dependencies: &[],
            scripts: &scripts,
            fs: &fs,
            plugins: &plugins,
            modules: None,
            nx: None,
            globs: &GlobCache::new(),
            sources: None,
            members: &[],
        };
        let text = |value: &str| ConfigValue::String(value.to_owned());
        for key in ["testIgnore", "watchExclude", "excludeFiles"] {
            let arguments = [ConfigValue::Object(vec![
                (
                    "setupFiles".to_owned(),
                    ConfigValue::Array(vec![text("./config/boot.ts")]),
                ),
                (key.to_owned(), ConfigValue::Array(vec![text("./config/skip.ts")])),
            ])];
            assert_eq!(
                argument_entries(&ctx, "", &arguments),
                [PluginInput::Entry("config/boot.ts".to_owned())],
                "{key}"
            );
        }
    }
}
