//! Fumadocs: the `source.config.*` of fumadocs-mdx and the MDX pages under `content/` are entries.

use crate::plugin::{Enabler, Plugin, PluginSpec};

/// The `fumadocs` plugin.
pub struct Fumadocs;

static SPEC: PluginSpec = PluginSpec {
    id: "fumadocs",
    enablers: &[
        Enabler::Dependency("fumadocs-core"),
        Enabler::Dependency("fumadocs-mdx"),
        Enabler::Dependency("fumadocs-ui"),
    ],
    entry: &["source.config.{js,ts,mjs}", "content/**/*.mdx"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Fumadocs {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn reads_config(&self) -> bool {
        false
    }
}
