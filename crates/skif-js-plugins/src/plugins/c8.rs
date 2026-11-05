//! `c8`: the binary wraps the command it runs, which the script parser follows.

use crate::plugin::{Enabler, Plugin, PluginSpec};

/// The `c8` plugin.
pub struct C8;

static SPEC: PluginSpec = PluginSpec {
    id: "c8",
    enablers: &[Enabler::Dependency("c8")],
    binaries: &["c8"],
    ..PluginSpec::DEFAULT
};

impl Plugin for C8 {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }
}
