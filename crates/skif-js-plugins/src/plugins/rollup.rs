//! Rollup: the `input` of each build in the config is a production entry.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{objects, strings, values_of};

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
        objects(config.value)
            .into_iter()
            .filter_map(|build| build.get("input"))
            .flat_map(|input| match input {
                skif_js_syntax::ConfigValue::Object(_) => values_of(input).into_iter().flat_map(strings).collect(),
                other => strings(other),
            })
            .map(|path| PluginInput::ProductionEntry(config.join(path)))
            .collect()
    }
}
