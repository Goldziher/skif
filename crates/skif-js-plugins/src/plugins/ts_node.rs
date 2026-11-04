//! ts-node: only its binaries; scripts that run it are resolved by the script parser.

use crate::plugin::{Enabler, Plugin, PluginSpec};

/// The `ts-node` plugin.
pub struct TsNode;

static SPEC: PluginSpec = PluginSpec {
    id: "ts-node",
    enablers: &[Enabler::Dependency("ts-node")],
    binaries: &[
        "ts-node",
        "ts-node-cwd",
        "ts-node-esm",
        "ts-node-script",
        "ts-node-transpile-only",
        "ts-script",
    ],
    ..PluginSpec::DEFAULT
};

impl Plugin for TsNode {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }
}
