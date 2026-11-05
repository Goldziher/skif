//! oclif: command modules are production entries, the plugins of the `oclif` key are packages.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{package_name, strings};

/// The `oclif` plugin.
pub struct Oclif;

static SPEC: PluginSpec = PluginSpec {
    id: "oclif",
    enablers: &[Enabler::Dependency("oclif"), Enabler::Dependency("@oclif/core")],
    package_json_key: Some("oclif"),
    production: &["{,src/}commands/**/*.{js,mjs,cjs,jsx,ts,tsx,mts,cts}"],
    binaries: &["oclif"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Oclif {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        ["plugins", "devPlugins"]
            .iter()
            .filter_map(|key| config.value.get(key))
            .flat_map(strings)
            .filter_map(package_name)
            .map(PluginInput::Dependency)
            .collect()
    }
}
