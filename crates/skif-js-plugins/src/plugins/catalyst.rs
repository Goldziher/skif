//! Catalyst: a class decorated with `@controller` from `@github/catalyst` is a registered custom element and so a
//! used export (ADR 0062).

use crate::plugin::{CustomElements, Enabler, Plugin, PluginSpec};

/// The `catalyst` plugin.
pub struct Catalyst;

static SPEC: PluginSpec = PluginSpec {
    id: "catalyst",
    enablers: &[Enabler::Dependency("@github/catalyst")],
    custom_elements: Some(CustomElements {
        modules: &["@github/catalyst"],
        decorator: "controller",
        base_class: None,
    }),
    ..PluginSpec::DEFAULT
};

impl Plugin for Catalyst {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn reads_config(&self) -> bool {
        false
    }
}
