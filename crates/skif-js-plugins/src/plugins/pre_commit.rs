//! pre-commit: the hook runner installs itself and is never imported, so it counts as used.

use crate::plugin::{Enabler, Plugin, PluginContext, PluginInput, PluginSpec};

/// The `pre-commit` plugin.
pub struct PreCommit;

const PACKAGES: &[&str] = &["pre-commit", "@fastify/pre-commit"];

static SPEC: PluginSpec = PluginSpec {
    id: "pre-commit",
    enablers: &[
        Enabler::Dependency("pre-commit"),
        Enabler::Dependency("@fastify/pre-commit"),
    ],
    root_only: true,
    ..PluginSpec::DEFAULT
};

impl Plugin for PreCommit {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn reads_config(&self) -> bool {
        false
    }

    fn resolve_workspace(&self, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        PACKAGES
            .iter()
            .filter(|name| cx.has_dependency(name))
            .map(|name| PluginInput::Dependency((*name).to_owned()))
            .collect()
    }
}
