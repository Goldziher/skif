//! rslib: every `entry` of the config is a production entry.
//!
//! knip also marks the exports of these entries as used (`allowIncludeExports`); skif has no per-entry switch for
//! that, so they follow `include-entry-exports`.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{production_entries, property_values};

/// The `rslib` plugin.
pub struct Rslib;

static SPEC: PluginSpec = PluginSpec {
    id: "rslib",
    enablers: &[Enabler::Dependency("@rslib/core")],
    config: &["rslib*.config.{mjs,ts,js,cjs,mts,cts}"],
    binaries: &["rslib"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Rslib {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        production_entries(config.base, property_values(config.value, "entry"))
    }
}
