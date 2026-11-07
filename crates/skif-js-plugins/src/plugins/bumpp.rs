//! bumpp: its c12 config file is an entry.

use crate::plugin::{Enabler, Plugin, PluginSpec};

/// The `bumpp` plugin.
pub struct Bumpp;

static SPEC: PluginSpec = PluginSpec {
    id: "bumpp",
    enablers: &[Enabler::Dependency("bumpp")],
    entry: &[
        ".bumprc",
        ".config/bumprc",
        ".bumprc.{json,jsonc,json5,yaml,yml,js,ts,mjs,cjs,mts,cts,toml}",
        "bump.config.{js,ts,mjs,cjs,mts,cts}",
        ".config/bumprc.{json,jsonc,json5,yaml,yml,js,ts,mjs,cjs,mts,cts,toml}",
    ],
    binaries: &["bumpp"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Bumpp {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn reads_config(&self) -> bool {
        false
    }
}
