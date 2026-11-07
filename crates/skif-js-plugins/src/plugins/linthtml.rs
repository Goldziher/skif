//! `LintHTML`: the shared configs of `extends` and the `plugins` of its cosmiconfig file or `package.json#linthtmlConfig`.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{config_ref, module_input, strings};

/// The `linthtml` plugin.
pub struct Linthtml;

static SPEC: PluginSpec = PluginSpec {
    id: "linthtml",
    enablers: &[Enabler::Dependency("@linthtml/linthtml")],
    config: &[
        ".linthtmlrc",
        ".config/linthtmlrc",
        ".linthtmlrc.{json,yaml,yml,js,ts,cjs,mjs}",
        "linthtml.config.{js,ts,cjs,mjs}",
        ".config/linthtmlrc.{json,yaml,yml,js,ts,cjs,mjs}",
    ],
    package_json_key: Some("linthtmlConfig"),
    binaries: &["linthtml"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Linthtml {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        if let Some(extends) = config.value.get("extends") {
            out.extend(
                strings(extends)
                    .into_iter()
                    .filter_map(|id| config_ref(config.base, id)),
            );
        }
        if let Some(plugins) = config.value.get("plugins") {
            out.extend(
                strings(plugins)
                    .into_iter()
                    .filter_map(|id| module_input(config.base, id, false)),
            );
        }
        out
    }
}
