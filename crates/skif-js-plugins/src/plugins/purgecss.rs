//! `PurgeCSS`: the config file, and one passed with `--config`, is an entry.

use crate::plugin::{Enabler, Plugin, PluginSpec};

/// The `purgecss` plugin.
pub struct Purgecss;

static SPEC: PluginSpec = PluginSpec {
    id: "purgecss",
    enablers: &[
        Enabler::Dependency("purgecss"),
        Enabler::Dependency("@fullhuman/postcss-purgecss"),
    ],
    entry: &["purgecss.config.js"],
    binaries: &["purgecss"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Purgecss {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn reads_config(&self) -> bool {
        false
    }
}
