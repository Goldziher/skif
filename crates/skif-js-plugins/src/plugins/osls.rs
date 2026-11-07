//! osls, the open-source fork of the Serverless Framework: its `serverless.*` config is read as the
//! `serverless-framework` plugin reads it.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::plugins::serverless_framework::ServerlessFramework;

/// The `osls` plugin.
pub struct Osls;

static SPEC: PluginSpec = PluginSpec {
    id: "osls",
    enablers: &[Enabler::Dependency("osls")],
    config: &["serverless.{js,cjs,mjs,ts,cts,mts,yml,yaml}"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Osls {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        ServerlessFramework.resolve_config(config, cx)
    }
}
