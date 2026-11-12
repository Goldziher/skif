//! Gatsby: browser, SSR, API, page and template files are production entries; plugins are packages.
//!
//! The babel plugins `gatsby-node` hands to `actions.setBabelPlugin({ name })` in `onCreateBabelConfig` are packages.
//! knip calls the hook; skif reads the text of the file for `setBabelPlugin` calls with a literal `name`.

use skif_js_syntax::ConfigValue;

use crate::files::find_files;
use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::literal_property;

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
        if !config.file_name().contains("gatsby-config") {
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

    /// `gatsby-node` files export hooks rather than a config object, so they are read as text.
    fn resolve_workspace(&self, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let patterns = ["gatsby-node.{js,jsx,ts,tsx}", "plugins/**/gatsby-node.{js,jsx,ts,tsx}"].map(str::to_owned);
        find_files(cx.fs, cx.globs, cx.dir, &patterns)
            .iter()
            .filter_map(|file| cx.fs.read_to_string(&cx.dir.join(file)).ok())
            .flat_map(|source| babel_plugins(&source))
            .collect()
    }
}

/// The `name` of every `setBabelPlugin({ name: "…" })` call in a `gatsby-node` file.
fn babel_plugins(source: &str) -> Vec<PluginInput> {
    source
        .match_indices("setBabelPlugin(")
        .filter_map(|(at, call)| literal_property(source[at + call.len()..].trim_start(), "name"))
        .filter_map(crate::support::package_name)
        .map(PluginInput::Dependency)
        .collect()
}
