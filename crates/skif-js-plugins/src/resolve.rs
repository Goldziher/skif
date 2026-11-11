//! The driver: decides which plugins are enabled for a workspace, finds and reads their config files and
//! collects their output.

use std::cell::RefCell;
use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};

use skif_config::{PluginConfig, PluginOptions};
use skif_js_syntax::{ConfigValue, ModuleLoader, place_file_dir};

use crate::config_load::{from_json, load_config_and_call_arguments};
use crate::files::find_files;
use crate::plugin::{
    BinaryRef, BuildDir, ConfigAst, DepRef, Enabler, IncompleteCause, IncompleteEntries, IncompleteScope, PathAlias,
    Plugin, PluginContext, PluginId, PluginInput, PluginOutput, PluginSpec,
};
use crate::plugins::nx;
use crate::registry;
use crate::scopes::{NxProjects, Scope, merge, rebase};
use crate::script_config::{config_flags, config_paths, flag_values};
use crate::scripts::{BinaryVia, ScriptContext, is_ignored_binary, parse_script};
use crate::support::{is_relative, join_lexically, join_rel, looks_like_file};

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
/// The Nx projects below the package are those [`NxProjects::find`] finds; [`resolve_plugins_with`] takes them
/// from the caller.
#[must_use]
pub fn resolve_plugins(ctx: &PluginContext<'_>) -> Vec<(PluginId, PluginOutput)> {
    resolve_plugins_with(ctx, &NxProjects::find(ctx))
}

/// Like [`resolve_plugins`], with the Nx projects below the package given (ADR 0109). The `nx` plugin reads the
/// `project.json` of each of [`NxProjects::dirs`]. Every other plugin also runs in each of [`NxProjects::scopes`] as
/// if it were a package with the dependencies of `ctx`, and what it finds there is part of its output for `ctx`.
#[must_use]
pub fn resolve_plugins_with(ctx: &PluginContext<'_>, projects: &NxProjects) -> Vec<(PluginId, PluginOutput)> {
    let manifest = read_manifest(ctx);
    let project_files: Vec<String> = projects.dirs.iter().map(|dir| join_rel(dir, "project.json")).collect();
    let scopes: Vec<Scope> = projects.scopes.iter().map(|dir| Scope::new(ctx, dir)).collect();
    registry::plugins()
        .iter()
        .filter_map(|&plugin| {
            let spec = plugin.spec();
            let setting = ctx.plugins.get(spec.id);
            let is_nx = spec.id == nx::ID;
            let extra: &[String] = if is_nx { &project_files } else { &[] };
            let mut output = is_enabled(spec, setting, ctx, manifest.as_ref())
                .then(|| run_plugin(plugin, setting, ctx, manifest.as_ref(), extra));
            // The `nx` plugin reads every `project.json` from the package, where Nx runs the commands of a target.
            for scope in scopes.iter().filter(|_| !is_nx) {
                let scope_ctx = scope.context(ctx);
                let setting = scope_ctx.plugins.get(spec.id);
                if is_enabled(spec, setting, &scope_ctx, None) {
                    let found = rebase(run_plugin(plugin, setting, &scope_ctx, None, &[]), &scope.dir);
                    merge(output.get_or_insert_with(PluginOutput::default), found);
                }
            }
            output.map(|output| (spec.id, output))
        })
        .collect()
}

/// Whether the plugin `id` is enabled for the workspace in `ctx`.
pub(crate) fn is_plugin_enabled(ctx: &PluginContext<'_>, id: PluginId) -> bool {
    registry::plugin(id)
        .is_some_and(|plugin| is_enabled(plugin.spec(), ctx.plugins.get(id), ctx, read_manifest(ctx).as_ref()))
}

fn read_manifest(ctx: &PluginContext<'_>) -> Option<ConfigValue> {
    let source = ctx.fs.read_to_string(&ctx.dir.join("package.json")).ok()?;
    let value: serde_json::Value = serde_json::from_str(&source).ok()?;
    Some(from_json(&value))
}

fn is_enabled(
    spec: &PluginSpec,
    setting: Option<&PluginConfig>,
    ctx: &PluginContext<'_>,
    manifest: Option<&ConfigValue>,
) -> bool {
    let forced = match setting {
        Some(PluginConfig::Enabled(enabled)) => Some(*enabled),
        Some(PluginConfig::Options(options)) => options.enabled,
        _ => None,
    };
    forced.unwrap_or_else(|| detected(spec, ctx, manifest))
}

fn detected(spec: &PluginSpec, ctx: &PluginContext<'_>, manifest: Option<&ConfigValue>) -> bool {
    (ctx.is_root || !spec.root_only)
        && spec
            .enablers
            .iter()
            .any(|enabler| enabler_matches(enabler, ctx, manifest))
}

fn enabler_matches(enabler: &Enabler, ctx: &PluginContext<'_>, manifest: Option<&ConfigValue>) -> bool {
    match *enabler {
        Enabler::Always => true,
        Enabler::Dependency(name) => ctx.has_dependency_in_ancestors(name),
        Enabler::DependencyPrefix(prefix) => ctx
            .dependencies
            .iter()
            .chain(ctx.inherited_dependencies)
            .any(|d| d.starts_with(prefix)),
        Enabler::ManifestNamePrefix(prefix) => ctx.package_name.is_some_and(|n| n.starts_with(prefix)),
        Enabler::ManifestNameContains(text) => ctx.package_name.is_some_and(|n| n.contains(text)),
        Enabler::File(glob) => !find_files(ctx.fs, ctx.dir, &[glob.to_string()]).is_empty(),
        Enabler::Script(text) => ctx.scripts.values().any(|script| script.contains(text)),
        Enabler::ManifestKey(key) => manifest.is_some_and(|manifest| {
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

struct Collector<'a, 'b> {
    ctx: &'a PluginContext<'b>,
    out: PluginOutput,
    queue: VecDeque<Queued>,
    /// Whether the module resolution options of a config inside another package stay with that package.
    confine_to_package: bool,
    /// The modules, with their source, that the config being absorbed imports values from.
    imported: Vec<(PathBuf, String)>,
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
                PluginInput::EntryRegex(pattern) => push_unique(&mut self.out.entry_regexes, pattern),
                PluginInput::Dependency(name) => {
                    let named_by = self.named_by(from, config, &name);
                    self.dependency(DepRef::new(name, &named_by, false, config.is_some()));
                }
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
                PluginInput::ScriptAt { dir, script } => self.script_at(&dir, &script),
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
            }
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
            find_files(self.ctx.fs, self.ctx.dir, &[path.to_owned()])
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
            .find(|d| d.name == reference.name && d.from == reference.from)
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

    fn parse_script(&mut self, text: &str, from: &Path, optional: bool) {
        let mut script_ctx = ScriptContext::new();
        script_ctx.scripts.clone_from(self.ctx.scripts);
        let info = parse_script(text, &script_ctx);
        for binary in info.binaries {
            // As for the scripts of `package.json`: a binary fetched on demand, run in another workspace or that the
            // script parser ignores is never unlisted here.
            let judged = !optional || binary.via != BinaryVia::Direct || binary.bin_path;
            // A filtered binary counts only as a credit to the package of the directory it runs in.
            let dir = binary.relocation_dir().map(str::to_owned);
            let placed = !binary.filtered || dir.is_some();
            if judged && placed && !binary.via.is_ephemeral() && !is_ignored_binary(&binary.name) {
                let run = BinaryRef::new(binary.name.clone(), from, dir, binary.filtered);
                if !self.out.run_binaries.contains(&run) {
                    self.out.run_binaries.push(run);
                }
            }
            push_unique(&mut self.out.referenced_binaries, binary.name);
        }
        for file in info.files {
            push_unique(&mut self.out.entries, file.path);
        }
        for dep in info.dependencies {
            self.dependency(DepRef::new(dep.name, from, dep.ephemeral, false));
        }
    }

    /// The files a script runs in `dir` are entries; what it names besides belongs to the package in `dir`.
    fn script_at(&mut self, dir: &str, text: &str) {
        let mut script_ctx = ScriptContext::new();
        script_ctx.scripts.clone_from(self.ctx.scripts);
        let mut texts = vec![text.to_owned()];
        texts.extend(flag_values(text, &["cdk"], &["-a", "--app"]));
        for text in texts {
            for file in parse_script(&text, &script_ctx).files {
                push_unique(&mut self.out.entries, join_rel(dir, &file.path));
            }
        }
    }
}

/// Runs `plugin` for the workspace in `ctx`, reading `extra_configs` (relative to the workspace) besides the config
/// files its globs find.
fn run_plugin(
    plugin: &dyn Plugin,
    setting: Option<&PluginConfig>,
    ctx: &PluginContext<'_>,
    manifest: Option<&ConfigValue>,
    extra_configs: &[String],
) -> PluginOutput {
    let spec = plugin.spec();
    let options = match setting {
        Some(PluginConfig::Options(options)) => options.clone(),
        _ => PluginOptions::default(),
    };
    let mut collector = Collector {
        ctx,
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
    collector
        .queue
        .extend(find_files(ctx.fs, ctx.dir, &patterns).into_iter().map(|path| Queued {
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
        // A plugin that reads the source rather than the exported value (`export const config` of Stencil) still
        // sees a file whose export is not understood.
        let recording = ctx.modules.map(|inner| Recording {
            inner,
            loaded: RefCell::new(Vec::new()),
        });
        let modules = recording.as_ref().map(|recording| recording as &dyn ModuleLoader);
        let (value, call_arguments) = match load_config_and_call_arguments(&path, &source, modules) {
            Some(found) => found,
            None if !reader.reads_config() => (ConfigValue::Null, Vec::new()),
            None => {
                let Some(value) = reader.read_script(&path, &source) else {
                    if is_structured(&path) && !source.trim().is_empty() {
                        let cause = IncompleteCause::Unparsable;
                        collector.mark_incomplete(reader.spec().id, &path, cause, IncompleteScope::Package);
                    }
                    continue;
                };
                (value, Vec::new())
            }
        };
        let ast = ConfigAst {
            path: &path,
            base: parent_of(&relative),
            value: &value,
            source: &source,
        };
        if reader.reads_config()
            && let Some(cause) = unreadable(&value)
        {
            let scope = reader.incomplete_scope(cause, &ast);
            collector.mark_incomplete(reader.spec().id, &path, cause, scope);
        }
        let mut inputs = reader.resolve_config(&ast, ctx);
        if reader.reads_config() {
            inputs.extend(argument_entries(ctx, ast.base, &call_arguments));
        }
        collector.imported = recording
            .map(|recording| recording.loaded.into_inner())
            .unwrap_or_default();
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
        };
        let mut collector = Collector {
            ctx: &ctx,
            out: PluginOutput::default(),
            queue: VecDeque::new(),
            confine_to_package: false,
            imported: Vec::new(),
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
