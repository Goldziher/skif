//! CSpell: imported dictionaries and configs.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{config_ref, strings};

/// The `cspell` plugin.
pub struct Cspell;

static SPEC: PluginSpec = PluginSpec {
    id: "cspell",
    enablers: &[Enabler::Dependency("cspell")],
    config: &[
        "{.,}cspell.config.{js,cjs,mjs,ts,cts,mts,json,jsonc,yaml,yml,toml}",
        "{.,}cspell.{jsonc,yaml,yml}",
        ".c{s,S}pell.json",
        "c{s,S}pell.json",
        ".vscode/c{s,S}pell.json",
        ".vscode/.cspell.json",
        ".config/{.,}cspell.config.{js,cjs,mjs,ts,cts,mts,json,jsonc,yaml,yml,toml}",
        ".config/.c{s,S}pell.json",
        ".config/c{s,S}pell.json",
        ".config/{.,}cspell.jsonc",
        ".config/cspell.{yaml,yml}",
    ],
    package_json_key: Some("cspell"),
    binaries: &["cspell"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Cspell {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        config
            .value
            .get("import")
            .map_or_default(strings)
            .into_iter()
            .filter_map(|name| config_ref(config.base, name))
            .collect()
    }
}
