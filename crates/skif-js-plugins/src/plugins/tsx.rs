//! tsx: its binary, and the test files of scripts that run `tsx --test`.

use crate::plugin::{Enabler, Plugin, PluginContext, PluginInput, PluginSpec};

/// The `tsx` plugin.
pub struct Tsx;

static SPEC: PluginSpec = PluginSpec {
    id: "tsx",
    enablers: &[Enabler::Dependency("tsx")],
    binaries: &["tsx"],
    ..PluginSpec::DEFAULT
};

const TEST_FILES: &[&str] = &[
    "**/*{.,-,_}test.{c,m,}{j,t}s",
    "**/test-*.{c,m,}{j,t}s",
    "**/test.{c,m,}{j,t}s",
    "**/test/**/*.{c,m,}{j,t}s",
];

impl Plugin for Tsx {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn reads_config(&self) -> bool {
        false
    }

    fn resolve_workspace(&self, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let tests = cx.scripts.values().any(|script| {
            let words: Vec<&str> = script.split_whitespace().collect();
            words
                .iter()
                .position(|word| *word == "tsx")
                .is_some_and(|at| words[at..].contains(&"--test"))
        });
        if !tests {
            return Vec::new();
        }
        TEST_FILES
            .iter()
            .map(|pattern| PluginInput::Entry((*pattern).to_owned()))
            .collect()
    }
}
