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
use crate::scripts::{ScriptContext, parse_script};

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
    queue: VecDeque<(String, u8)>,
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
                        self.queue.push_back((path, depth + 1));
                    }
                }
                PluginInput::Script(text) => self.script(&text, from),
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
            }
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
    let patterns = globs(&options.config, spec.config);
    collector
        .queue
        .extend(find_files(ctx.fs, ctx.dir, &patterns).into_iter().map(|p| (p, 0)));

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
    collector.out
}

/// Reads every queued config file, which may queue further ones.
fn drain(collector: &mut Collector<'_, '_>, plugin: &dyn Plugin, visited: &mut BTreeSet<String>) {
    let ctx = collector.ctx;
    while let Some((relative, depth)) = collector.queue.pop_front() {
        if !visited.insert(relative.clone()) {
            continue;
        }
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
        let inputs = plugin.resolve_config(&ast, ctx);
        collector.absorb(inputs, &path, depth);
    }
}
