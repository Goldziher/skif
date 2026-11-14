//! Web Test Runner: the `files` of `web-test-runner.config.*` and of each of its `groups` are test entries, relative to
//! the workspace directory, where the runner resolves them; a negated pattern removes files from the entries. The
//! test files a script passes to `wtr` or `web-test-runner` are read by the script parser.

use crate::plugin::{ConfigAst, Enabler, PORTED_KNIP_COMMIT, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{config_unknown, entry_pattern, entry_strings};

/// The `web-test-runner` plugin.
pub struct WebTestRunner;

static SPEC: PluginSpec = PluginSpec {
    id: "web-test-runner",
    enablers: &[Enabler::Dependency("@web/test-runner")],
    config: &["web-test-runner.config.{js,cjs,mjs}"],
    binaries: &["web-test-runner", "wtr"],
    knip_commit: PORTED_KNIP_COMMIT,
    ..PluginSpec::DEFAULT
};

impl Plugin for WebTestRunner {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        let mut patterns = entry_strings(config.value, "files", &mut out);
        if let Some(groups) = config.value.get("groups") {
            if config_unknown(groups) {
                out.push(PluginInput::EntriesIncomplete);
            }
            for group in groups.as_array().unwrap_or_default() {
                patterns.extend(entry_strings(group, "files", &mut out));
            }
        }
        for pattern in patterns {
            out.extend(entry_pattern("", pattern).map(PluginInput::Entry));
        }
        out
    }
}
