//! lockfile-lint: its cosmiconfig file is an entry.

use crate::plugin::{Enabler, Plugin, PluginSpec};

/// The `lockfile-lint` plugin.
pub struct LockfileLint;

static SPEC: PluginSpec = PluginSpec {
    id: "lockfile-lint",
    enablers: &[Enabler::Dependency("lockfile-lint")],
    entry: &[
        ".lockfile-lintrc",
        ".config/lockfile-lintrc",
        ".lockfile-lintrc.{json,yaml,yml,js,ts,cjs,mjs,toml}",
        "lockfile-lint.config.{js,ts,cjs,mjs}",
        ".config/lockfile-lintrc.{json,yaml,yml,js,ts,cjs,mjs,toml}",
    ],
    binaries: &["lockfile-lint"],
    ..PluginSpec::DEFAULT
};

impl Plugin for LockfileLint {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn reads_config(&self) -> bool {
        false
    }
}
