//! create-typescript-app: its config file is an entry.

use crate::plugin::{Enabler, Plugin, PluginSpec};

/// The `create-typescript-app` plugin.
pub struct CreateTypescriptApp;

static SPEC: PluginSpec = PluginSpec {
    id: "create-typescript-app",
    enablers: &[Enabler::Dependency("create-typescript-app")],
    entry: &["create-typescript-app.config.{js,cjs,mjs,ts}"],
    ..PluginSpec::DEFAULT
};

impl Plugin for CreateTypescriptApp {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn reads_config(&self) -> bool {
        false
    }
}
