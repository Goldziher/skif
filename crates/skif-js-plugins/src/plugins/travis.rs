//! Travis CI: the `before_install`, `before_script` and `before_deploy` commands of `.travis.yml` are scripts.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::script_inputs;

/// The `travis` plugin.
pub struct Travis;

static SPEC: PluginSpec = PluginSpec {
    id: "travis",
    enablers: &[Enabler::File(".travis.yml")],
    root_only: true,
    config: &[".travis.yml"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Travis {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        ["before_deploy", "before_install", "before_script"]
            .iter()
            .filter_map(|key| config.value.get(key))
            .flat_map(script_inputs)
            .collect()
    }
}
