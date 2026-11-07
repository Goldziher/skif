//! node-tap: its default test locations, and the `before` and `after` scripts of its config. knip has no `tap` plugin;
//! the patterns are the defaults tap documents.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::module_input;

/// The `tap` plugin.
pub struct Tap;

static SPEC: PluginSpec = PluginSpec {
    id: "tap",
    enablers: &[Enabler::Dependency("tap")],
    config: &[".taprc"],
    package_json_key: Some("tap"),
    entry: &[
        "{test,tests,__tests__}/**/*.{js,cjs,mjs,jsx,ts,cts,mts,tsx}",
        "**/*.{test,spec}.{js,cjs,mjs,jsx,ts,cts,mts,tsx}",
        "**/test.{js,cjs,mjs,jsx,ts,cts,mts,tsx}",
    ],
    binaries: &["tap"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Tap {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        ["before", "after"]
            .into_iter()
            .filter_map(|key| config.value.get(key).and_then(ConfigValue::as_str))
            .filter_map(|script| module_input(config.base, script, true))
            .collect()
    }
}
