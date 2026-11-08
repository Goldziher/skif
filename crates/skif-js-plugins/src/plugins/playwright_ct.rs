//! Playwright component testing: what the `playwright` plugin reads from `playwright-ct.config.*`, plus the
//! `playwright/index.*` file that sets up the component page.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::plugins::playwright::{Playwright, TEST_FILES};

/// The `playwright-ct` plugin.
pub struct PlaywrightCt;

const CT_ENTRY: &str = "playwright/index.{js,ts,jsx,tsx}";

static SPEC: PluginSpec = PluginSpec {
    id: "playwright-ct",
    enablers: &[Enabler::DependencyPrefix("@playwright/experimental-ct-")],
    config: &["playwright-ct.config.{js,ts}"],
    entry: &[TEST_FILES, CT_ENTRY],
    ..PluginSpec::DEFAULT
};

impl Plugin for PlaywrightCt {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Playwright.resolve_config(config, cx);
        out.push(PluginInput::Entry(config.join(CT_ENTRY)));
        out
    }
}
