//! VitePress: the config and theme are entries.

use crate::plugin::{Enabler, Plugin, PluginSpec};

/// The `vitepress` plugin.
pub struct Vitepress;

static SPEC: PluginSpec = PluginSpec {
    id: "vitepress",
    enablers: &[Enabler::Dependency("vitepress")],
    entry: &[
        "{,docs/}.vitepress/config.{js,ts,mjs,mts,cjs,cts}",
        "{,docs/}.vitepress/config/index.{js,ts,mjs,mts,cjs,cts}",
        "{,docs/}.vitepress/theme/index.{js,ts,mjs,mts}",
    ],
    binaries: &["vitepress"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Vitepress {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }
}
