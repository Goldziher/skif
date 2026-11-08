//! Borp: test files are entries; the `files` and reporters of `.borp.yaml`, and the files, `--pattern` and
//! `--reporter` a script passes to `borp`, are read.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::script_config::{flag_values, positionals};
use crate::support::{entry_glob, module_input, strings};

/// The `borp` plugin.
pub struct Borp;

static SPEC: PluginSpec = PluginSpec {
    id: "borp",
    enablers: &[Enabler::Dependency("borp")],
    config: &[".borp.yaml", ".borp.yml"],
    entry: &["**/*.test.{js,mjs,cjs,ts,mts,cts}"],
    binaries: &["borp"],
    ..PluginSpec::DEFAULT
};

const BUILTIN_REPORTERS: &[&str] = &["spec", "tap", "dot", "junit", "lcov", "gh"];

const BOOLEAN_FLAGS: &[&str] = &[
    "coverage",
    "C",
    "watch",
    "w",
    "only",
    "o",
    "no-timeout",
    "expose-gc",
    "check-coverage",
    "coverage-html",
    "quiet",
    "verbose",
];

/// A reporter that is not built in, without its `:destination`: a local module or a package.
fn reporter(name: &str) -> Option<PluginInput> {
    let name = name.split(':').next().unwrap_or_default();
    if BUILTIN_REPORTERS.contains(&name) {
        return None;
    }
    module_input("", name, false)
}

impl Plugin for Borp {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out: Vec<PluginInput> = config
            .value
            .get("reporters")
            .map(strings)
            .unwrap_or_default()
            .into_iter()
            .filter_map(reporter)
            .collect();
        if let Some(files) = config.value.get("files") {
            out.extend(
                strings(files)
                    .into_iter()
                    .filter_map(|pattern| entry_glob("", pattern))
                    .map(PluginInput::Entry),
            );
        }
        out
    }

    fn resolve_workspace(&self, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        for script in cx.scripts.values() {
            for name in flag_values(script, SPEC.binaries, &["-r", "--reporter"]) {
                out.extend(reporter(&name));
            }
            let patterns = flag_values(script, SPEC.binaries, &["-p", "--pattern"])
                .into_iter()
                .chain(positionals(script, SPEC.binaries, BOOLEAN_FLAGS).into_iter().flatten());
            out.extend(
                patterns
                    .filter(|pattern| !pattern.starts_with("..") && !pattern.contains('$'))
                    .filter_map(|pattern| entry_glob("", &pattern))
                    .map(PluginInput::Entry),
            );
        }
        out
    }
}
