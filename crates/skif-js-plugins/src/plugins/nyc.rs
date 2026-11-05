//! nyc: the configs it extends and the modules it requires.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{module_input, strings};

/// The `nyc` plugin.
pub struct Nyc;

static SPEC: PluginSpec = PluginSpec {
    id: "nyc",
    enablers: &[Enabler::Dependency("nyc")],
    config: &[".nycrc", ".nycrc.{json,yml,yaml}", "nyc.config.{js,cjs,mjs}"],
    package_json_key: Some("nyc"),
    binaries: &["nyc"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Nyc {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        ["extends", "require"]
            .iter()
            .filter_map(|key| config.value.get(key))
            .flat_map(strings)
            .filter_map(|name| module_input(config.base, name, true))
            .collect()
    }
}
