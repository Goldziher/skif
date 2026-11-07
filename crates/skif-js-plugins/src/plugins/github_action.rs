//! GitHub Action: a package that is a JavaScript action runs the files its `action.yml` names. Files under `dist/` or
//! `build/` are taken to be build output and are not entries.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::plugins::github_actions::node_action_files;

/// The `github-action` plugin.
pub struct GithubAction;

static SPEC: PluginSpec = PluginSpec {
    id: "github-action",
    enablers: &[Enabler::Dependency("@actions/core")],
    config: &["action.{yml,yaml}"],
    ..PluginSpec::DEFAULT
};

impl Plugin for GithubAction {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        node_action_files(config.value)
            .into_iter()
            .map(|file| config.join(file))
            .filter(|file| !file.starts_with("dist/") && !file.starts_with("build/"))
            .map(PluginInput::Entry)
            .collect()
    }
}
