//! `unplugin-icons`: `~icons/<collection>/<icon>` imports are modules the bundler plugin generates, and the iconify
//! collections it loads them from are packages it reads at build time.

use crate::plugin::{Enabler, Plugin, PluginContext, PluginInput, PluginSpec};

/// The `unplugin-icons` plugin.
pub struct UnpluginIcons;

static SPEC: PluginSpec = PluginSpec {
    id: "unplugin-icons",
    enablers: &[Enabler::Dependency("unplugin-icons")],
    ..PluginSpec::DEFAULT
};

impl Plugin for UnpluginIcons {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_workspace(&self, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        ["~icons/*", "@iconify-json/*", "@iconify/json"]
            .into_iter()
            .map(|pattern| PluginInput::VirtualModule(pattern.to_owned()))
            .collect()
    }
}
