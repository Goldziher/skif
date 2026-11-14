//! Evalite: the `*.eval.ts` and `*.eval.mts` files are entries, and the `setupFiles` of `evalite.config.*` are modules
//! (a local file or a package such as `dotenv/config`).

use crate::plugin::{ConfigAst, Enabler, PORTED_KNIP_COMMIT, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{entry_strings, module_input};

/// The `evalite` plugin.
pub struct Evalite;

static SPEC: PluginSpec = PluginSpec {
    id: "evalite",
    enablers: &[Enabler::Dependency("evalite")],
    config: &["evalite.config.{ts,mts,js,mjs}"],
    entry: &["**/*.eval.{ts,mts}"],
    binaries: &["evalite"],
    knip_commit: PORTED_KNIP_COMMIT,
    ..PluginSpec::DEFAULT
};

impl Plugin for Evalite {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        for specifier in entry_strings(config.value, "setupFiles", &mut out) {
            out.extend(module_input(config.base, specifier, true));
        }
        out
    }
}
