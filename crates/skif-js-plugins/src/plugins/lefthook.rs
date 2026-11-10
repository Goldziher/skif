//! Lefthook: every `run` command in the config is a script.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};

/// The `lefthook` plugin.
pub struct Lefthook;

static SPEC: PluginSpec = PluginSpec {
    id: "lefthook",
    enablers: &[
        Enabler::Dependency("lefthook"),
        Enabler::Dependency("@arkweid/lefthook"),
        Enabler::Dependency("@evilmartians/lefthook"),
    ],
    config: &[
        "lefthook.{yml,yaml,json,jsonc,toml}",
        ".lefthook.{yml,yaml,json,jsonc,toml}",
        ".config/lefthook.{yml,yaml,json,jsonc,toml}",
        "lefthook-local.{yml,yaml,json,jsonc,toml}",
        ".lefthook-local.{yml,yaml,json,jsonc,toml}",
        ".config/lefthook-local.{yml,yaml,json,jsonc,toml}",
    ],
    binaries: &["lefthook"],
    optional_binaries: true,
    ..PluginSpec::DEFAULT
};

impl Plugin for Lefthook {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        collect_runs(config.value, 0, &mut out);
        out
    }
}

fn collect_runs(value: &ConfigValue, depth: u8, out: &mut Vec<PluginInput>) {
    if depth > 8 {
        return;
    }
    match value {
        ConfigValue::Object(entries) => {
            for (key, child) in entries {
                match child {
                    ConfigValue::String(script) if key == "run" => out.push(PluginInput::Script(script.clone())),
                    other => collect_runs(other, depth + 1, out),
                }
            }
        }
        ConfigValue::Array(items) => {
            for item in items {
                collect_runs(item, depth + 1, out);
            }
        }
        _ => {}
    }
}
