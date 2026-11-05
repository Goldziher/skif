//! swc: experimental plugins are packages and `externalHelpers` needs `@swc/helpers`.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{get_in, name_of, objects, package_name, values_of};
use skif_js_syntax::ConfigValue;

/// The `swc` plugin.
pub struct Swc;

static SPEC: PluginSpec = PluginSpec {
    id: "swc",
    enablers: &[Enabler::Dependency("@swc/core")],
    config: &[".swcrc"],
    binaries: &["swc"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Swc {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        for item in objects(config.value) {
            out.extend(plugin_packages(item).into_iter().map(PluginInput::Dependency));
            if get_in(item, &["jsc", "externalHelpers"]).and_then(skif_js_syntax::ConfigValue::as_bool) == Some(true) {
                out.push(PluginInput::Dependency("@swc/helpers".to_owned()));
            }
        }
        out
    }
}

/// The packages of the `jsc.experimental.plugins` of an swc options object (a name or `[name, options]`), which swc
/// loads, so that a bundler that runs swc (`rsbuild`'s `tools.swc`, an `swc-loader` rule) needs them too.
pub(crate) fn plugin_packages(options: &ConfigValue) -> Vec<String> {
    get_in(options, &["jsc", "experimental", "plugins"])
        .map(|plugins| {
            values_of(plugins)
                .into_iter()
                .filter_map(name_of)
                .filter_map(package_name)
                .collect()
        })
        .unwrap_or_default()
}
