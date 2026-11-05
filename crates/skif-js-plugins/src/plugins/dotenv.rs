//! `dotenv-cli`: the binary wraps the command it runs, which the script parser follows.

use crate::plugin::{Enabler, Plugin, PluginSpec};

/// The `dotenv` plugin.
pub struct Dotenv;

static SPEC: PluginSpec = PluginSpec {
    id: "dotenv",
    enablers: &[Enabler::Dependency("dotenv-cli")],
    binaries: &["dotenv"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Dotenv {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }
}
