//! release-it: its plugins are packages, its hooks and release notes commands are scripts.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{package_name, script_inputs, values_of};

/// The `release-it` plugin.
pub struct ReleaseIt;

static SPEC: PluginSpec = PluginSpec {
    id: "release-it",
    enablers: &[Enabler::Dependency("release-it")],
    config: &[".release-it.{json,jsonc,json5,js,mjs,cjs,ts,mts,cts,yml,yaml,toml}"],
    package_json_key: Some("release-it"),
    binaries: &["release-it"],
    ..PluginSpec::DEFAULT
};

impl Plugin for ReleaseIt {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        if let Some(ConfigValue::Object(plugins)) = config.value.get("plugins") {
            out.extend(
                plugins
                    .iter()
                    .filter_map(|(name, _)| package_name(name))
                    .map(PluginInput::Dependency),
            );
        }
        if let Some(hooks) = config.value.get("hooks") {
            out.extend(values_of(hooks).into_iter().flat_map(script_inputs));
        }
        for host in ["github", "gitlab"] {
            if let Some(notes) = config.value.get(host).and_then(|h| h.get("releaseNotes")) {
                out.extend(script_inputs(notes));
            }
        }
        out
    }
}
