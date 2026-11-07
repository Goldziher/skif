//! Oxfmt: its config file; the config names no dependencies.

use crate::plugin::{Enabler, Plugin, PluginSpec};

/// The `oxfmt` plugin.
pub struct Oxfmt;

static SPEC: PluginSpec = PluginSpec {
    id: "oxfmt",
    enablers: &[Enabler::Dependency("oxfmt")],
    config: &[".oxfmtrc.{json,jsonc}", "oxfmt.config.{ts,mts}"],
    binaries: &["oxfmt"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Oxfmt {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }
}
