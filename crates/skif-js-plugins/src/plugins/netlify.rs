//! Netlify: function files are production entries, build plugins and bundler externals are packages.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{join_rel, production_entries, strings, values_of};

/// The `netlify` plugin.
pub struct Netlify;

const FUNCTIONS_DIRECTORY: &str = "netlify/functions";
const FUNCTIONS_EXTENSIONS: &str = "js,mjs,cjs,ts,mts,cts";

static SPEC: PluginSpec = PluginSpec {
    id: "netlify",
    enablers: &[
        Enabler::DependencyPrefix("@netlify/plugin-"),
        Enabler::Dependency("netlify-cli"),
        Enabler::Dependency("@netlify/functions"),
    ],
    config: &["netlify.toml"],
    production: &["netlify/functions/**/*.{js,mjs,cjs,ts,mts,cts}"],
    binaries: &["netlify"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Netlify {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let functions = config.value.get("functions");
        let directory = functions
            .and_then(|f| f.get("directory"))
            .and_then(ConfigValue::as_str)
            .unwrap_or(FUNCTIONS_DIRECTORY);
        let mut files = vec![join_rel(directory, &format!("**/*.{{{FUNCTIONS_EXTENSIONS}}}"))];
        files.extend(function_settings(functions, "included_files"));
        let mut out = production_entries(config.base, files.iter().map(String::as_str));
        let plugins = config.value.get("plugins").and_then(ConfigValue::as_array);
        out.extend(
            plugins
                .into_iter()
                .flatten()
                .filter_map(|plugin| plugin.get("package").and_then(ConfigValue::as_str))
                .filter_map(crate::support::package_name)
                .map(PluginInput::Dependency),
        );
        out.extend(
            function_settings(functions, "external_node_modules")
                .iter()
                .filter_map(|name| crate::support::package_name(name))
                .map(PluginInput::Dependency),
        );
        out
    }
}

/// A setting of the `[functions]` table and of each `[functions."<name>"]` table below it.
fn function_settings(functions: Option<&ConfigValue>, key: &str) -> Vec<String> {
    let Some(functions) = functions else {
        return Vec::new();
    };
    let mut found: Vec<String> = functions
        .get(key)
        .map(strings)
        .unwrap_or_default()
        .into_iter()
        .map(str::to_owned)
        .collect();
    for function in values_of(functions) {
        if matches!(function, ConfigValue::Object(_)) {
            found.extend(
                function
                    .get(key)
                    .map(strings)
                    .unwrap_or_default()
                    .into_iter()
                    .map(str::to_owned),
            );
        }
    }
    found
}
