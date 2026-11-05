//! unbuild: the `input` of each build entry is an entry file.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{entry_glob, objects};

/// The `unbuild` plugin.
pub struct Unbuild;

static SPEC: PluginSpec = PluginSpec {
    id: "unbuild",
    enablers: &[Enabler::Dependency("unbuild")],
    config: &["build.config.{js,cjs,mjs,ts,mts,cts,json}"],
    binaries: &["unbuild"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Unbuild {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        objects(config.value)
            .into_iter()
            .filter_map(|build| build.get("entries").and_then(ConfigValue::as_array))
            .flatten()
            .filter_map(|entry| match entry {
                ConfigValue::String(path) => Some(path.as_str()),
                other => other.get("input").and_then(ConfigValue::as_str),
            })
            .filter_map(|path| entry_glob(config.base, path))
            .map(PluginInput::Entry)
            .collect()
    }
}
