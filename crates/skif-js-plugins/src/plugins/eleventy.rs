//! Eleventy: the data files and JavaScript templates under the input directory `eleventy.config.*` returns
//! (`dir.input`, `dir.data`, `templateFormats`).

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{get_in, join_rel, strings};

/// The `eleventy` plugin.
pub struct Eleventy;

static SPEC: PluginSpec = PluginSpec {
    id: "eleventy",
    enablers: &[Enabler::Dependency("@11ty/eleventy")],
    config: &[".eleventy.js", "eleventy.config.{js,cjs,mjs}"],
    production: &["posts/**/*.11tydata.{js,cjs,mjs}", "_data/**/*.{js,cjs,mjs}"],
    binaries: &["eleventy"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Eleventy {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let dir = |key: &str, default: &'static str| {
            get_in(config.value, &["dir", key])
                .and_then(ConfigValue::as_str)
                .filter(|dir| !dir.is_empty())
                .unwrap_or(default)
        };
        let input = dir("input", ".");
        let data = dir("data", "_data");
        let formats = match config.value.get("templateFormats") {
            Some(formats @ ConfigValue::Array(_)) => strings(formats).join(","),
            Some(ConfigValue::String(formats)) => formats.clone(),
            _ => "11ty.js".to_owned(),
        };
        [
            join_rel(&join_rel(input, data), "**/*.{js,cjs,mjs}"),
            join_rel(input, &format!("**/*.{{{formats}}}")),
            join_rel(input, "**/*.11tydata.{js,cjs,mjs}"),
        ]
        .into_iter()
        .map(|pattern| PluginInput::ProductionEntry(config.join(&pattern)))
        .collect()
    }
}
