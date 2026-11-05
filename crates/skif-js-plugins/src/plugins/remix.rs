//! `Remix`: the app root, entries and route modules are production entries.

use crate::plugin::{Enabler, Plugin, PluginSpec};

/// The `remix` plugin.
pub struct Remix;

static SPEC: PluginSpec = PluginSpec {
    id: "remix",
    enablers: &[Enabler::Dependency("@remix-run/dev")],
    entry: &["remix.config.js", "remix.init/index.js"],
    production: &[
        "app/root.tsx",
        "app/entry.{client,server}.{js,jsx,ts,tsx}",
        "app/routes/**/*.{js,ts,tsx}",
        "server.{js,ts}",
    ],
    ..PluginSpec::DEFAULT
};

impl Plugin for Remix {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }
}
