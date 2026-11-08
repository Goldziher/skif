//! @vite-pwa/assets-generator: its config file is an entry.

use crate::plugin::{Enabler, Plugin, PluginSpec};

/// The `vite-pwa-assets-generator` plugin.
pub struct VitePwaAssetsGenerator;

static SPEC: PluginSpec = PluginSpec {
    id: "vite-pwa-assets-generator",
    enablers: &[Enabler::Dependency("@vite-pwa/assets-generator")],
    entry: &["pwa-assets.config.{js,cjs,mjs,ts,cts,mts}"],
    binaries: &["pwa-assets-generator"],
    ..PluginSpec::DEFAULT
};

impl Plugin for VitePwaAssetsGenerator {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn reads_config(&self) -> bool {
        false
    }
}
