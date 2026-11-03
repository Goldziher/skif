//! tsup: the `entry` of each build in the config is a production entry.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{config_unknown, entry_glob, key_unknown, objects, strings, values_of};

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
        let mut out = Vec::new();
        if config_unknown(config.value) {
            out.push(PluginInput::EntriesIncomplete);
        }
        for build in objects(config.value) {
            if let Some(entry) = build.get("entry").or_else(|| build.get("entryPoints")) {
                let paths = match entry {
                    ConfigValue::Object(_) => values_of(entry).into_iter().flat_map(strings).collect(),
                    other => strings(other),
                };
                out.extend(
                    paths
                        .into_iter()
                        .filter_map(|path| entry_glob(config.base, path))
                        .map(PluginInput::ProductionEntry),
                );
            }
            let key = if build.get("entry").is_some() {
                "entry"
            } else {
                "entryPoints"
            };
            if key_unknown(build, key) {
                out.push(PluginInput::EntriesIncomplete);
            }
        }
        out
    }
}
