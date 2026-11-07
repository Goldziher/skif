//! Astro DB: the database config and seed files are entries.

use crate::plugin::{Enabler, Plugin, PluginSpec};

/// The `astro-db` plugin.
pub struct AstroDb;

static SPEC: PluginSpec = PluginSpec {
    id: "astro-db",
    enablers: &[Enabler::Dependency("@astrojs/db")],
    entry: &["db/config.{js,ts}", "db/seed.{js,ts}"],
    ..PluginSpec::DEFAULT
};

impl Plugin for AstroDb {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn reads_config(&self) -> bool {
        false
    }
}
