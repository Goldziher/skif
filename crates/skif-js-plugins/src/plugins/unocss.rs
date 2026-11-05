//! `UnoCSS`: its config files are entry files.

use crate::plugin::{Enabler, Plugin, PluginSpec};

/// The `unocss` plugin.
pub struct Unocss;

static SPEC: PluginSpec = PluginSpec {
    id: "unocss",
    enablers: &[Enabler::Dependency("unocss")],
    entry: &["{uno,unocss}.config.{json,ts,mts,cts,js,mjs,cjs}"],
    binaries: &["unocss"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Unocss {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }
}
