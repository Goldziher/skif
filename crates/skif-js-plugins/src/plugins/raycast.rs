//! Raycast: the source file of every command and AI tool the extension's `package.json` declares is a production
//! entry.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::objects;

/// The `raycast` plugin.
pub struct Raycast;

static SPEC: PluginSpec = PluginSpec {
    id: "raycast",
    enablers: &[Enabler::Dependency("@raycast/api")],
    config: &["package.json"],
    binaries: &["ray"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Raycast {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        for (key, dir) in [("commands", "src/"), ("tools", "src/tools/")] {
            for item in config.value.get(key).map(objects).unwrap_or_default() {
                if let Some(name) = item.get("name").and_then(ConfigValue::as_str)
                    && !name.is_empty()
                    && !name.contains(['/', '*', '{'])
                {
                    out.push(PluginInput::ProductionEntry(
                        config.join(&format!("{dir}{name}.{{js,jsx,ts,tsx}}")),
                    ));
                }
            }
        }
        out
    }
}
