//! `Plop`: the plopfile is an entry file.

use crate::plugin::{Enabler, Plugin, PluginSpec};

/// The `plop` plugin.
pub struct Plop;

static SPEC: PluginSpec = PluginSpec {
    id: "plop",
    enablers: &[Enabler::Dependency("plop")],
    entry: &["plopfile.{cjs,mjs,js,ts}"],
    binaries: &["plop"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Plop {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }
}
