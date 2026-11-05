//! syncpack: its config files are loaded by the tool, not imported.

use crate::plugin::{Enabler, Plugin, PluginSpec};

/// The `syncpack` plugin.
pub struct Syncpack;

static SPEC: PluginSpec = PluginSpec {
    id: "syncpack",
    enablers: &[Enabler::Dependency("syncpack")],
    entry: &[
        ".syncpackrc",
        ".syncpackrc.{json,yaml,yml,js,cjs}",
        "syncpack.config.{js,cjs}",
    ],
    binaries: &["syncpack"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Syncpack {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn reads_config(&self) -> bool {
        false
    }
}
