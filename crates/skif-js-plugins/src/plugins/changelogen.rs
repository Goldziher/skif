//! changelogen: its c12 config file is an entry.

use crate::plugin::{Enabler, Plugin, PluginSpec};

/// The `changelogen` plugin.
pub struct Changelogen;

static SPEC: PluginSpec = PluginSpec {
    id: "changelogen",
    enablers: &[Enabler::Dependency("changelogen")],
    root_only: true,
    entry: &[
        ".changelogrc",
        ".config/changelogrc",
        ".changelogrc.{json,jsonc,json5,yaml,yml,js,ts,mjs,cjs,mts,cts,toml}",
        "changelog.config.{js,ts,mjs,cjs,mts,cts}",
        ".config/changelogrc.{json,jsonc,json5,yaml,yml,js,ts,mjs,cjs,mts,cts,toml}",
    ],
    binaries: &["changelogen"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Changelogen {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn reads_config(&self) -> bool {
        false
    }
}
