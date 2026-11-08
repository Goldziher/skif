//! tsrs, a TypeScript compiler: its `tsconfig.json` files are read as the `typescript` plugin reads them.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::plugins::typescript::Typescript;

/// The `tsrs` plugin.
pub struct Tsrs;

static SPEC: PluginSpec = PluginSpec {
    id: "tsrs",
    enablers: &[Enabler::Dependency("tsrs")],
    config: &["tsconfig.json", "tsconfig.*.json", "jsconfig.json"],
    binaries: &["tsrs"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Tsrs {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        Typescript.resolve_config(config, cx)
    }
}
