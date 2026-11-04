//! Yarn Berry: the plugins, `yarnPath` and constraints file of `.yarnrc.yml`.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::plugins::pnpm::peer_dependencies;
use crate::support::join_rel;

/// The `yarn` plugin.
pub struct Yarn;

static SPEC: PluginSpec = PluginSpec {
    id: "yarn",
    enablers: &[Enabler::File(".yarnrc.yml")],
    root_only: true,
    config: &[".yarnrc.yml"],
    entry: &["yarn.config.cjs"],
    binaries: &["yarn", "yarnpkg"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Yarn {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        if let Some(ConfigValue::Array(plugins)) = config.value.get("plugins") {
            out.extend(
                plugins
                    .iter()
                    .filter_map(|plugin| match plugin {
                        ConfigValue::String(path) => Some(path.as_str()),
                        other => other.get("path").and_then(ConfigValue::as_str),
                    })
                    .map(|path| PluginInput::Entry(join_rel(config.base, path))),
            );
        }
        if let Some(path) = config.value.get("yarnPath").and_then(ConfigValue::as_str) {
            out.push(PluginInput::Entry(join_rel(config.base, path)));
        }
        out.extend(peer_dependencies(config.value.get("packageExtensions")));
        out
    }
}
