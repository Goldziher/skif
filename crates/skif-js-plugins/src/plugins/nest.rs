//! Nest: the schematics collection of `nest-cli.json` is a package.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::package_name;

/// The `nest` plugin.
pub struct Nest;

static SPEC: PluginSpec = PluginSpec {
    id: "nest",
    enablers: &[Enabler::DependencyPrefix("@nestjs/")],
    config: &["nest-cli.json", ".nestcli.json", ".nest-cli.json", "nest.json"],
    binaries: &["nest"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Nest {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        config
            .value
            .get("collection")
            .and_then(ConfigValue::as_str)
            .and_then(package_name)
            .map(PluginInput::Dependency)
            .into_iter()
            .collect()
    }
}
