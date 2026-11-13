//! Preconstruct: every entrypoint of the package is a production entry below `src`.
//!
//! knip also marks the exports of these entries as used (`allowIncludeExports`); skif has no per-entry switch for
//! that, so they follow `include-entry-exports`.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{join_rel, production_entries, strings};

/// The `preconstruct` plugin.
pub struct Preconstruct;

static SPEC: PluginSpec = PluginSpec {
    id: "preconstruct",
    enablers: &[Enabler::Dependency("@preconstruct/cli")],
    package_json_key: Some("preconstruct"),
    binaries: &["preconstruct"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Preconstruct {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let entrypoints = config.value.get("entrypoints").map_or_default(strings);
        let paths: Vec<String> = entrypoints.into_iter().map(|id| join_rel("src", id)).collect();
        production_entries(config.base, paths.iter().map(String::as_str))
    }
}
