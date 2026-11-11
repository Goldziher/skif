//! Cypress: spec and support files of the e2e and component configs.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{entry_glob, strings};

/// The `cypress` plugin.
pub struct Cypress;

/// The plugin's identifier.
pub const ID: crate::plugin::PluginId = "cypress";

static SPEC: PluginSpec = PluginSpec {
    id: ID,
    enablers: &[Enabler::Dependency("cypress")],
    config: &["cypress.config.{js,ts,mjs,cjs}"],
    entry: &[
        "cypress/e2e/**/*.cy.{js,jsx,ts,tsx}",
        "cypress/support/e2e.{js,jsx,ts,tsx}",
        "cypress/support/commands.{js,ts}",
        "cypress/support/component.{js,ts}",
        "cypress/plugins/index.js",
    ],
    binaries: &["cypress"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Cypress {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        ["e2e", "component"]
            .into_iter()
            .filter_map(|kind| config.value.get(kind))
            .flat_map(|section| {
                ["specPattern", "supportFile"]
                    .into_iter()
                    .filter_map(move |key| section.get(key))
                    .flat_map(strings)
            })
            .filter_map(|pattern| entry_glob(config.base, pattern))
            .map(PluginInput::Entry)
            .collect()
    }
}
