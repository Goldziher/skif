//! Nano Staged: every command in the config is a script.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::script_inputs;

/// The `nano-staged` plugin.
pub struct NanoStaged;

static SPEC: PluginSpec = PluginSpec {
    id: "nano-staged",
    enablers: &[Enabler::Dependency("nano-staged")],
    config: &[
        ".nano-staged.{js,cjs,mjs,json}",
        "nano-staged.{js,cjs,mjs,json}",
        ".nanostagedrc",
    ],
    package_json_key: Some("nano-staged"),
    binaries: &["nano-staged"],
    ..PluginSpec::DEFAULT
};

impl Plugin for NanoStaged {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let ConfigValue::Object(globs) = config.value else {
            return Vec::new();
        };
        globs.iter().flat_map(|(_, commands)| script_inputs(commands)).collect()
    }
}
