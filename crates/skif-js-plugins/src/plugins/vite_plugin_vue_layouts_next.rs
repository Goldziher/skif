//! vite-plugin-vue-layouts-next: the layouts under the `layoutsDirs` of `Layouts({ … })` in `vite.config.*`
//! (`src/layouts` by default) are production entries.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{join_rel, vite_plugin_dirs};

/// The `vite-plugin-vue-layouts-next` plugin.
pub struct VitePluginVueLayoutsNext;

const MODULES: &[&str] = &[
    "vite-plugin-vue-layouts-next",
    "vite-plugin-vue-layouts",
    "vite-plugin-vue-meta-layouts",
];

static SPEC: PluginSpec = PluginSpec {
    id: "vite-plugin-vue-layouts-next",
    enablers: &[
        Enabler::Dependency("vite-plugin-vue-layouts-next"),
        Enabler::Dependency("vite-plugin-vue-layouts"),
        Enabler::Dependency("vite-plugin-vue-meta-layouts"),
    ],
    config: &["vite.config.{js,mjs,ts,cjs,mts,cts}"],
    production: &["src/layouts/**/*.vue"],
    ..PluginSpec::DEFAULT
};

impl Plugin for VitePluginVueLayoutsNext {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    /// The options are read from the plugin call in the source.
    fn reads_config(&self) -> bool {
        false
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        vite_plugin_dirs(config, MODULES, "layoutsDirs")
            .unwrap_or_else(|| vec!["src/layouts".to_owned()])
            .iter()
            .map(|dir| PluginInput::ProductionEntry(config.join(&join_rel(dir, "**/*.vue"))))
            .collect()
    }
}
