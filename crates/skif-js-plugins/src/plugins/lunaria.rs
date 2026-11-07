//! Lunaria: the tracked file locations, the renderer, and the dashboard's custom CSS and inline favicon named in
//! `lunaria.config.json` are production entries.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{entry_glob, get_in, objects, strings};

/// The `lunaria` plugin.
pub struct Lunaria;

static SPEC: PluginSpec = PluginSpec {
    id: "lunaria",
    enablers: &[
        Enabler::Dependency("@lunariajs/core"),
        Enabler::Dependency("@lunariajs/starlight"),
    ],
    config: &["lunaria.config.json"],
    binaries: &["lunaria"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Lunaria {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let value = config.value;
        let mut paths: Vec<&str> = value
            .get("files")
            .map(objects)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|file| file.get("location").and_then(ConfigValue::as_str))
            .collect();
        paths.extend(value.get("renderer").and_then(ConfigValue::as_str));
        paths.extend(
            get_in(value, &["dashboard", "customCss"])
                .map(strings)
                .unwrap_or_default(),
        );
        paths.extend(get_in(value, &["dashboard", "favicon", "inline"]).and_then(ConfigValue::as_str));
        paths
            .into_iter()
            .filter(|path| !path.contains(['<', ':']))
            .filter_map(|path| entry_glob(config.base, path))
            .map(PluginInput::ProductionEntry)
            .collect()
    }
}
