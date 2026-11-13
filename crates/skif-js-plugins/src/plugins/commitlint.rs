//! commitlint: shared configs, plugins, formatters and parser presets named in the config.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{conventional, is_relative, module_input, package_name, strings};

/// The `commitlint` plugin.
pub struct Commitlint;

static SPEC: PluginSpec = PluginSpec {
    id: "commitlint",
    enablers: &[Enabler::Dependency("@commitlint/cli")],
    config: &[
        ".commitlintrc",
        ".commitlintrc.{json,yaml,yml,js,cjs,mjs,ts,cts,mts}",
        "commitlint.config.{js,cjs,mjs,ts,cts,mts}",
    ],
    package_json_key: Some("commitlint"),
    binaries: &["commitlint"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Commitlint {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let value = config.value;
        let mut out = Vec::new();
        for specifier in value.get("extends").map_or_default(strings) {
            if is_relative(specifier) {
                out.push(PluginInput::ConfigFile(config.join(specifier)));
            } else if specifier.starts_with('@') && specifier.contains('/') {
                out.extend(package_name(specifier).map(PluginInput::Dependency));
            } else {
                out.push(PluginInput::Dependency(conventional(specifier, "commitlint-config")));
            }
        }
        for plugin in value.get("plugins").map_or_default(strings) {
            out.push(PluginInput::Dependency(conventional(plugin, "commitlint-plugin")));
        }
        if let Some(formatter) = value.get("formatter").and_then(ConfigValue::as_str) {
            out.extend(module_input(config.base, formatter, false));
        }
        if let Some(preset) = value.get("parserPreset") {
            let path = preset
                .as_str()
                .or_else(|| preset.get("path").and_then(ConfigValue::as_str));
            out.extend(path.and_then(|p| module_input(config.base, p, false)));
        }
        out
    }
}
