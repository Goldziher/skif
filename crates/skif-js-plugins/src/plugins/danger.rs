//! Danger: the dangerfile is an entry.

use crate::plugin::{Enabler, Plugin, PluginSpec};

/// The `danger` plugin.
pub struct Danger;

static SPEC: PluginSpec = PluginSpec {
    id: "danger",
    enablers: &[Enabler::Dependency("danger")],
    entry: &["dangerfile.{js,cjs,mjs,ts}"],
    binaries: &["danger"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Danger {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn reads_config(&self) -> bool {
        false
    }
}
