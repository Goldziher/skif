//! The input source that feeds the plugin table of `skif-js-plugins` to the module graph.

use std::path::Path;

use indexmap::IndexMap;
use skif_config::{JsWorkspace, Override, PluginConfig, PluginOptions};
use skif_js_plugins::{PluginContext, PluginOutput, resolve_plugins};

use crate::config_check::relative_tsconfig;
use crate::config_modules::ConfigModules;
use crate::input::{
    BuildDir, ConfigDependency, CustomElementLibrary, IncompleteCause, IncompleteEntries, IncompleteScope,
    InputRequest, InputSource, PathAlias, RunBinary, WorkspaceInputs,
};
use crate::setup::WorkspaceKeys;
use crate::specifier::types_package;

/// Feeds the entries, project files and references of every enabled plugin to the module graph.
#[derive(Debug, Clone)]
pub struct PluginSource {
    plugins: IndexMap<String, PluginConfig>,
    /// The `[js.workspaces]` tables that set `plugins`, applied to the packages their keys select.
    workspaces: IndexMap<String, JsWorkspace>,
    workspace_keys: WorkspaceKeys,
    /// The `[[overrides]]` that set `js.plugins`, applied to the packages they select.
    scoped: Vec<Override>,
    /// `[js] tsconfig`, which the `typescript` plugin of the root package also reads.
    root_tsconfig: Option<String>,
}

impl PluginSource {
    /// A source that applies the `[js.plugins]` switches and overrides in `plugins` on top of detection.
    #[must_use]
    pub fn new(plugins: IndexMap<String, PluginConfig>) -> Self {
        Self {
            plugins,
            workspaces: IndexMap::new(),
            workspace_keys: WorkspaceKeys::default(),
            scoped: Vec::new(),
            root_tsconfig: None,
        }
    }

    /// Also has the `typescript` plugin of the root package read `path` (`[js] tsconfig`, relative to the workspace
    /// root), besides the config files it looks for by default.
    #[must_use]
    pub fn with_root_tsconfig(mut self, path: Option<&str>) -> Self {
        self.root_tsconfig = path.and_then(relative_tsconfig);
        self
    }

    /// Also applies the `plugins` of the `[js.workspaces]` tables in `workspaces` to the packages their keys select
    /// (`"."` is the root), after the table given to [`new`](Self::new) and before the overrides.
    #[must_use]
    pub fn with_workspaces(mut self, workspaces: &IndexMap<String, JsWorkspace>) -> Self {
        self.workspaces = workspaces
            .iter()
            .filter(|(_, workspace)| !workspace.plugins.is_empty())
            .map(|(key, workspace)| (key.clone(), workspace.clone()))
            .collect();
        self.workspace_keys = WorkspaceKeys::new(&self.workspaces);
        self
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
    fn plugins_for(&self, name: &str, dir: &Path, key: &str) -> IndexMap<String, PluginConfig> {
        let mut plugins = self.plugins.clone();
        if let Some(workspace) = self.workspace_keys.table(&self.workspaces, key) {
            for (plugin, incoming) in &workspace.plugins {
                let merged = merge(plugins.get(plugin), incoming);
                plugins.insert(plugin.clone(), merged);
            }
        }
        if key == "."
            && let Some(path) = &self.root_tsconfig
        {
            let current = plugins.get(TYPESCRIPT);
            if let Some(read) = with_config_file(current, path) {
                plugins.insert(TYPESCRIPT.to_owned(), read);
            }
        }
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

const TYPESCRIPT: &str = "typescript";

/// The `typescript` plugin setting `current` with `path` added to the config files it reads: to its own `config`
/// globs, or to the plugin's defaults. `None` when the plugin is switched off or already reads `path`.
fn with_config_file(current: Option<&PluginConfig>, path: &str) -> Option<PluginConfig> {
    let mut options = match current {
        Some(PluginConfig::Enabled(false)) => return None,
        Some(PluginConfig::Options(options)) if options.enabled == Some(false) => return None,
        Some(PluginConfig::Options(options)) => options.clone(),
        Some(PluginConfig::Enabled(true)) => PluginOptions::new().with_enabled(true),
        _ => PluginOptions::new(),
    };
    if options.config.is_empty() {
        options.config = skif_js_plugins::registry::plugin(TYPESCRIPT)
            .map(|plugin| plugin.spec().config.iter().map(|glob| (*glob).to_owned()).collect())
            .unwrap_or_default();
    }
    if options.config.iter().any(|glob| glob == path) {
        return None;
    }
    options.config.push(path.to_owned());
    Some(PluginConfig::Options(options))
}

/// `incoming` over `inherited`: a switch keeps the options, a table replaces them and keeps an inherited switch it
/// does not set itself.
fn merge(inherited: Option<&PluginConfig>, incoming: &PluginConfig) -> PluginConfig {
    let inherited_enabled = match inherited {
        Some(PluginConfig::Enabled(enabled)) => Some(*enabled),
        Some(PluginConfig::Options(options)) => options.enabled,
        _ => None,
    };
    match (incoming, inherited) {
        (PluginConfig::Enabled(enabled), Some(PluginConfig::Options(options))) => {
            PluginConfig::Options(options.clone().with_enabled(*enabled))
        }
        (PluginConfig::Options(options), _) if options.enabled.is_none() => {
            let mut merged = options.clone();
            merged.enabled = inherited_enabled;
            PluginConfig::Options(merged)
        }
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

/// The packages of the `optional` (or required) references; a reference to types only also names the package's
/// `DefinitelyTyped` package, which a manifest may list in its place.
fn names(output: &[skif_js_plugins::DepRef], optional: bool) -> impl Iterator<Item = String> + '_ {
    output
        .iter()
        .filter(move |dependency| dependency.optional == optional)
        .flat_map(|dependency| {
            let types = types_package(&dependency.name).filter(|_| dependency.type_only);
            std::iter::once(dependency.name.clone()).chain(types)
        })
}

/// The scope of every reason together; the whole package when there is none.
fn incomplete_scope(reasons: &[skif_js_plugins::IncompleteEntries]) -> IncompleteScope {
    reasons
        .iter()
        .map(|reason| match &reason.scope {
            skif_js_plugins::IncompleteScope::Within(globs) => IncompleteScope::Within(globs.clone()),
            _ => IncompleteScope::Package,
        })
        .reduce(IncompleteScope::union)
        .unwrap_or_default()
}

fn inputs(output: PluginOutput) -> WorkspaceInputs {
    let incomplete_scope = incomplete_scope(&output.incomplete_reasons);
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
        entry_regex: output.entry_regexes,
        project: output.project,
        ignore: output.ignore,
        ignore_dependencies: names(&output.referenced_dependencies, true).collect(),
        ignore_binaries,
        virtual_modules: output.virtual_modules,
        dependencies: names(&output.referenced_dependencies, false).collect(),
        config_dependencies: output
            .referenced_dependencies
            .iter()
            .filter(|dependency| dependency.named_by_config)
            .map(|dependency| ConfigDependency {
                name: dependency.name.clone(),
                file: dependency.from.clone(),
                type_only: dependency.type_only,
                reader_may_list: dependency.reader_may_list,
            })
            .collect(),
        binaries: output.referenced_binaries,
        run_binaries: output
            .run_binaries
            .into_iter()
            .map(|run| RunBinary::new(run.name, run.from, run.dir, run.filtered))
            .collect(),
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
        custom_elements: output
            .custom_elements
            .into_iter()
            .map(|library| CustomElementLibrary {
                modules: library.modules.iter().map(|module| (*module).to_owned()).collect(),
                decorator: library.decorator.to_owned(),
                base_class: library.base_class.map(str::to_owned),
            })
            .collect(),
        entries_incomplete: output.entries_incomplete,
        incomplete_reasons: output
            .incomplete_reasons
            .into_iter()
            .map(|reason| IncompleteEntries {
                cause: match reason.cause {
                    skif_js_plugins::IncompleteCause::Unparsable => IncompleteCause::UnparsableConfig,
                    skif_js_plugins::IncompleteCause::UnknownValue => IncompleteCause::UnknownConfig,
                    skif_js_plugins::IncompleteCause::UnknownSpread => IncompleteCause::UnknownSpread,
                    skif_js_plugins::IncompleteCause::ComputedEntries => IncompleteCause::ComputedEntries,
                    _ => IncompleteCause::Unspecified,
                },
                plugin: Some(reason.plugin.to_owned()),
                file: Some(reason.file),
            })
            .collect(),
        incomplete_scope,
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
        let key = if package.is_root {
            ".".to_owned()
        } else {
            package.dir.to_string_lossy().replace('\\', "/")
        };
        let plugins = self.plugins_for(
            package.display_name(),
            if package.is_root { &request.workspace.root } else { &dir },
            &key,
        );
        let modules = ConfigModules::new(request.fs, request.workspace);
        let context = PluginContext {
            dir: &dir,
            is_root: package.is_root,
            package_name: package.name.as_deref(),
            dependencies: &dependencies,
            inherited_dependencies: &inherited,
            scripts: &scripts,
            fs: request.fs,
            plugins: &plugins,
            modules: Some(&modules),
        };
        resolve_plugins(&context)
            .into_iter()
            .fold(WorkspaceInputs::default(), |mut all, (_, output)| {
                all.extend(inputs(output));
                all
            })
    }
}

#[cfg(test)]
mod tests {
    use skif_config::{PluginConfig, PluginOptions};

    use super::with_config_file;

    #[test]
    fn the_root_tsconfig_is_added_to_the_typescript_config_files() {
        let Some(PluginConfig::Options(options)) = with_config_file(None, "config/tsconfig.app.json") else {
            panic!("expected options");
        };
        assert_eq!(options.enabled, None);
        assert!(options.config.iter().any(|glob| glob == "tsconfig.json"));
        assert_eq!(
            options.config.last().map(String::as_str),
            Some("config/tsconfig.app.json")
        );

        let custom = PluginConfig::Options(PluginOptions::new().with_config(["base.json"]));
        let Some(PluginConfig::Options(options)) = with_config_file(Some(&custom), "app.json") else {
            panic!("expected options");
        };
        assert_eq!(options.config, ["base.json", "app.json"]);

        assert_eq!(with_config_file(Some(&PluginConfig::Enabled(false)), "app.json"), None);
        let reads = PluginConfig::Options(PluginOptions::new().with_config(["app.json"]));
        assert_eq!(with_config_file(Some(&reads), "app.json"), None);
    }
}
