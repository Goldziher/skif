//! The driver: decides which plugins are enabled for a workspace, finds and reads their config files and
//! collects their output.

use std::collections::{BTreeSet, VecDeque};
use std::path::Path;

use skif_config::{PluginConfig, PluginOptions};
use skif_js_syntax::ConfigValue;

use crate::config_load::{from_json, load_config};
use crate::files::find_files;
use crate::plugin::{
    BuildDir, ConfigAst, DepRef, Enabler, PathAlias, Plugin, PluginContext, PluginId, PluginInput, PluginOutput,
    PluginSpec,
};
use crate::registry;
use crate::script_config::{config_flags, config_paths, flag_values};
use crate::scripts::{ScriptContext, parse_script};
use crate::support::{join_rel, looks_like_file};

const MAX_CONFIG_DEPTH: u8 = 16;

/// Resolves every enabled plugin for the workspace in `ctx`, in registry order.
///
/// A plugin is enabled when an enabler matches or `[js.plugins] <id> = true` forces it; `<id> = false`
/// disables it. A table form (`{ config = [...], entry = [...], project = [...] }`) replaces the matching
/// default globs and enables the plugin unless `enabled = false`.
#[must_use]
pub fn resolve_plugins(ctx: &PluginContext<'_>) -> Vec<(PluginId, PluginOutput)> {
    let manifest = read_manifest(ctx);
    registry::plugins()
        .iter()
        .filter_map(|&plugin| {
            let spec = plugin.spec();
            let setting = ctx.plugins.get(spec.id);
            is_enabled(spec, setting, ctx).then(|| (spec.id, run_plugin(plugin, setting, ctx, manifest.as_ref())))
        })
        .collect()
}

fn read_manifest(ctx: &PluginContext<'_>) -> Option<ConfigValue> {
    let source = ctx.fs.read_to_string(&ctx.dir.join("package.json")).ok()?;
    let value: serde_json::Value = serde_json::from_str(&source).ok()?;
    Some(from_json(&value))
}

fn is_enabled(spec: &PluginSpec, setting: Option<&PluginConfig>, ctx: &PluginContext<'_>) -> bool {
    let forced = match setting {
        Some(PluginConfig::Enabled(enabled)) => Some(*enabled),
        Some(PluginConfig::Options(options)) => options.enabled,
        None => None,
    };
    forced.unwrap_or_else(|| detected(spec, ctx))
}

fn detected(spec: &PluginSpec, ctx: &PluginContext<'_>) -> bool {
    (ctx.is_root || !spec.root_only) && spec.enablers.iter().any(|enabler| enabler_matches(enabler, ctx))
}

fn enabler_matches(enabler: &Enabler, ctx: &PluginContext<'_>) -> bool {
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
    fn absorb(&mut self, inputs: Vec<PluginInput>, from: &Path, depth: u8) {
        for input in inputs {
            match input {
                PluginInput::Entry(path) => push_unique(&mut self.out.entries, path),
                PluginInput::ProductionEntry(path) => {
                    push_unique(&mut self.out.entries, path.clone());
                    push_unique(&mut self.out.production_entries, path);
                }
                PluginInput::Dependency(name) => self.dependency(name, from, false),
                PluginInput::OptionalDependency(name) => self.dependency(name, from, true),
                PluginInput::Binary(name) => push_unique(&mut self.out.referenced_binaries, name),
                PluginInput::ConfigFile(path) => {
                    if depth < MAX_CONFIG_DEPTH && self.ctx.fs.is_file(&self.ctx.dir.join(&path)) {
                        self.queue.push_back(Queued {
                            path,
                            depth: depth + 1,
                            reader: None,
                        });
                    }
                }
                PluginInput::LinkedConfig { plugin, path } => self.linked_config(plugin, &path, depth),
                PluginInput::VirtualModule(pattern) => push_unique(&mut self.out.virtual_modules, pattern),
                PluginInput::Script(text) => self.script(&text, from),
                PluginInput::ScriptAt { dir, script } => self.script_at(&dir, &script),
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
                PluginInput::EntriesIncomplete => self.out.entries_incomplete = true,
            }
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

    fn dependency(&mut self, name: String, from: &Path, optional: bool) {
        match self
            .out
            .referenced_dependencies
            .iter_mut()
            .find(|d| d.name == name && d.from == from)
        {
            Some(existing) => existing.optional &= optional,
            None => self.out.referenced_dependencies.push(DepRef {
                name,
                from: from.to_path_buf(),
                span: None,
                optional,
            }),
        }
    }

    fn script(&mut self, text: &str, from: &Path) {
        self.parse_script(text, from);
        // `cdk deploy -a "npx ts-node bin/app.ts"` runs the app command, which is a script of its own.
        for app in flag_values(text, &["cdk"], &["-a", "--app"]) {
            self.parse_script(&app, from);
        }
    }

    fn parse_script(&mut self, text: &str, from: &Path) {
        let mut script_ctx = ScriptContext::new();
        script_ctx.scripts.clone_from(self.ctx.scripts);
        let info = parse_script(text, &script_ctx);
        for binary in info.binaries {
            push_unique(&mut self.out.referenced_binaries, binary.name);
        }
        for file in info.files {
            push_unique(&mut self.out.entries, file.path);
        }
        for dep in info.dependencies {
            self.dependency(dep.name, from, dep.ephemeral);
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

fn run_plugin(
    plugin: &dyn Plugin,
    setting: Option<&PluginConfig>,
    ctx: &PluginContext<'_>,
    manifest: Option<&ConfigValue>,
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
            ..PluginOutput::default()
        },
        queue: VecDeque::new(),
    };
    for production in spec.production {
        push_unique(&mut collector.out.entries, (*production).to_string());
    }
    let mut patterns = globs(&options.config, spec.config);
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
        && let Some(value) = manifest.get(key)
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
        collector.absorb(inputs, &path, 0);
        drain(&mut collector, plugin, &mut visited);
    } else if !found_config {
        let inputs = plugin.resolve_without_config(ctx);
        collector.absorb(inputs, &ctx.dir.join("package.json"), 0);
        drain(&mut collector, plugin, &mut visited);
    }
    let inputs = plugin.resolve_workspace(ctx);
    collector.absorb(inputs, &ctx.dir.join("package.json"), 0);
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
    for path in config_paths(ctx.scripts, spec.binaries, &config_flags(spec.id)) {
        if looks_like_file(&path) {
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
        let path = ctx.dir.join(&relative);
        let Ok(source) = ctx.fs.read_to_string(&path) else {
            continue;
        };
        collector.out.config_files.push(path.clone());
        let Some(value) = load_config(&path, &source) else {
            continue;
        };
        let ast = ConfigAst {
            path: &path,
            base: parent_of(&relative),
            value: &value,
            source: &source,
        };
        let inputs = reader.resolve_config(&ast, ctx);
        collector.absorb(inputs, &path, depth);
    }
}
