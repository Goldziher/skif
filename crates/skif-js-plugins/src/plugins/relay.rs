//! Relay: the generated artifacts, in each project's `artifactDirectory` or in `__generated__` directories next to
//! the sources, are production entries.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};

/// The `relay` plugin.
pub struct Relay;

static SPEC: PluginSpec = PluginSpec {
    id: "relay",
    enablers: &[
        Enabler::Dependency("vite-plugin-relay"),
        Enabler::Dependency("@swc/plugin-relay"),
        Enabler::Dependency("babel-plugin-relay"),
    ],
    config: &["relay.config.json", "relay.config.js"],
    package_json_key: Some("relay"),
    binaries: &["relay-compiler"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Relay {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let projects: Vec<&ConfigValue> = match config.value.get("projects") {
            Some(ConfigValue::Object(projects)) => projects.iter().map(|(_, project)| project).collect(),
            _ => vec![config.value],
        };
        projects
            .into_iter()
            .map(
                |project| match project.get("artifactDirectory").and_then(ConfigValue::as_str) {
                    Some(dir) => PluginInput::ProductionEntry(config.join(&format!("{dir}/**"))),
                    None => PluginInput::ProductionEntry(config.join("**/__generated__/*")),
                },
            )
            .collect()
    }
}
