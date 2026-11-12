//! Lit: a class decorated with `@customElement('x-y')` from `lit/decorators.js` (or `@lit/reactive-element` and
//! `lit-element`) is a registered custom element and so a used export (ADR 0020).

use crate::plugin::{CustomElements, Enabler, Plugin, PluginSpec};

/// The `lit` plugin.
pub struct Lit;

static SPEC: PluginSpec = PluginSpec {
    id: "lit",
    enablers: &[
        Enabler::Dependency("lit"),
        Enabler::Dependency("lit-element"),
        Enabler::Dependency("@lit/reactive-element"),
    ],
    custom_elements: Some(CustomElements {
        modules: &[
            "lit/decorators.js",
            "lit/decorators",
            "lit/decorators/",
            "@lit/reactive-element/decorators.js",
            "@lit/reactive-element/decorators",
            "@lit/reactive-element/decorators/",
            "lit-element",
            "lit-element/decorators.js",
            "lit-element/decorators",
            "lit-element/decorators/",
        ],
        decorator: "customElement",
        base_class: None,
    }),
    ..PluginSpec::DEFAULT
};

impl Plugin for Lit {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn reads_config(&self) -> bool {
        false
    }
}
