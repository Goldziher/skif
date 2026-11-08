//! Wireit: the `command` of every script of the `wireit` key of `package.json` is a script.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};

/// The `wireit` plugin.
pub struct Wireit;

static SPEC: PluginSpec = PluginSpec {
    id: "wireit",
    enablers: &[Enabler::Dependency("wireit")],
    package_json_key: Some("wireit"),
    ..PluginSpec::DEFAULT
};

impl Plugin for Wireit {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let ConfigValue::Object(scripts) = config.value else {
            return Vec::new();
        };
        scripts
            .iter()
            .filter_map(|(_, script)| script.get("command").and_then(ConfigValue::as_str))
            .map(|command| PluginInput::Script(command.to_owned()))
            .collect()
    }
}
