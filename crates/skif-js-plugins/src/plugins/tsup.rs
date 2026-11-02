//! tsup: the `entry` of each build in the config is a production entry.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{entry_glob, objects, strings, values_of};

/// The `tsup` plugin.
pub struct Tsup;

static SPEC: PluginSpec = PluginSpec {
    id: "tsup",
    enablers: &[Enabler::Dependency("tsup")],
    config: &["tsup.config.{js,ts,cjs,mjs,cts,mts,json}"],
    package_json_key: Some("tsup"),
    binaries: &["tsup", "tsup-node"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Tsup {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        objects(config.value)
            .into_iter()
            .filter_map(|build| build.get("entry").or_else(|| build.get("entryPoints")))
            .flat_map(|entry| match entry {
                ConfigValue::Object(_) => values_of(entry).into_iter().flat_map(strings).collect(),
                other => strings(other),
            })
            .filter_map(|path| entry_glob(config.base, path))
            .map(PluginInput::ProductionEntry)
            .collect()
    }
}
