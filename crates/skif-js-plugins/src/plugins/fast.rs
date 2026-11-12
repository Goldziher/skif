//! FAST: a class decorated with `@customElement` from `@microsoft/fast-element`, or a subclass of `FASTElement` that
//! calls its static `define(…)` or `defineAsync(…)`, is a registered custom element and so a used export (ADR 0020).

use crate::plugin::{CustomElements, Enabler, Plugin, PluginSpec};

/// The `fast` plugin.
pub struct Fast;

static SPEC: PluginSpec = PluginSpec {
    id: "fast",
    enablers: &[Enabler::Dependency("@microsoft/fast-element")],
    custom_elements: Some(CustomElements {
        modules: &["@microsoft/fast-element"],
        decorator: "customElement",
        base_class: Some("FASTElement"),
    }),
    ..PluginSpec::DEFAULT
};

impl Plugin for Fast {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn reads_config(&self) -> bool {
        false
    }
}
