//! vite-plugin-pwa: with the `injectManifest` strategy, the service worker source (`srcDir`/`filename`, `public/sw.js`
//! by default) named in the options of `VitePWA({ … })` in `vite.config.*`, or of `pwa: { … }` in `nuxt.config.*`, is
//! a production entry.

use skif_js_syntax::{ConfigValue, extract_imported_calls, extract_property_values};

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::join_rel;

/// The `vite-plugin-pwa` plugin.
pub struct VitePluginPwa;

static SPEC: PluginSpec = PluginSpec {
    id: "vite-plugin-pwa",
    enablers: &[
        Enabler::Dependency("vite-plugin-pwa"),
        Enabler::Dependency("@vite-pwa/nuxt"),
    ],
    config: &[
        "vite.config.{js,mjs,ts,cjs,mts,cts}",
        "nuxt.config.{js,cjs,mjs,ts,cts,mts}",
    ],
    ..PluginSpec::DEFAULT
};

impl Plugin for VitePluginPwa {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    /// The options are read from the plugin call or the `pwa` key in the source.
    fn reads_config(&self) -> bool {
        false
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let options: Vec<ConfigValue> = if config.file_name().starts_with("nuxt.config.") {
            extract_property_values(config.source, config.path, "pwa")
        } else {
            extract_imported_calls(config.source, config.path, &["vite-plugin-pwa"], &[])
                .into_iter()
                .filter_map(|call| call.arguments.into_iter().next())
                .collect()
        };
        options
            .iter()
            .filter(|options| options.get("strategies").and_then(ConfigValue::as_str) == Some("injectManifest"))
            .map(|options| {
                let text = |key: &str, default: &'static str| {
                    options
                        .get(key)
                        .and_then(ConfigValue::as_str)
                        .unwrap_or(default)
                        .to_owned()
                };
                let file = join_rel(&text("srcDir", "public"), &text("filename", "sw.js"));
                PluginInput::ProductionEntry(config.join(&file))
            })
            .collect()
    }
}
