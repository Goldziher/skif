//! rolldown: every `input` of the config is a production entry.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{production_entries, property_values};

/// The `rolldown` plugin.
pub struct Rolldown;

static SPEC: PluginSpec = PluginSpec {
    id: "rolldown",
    enablers: &[Enabler::Dependency("rolldown")],
    config: &["rolldown.config.{js,cjs,mjs,ts,cts,mts}"],
    binaries: &["rolldown"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Rolldown {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        production_entries(config.base, property_values(config.value, "input"))
    }
}
