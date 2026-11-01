//! lint-staged: every command in the config is a script.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::strings;
use skif_js_syntax::ConfigValue;

/// The `lint-staged` plugin.
pub struct LintStaged;

static SPEC: PluginSpec = PluginSpec {
    id: "lint-staged",
    enablers: &[Enabler::Dependency("lint-staged")],
    config: &[
        ".lintstagedrc",
        ".lintstagedrc.{json,yaml,yml,mjs,mts,js,ts,cjs,cts}",
        "lint-staged.config.{mjs,mts,js,ts,cjs,cts}",
    ],
    package_json_key: Some("lint-staged"),
    binaries: &["lint-staged"],
    ..PluginSpec::DEFAULT
};

impl Plugin for LintStaged {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let ConfigValue::Object(globs) = config.value else { return Vec::new() };
        globs
            .iter()
            .filter(|(key, _)| !key.starts_with('_'))
            .flat_map(|(_, commands)| strings(commands))
            .map(|command| PluginInput::Script(command.to_string()))
            .collect()
    }
}
