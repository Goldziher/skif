//! openapi-ts (`@hey-api/openapi-ts`): its c12 config file is an entry.

use crate::plugin::{Enabler, Plugin, PluginSpec};

/// The `openapi-ts` plugin.
pub struct OpenapiTs;

static SPEC: PluginSpec = PluginSpec {
    id: "openapi-ts",
    enablers: &[Enabler::Dependency("@hey-api/openapi-ts")],
    entry: &[
        ".openapi-tsrc",
        ".config/openapi-tsrc",
        ".openapi-tsrc.{json,jsonc,json5,yaml,yml,js,ts,mjs,cjs,mts,cts,toml}",
        "openapi-ts.config.{js,ts,mjs,cjs,mts,cts}",
        ".config/openapi-tsrc.{json,jsonc,json5,yaml,yml,js,ts,mjs,cjs,mts,cts,toml}",
    ],
    binaries: &["openapi-ts"],
    ..PluginSpec::DEFAULT
};

impl Plugin for OpenapiTs {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn reads_config(&self) -> bool {
        false
    }
}
