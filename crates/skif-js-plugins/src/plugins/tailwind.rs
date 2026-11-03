//! Tailwind CSS: the config file is an entry; plugins and presets named by string are dependencies.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{module_input, strings};

/// The `tailwind` plugin.
pub struct Tailwind;

static SPEC: PluginSpec = PluginSpec {
    id: "tailwind",
    enablers: &[
        Enabler::Dependency("tailwindcss"),
        Enabler::Dependency("@tailwindcss/vite"),
        Enabler::Dependency("@tailwindcss/webpack"),
        Enabler::Dependency("@tailwindcss/postcss"),
        Enabler::Dependency("@tailwindcss/cli"),
    ],
    config: &["{,config/}tailwind.config.{js,cjs,mjs,ts,cts,mts}"],
    entry: &["{,config/}tailwind.config.{js,cjs,mjs,ts,cts,mts}"],
    binaries: &["tailwindcss"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Tailwind {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        ["plugins", "presets"]
            .into_iter()
            .filter_map(|key| config.value.get(key))
            .flat_map(strings)
            .filter_map(|name| module_input(config.base, name, false))
            .collect()
    }
}
