//! Bun: the test files `bun test` finds and the modules `bunfig.toml` preloads for them.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{get_in, module_input, strings};

/// The `bun` plugin.
pub struct Bun;

static SPEC: PluginSpec = PluginSpec {
    id: "bun",
    enablers: &[
        Enabler::File("bun.lock"),
        Enabler::File("bun.lockb"),
        Enabler::Script("bun test"),
    ],
    config: &["bunfig.toml"],
    entry: &["**/*.{test,spec}.{js,jsx,ts,tsx}", "**/*_{test,spec}.{js,jsx,ts,tsx}"],
    binaries: &["bun", "bunx"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Bun {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        get_in(config.value, &["test", "preload"])
            .map(strings)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|specifier| module_input(config.base, specifier, false))
            .collect()
    }
}
