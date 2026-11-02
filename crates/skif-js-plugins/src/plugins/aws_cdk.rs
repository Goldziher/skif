//! AWS CDK: the `app` command of `cdk.json` is a script whose file is the entry.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};

/// The `aws-cdk` plugin.
pub struct AwsCdk;

static SPEC: PluginSpec = PluginSpec {
    id: "aws-cdk",
    enablers: &[Enabler::Dependency("aws-cdk"), Enabler::Dependency("aws-cdk-lib")],
    config: &["cdk.json"],
    binaries: &["cdk"],
    ..PluginSpec::DEFAULT
};

impl Plugin for AwsCdk {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        config
            .value
            .get("app")
            .and_then(|app| app.as_str())
            .map(|command| PluginInput::Script(command.to_string()))
            .into_iter()
            .collect()
    }
}
