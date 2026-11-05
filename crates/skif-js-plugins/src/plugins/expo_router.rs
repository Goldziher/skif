//! Expo Router: every file below the routes directory (`app`, `src/app` or the `root` option) is a route, which the
//! router loads without an import.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use skif_js_syntax::ConfigValue;

/// The `expo-router` plugin.
pub struct ExpoRouter;

/// Where the routes are when the `expo-router` config plugin names no `root`.
const DEFAULT_ROUTES: [&str; 2] = ["app/**/*.{js,jsx,ts,tsx}", "src/app/**/*.{js,jsx,ts,tsx}"];

static SPEC: PluginSpec = PluginSpec {
    id: "expo-router",
    enablers: &[Enabler::Dependency("expo-router")],
    config: &["app.json", "app.config.{json,ts,js,mts,cts,mjs,cjs}"],
    ..PluginSpec::DEFAULT
};

impl Plugin for ExpoRouter {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_without_config(&self, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        DEFAULT_ROUTES
            .iter()
            .map(|glob| PluginInput::ProductionEntry((*glob).to_owned()))
            .collect()
    }

    /// The `root` option of the `expo-router` config plugin names the routes directory and replaces the defaults.
    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let app = config.value.get("expo").unwrap_or(config.value);
        let root = match app.get("plugins") {
            Some(ConfigValue::Array(plugins)) => plugins
                .iter()
                .filter_map(ConfigValue::as_array)
                .filter(|parts| parts.first().and_then(ConfigValue::as_str) == Some("expo-router"))
                .find_map(|parts| parts.get(1)?.get("root")?.as_str()),
            _ => None,
        };
        match root {
            Some(root) => {
                let glob = format!("{}/**/*.{{js,jsx,ts,tsx}}", root.trim_end_matches('/'));
                vec![PluginInput::ProductionEntry(config.join(&glob))]
            }
            None => DEFAULT_ROUTES
                .iter()
                .map(|glob| PluginInput::ProductionEntry(config.join(glob)))
                .collect(),
        }
    }
}
