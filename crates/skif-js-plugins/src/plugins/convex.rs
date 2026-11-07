//! Convex: the functions directory, `convex/` or the one `convex.json` names, holds production entries.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};

/// The `convex` plugin.
pub struct Convex;

static SPEC: PluginSpec = PluginSpec {
    id: "convex",
    enablers: &[Enabler::Dependency("convex")],
    config: &["convex.json"],
    production: &["convex/**/*.{js,ts}"],
    binaries: &["convex"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Convex {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let functions = config
            .value
            .get("functions")
            .and_then(ConfigValue::as_str)
            .unwrap_or("convex");
        vec![PluginInput::ProductionEntry(
            config.join(&format!("{functions}/**/*.{{js,ts}}")),
        )]
    }
}
