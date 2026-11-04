//! GraphQL Code Generator: presets and plugins are packages, the generated outputs are production entries.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{entry_glob, is_relative, join_rel, package_name};

/// The `graphql-codegen` plugin.
pub struct GraphqlCodegen;

static SPEC: PluginSpec = PluginSpec {
    id: "graphql-codegen",
    enablers: &[
        Enabler::DependencyPrefix("@graphql-codegen/"),
        Enabler::Dependency("graphql-config"),
    ],
    config: &[
        "codegen.{json,yml,yaml,js,ts}",
        ".codegenrc.{json,yml,yaml,js,ts}",
        "codegen.config.js",
        ".graphqlrc",
        ".graphqlrc.{json,yml,yaml,toml,js,ts}",
        "graphql.config.{json,yml,yaml,toml,js,cjs,ts}",
    ],
    package_json_key: Some("codegen"),
    binaries: &["graphql-codegen", "gql-gen", "graphql-codegen-esm"],
    ..PluginSpec::DEFAULT
};

impl Plugin for GraphqlCodegen {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut sections: Vec<&ConfigValue> = Vec::new();
        if let Some(ConfigValue::Object(projects)) = config.value.get("projects") {
            sections.extend(
                projects
                    .iter()
                    .filter_map(|(_, project)| project.get("extensions")?.get("codegen")),
            );
        } else if let Some(codegen) = config.value.get("extensions").and_then(|e| e.get("codegen")) {
            sections.push(codegen);
        } else {
            sections.push(config.value);
        }
        let mut out = Vec::new();
        for section in sections {
            if let Some(ConfigValue::Object(generates)) = section.get("generates") {
                for (output, output_config) in generates {
                    out.extend(output_inputs(config, output, output_config));
                }
            }
        }
        out
    }
}

fn output_inputs(config: &ConfigAst<'_>, output: &str, output_config: &ConfigValue) -> Vec<PluginInput> {
    let mut out = Vec::new();
    let pattern = if output.ends_with('/') {
        format!("{output}**")
    } else {
        output.to_owned()
    };
    out.extend(entry_glob(config.base, &pattern).map(PluginInput::ProductionEntry));
    let mut plugins: Vec<&ConfigValue> = Vec::new();
    match output_config {
        ConfigValue::Array(items) => plugins.extend(items),
        ConfigValue::Object(_) => {
            if let Some(preset) = output_config.get("preset").and_then(ConfigValue::as_str) {
                out.extend(package_name(&preset_package(preset)).map(PluginInput::Dependency));
            }
            if let Some(ConfigValue::Array(items)) = output_config.get("plugins") {
                plugins.extend(items);
            }
        }
        _ => {}
    }
    for plugin in plugins {
        let names: Vec<&str> = match plugin {
            ConfigValue::String(name) => vec![name.as_str()],
            ConfigValue::Object(entries) => entries.iter().map(|(name, _)| name.as_str()).collect(),
            _ => Vec::new(),
        };
        for name in names {
            if is_relative(name) {
                out.push(PluginInput::Entry(join_rel(config.base, name)));
            } else {
                out.extend(package_name(&plugin_package(name)).map(PluginInput::Dependency));
            }
        }
    }
    out
}

fn preset_package(preset: &str) -> String {
    if preset.starts_with("@graphql-codegen/") {
        preset.to_owned()
    } else if preset.ends_with("-preset") {
        format!("@graphql-codegen/{preset}")
    } else {
        format!("@graphql-codegen/{preset}-preset")
    }
}

fn plugin_package(name: &str) -> String {
    if name.starts_with('@') || name.contains("codegen-") {
        name.to_owned()
    } else {
        format!("@graphql-codegen/{name}")
    }
}
