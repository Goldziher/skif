//! knip: a project that runs knip next to skif keeps knip's config, which knip loads, so the config files are entries
//! and what a JavaScript or TypeScript config imports is used.

use crate::plugin::{Enabler, Plugin, PluginSpec};

/// The `knip` plugin.
pub struct Knip;

static SPEC: PluginSpec = PluginSpec {
    id: "knip",
    enablers: &[Enabler::Dependency("knip")],
    config: &[
        "knip.{json,jsonc}",
        ".knip.{json,jsonc}",
        "knip.{js,ts,mjs,cjs,mts,cts}",
        "knip.config.{js,ts,mjs,cjs,mts,cts}",
    ],
    entry: &["knip.{js,ts,mjs,cjs,mts,cts}", "knip.config.{js,ts,mjs,cjs,mts,cts}"],
    package_json_key: Some("knip"),
    binaries: &["knip", "knip-bun"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Knip {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn reads_config(&self) -> bool {
        false
    }
}
