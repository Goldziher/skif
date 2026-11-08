//! Astro Markdoc: the components that render the `nodes` and `tags` of `markdoc.config.*`, written as a path or
//! with `component("./src/x.astro")` of `@astrojs/markdoc/config`, are production entries.

use skif_js_syntax::{ConfigValue, extract_imported_calls};

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{module_input, values_of};

/// The `astro-markdoc` plugin.
pub struct AstroMarkdoc;

static SPEC: PluginSpec = PluginSpec {
    id: "astro-markdoc",
    enablers: &[Enabler::Dependency("@astrojs/markdoc")],
    config: &["markdoc.config.{js,ts,mjs,mts}"],
    ..PluginSpec::DEFAULT
};

impl Plugin for AstroMarkdoc {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    /// The components are also read from the `component(…)` calls in the source.
    fn reads_config(&self) -> bool {
        false
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut specifiers: Vec<String> = Vec::new();
        for key in ["nodes", "tags"] {
            let Some(group) = config.value.get(key) else { continue };
            for item in values_of(group) {
                if let Some(specifier) = item.get("render").and_then(render_specifier) {
                    specifiers.push(specifier.to_owned());
                }
            }
        }
        for call in extract_imported_calls(config.source, config.path, &["@astrojs/markdoc/config"], &[]) {
            if call.export == "component"
                && let Some(path) = call.arguments.first().and_then(ConfigValue::as_str)
            {
                specifiers.push(path.to_owned());
            }
        }
        specifiers
            .iter()
            .filter_map(|specifier| {
                let specifier = specifier.split('#').next().unwrap_or_default();
                match module_input(config.base, specifier, true)? {
                    PluginInput::Entry(path) => Some(PluginInput::ProductionEntry(path)),
                    other => Some(other),
                }
            })
            .collect()
    }
}

/// The module a `render` value names: a string, or the `Component`, `file` or `path` of an object.
fn render_specifier(render: &ConfigValue) -> Option<&str> {
    render.as_str().or_else(|| {
        ["Component", "file", "path"]
            .into_iter()
            .find_map(|key| render.get(key).and_then(ConfigValue::as_str))
    })
}
