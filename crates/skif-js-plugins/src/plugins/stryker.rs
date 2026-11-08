//! Stryker: the test runner, checkers and plugins the config names are packages, the runner config it names is a
//! config of that runner, and `stryker run <file>` names a config file.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::script_config::{positionals, workspace_path};
use crate::support::strings;

/// The `stryker` plugin.
pub struct Stryker;

const BOOLEAN_FLAGS: &[&str] = &[
    "allowEmpty",
    "disableBail",
    "dryRunOnly",
    "force",
    "ignoreStatic",
    "incremental",
    "inPlace",
];

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
        let mut out: Vec<PluginInput> = names.into_iter().map(PluginInput::OptionalDependency).collect();
        // The runner reads its own config from the file named here (`vitest.configFile: 'vitest.stryker.config.ts'`).
        for (runner, plugin) in [("vitest", "vitest"), ("jest", "jest")] {
            if let Some(file) = config
                .value
                .get(runner)
                .and_then(|options| options.get("configFile"))
                .and_then(skif_js_syntax::ConfigValue::as_str)
            {
                out.push(PluginInput::LinkedConfig {
                    plugin,
                    path: config.join(file),
                });
            }
        }
        out
    }

    fn resolve_workspace(&self, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        cx.scripts
            .values()
            .flat_map(|script| positionals(script, SPEC.binaries, BOOLEAN_FLAGS))
            .filter(|words| words.first().is_some_and(|word| word == "run"))
            .filter_map(|words| words.get(1).and_then(|file| workspace_path(file)))
            .map(PluginInput::ConfigFile)
            .collect()
    }
}
