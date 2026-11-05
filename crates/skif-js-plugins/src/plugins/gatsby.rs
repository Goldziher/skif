//! Gatsby: browser, SSR, API, page and template files are production entries; plugins are packages.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};

/// The `gatsby` plugin.
pub struct Gatsby;

static SPEC: PluginSpec = PluginSpec {
    id: "gatsby",
    enablers: &[Enabler::Dependency("gatsby"), Enabler::Dependency("gatsby-cli")],
    config: &[
        "gatsby-{config,node}.{js,jsx,ts,tsx}",
        "plugins/**/gatsby-node.{js,jsx,ts,tsx}",
    ],
    production: &[
        "gatsby-{browser,ssr}.{js,jsx,ts,tsx}",
        "src/api/**/*.{js,ts}",
        "src/pages/**/*.{js,jsx,ts,tsx}",
        "src/templates/**/*.{js,jsx,ts,tsx}",
        "src/html.{js,jsx,ts,tsx}",
        "plugins/**/gatsby-{browser,ssr}.{js,jsx,ts,tsx}",
    ],
    binaries: &["gatsby"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Gatsby {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        if !config.file_name().starts_with("gatsby-config") {
            return Vec::new();
        }
        config
            .value
            .get("plugins")
            .and_then(ConfigValue::as_array)
            .into_iter()
            .flatten()
            .filter_map(|plugin| match plugin {
                ConfigValue::String(name) => Some(name.as_str()),
                other => other.get("resolve").and_then(ConfigValue::as_str),
            })
            .filter_map(crate::support::package_name)
            .map(PluginInput::OptionalDependency)
            .collect()
    }
}
