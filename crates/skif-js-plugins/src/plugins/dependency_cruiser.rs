//! dependency-cruiser: the config file is an entry file, and so is a config passed with `--config`.

use crate::plugin::{Enabler, Plugin, PluginSpec};

/// The `dependency-cruiser` plugin.
pub struct DependencyCruiser;

static SPEC: PluginSpec = PluginSpec {
    id: "dependency-cruiser",
    enablers: &[Enabler::Dependency("dependency-cruiser")],
    entry: &[".dependency-cruiser.{js,cjs,mjs,json}"],
    binaries: &[
        "depcruise",
        "dependency-cruise",
        "dependency-cruiser",
        "depcruise-baseline",
    ],
    ..PluginSpec::DEFAULT
};

impl Plugin for DependencyCruiser {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn reads_config(&self) -> bool {
        false
    }
}
