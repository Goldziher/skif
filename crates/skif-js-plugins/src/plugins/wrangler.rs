//! Wrangler: the worker script named by `main` is a production entry.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::entry_glob;

/// The `wrangler` plugin.
pub struct Wrangler;

static SPEC: PluginSpec = PluginSpec {
    id: "wrangler",
    enablers: &[Enabler::Dependency("wrangler")],
    config: &["wrangler.{json,jsonc,toml}"],
    binaries: &["wrangler"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Wrangler {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        config
            .value
            .get("main")
            .and_then(skif_js_syntax::ConfigValue::as_str)
            .and_then(|main| entry_glob(config.base, main))
            .map(PluginInput::ProductionEntry)
            .into_iter()
            .collect()
    }
}
