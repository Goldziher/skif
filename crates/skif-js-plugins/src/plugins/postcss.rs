//! `PostCSS`: plugins named in the config, as object keys or array items.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{module_input, name_of};

/// The `postcss` plugin.
pub struct Postcss;

static SPEC: PluginSpec = PluginSpec {
    id: "postcss",
    enablers: &[
        Enabler::Dependency("postcss"),
        Enabler::Dependency("postcss-cli"),
        Enabler::Dependency("next"),
        Enabler::Dependency("@tailwindcss/postcss"),
    ],
    config: &[
        "postcss.config.{json,js,cjs,mjs,ts,cts,mts}",
        ".postcssrc",
        ".postcssrc.{json,js,cjs,mjs,ts,cts,mts,yaml,yml}",
    ],
    package_json_key: Some("postcss"),
    binaries: &["postcss"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Postcss {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let names: Vec<&str> = match config.value.get("plugins") {
            Some(ConfigValue::Array(items)) => items.iter().filter_map(name_of).collect(),
            Some(ConfigValue::Object(entries)) => entries
                .iter()
                .filter(|(_, options)| options.as_bool() != Some(false))
                .map(|(name, _)| name.as_str())
                .collect(),
            _ => Vec::new(),
        };
        let mut out: Vec<PluginInput> = names
            .iter()
            .filter_map(|name| module_input(config.base, name, false))
            .collect();
        if names.contains(&"tailwindcss") {
            out.push(PluginInput::Dependency("postcss".to_string()));
        } else if names.contains(&"@tailwindcss/postcss") {
            out.push(PluginInput::OptionalDependency("postcss".to_string()));
        }
        out
    }
}
