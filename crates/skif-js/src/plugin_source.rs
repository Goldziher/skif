//! The input source that feeds the plugin table of `skif-js-plugins` to the module graph.

use std::path::Path;

use indexmap::IndexMap;
use skif_config::{Override, PluginConfig, PluginOptions};
use skif_js_plugins::{PluginContext, PluginOutput, resolve_plugins};

use crate::input::{BuildDir, InputRequest, InputSource, PathAlias, WorkspaceInputs};

/// Feeds the entries, project files and references of every enabled plugin to the module graph.
#[derive(Debug, Clone)]
pub struct PluginSource {
    plugins: IndexMap<String, PluginConfig>,
    /// The `[[overrides]]` that set `js.plugins`, applied to the packages they select.
    scoped: Vec<Override>,
}

impl PluginSource {
    /// A source that applies the `[js.plugins]` switches and overrides in `plugins` on top of detection.
    #[must_use]
    pub fn new(plugins: IndexMap<String, PluginConfig>) -> Self {
        Self {
            plugins,
            scoped: Vec::new(),
        }
    }

    /// Also applies the `js.plugins` of the `[[overrides]]` blocks in `overrides` to the packages they select, in
    /// order, after the table given to [`new`](Self::new).
    #[must_use]
    pub fn with_overrides(mut self, overrides: &[Override]) -> Self {
        self.scoped = overrides
            .iter()
            .filter(|scoped| !scoped.js.plugins.is_empty())
            .cloned()
            .collect();
        self
    }

    /// The plugin table for one package: the global table with the matching overrides folded in.
    fn plugins_for(&self, name: &str, dir: &Path) -> IndexMap<String, PluginConfig> {
        let mut plugins = self.plugins.clone();
        for scoped in self
            .scoped
            .iter()
            .filter(|scoped| scoped.applies_to(Some((name, dir)), None))
        {
            for (plugin, incoming) in &scoped.js.plugins {
                let merged = merge(plugins.get(plugin), incoming);
                plugins.insert(plugin.clone(), merged);
            }
        }
        plugins
    }
}

/// `incoming` over `inherited`: a switch keeps the options, a table replaces them and keeps an inherited switch it
/// does not set itself.
fn merge(inherited: Option<&PluginConfig>, incoming: &PluginConfig) -> PluginConfig {
    let inherited_enabled = match inherited {
        Some(PluginConfig::Enabled(enabled)) => Some(*enabled),
        Some(PluginConfig::Options(options)) => options.enabled,
        None => None,
    };
    match (incoming, inherited) {
        (PluginConfig::Enabled(enabled), Some(PluginConfig::Options(options))) => {
            PluginConfig::Options(PluginOptions {
                enabled: Some(*enabled),
                ..options.clone()
            })
        }
        (PluginConfig::Options(options), _) if options.enabled.is_none() => PluginConfig::Options(PluginOptions {
            enabled: inherited_enabled,
            ..options.clone()
        }),
        _ => incoming.clone(),
    }
}

fn scripts(request: &InputRequest<'_>) -> IndexMap<String, String> {
    request
        .package
        .manifest
        .value()
        .get("scripts")
        .and_then(serde_json::Value::as_object)
        .map(|scripts| {
            scripts
                .iter()
                .filter_map(|(name, command)| Some((name.clone(), command.as_str()?.to_owned())))
                .collect()
        })
        .unwrap_or_default()
}

fn names(output: &[skif_js_plugins::DepRef], optional: bool) -> impl Iterator<Item = String> + '_ {
    output
        .iter()
        .filter(move |dependency| dependency.optional == optional)
        .map(|dependency| dependency.name.clone())
}

fn inputs(output: PluginOutput) -> WorkspaceInputs {
    let entry = output
        .entries
        .iter()
        .filter(|glob| !output.production_entries.contains(glob))
        .cloned()
        .collect();
    let mut ignore_binaries = output.provided_binaries;
    ignore_binaries.sort();
    ignore_binaries.dedup();
    WorkspaceInputs {
        entry,
        production_entry: output.production_entries,
        project: output.project,
        ignore: output.ignore,
        ignore_dependencies: names(&output.referenced_dependencies, true).collect(),
        ignore_binaries,
        dependencies: names(&output.referenced_dependencies, false).collect(),
        binaries: output.referenced_binaries,
        config_files: output.config_files,
        aliases: output
            .aliases
            .into_iter()
            .map(|alias| PathAlias {
                find: alias.find,
                target: alias.target,
            })
            .collect(),
        build_dirs: output
            .build_dirs
            .into_iter()
            .map(|dir| BuildDir {
                out_dir: dir.out_dir,
                root_dir: dir.root_dir,
            })
            .collect(),
        conditions: output.conditions,
        module_dirs: output.module_dirs,
        extensions: output.extensions,
        entries_incomplete: output.entries_incomplete,
    }
}

/// Names of the dependencies of every ancestor workspace and of the manifests above the workspace root, so a tool
/// installed at the repository root is known in the packages below it and in a nested root.
fn inherited_dependencies(request: &InputRequest<'_>) -> Vec<String> {
    let dir = &request.package.dir;
    let mut names: Vec<String> = request
        .workspace
        .iter()
        .filter(|(_, other)| other.dir != *dir && (other.is_root || dir.starts_with(&other.dir)))
        .flat_map(|(_, other)| other.dependencies.iter().map(|entry| entry.name.clone()))
        .chain(request.ancestor_dependencies.iter().cloned())
        .collect();
    names.sort();
    names.dedup();
    names
}

impl InputSource for PluginSource {
    fn inputs(&self, request: &InputRequest<'_>) -> WorkspaceInputs {
        let package = request.package;
        let dependencies: Vec<String> = package.dependencies.iter().map(|entry| entry.name.clone()).collect();
        let inherited = inherited_dependencies(request);
        let scripts = scripts(request);
        let dir = request.workspace.root.join(Path::new(&package.dir));
        let plugins = self.plugins_for(
            package.display_name(),
            if package.is_root { &request.workspace.root } else { &dir },
        );
        let context = PluginContext {
            dir: &dir,
            is_root: package.is_root,
            package_name: package.name.as_deref(),
            dependencies: &dependencies,
            inherited_dependencies: &inherited,
            scripts: &scripts,
            fs: request.fs,
            plugins: &plugins,
        };
        resolve_plugins(&context)
            .into_iter()
            .fold(WorkspaceInputs::default(), |mut all, (_, output)| {
                all.extend(inputs(output));
                all
            })
    }
}
