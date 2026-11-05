//! `MSW`: the service worker script sits in the `workerDirectory` of the `msw` key.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{join_rel, strings};

/// The `msw` plugin.
pub struct Msw;

const WORKER: &str = "mockServiceWorker.js";

static SPEC: PluginSpec = PluginSpec {
    id: "msw",
    enablers: &[Enabler::Dependency("msw")],
    package_json_key: Some("msw"),
    entry: &[WORKER],
    binaries: &["msw"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Msw {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        config
            .value
            .get("workerDirectory")
            .map(strings)
            .and_then(|directories| directories.first().copied())
            .map(|directory| PluginInput::Entry(join_rel(directory, WORKER)))
            .into_iter()
            .collect()
    }
}
