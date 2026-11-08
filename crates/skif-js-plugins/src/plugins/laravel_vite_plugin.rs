//! laravel-vite-plugin: the inputs and SSR entries of `laravel({ input, ssr })` (or `laravel("…")`) in
//! `vite.config.*` are production entries.

use skif_js_syntax::extract_imported_calls;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{entry_glob, strings};

/// The `laravel-vite-plugin` plugin.
pub struct LaravelVitePlugin;

static SPEC: PluginSpec = PluginSpec {
    id: "laravel-vite-plugin",
    enablers: &[Enabler::Dependency("laravel-vite-plugin")],
    config: &["vite.config.{js,mjs,ts,cjs,mts,cts}"],
    ..PluginSpec::DEFAULT
};

impl Plugin for LaravelVitePlugin {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    /// The options are read from the plugin call in the source.
    fn reads_config(&self) -> bool {
        false
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out: Vec<PluginInput> = Vec::new();
        for call in extract_imported_calls(config.source, config.path, &["laravel-vite-plugin"], &[]) {
            let Some(options) = call.arguments.first() else {
                continue;
            };
            let mut ids: Vec<&str> = strings(options);
            for key in ["input", "ssr"] {
                ids.extend(options.get(key).map(strings).unwrap_or_default());
            }
            for id in ids {
                if let Some(path) = entry_glob(config.base, id) {
                    let input = PluginInput::ProductionEntry(path);
                    if !out.contains(&input) {
                        out.push(input);
                    }
                }
            }
        }
        out
    }
}
