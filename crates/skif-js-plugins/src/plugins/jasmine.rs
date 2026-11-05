//! `Jasmine`: spec and helper files resolve against `spec_dir`, `requires` and `loader` are modules.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{entry_glob, join_rel, module_input, strings};

/// The `jasmine` plugin.
pub struct Jasmine;

static SPEC: PluginSpec = PluginSpec {
    id: "jasmine",
    enablers: &[Enabler::Dependency("jasmine")],
    config: &["spec/support/jasmine.{json,jsonc,js,cjs,mjs}"],
    entry: &["spec/**/*[sS]pec.{js,mjs}", "spec/helpers/**/*.{js,mjs}"],
    binaries: &["jasmine"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Jasmine {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    /// The config sits in `spec/support`, but its paths and modules are relative to the working directory, the
    /// workspace.
    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let spec_dir = config
            .value
            .get("spec_dir")
            .and_then(skif_js_syntax::ConfigValue::as_str)
            .unwrap_or("");
        let mut out: Vec<PluginInput> = ["spec_files", "helpers"]
            .iter()
            .filter_map(|key| config.value.get(key))
            .flat_map(strings)
            .filter(|pattern| !pattern.starts_with('!'))
            .filter_map(|pattern| entry_glob("", &join_rel(spec_dir, pattern)))
            .map(PluginInput::Entry)
            .collect();
        let requires = config.value.get("requires").map(strings).unwrap_or_default();
        let loader = config.value.get("loader").map(strings).unwrap_or_default();
        out.extend(
            requires
                .into_iter()
                .chain(loader)
                .filter_map(|module| module_input("", module, true)),
        );
        out
    }
}
