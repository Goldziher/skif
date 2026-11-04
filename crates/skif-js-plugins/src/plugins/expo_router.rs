//! Expo Router: every file below the `app` directory is a route, which the router loads without an import.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use skif_js_syntax::ConfigValue;

/// The `expo-router` plugin.
pub struct ExpoRouter;

static SPEC: PluginSpec = PluginSpec {
    id: "expo-router",
    enablers: &[Enabler::Dependency("expo-router")],
    config: &["app.json", "app.config.{json,ts,js,mts,cts,mjs,cjs}"],
    production: &["app/**/*.{js,jsx,ts,tsx}", "src/app/**/*.{js,jsx,ts,tsx}"],
    ..PluginSpec::DEFAULT
};

impl Plugin for ExpoRouter {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    /// The `root` option of the `expo-router` config plugin names the routes directory.
    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let app = config.value.get("expo").unwrap_or(config.value);
        let Some(ConfigValue::Array(plugins)) = app.get("plugins") else {
            return Vec::new();
        };
        plugins
            .iter()
            .filter_map(ConfigValue::as_array)
            .filter(|parts| parts.first().and_then(ConfigValue::as_str) == Some("expo-router"))
            .filter_map(|parts| parts.get(1)?.get("root")?.as_str())
            .map(|root| {
                let glob = format!("{}/**/*.{{js,jsx,ts,tsx}}", root.trim_end_matches('/'));
                PluginInput::ProductionEntry(config.join(&glob))
            })
            .collect()
    }
}
