//! Node.js: `server.js` is a production entry, and the test runner finds its files by convention.

use crate::plugin::{Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::scripts::{ScriptContext, parse_script};

/// The `node` plugin.
pub struct Node;

static SPEC: PluginSpec = PluginSpec {
    id: "node",
    enablers: &[Enabler::Always],
    production: &["server.js"],
    ..PluginSpec::DEFAULT
};

/// The files `node --test` runs when it is given no path.
const TEST_PATTERNS: &[&str] = &[
    "**/*{.,-,_}test.{cjs,mjs,js,cts,mts,ts}",
    "**/test-*.{cjs,mjs,js,cts,mts,ts}",
    "**/test.{cjs,mjs,js,cts,mts,ts}",
    "**/test/**/*.{cjs,mjs,js,cts,mts,ts}",
];

impl Plugin for Node {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn reads_config(&self) -> bool {
        false
    }

    fn resolve_workspace(&self, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut script_context = ScriptContext::new();
        script_context.scripts.clone_from(cx.scripts);
        let runs_tests = cx
            .scripts
            .values()
            .any(|script| script.contains("--test") && parse_script(script, &script_context).node_test);
        if runs_tests {
            TEST_PATTERNS
                .iter()
                .map(|pattern| PluginInput::Entry((*pattern).to_owned()))
                .collect()
        } else {
            Vec::new()
        }
    }
}
