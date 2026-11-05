//! `SVGR`: the plugins the config names are packages.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{package_name, strings};

/// The `svgr` plugin.
pub struct Svgr;

static SPEC: PluginSpec = PluginSpec {
    id: "svgr",
    enablers: &[Enabler::Dependency("@svgr/cli"), Enabler::Dependency("@svgr/core")],
    config: &[
        ".svgrrc",
        ".svgrrc.{json,yaml,yml,js,ts,cjs,mjs}",
        "svgr.config.{json,yaml,yml,js,ts,cjs,mjs}",
    ],
    package_json_key: Some("svgr"),
    binaries: &["svgr"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Svgr {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        config
            .value
            .get("plugins")
            .map(strings)
            .unwrap_or_default()
            .into_iter()
            .filter_map(package_name)
            .map(PluginInput::Dependency)
            .collect()
    }
}
