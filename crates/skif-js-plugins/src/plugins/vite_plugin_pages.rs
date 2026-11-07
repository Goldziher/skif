//! vite-plugin-pages: the page files under the `dirs` of `Pages({ … })` in `vite.config.*` (`src/pages` by default)
//! are production entries; the `~pages` virtual modules are resolved by the plugin.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{join_rel, vite_plugin_dirs};

/// The `vite-plugin-pages` plugin.
pub struct VitePluginPages;

const PAGES: &str = "**/*.{vue,jsx,tsx,md,mdx}";

static SPEC: PluginSpec = PluginSpec {
    id: "vite-plugin-pages",
    enablers: &[Enabler::Dependency("vite-plugin-pages")],
    config: &["vite.config.{js,mjs,ts,cjs,mts,cts}"],
    production: &["src/pages/**/*.{vue,jsx,tsx,md,mdx}"],
    ..PluginSpec::DEFAULT
};

impl Plugin for VitePluginPages {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    /// The options are read from the plugin call in the source.
    fn reads_config(&self) -> bool {
        false
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        vite_plugin_dirs(config, &["vite-plugin-pages"], "dirs")
            .unwrap_or_else(|| vec!["src/pages".to_owned()])
            .iter()
            .map(|dir| PluginInput::ProductionEntry(config.join(&join_rel(dir, PAGES))))
            .collect()
    }

    fn resolve_workspace(&self, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        ["~pages", "~react-pages", "~solid-pages"]
            .into_iter()
            .map(|name| PluginInput::VirtualModule(name.to_owned()))
            .collect()
    }
}
