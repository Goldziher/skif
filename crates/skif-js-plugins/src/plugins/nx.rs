//! Nx: executors, plugins and generators are packages; `run-commands` targets are scripts.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{package_name, script_inputs};

/// The `nx` plugin.
pub struct Nx;

static SPEC: PluginSpec = PluginSpec {
    id: "nx",
    enablers: &[
        Enabler::Dependency("nx"),
        Enabler::DependencyPrefix("@nrwl/"),
        Enabler::DependencyPrefix("@nx/"),
    ],
    config: &["nx.json", "project.json", "{apps,libs}/**/project.json"],
    package_json_key: Some("nx"),
    binaries: &["nx"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Nx {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        if config.file_name() == "nx.json" {
            if let Some(ConfigValue::Object(defaults)) = config.value.get("targetDefaults") {
                out.extend(
                    defaults
                        .iter()
                        .filter(|(key, _)| key.starts_with('@') && key.contains(':'))
                        .filter_map(|(key, _)| executor_package(key)),
                );
            }
            if let Some(ConfigValue::Array(plugins)) = config.value.get("plugins") {
                out.extend(
                    plugins
                        .iter()
                        .filter_map(|plugin| match plugin {
                            ConfigValue::String(name) => Some(name.as_str()),
                            other => other.get("plugin").and_then(ConfigValue::as_str),
                        })
                        .filter_map(package_name)
                        .map(PluginInput::Dependency),
                );
            }
            if let Some(ConfigValue::Object(generators)) = config.value.get("generators") {
                out.extend(generators.iter().filter_map(|(key, _)| executor_package(key)));
            }
        }
        if let Some(ConfigValue::Object(targets)) = config.value.get("targets") {
            for (_, target) in targets {
                out.extend(target_inputs(target));
            }
        }
        out
    }
}

/// `@nx/vite:build` names the package `@nx/vite`; `./tools:build` names a local executor.
fn executor_package(executor: &str) -> Option<PluginInput> {
    let name = executor.split(':').next()?;
    if name.starts_with('.') {
        return None;
    }
    package_name(name).map(PluginInput::Dependency)
}

fn target_inputs(target: &ConfigValue) -> Vec<PluginInput> {
    let mut out = Vec::new();
    if let Some(executor) = target.get("executor").and_then(ConfigValue::as_str) {
        out.extend(executor_package(executor));
    }
    if let Some(command) = target.get("command") {
        out.extend(script_inputs(command));
    }
    if let Some(options) = target.get("options") {
        if let Some(command) = options.get("command") {
            out.extend(script_inputs(command));
        }
        if let Some(ConfigValue::Array(commands)) = options.get("commands") {
            for command in commands {
                match command {
                    ConfigValue::String(script) => out.push(PluginInput::Script(script.clone())),
                    other => out.extend(other.get("command").map(script_inputs).unwrap_or_default()),
                }
            }
        }
    }
    out
}
