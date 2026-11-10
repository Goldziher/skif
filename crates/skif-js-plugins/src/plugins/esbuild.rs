//! esbuild: the `entryPoints` of a build config, or of the builds a build script runs, are production entries.

use std::path::Path;

use skif_js_syntax::{ConfigValue, extract_property_values};

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::plugins::tsup::entry_inputs;

/// The `esbuild` plugin.
pub struct Esbuild;

static SPEC: PluginSpec = PluginSpec {
    id: "esbuild",
    enablers: &[Enabler::Dependency("esbuild")],
    config: &[
        "esbuild.config.{js,mjs,cjs,ts,mts,cts}",
        "esbuild.{js,mjs,cjs,ts,mts,cts}",
    ],
    binaries: &["esbuild"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Esbuild {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        entry_inputs(config, &["entryPoints"])
    }

    /// esbuild has no config file format: `esbuild.config.mjs` is a script that calls `build({ ... })`, often through
    /// a shared helper (`run({ entryPoints, ... })`). Each `entryPoints` it writes, wherever it is, is read as one
    /// build, evaluated against the script's top-level bindings; one it computes leaves the entries incomplete. A
    /// script that writes none stays unparsable.
    fn read_script(&self, path: &Path, source: &str) -> Option<ConfigValue> {
        let builds: Vec<ConfigValue> = extract_property_values(source, path, "entryPoints")
            .into_iter()
            .map(|entry_points| ConfigValue::Object(vec![("entryPoints".to_owned(), entry_points)]))
            .collect();
        (!builds.is_empty()).then_some(ConfigValue::Array(builds))
    }
}
