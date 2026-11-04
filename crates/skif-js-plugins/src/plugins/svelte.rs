//! Svelte: the compiler config is loaded by the tooling, not imported.

use crate::plugin::{Enabler, Plugin, PluginSpec};

/// The `svelte` plugin.
pub struct Svelte;

static SPEC: PluginSpec = PluginSpec {
    id: "svelte",
    enablers: &[Enabler::Dependency("svelte")],
    entry: &["svelte.config.{js,cjs,mjs,ts}"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Svelte {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }
}
