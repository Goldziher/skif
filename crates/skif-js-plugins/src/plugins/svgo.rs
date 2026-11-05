//! `SVGO`: the config file is an entry file.

use crate::plugin::{Enabler, Plugin, PluginSpec};

/// The `svgo` plugin.
pub struct Svgo;

static SPEC: PluginSpec = PluginSpec {
    id: "svgo",
    enablers: &[Enabler::Dependency("svgo"), Enabler::Dependency("@svgr/plugin-svgo")],
    entry: &["svgo.config.{js,cjs,mjs}"],
    binaries: &["svgo"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Svgo {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }
}
