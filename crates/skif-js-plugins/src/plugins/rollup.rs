//! Rollup: the `input` of each build in the config is a production entry.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{config_unknown, key_unknown, objects, strings, values_of};

/// The `rollup` plugin.
pub struct Rollup;

static SPEC: PluginSpec = PluginSpec {
    id: "rollup",
    enablers: &[Enabler::Dependency("rollup")],
    config: &["rollup.config.{js,cjs,mjs,ts}"],
    binaries: &["rollup"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Rollup {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        if config_unknown(config.value) {
            out.push(PluginInput::EntriesIncomplete);
        }
        for build in objects(config.value) {
            if let Some(input) = build.get("input") {
                let paths = match input {
                    ConfigValue::Object(_) => values_of(input).into_iter().flat_map(strings).collect(),
                    other => strings(other),
                };
                out.extend(
                    paths
                        .into_iter()
                        .map(|path| PluginInput::ProductionEntry(config.join(path))),
                );
            }
            if key_unknown(build, "input") {
                out.push(PluginInput::EntriesIncomplete);
            }
        }
        out
    }
}
