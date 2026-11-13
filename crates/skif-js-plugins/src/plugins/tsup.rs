//! tsup: the `entry` of each build in the config is a production entry.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{config_unknown, entry_glob, file_strings, files_key_unknown, objects, values_of};

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
        entry_inputs(config, &["entry", "entryPoints"])
    }
}

/// The production entries of the builds in a bundler config: the first of `keys` a build sets, as a string, a list,
/// an object of named entries or a list of `{ in, out }` objects. Shared by the bundlers that take the tsup shape.
pub(crate) fn entry_inputs(config: &ConfigAst<'_>, keys: &[&str]) -> Vec<PluginInput> {
    let mut out = Vec::new();
    if config_unknown(config.value) {
        out.push(PluginInput::EntriesIncomplete);
    }
    for build in objects(config.value) {
        let key = keys
            .iter()
            .copied()
            .find(|key| build.get(key).is_some())
            .or_else(|| keys.last().copied())
            .unwrap_or("entry");
        if let Some(entry) = build.get(key) {
            let paths: Vec<&str> = match entry {
                ConfigValue::Object(_) => values_of(entry).into_iter().flat_map(file_strings).collect(),
                ConfigValue::Array(items) => items
                    .iter()
                    .flat_map(|item| match item {
                        ConfigValue::Object(_) => item.get("in").map_or_default(file_strings),
                        other => file_strings(other),
                    })
                    .collect(),
                other => file_strings(other),
            };
            out.extend(
                paths
                    .into_iter()
                    .filter_map(|path| entry_glob(config.base, path))
                    .map(PluginInput::ProductionEntry),
            );
        }
        if files_key_unknown(build, key) {
            out.push(PluginInput::EntriesIncomplete);
        }
    }
    out
}
