//! esbuild: the `entryPoints` of a build config are production entries.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::plugins::tsup::entry_inputs;

/// The `esbuild` plugin.
pub struct Esbuild;

static SPEC: PluginSpec = PluginSpec {
    id: "esbuild",
    enablers: &[Enabler::Dependency("esbuild")],
    config: &[
        "esbuild.config.{js,mjs,cjs,ts,mts,cts}",
        "esbuild.{js,mjs,cjs,ts,mts,cts}",
    ],
    binaries: &["esbuild"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Esbuild {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        entry_inputs(config, &["entryPoints"])
    }
}
