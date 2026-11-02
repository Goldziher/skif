//! Mastra: the framework entry in `src/mastra` and the agents, tools and workflows discovered beside it.

use crate::plugin::{Enabler, Plugin, PluginSpec};

/// The `mastra` plugin.
pub struct Mastra;

static SPEC: PluginSpec = PluginSpec {
    id: "mastra",
    enablers: &[Enabler::Dependency("mastra"), Enabler::Dependency("@mastra/core")],
    config: &["mastra.config.{ts,js}"],
    production: &[
        "src/mastra/index.{ts,js,mts,mjs}",
        "src/mastra/{storage,observability,logger,server,studio}.{js,ts,mjs,mts}",
        "src/mastra/workflows/*.{js,ts}",
        "src/mastra/{,agents/*/}tools/**/*.{js,ts}",
        "!src/mastra/**/*.{test,spec}.{js,ts}",
        "!src/mastra/**/__tests__/**",
    ],
    binaries: &["mastra"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Mastra {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }
}
