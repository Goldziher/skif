//! TanStack Router and Start: the generated route tree and the router and start modules.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use skif_js_syntax::ConfigValue;

/// The `tanstack-router` plugin.
pub struct TanstackRouter;

static SPEC: PluginSpec = PluginSpec {
    id: "tanstack-router",
    enablers: &[
        Enabler::Dependency("@tanstack/react-router"),
        Enabler::Dependency("@tanstack/solid-router"),
        Enabler::Dependency("@tanstack/vue-router"),
        Enabler::Dependency("@tanstack/svelte-router"),
        Enabler::Dependency("@tanstack/router-cli"),
        Enabler::Dependency("@tanstack/router-plugin"),
        Enabler::Dependency("@tanstack/react-start"),
        Enabler::Dependency("@tanstack/solid-start"),
    ],
    config: &["tsr.config.json"],
    production: &[
        "src/routeTree.gen.{ts,js}",
        "src/{router,start,client,server}.{js,jsx,ts,tsx}",
    ],
    ..PluginSpec::DEFAULT
};

impl Plugin for TanstackRouter {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    /// `generatedRouteTree` of `tsr.config.json` moves the route tree.
    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        if let Some(tree) = config.value.get("generatedRouteTree").and_then(ConfigValue::as_str) {
            out.push(PluginInput::ProductionEntry(config.join(tree)));
        }
        out
    }
}
