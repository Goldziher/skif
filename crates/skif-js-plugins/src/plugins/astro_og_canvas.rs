//! astro-og-canvas: it loads `canvaskit-wasm` at run time, which may be absent.

use crate::plugin::{Enabler, Plugin, PluginContext, PluginInput, PluginSpec};

/// The `astro-og-canvas` plugin.
pub struct AstroOgCanvas;

static SPEC: PluginSpec = PluginSpec {
    id: "astro-og-canvas",
    enablers: &[Enabler::Dependency("astro-og-canvas")],
    ..PluginSpec::DEFAULT
};

impl Plugin for AstroOgCanvas {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn reads_config(&self) -> bool {
        false
    }

    fn resolve_workspace(&self, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        vec![PluginInput::OptionalDependency("canvaskit-wasm".to_owned())]
    }
}
