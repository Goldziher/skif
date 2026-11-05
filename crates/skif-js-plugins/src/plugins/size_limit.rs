//! size-limit: the config files are entry files and every `@size-limit/*` preset the workspace depends on is used.
//!
//! A preset of any dependency kind counts; knip reads `dependencies` and `devDependencies` only. The entry patterns
//! are a superset of knip's (`size-limit.{json,…}` next to `size-limit.config.*`).

use crate::plugin::{Enabler, Plugin, PluginContext, PluginInput, PluginSpec};

/// The `size-limit` plugin.
pub struct SizeLimit;

static SPEC: PluginSpec = PluginSpec {
    id: "size-limit",
    enablers: &[Enabler::Dependency("size-limit")],
    entry: &[
        "size-limit.{json,js,cjs,mjs,ts,mts,cts}",
        "size-limit.config.{js,cjs,mjs,ts,mts,cts}",
        ".size-limit.{json,js,cjs,mjs,ts,mts,cts}",
        ".size-limit",
    ],
    binaries: &["size-limit"],
    ..PluginSpec::DEFAULT
};

impl Plugin for SizeLimit {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_workspace(&self, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        cx.dependencies
            .iter()
            .filter(|name| name.starts_with("@size-limit/"))
            .cloned()
            .map(PluginInput::Dependency)
            .collect()
    }
}
