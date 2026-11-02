//! The JS analysis provider, wired to the plugin table of `skif-js-plugins`.

use std::path::Path;
use std::sync::Arc;

use indexmap::IndexMap;
use skif_config::{Config, PluginConfig};
use skif_core::FileSystem;
use skif_js::{InputRequest, InputSource, JsProvider, WorkspaceInputs};
use skif_js_plugins::{PluginContext, PluginOutput, resolve_plugins};

/// Builds the JS provider that reads through `fs` and takes its tool knowledge from the plugin table.
pub(crate) fn provider(config: &Config, fs: Arc<dyn FileSystem>) -> JsProvider {
    JsProvider::new(fs).with_input_source(Arc::new(PluginSource {
        plugins: config.js.plugins.clone(),
    }))
}

/// Feeds the entries, project files and references of every enabled plugin to the module graph.
struct PluginSource {
    plugins: IndexMap<String, PluginConfig>,
}

fn scripts(request: &InputRequest<'_>) -> IndexMap<String, String> {
    request
        .package
        .manifest
        .value
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
    }
}

impl InputSource for PluginSource {
    fn inputs(&self, request: &InputRequest<'_>) -> WorkspaceInputs {
        let package = request.package;
        let dependencies: Vec<String> = package.dependencies.iter().map(|entry| entry.name.clone()).collect();
        let scripts = scripts(request);
        let dir = request.workspace.root.join(Path::new(&package.dir));
        let context = PluginContext {
            dir: &dir,
            is_root: package.is_root,
            package_name: package.name.as_deref(),
            dependencies: &dependencies,
            scripts: &scripts,
            fs: request.fs,
            plugins: &self.plugins,
        };
        resolve_plugins(&context)
            .into_iter()
            .fold(WorkspaceInputs::default(), |mut all, (_, output)| {
                all.extend(inputs(output));
                all
            })
    }
}
