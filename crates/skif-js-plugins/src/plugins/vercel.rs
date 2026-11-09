//! `Vercel`: the programmatic project config is an entry file, and the commands `vercel.json` overrides
//! (`buildCommand`, `devCommand`, `installCommand`, `ignoreCommand`) are scripts.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};

/// The `vercel.json` keys that replace a command of the build.
const COMMANDS: [&str; 4] = ["buildCommand", "devCommand", "installCommand", "ignoreCommand"];

/// The `vercel` plugin.
pub struct Vercel;

static SPEC: PluginSpec = PluginSpec {
    id: "vercel",
    enablers: &[Enabler::Dependency("@vercel/config"), Enabler::File("vercel.json")],
    config: &["vercel.json"],
    entry: &["vercel.{js,mjs,cjs,ts,mts}"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Vercel {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        COMMANDS
            .iter()
            .filter_map(|key| config.value.get(key).and_then(ConfigValue::as_str))
            .map(|command| PluginInput::Script(command.to_owned()))
            .collect()
    }
}
