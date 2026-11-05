//! `Vike`: page and renderer files are production entries.

use crate::plugin::{Enabler, Plugin, PluginSpec};

/// The `vike` plugin.
pub struct Vike;

static SPEC: PluginSpec = PluginSpec {
    id: "vike",
    enablers: &[Enabler::Dependency("vike")],
    production: &[
        "{pages,renderer}/**/+*.{js,jsx,ts,tsx,vue,react,solid}",
        "*/{pages,renderer}/**/+*.{js,jsx,ts,tsx,vue,react,solid}",
    ],
    ..PluginSpec::DEFAULT
};

impl Plugin for Vike {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }
}
