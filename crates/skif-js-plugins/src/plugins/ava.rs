//! `AVA`: the `files` of the config (negated patterns stay negated), the modules it requires, and its
//! `nodeArguments`, which are read like the arguments of a `node` command (`--import=tsx`, `--loader=ts-node/esm`).

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{entry_pattern, module_input, strings};

/// The `ava` plugin.
pub struct Ava;

static SPEC: PluginSpec = PluginSpec {
    id: "ava",
    enablers: &[Enabler::Dependency("ava")],
    config: &["ava.config.{js,cjs,mjs}"],
    package_json_key: Some("ava"),
    entry: &[
        "test.{js,cjs,mjs,ts}",
        "{src,source}/test.{js,cjs,mjs,ts}",
        "**/__tests__/**/*.{js,cjs,mjs,ts}",
        "**/*.spec.{js,cjs,mjs,ts}",
        "**/*.test.{js,cjs,mjs,ts}",
        "**/test-*.{js,cjs,mjs,ts}",
        "**/test/**/*.{js,cjs,mjs,ts}",
        "**/tests/**/*.{js,cjs,mjs,ts}",
        "!**/__tests__/**/__{helper,fixture}?(s)__/**/*",
        "!**/test?(s)/**/{helper,fixture}?(s)/**/*",
    ],
    binaries: &["ava"],
    optional_binaries: true,
    ..PluginSpec::DEFAULT
};

impl Plugin for Ava {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out: Vec<PluginInput> = config
            .value
            .get("files")
            .map(strings)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|pattern| entry_pattern(config.base, pattern))
            .map(PluginInput::Entry)
            .collect();
        out.extend(
            config
                .value
                .get("require")
                .map(strings)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|module| module_input(config.base, module, true)),
        );
        let node_arguments = config.value.get("nodeArguments").map(strings).unwrap_or_default();
        if !node_arguments.is_empty() {
            out.push(PluginInput::Script(format!("node {}", node_arguments.join(" "))));
        }
        out
    }
}
