//! Lost Pixel: its config file is an entry.

use crate::plugin::{Enabler, Plugin, PluginSpec};

/// The `lost-pixel` plugin.
pub struct LostPixel;

static SPEC: PluginSpec = PluginSpec {
    id: "lost-pixel",
    enablers: &[Enabler::Dependency("lost-pixel")],
    entry: &["lostpixel.config.{js,ts}"],
    binaries: &["lost-pixel"],
    ..PluginSpec::DEFAULT
};

impl Plugin for LostPixel {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn reads_config(&self) -> bool {
        false
    }
}
