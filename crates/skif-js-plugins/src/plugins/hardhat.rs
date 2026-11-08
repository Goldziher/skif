//! Hardhat: the config file is an entry, and `hardhat` itself is used whenever the plugin is enabled.

use crate::plugin::{Enabler, Plugin, PluginContext, PluginInput, PluginSpec};

/// The `hardhat` plugin.
pub struct Hardhat;

static SPEC: PluginSpec = PluginSpec {
    id: "hardhat",
    enablers: &[Enabler::Dependency("hardhat")],
    entry: &["hardhat.config.{js,cjs,mjs,ts,cts,mts}"],
    binaries: &["hardhat"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Hardhat {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn reads_config(&self) -> bool {
        false
    }

    fn resolve_workspace(&self, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        vec![PluginInput::Dependency("hardhat".to_owned())]
    }
}
