//! WXT: every file under `entrypoints/` is a production entry; the modules of `wxt.config.*` are dependencies.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{package_name, strings};

/// The `wxt` plugin.
pub struct Wxt;

static SPEC: PluginSpec = PluginSpec {
    id: "wxt",
    enablers: &[Enabler::Dependency("wxt")],
    config: &["wxt.config.{js,cjs,mjs,ts,cts,mts}"],
    production: &["entrypoints/**/*"],
    binaries: &["wxt"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Wxt {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        config
            .value
            .get("modules")
            .map_or_default(strings)
            .into_iter()
            .filter_map(package_name)
            .map(PluginInput::Dependency)
            .collect()
    }
}
