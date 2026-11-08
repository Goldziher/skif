//! orval: the mutators, hooks and transformers named by a `path` or `transformer` property anywhere in
//! `orval.config.*` are entries.

use skif_js_syntax::extract_property_values;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::leaf_strings;

/// The `orval` plugin.
pub struct Orval;

static SPEC: PluginSpec = PluginSpec {
    id: "orval",
    enablers: &[Enabler::Dependency("orval")],
    config: &["orval.config.{js,mjs,ts,mts}"],
    binaries: &["orval"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Orval {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    /// The options are read wherever they are written in the source.
    fn reads_config(&self) -> bool {
        false
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        for key in ["path", "transformer"] {
            for value in extract_property_values(config.source, config.path, key) {
                for path in leaf_strings(&value) {
                    out.push(PluginInput::Entry(config.join(path)));
                }
            }
        }
        out
    }
}
