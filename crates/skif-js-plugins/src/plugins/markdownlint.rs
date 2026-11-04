//! markdownlint: shared configs, custom rules, markdown-it plugins and formatters.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{config_ref, module_input, strings};

/// The `markdownlint` plugin.
pub struct Markdownlint;

static SPEC: PluginSpec = PluginSpec {
    id: "markdownlint",
    enablers: &[
        Enabler::Dependency("markdownlint-cli"),
        Enabler::Dependency("markdownlint-cli2"),
    ],
    config: &[
        ".markdownlint-cli2.{jsonc,yaml,cjs,mjs}",
        ".markdownlint.{json,jsonc,yaml,yml,cjs,mjs}",
    ],
    binaries: &["markdownlint", "markdownlint-cli2"],
    ..PluginSpec::DEFAULT
};

/// Specifiers the tool ships itself.
fn bundled(name: &str) -> bool {
    name.starts_with("markdownlint/style/") || name == "markdownlint-cli2-formatter-default"
}

impl Plugin for Markdownlint {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        let nested = config.value.get("config");
        for extends in [config.value.get("extends"), nested.and_then(|c| c.get("extends"))]
            .into_iter()
            .flatten()
        {
            out.extend(
                strings(extends)
                    .into_iter()
                    .filter(|name| !bundled(name))
                    .filter_map(|name| config_ref(config.base, name)),
            );
        }
        if let Some(ConfigValue::Array(rules)) = config.value.get("customRules") {
            for rule in rules {
                out.extend(
                    strings(rule)
                        .into_iter()
                        .filter(|name| !bundled(name))
                        .filter_map(|name| module_input(config.base, name, false)),
                );
            }
        }
        for key in ["markdownItPlugins", "outputFormatters"] {
            if let Some(ConfigValue::Array(modules)) = config.value.get(key) {
                out.extend(
                    modules
                        .iter()
                        .filter_map(|module| module.as_array()?.first()?.as_str())
                        .filter(|name| !bundled(name))
                        .filter_map(|name| module_input(config.base, name, false)),
                );
            }
        }
        out
    }
}
