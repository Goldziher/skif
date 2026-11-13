//! `Jasmine`: spec and helper files resolve against `spec_dir`, `requires` and `loader` are modules.
//!
//! A negated pattern (`!**/skip.js`) stays a negated entry, as in knip. The command line is read as knip reads it:
//! every positional argument of `jasmine` is an entry, except the commands `init`, `examples`, `help` and `version`
//! and `name=value` words, and the values of `--require` and `--reporter` are modules.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::script_config::{flag_values, positionals};
use crate::support::{entry_pattern, join_rel, module_input, strings};

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

const COMMANDS: &[&str] = &["init", "examples", "help", "version"];

const BOOLEAN_FLAGS: &[&str] = &["color", "fail-fast", "list-not-applicable", "no-color"];

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
            .filter_map(|pattern| {
                let joined = match pattern.strip_prefix('!') {
                    Some(rest) => format!("!{}", join_rel(spec_dir, rest)),
                    None => join_rel(spec_dir, pattern),
                };
                entry_pattern("", &joined)
            })
            .map(PluginInput::Entry)
            .collect();
        let requires = config.value.get("requires").map_or_default(strings);
        let loader = config.value.get("loader").map_or_default(strings);
        out.extend(
            requires
                .into_iter()
                .chain(loader)
                .filter_map(|module| module_input("", module, true)),
        );
        out
    }

    fn resolve_workspace(&self, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        for script in cx.scripts.values() {
            for command in positionals(script, SPEC.binaries, BOOLEAN_FLAGS) {
                out.extend(
                    command
                        .iter()
                        .filter(|word| !COMMANDS.contains(&word.as_str()) && !is_assignment(word))
                        .filter_map(|word| entry_pattern("", word))
                        .map(PluginInput::Entry),
                );
            }
            out.extend(
                flag_values(script, SPEC.binaries, &["--require", "--reporter"])
                    .iter()
                    .filter_map(|module| module_input("", module, true)),
            );
        }
        out
    }
}

/// `name=value`, which `jasmine` reads as an environment setting.
fn is_assignment(word: &str) -> bool {
    word.split_once('=')
        .is_some_and(|(name, _)| !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_'))
}
