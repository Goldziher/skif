//! Prettier: plugins named in the config and shared configs named as a string.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{module_input, strings};

/// The `prettier` plugin.
pub struct Prettier;

static SPEC: PluginSpec = PluginSpec {
    id: "prettier",
    enablers: &[Enabler::Dependency("prettier")],
    config: &[
        ".prettierrc",
        ".prettierrc.{json,js,cjs,mjs,ts,cts,mts,yml,yaml,toml,json5}",
        "prettier.config.{js,cjs,mjs,ts,cts,mts}",
    ],
    package_json_key: Some("prettier"),
    binaries: &["prettier"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Prettier {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        if let ConfigValue::String(shared) = config.value {
            return module_input(config.base, shared, false).into_iter().collect();
        }
        let mut names = plugin_names(config.value);
        if let Some(ConfigValue::Array(overrides)) = config.value.get("overrides") {
            for options in overrides.iter().filter_map(|o| o.get("options")) {
                names.extend(plugin_names(options));
            }
        }
        names
            .into_iter()
            .filter_map(|name| module_input(config.base, name, false))
            .collect()
    }
}

fn plugin_names(options: &ConfigValue) -> Vec<&str> {
    options.get("plugins").map(strings).unwrap_or_default()
}
