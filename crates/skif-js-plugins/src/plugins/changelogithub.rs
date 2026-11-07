//! changelogithub: its c12 config file is an entry.

use crate::plugin::{Enabler, Plugin, PluginSpec};

/// The `changelogithub` plugin.
pub struct Changelogithub;

static SPEC: PluginSpec = PluginSpec {
    id: "changelogithub",
    enablers: &[Enabler::Dependency("changelogithub")],
    root_only: true,
    entry: &[
        ".changelogithubrc",
        ".config/changelogithubrc",
        ".changelogithubrc.{json,jsonc,json5,yaml,yml,js,ts,mjs,cjs,mts,cts,toml}",
        "changelogithub.config.{js,ts,mjs,cjs,mts,cts}",
        ".config/changelogithubrc.{json,jsonc,json5,yaml,yml,js,ts,mjs,cjs,mts,cts,toml}",
    ],
    binaries: &["changelogithub"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Changelogithub {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn reads_config(&self) -> bool {
        false
    }
}
