//! Taskr: the taskfile is an entry, the local plugins of `package.json#taskr.requires` are entries too, and the
//! `@taskr/*` and `taskr-*` dependencies are plugins taskr loads by name.
//!
//! knip 6.44.1 has no `taskr` plugin; this one follows taskr's own loading rules (issue #20).

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{module_input, strings};

/// The `taskr` plugin.
pub struct Taskr;

static SPEC: PluginSpec = PluginSpec {
    id: "taskr",
    enablers: &[Enabler::Dependency("taskr")],
    entry: &["{T,t}askfile.js"],
    package_json_key: Some("taskr"),
    binaries: &["taskr"],
    ..PluginSpec::DEFAULT
};

/// Whether taskr loads the dependency `name` as a plugin by its name.
fn is_plugin_package(name: &str) -> bool {
    name.starts_with("@taskr/") || name.starts_with("taskr-")
}

impl Plugin for Taskr {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        config
            .value
            .get("requires")
            .map_or_default(strings)
            .into_iter()
            .filter_map(|specifier| module_input(config.base, specifier, false))
            .collect()
    }

    fn resolve_workspace(&self, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        cx.dependencies
            .iter()
            .filter(|name| is_plugin_package(name))
            .map(|name| PluginInput::Dependency(name.clone()))
            .collect()
    }
}
