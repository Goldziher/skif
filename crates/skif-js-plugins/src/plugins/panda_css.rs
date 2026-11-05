//! `PandaCSS`: presets are packages and the generator needs `postcss`.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{package_name, strings};

/// The `panda-css` plugin.
pub struct PandaCss;

static SPEC: PluginSpec = PluginSpec {
    id: "panda-css",
    enablers: &[Enabler::Dependency("@pandacss/dev")],
    config: &["panda.config.{ts,js,mjs,cjs,mts,cts}"],
    binaries: &["panda"],
    ..PluginSpec::DEFAULT
};

impl Plugin for PandaCss {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let presets = config.value.get("presets").map(strings).unwrap_or_default();
        presets
            .into_iter()
            .filter_map(package_name)
            .map(PluginInput::OptionalDependency)
            .chain([PluginInput::Dependency("postcss".to_owned())])
            .collect()
    }
}
