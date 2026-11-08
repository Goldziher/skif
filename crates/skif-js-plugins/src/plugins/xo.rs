//! XO: the config files are entries (a flat `xo.config.*` imports what it uses), and a legacy config in `package.json`
//! or `.xo-config*` names shared configs, plugins and parsers as an `ESLint` config does.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::plugins::eslint;

/// The `xo` plugin.
pub struct Xo;

static SPEC: PluginSpec = PluginSpec {
    id: "xo",
    enablers: &[Enabler::Dependency("xo")],
    config: &[
        ".xo-config",
        ".xo-config.{js,cjs,json}",
        "xo.config.{js,cjs,mjs,ts,cts,mts}",
    ],
    entry: &[".xo-config.{js,cjs}", "xo.config.{js,cjs,mjs,ts,cts,mts}"],
    package_json_key: Some("xo"),
    binaries: &["xo"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Xo {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        eslint::collect(config.value, config, &mut out, 0);
        out
    }
}
