//! Sapper: the client, server and service worker modules and the routes of the file-system router. knip has no
//! counterpart; the layout is Sapper's fixed one.

use crate::plugin::{Enabler, Plugin, PluginSpec};

/// The `sapper` plugin.
pub struct Sapper;

static SPEC: PluginSpec = PluginSpec {
    id: "sapper",
    enablers: &[Enabler::Dependency("sapper")],
    production: &[
        "src/{client,server,service-worker}.{js,ts}",
        "src/routes/**/*.{svelte,html,js,ts}",
    ],
    binaries: &["sapper"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Sapper {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn reads_config(&self) -> bool {
        false
    }
}
