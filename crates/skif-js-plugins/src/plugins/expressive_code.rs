//! Expressive Code: the `ec.config.*` file is an entry.

use crate::plugin::{Enabler, Plugin, PluginSpec};

/// The `expressive-code` plugin.
pub struct ExpressiveCode;

static SPEC: PluginSpec = PluginSpec {
    id: "expressive-code",
    enablers: &[
        Enabler::Dependency("astro-expressive-code"),
        Enabler::Dependency("rehype-expressive-code"),
        Enabler::Dependency("@astrojs/starlight"),
    ],
    entry: &["ec.config.{js,mjs,cjs,ts}"],
    ..PluginSpec::DEFAULT
};

impl Plugin for ExpressiveCode {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn reads_config(&self) -> bool {
        false
    }
}
