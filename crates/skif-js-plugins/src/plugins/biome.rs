//! Biome: a config it extends is a further config file or a shared config package.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{config_ref, strings};

/// The `biome` plugin.
pub struct Biome;

static SPEC: PluginSpec = PluginSpec {
    id: "biome",
    enablers: &[Enabler::Dependency("@biomejs/biome")],
    config: &["biome.json", "biome.jsonc", ".biome.json", ".biome.jsonc"],
    binaries: &["biome"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Biome {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        config
            .value
            .get("extends")
            .map(strings)
            .unwrap_or_default()
            .into_iter()
            .filter(|specifier| *specifier != "//")
            .filter_map(|specifier| config_ref(config.base, specifier))
            .collect()
    }
}
