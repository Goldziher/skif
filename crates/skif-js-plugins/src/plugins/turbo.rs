//! Turborepo: generator and plop entry files. Task names are resolved by the script parser.

use crate::plugin::{Enabler, Plugin, PluginSpec};

/// The `turbo` plugin.
pub struct Turbo;

static SPEC: PluginSpec = PluginSpec {
    id: "turbo",
    enablers: &[Enabler::Dependency("turbo"), Enabler::Dependency("@turbo/gen")],
    config: &["turbo.json", "turbo.jsonc"],
    entry: &[
        "turbo/generators/config.{ts,js,cjs,mts,mjs}",
        "plopfile.{ts,js,cjs,mts,mjs}",
    ],
    binaries: &["turbo"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Turbo {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn reads_config(&self) -> bool {
        false
    }
}
