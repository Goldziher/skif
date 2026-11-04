//! Oxlint: shared configs in `extends`, JS plugins and the plugins of `overrides`.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{config_ref, module_input, objects};

/// The `oxlint` plugin.
pub struct Oxlint;

static SPEC: PluginSpec = PluginSpec {
    id: "oxlint",
    enablers: &[Enabler::Dependency("oxlint"), Enabler::Dependency("vite-plus")],
    config: &[".oxlintrc.{json,jsonc}", "oxlint.config.{ts,mts}"],
    binaries: &["oxlint"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Oxlint {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        inputs(config.value, config.base, 0)
    }
}

fn inputs(value: &ConfigValue, base: &str, depth: u8) -> Vec<PluginInput> {
    let mut out = Vec::new();
    if depth > 8 {
        return out;
    }
    for object in objects(value) {
        if let Some(ConfigValue::Array(extended)) = object.get("extends") {
            for entry in extended {
                match entry {
                    ConfigValue::String(path) => out.extend(config_ref(base, path)),
                    nested => out.extend(inputs(nested, base, depth + 1)),
                }
            }
        }
        out.extend(js_plugins(object, base));
        if let Some(ConfigValue::Array(overrides)) = object.get("overrides") {
            for item in overrides {
                out.extend(js_plugins(item, base));
            }
        }
    }
    out
}

/// `jsPlugins` entries are a specifier or an object with a `specifier`.
fn js_plugins(object: &ConfigValue, base: &str) -> Vec<PluginInput> {
    let Some(ConfigValue::Array(plugins)) = object.get("jsPlugins") else {
        return Vec::new();
    };
    plugins
        .iter()
        .filter_map(|plugin| match plugin {
            ConfigValue::String(specifier) => Some(specifier.as_str()),
            other => other.get("specifier").and_then(ConfigValue::as_str),
        })
        .filter_map(|specifier| module_input(base, specifier, true))
        .collect()
}
