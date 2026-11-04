//! Mocha: spec files, required modules and a custom reporter.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{entry_glob, module_input, strings};

/// The `mocha` plugin.
pub struct Mocha;

static SPEC: PluginSpec = PluginSpec {
    id: "mocha",
    enablers: &[Enabler::Dependency("mocha")],
    config: &[".mocharc.{js,cjs,mjs,json,jsonc,yml,yaml}"],
    package_json_key: Some("mocha"),
    entry: &["**/test/*.{js,cjs,mjs}"],
    binaries: &["mocha", "_mocha"],
    ..PluginSpec::DEFAULT
};

const BUILT_IN_REPORTERS: &[&str] = &[
    "base",
    "dot",
    "doc",
    "tap",
    "json",
    "html",
    "list",
    "min",
    "spec",
    "nyan",
    "xunit",
    "markdown",
    "progress",
    "landing",
    "json-stream",
    "github-actions",
];

impl Plugin for Mocha {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        for key in ["spec", "file"] {
            if let Some(value) = config.value.get(key) {
                out.extend(
                    strings(value)
                        .into_iter()
                        .filter_map(|pattern| entry_glob(config.base, pattern))
                        .map(PluginInput::Entry),
                );
            }
        }
        if let Some(require) = config.value.get("require") {
            out.extend(
                strings(require)
                    .into_iter()
                    .filter_map(|name| module_input(config.base, name, true)),
            );
        }
        if let Some(reporter) = config
            .value
            .get("reporter")
            .and_then(skif_js_syntax::ConfigValue::as_str)
            && !BUILT_IN_REPORTERS.contains(&reporter.to_ascii_lowercase().as_str())
        {
            out.extend(module_input(config.base, reporter, false));
        }
        out
    }
}
