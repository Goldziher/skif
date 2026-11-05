//! `Sentry`: the client, server and edge configs of the framework SDKs are production entries.

use crate::plugin::{Enabler, Plugin, PluginSpec};

/// The `sentry` plugin.
pub struct Sentry;

static SPEC: PluginSpec = PluginSpec {
    id: "sentry",
    enablers: &[Enabler::DependencyPrefix("@sentry/")],
    production: &["sentry.{client,server,edge}.config.{js,ts}"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Sentry {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }
}
