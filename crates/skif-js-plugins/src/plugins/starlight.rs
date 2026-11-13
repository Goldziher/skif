//! Starlight: the component overrides and custom CSS named in the options of the `@astrojs/starlight` integration in
//! `astro.config.*`.

use skif_js_syntax::{ConfigValue, extract_imported_calls};

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{module_input, strings, values_of};

/// The `starlight` plugin.
pub struct Starlight;

static SPEC: PluginSpec = PluginSpec {
    id: "starlight",
    enablers: &[Enabler::Dependency("@astrojs/starlight")],
    config: &["astro.config.{js,cjs,mjs,ts,mts}"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Starlight {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    /// The integration's options are read from the call in the source, not from what the config exports.
    fn reads_config(&self) -> bool {
        false
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        for call in extract_imported_calls(config.source, config.path, &["@astrojs/starlight"], &[]) {
            let Some(options) = call.arguments.first() else {
                continue;
            };
            let components = options
                .get("components")
                .map_or_default(values_of)
                .into_iter()
                .filter_map(ConfigValue::as_str);
            let css = options.get("customCss").map_or_default(strings);
            for path in components.chain(css) {
                out.extend(module_input(config.base, path, false).map(|input| match input {
                    PluginInput::Entry(file) => PluginInput::ProductionEntry(file),
                    other => other,
                }));
            }
        }
        out
    }
}
