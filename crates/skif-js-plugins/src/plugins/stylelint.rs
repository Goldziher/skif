//! Stylelint: shared configs, plugins and a custom syntax are packages; `overrides` are read recursively.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{config_ref, module_input, objects, strings};

/// The `stylelint` plugin.
pub struct Stylelint;

static SPEC: PluginSpec = PluginSpec {
    id: "stylelint",
    enablers: &[Enabler::Dependency("stylelint")],
    config: &[
        ".stylelintrc",
        ".stylelintrc.{cjs,mjs,js,json,yaml,yml}",
        "stylelint.config.{cjs,mjs,js}",
    ],
    package_json_key: Some("stylelint"),
    binaries: &["stylelint"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Stylelint {
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
        match object.get("extends") {
            Some(ConfigValue::Array(items)) => {
                for item in items {
                    match item {
                        ConfigValue::String(name) => out.extend(config_ref(base, name)),
                        nested => out.extend(inputs(nested, base, depth + 1)),
                    }
                }
            }
            Some(ConfigValue::String(name)) => out.extend(config_ref(base, name)),
            Some(nested) => out.extend(inputs(nested, base, depth + 1)),
            None => {}
        }
        if let Some(plugins) = object.get("plugins") {
            out.extend(
                strings(plugins)
                    .into_iter()
                    .filter_map(|name| module_input(base, name, false)),
            );
        }
        if let Some(syntax) = object.get("customSyntax").and_then(ConfigValue::as_str) {
            out.extend(module_input(base, syntax, false));
        }
        if let Some(overrides) = object.get("overrides") {
            out.extend(inputs(overrides, base, depth + 1));
        }
    }
    out
}
