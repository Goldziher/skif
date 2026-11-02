//! rspack: the webpack-compatible config (entries, loaders, aliases, lookup directories).

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::plugins::webpack::bundler_inputs;

/// The `rspack` plugin.
pub struct Rspack;

static SPEC: PluginSpec = PluginSpec {
    id: "rspack",
    enablers: &[Enabler::Dependency("@rspack/core"), Enabler::Dependency("@rspack/cli")],
    config: &["rspack.config.{js,ts,mjs,cjs,mts,cts}"],
    binaries: &["rspack"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Rspack {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        bundler_inputs(config, cx)
    }
}
