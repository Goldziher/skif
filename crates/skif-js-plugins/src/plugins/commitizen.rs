//! Commitizen: the adapter named by `path` in `.czrc`, `.cz.json` or the `config.commitizen` key of `package.json`.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{module_input, package_name};

/// The `commitizen` plugin.
pub struct Commitizen;

static SPEC: PluginSpec = PluginSpec {
    id: "commitizen",
    enablers: &[Enabler::Dependency("commitizen")],
    root_only: true,
    config: &[".czrc", ".cz.json"],
    package_json_key: Some("config.commitizen"),
    ..PluginSpec::DEFAULT
};

impl Plugin for Commitizen {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    /// `path` names the adapter package, often through `./node_modules/<name>`.
    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let Some(path) = config.value.get("path").and_then(ConfigValue::as_str) else {
            return Vec::new();
        };
        let installed = path.trim_start_matches("./").strip_prefix("node_modules/");
        match installed {
            Some(name) => package_name(name).map(PluginInput::Dependency),
            None => module_input(config.base, path, false),
        }
        .into_iter()
        .collect()
    }
}
