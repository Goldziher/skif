//! kysely-codegen: its config file, or the one a script passes with `--config-file`, is an entry.

use crate::plugin::{Enabler, Plugin, PluginSpec};

/// The `kysely-codegen` plugin.
pub struct KyselyCodegen;

static SPEC: PluginSpec = PluginSpec {
    id: "kysely-codegen",
    enablers: &[Enabler::Dependency("kysely-codegen")],
    entry: &[
        ".kysely-codegenrc.{js,ts,cjs}",
        ".config/kysely-codegenrc.{js,ts,cjs}",
        "kysely-codegen.config.{js,ts,cjs}",
    ],
    binaries: &["kysely-codegen"],
    ..PluginSpec::DEFAULT
};

impl Plugin for KyselyCodegen {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn reads_config(&self) -> bool {
        false
    }
}
