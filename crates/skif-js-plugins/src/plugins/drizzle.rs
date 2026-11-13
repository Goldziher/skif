//! Drizzle Kit: the schema files named in the config are production entries.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{entry_glob, strings};

/// The `drizzle` plugin.
pub struct Drizzle;

static SPEC: PluginSpec = PluginSpec {
    id: "drizzle",
    enablers: &[Enabler::Dependency("drizzle-kit")],
    config: &["drizzle.config.{ts,js,json}"],
    binaries: &["drizzle-kit"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Drizzle {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        config
            .value
            .get("schema")
            .map_or_default(strings)
            .into_iter()
            .filter_map(|schema| entry_glob(config.base, schema))
            .map(PluginInput::ProductionEntry)
            .collect()
    }
}
