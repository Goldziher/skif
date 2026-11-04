//! nodemon: the `exec` command and the event commands of the config are scripts.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{script_inputs, values_of};

/// The `nodemon` plugin.
pub struct Nodemon;

static SPEC: PluginSpec = PluginSpec {
    id: "nodemon",
    enablers: &[Enabler::Dependency("nodemon")],
    config: &["nodemon.json"],
    package_json_key: Some("nodemonConfig"),
    binaries: &["nodemon"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Nodemon {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        if let Some(exec) = config.value.get("exec") {
            out.extend(script_inputs(exec));
        }
        if let Some(events) = config.value.get("events") {
            out.extend(values_of(events).into_iter().flat_map(script_inputs));
        }
        out
    }
}
