//! Stryker: the test runner, checkers and plugins the config names are packages.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::strings;

/// The `stryker` plugin.
pub struct Stryker;

static SPEC: PluginSpec = PluginSpec {
    id: "stryker",
    enablers: &[Enabler::Dependency("@stryker-mutator/core")],
    config: &["{.,}stryker.{conf,config}.{js,mjs,cjs,json}"],
    binaries: &["stryker"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Stryker {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut names: Vec<String> = Vec::new();
        if let Some(runner) = config
            .value
            .get("testRunner")
            .and_then(skif_js_syntax::ConfigValue::as_str)
        {
            names.push(format!("@stryker-mutator/{runner}-runner"));
        }
        if let Some(checkers) = config.value.get("checkers") {
            names.extend(
                strings(checkers)
                    .into_iter()
                    .map(|checker| format!("@stryker-mutator/{checker}-checker")),
            );
        }
        if let Some(plugins) = config.value.get("plugins") {
            names.extend(
                strings(plugins)
                    .into_iter()
                    .filter(|p| !p.contains('*'))
                    .map(str::to_owned),
            );
        }
        names.into_iter().map(PluginInput::OptionalDependency).collect()
    }
}
