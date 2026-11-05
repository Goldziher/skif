//! `tsd`: type test files, or the `testFiles` and `directory` of the `tsd` key. `tsd --files <glob>` and `tsd <dir>`
//! name test files on the command line.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::script_config::{flag_values, positionals};
use crate::support::{entry_glob, join_rel, strings};

/// The `tsd` plugin.
pub struct Tsd;

static SPEC: PluginSpec = PluginSpec {
    id: "tsd",
    enablers: &[Enabler::Dependency("tsd")],
    package_json_key: Some("tsd"),
    entry: &["*.test-d.{ts,tsx}", "test-d/**/*.test-d.{ts,tsx}"],
    binaries: &["tsd"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Tsd {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut patterns: Vec<String> = config
            .value
            .get("testFiles")
            .map(strings)
            .unwrap_or_default()
            .into_iter()
            .map(str::to_owned)
            .collect();
        if let Some(directory) = config
            .value
            .get("directory")
            .and_then(skif_js_syntax::ConfigValue::as_str)
        {
            patterns.push(join_rel(directory, "**/*.test-d.{ts,tsx}"));
        }
        patterns
            .iter()
            .filter_map(|pattern| entry_glob(config.base, pattern))
            .map(PluginInput::Entry)
            .collect()
    }

    fn resolve_workspace(&self, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        for script in cx.scripts.values() {
            out.extend(
                flag_values(script, SPEC.binaries, &["--files", "-f"])
                    .iter()
                    .filter_map(|pattern| entry_glob("", pattern))
                    .map(PluginInput::Entry),
            );
            let directories = positionals(script, SPEC.binaries, &[])
                .into_iter()
                .filter_map(|words| words.into_iter().next());
            out.extend(
                directories
                    .filter_map(|directory| entry_glob("", &join_rel(&directory, "**/*.test-d.{ts,tsx}")))
                    .map(PluginInput::Entry),
            );
        }
        out
    }
}
