//! `Lingui`: the config files are entry files.

use crate::plugin::{Enabler, Plugin, PluginSpec};

/// The `lingui` plugin.
pub struct Lingui;

static SPEC: PluginSpec = PluginSpec {
    id: "lingui",
    enablers: &[Enabler::Dependency("@lingui/cli")],
    entry: &["lingui.config.{js,cjs,ts,mjs}", ".linguirc.{js,ts}"],
    binaries: &["lingui"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Lingui {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }
}
