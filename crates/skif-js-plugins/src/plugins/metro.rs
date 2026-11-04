//! Metro: platform-specific source files are production entries.

use crate::plugin::{Enabler, Plugin, PluginSpec};

/// The `metro` plugin.
pub struct Metro;

static SPEC: PluginSpec = PluginSpec {
    id: "metro",
    enablers: &[
        Enabler::Dependency("metro"),
        Enabler::Dependency("@react-native/metro-config"),
        Enabler::Dependency("expo"),
    ],
    config: &["metro.config.{js,cjs,mjs,ts}"],
    production: &["src/**/*.{ios,android,windows,web}.{js,jsx,json,ts,tsx}"],
    binaries: &["metro"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Metro {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }
}
