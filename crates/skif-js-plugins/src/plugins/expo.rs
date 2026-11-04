//! Expo: the config plugins of `app.json` and `app.config.*` are packages or local modules.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{module_input, name_of};
use skif_js_syntax::ConfigValue;

/// The `expo` plugin.
pub struct Expo;

static SPEC: PluginSpec = PluginSpec {
    id: "expo",
    enablers: &[Enabler::Dependency("expo")],
    config: &["app.json", "app.config.{json,ts,js,mts,cts,mjs,cjs}"],
    binaries: &["expo", "expo-cli", "eas"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Expo {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let app = config.value.get("expo").unwrap_or(config.value);
        let Some(ConfigValue::Array(plugins)) = app.get("plugins") else {
            return Vec::new();
        };
        plugins
            .iter()
            .filter_map(name_of)
            .filter_map(|name| module_input(config.base, name, false))
            .collect()
    }
}
