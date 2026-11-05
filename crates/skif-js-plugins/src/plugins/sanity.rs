//! `Sanity`: the studio, CLI and blueprint configs are entry files.

use crate::plugin::{Enabler, Plugin, PluginSpec};

/// The `sanity` plugin.
pub struct Sanity;

static SPEC: PluginSpec = PluginSpec {
    id: "sanity",
    enablers: &[Enabler::Dependency("sanity")],
    entry: &[
        "sanity.config.{js,jsx,ts,tsx}",
        "sanity.cli.{ts,js}",
        "sanity.blueprint.{ts,js,json}",
    ],
    binaries: &["sanity"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Sanity {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }
}
