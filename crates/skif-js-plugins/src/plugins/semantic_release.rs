//! semantic-release: shareable configs and plugins are packages; the plugins it bundles are left alone.

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{config_ref, module_input, name_of, strings};
use skif_js_syntax::ConfigValue;

/// The `semantic-release` plugin.
pub struct SemanticRelease;

static SPEC: PluginSpec = PluginSpec {
    id: "semantic-release",
    enablers: &[Enabler::Dependency("semantic-release")],
    root_only: true,
    config: &[
        ".releaserc",
        ".releaserc.{json,yaml,yml,js,ts,mjs,cjs}",
        "release.config.{js,ts,mjs,cjs}",
    ],
    package_json_key: Some("release"),
    binaries: &["semantic-release"],
    ..PluginSpec::DEFAULT
};

/// Plugins `semantic-release` itself depends on.
const BUNDLED: &[&str] = &[
    "@semantic-release/commit-analyzer",
    "@semantic-release/github",
    "@semantic-release/npm",
    "@semantic-release/release-notes-generator",
];

impl Plugin for SemanticRelease {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        if let Some(extends) = config.value.get("extends") {
            out.extend(
                strings(extends)
                    .into_iter()
                    .filter_map(|name| config_ref(config.base, name)),
            );
        }
        if let Some(ConfigValue::Array(plugins)) = config.value.get("plugins") {
            out.extend(
                plugins
                    .iter()
                    .filter_map(name_of)
                    .filter(|name| !BUNDLED.contains(name))
                    .filter_map(|name| module_input(config.base, name, false)),
            );
        }
        out
    }
}
