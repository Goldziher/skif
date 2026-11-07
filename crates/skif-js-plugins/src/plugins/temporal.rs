//! Temporal: the workflows module the worker bundles by path is a production entry.

use crate::plugin::{Enabler, Plugin, PluginSpec};

/// The `temporal` plugin.
pub struct Temporal;

static SPEC: PluginSpec = PluginSpec {
    id: "temporal",
    enablers: &[Enabler::Dependency("@temporalio/worker")],
    production: &["src/workflows{,/index}.{js,cjs,mjs,ts,cts,mts}"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Temporal {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn reads_config(&self) -> bool {
        false
    }
}
