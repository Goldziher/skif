//! Nx: executors, plugins and generators are packages; `run-commands` targets are scripts, and target options name
//! build entries and the configs of other plugins.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginId, PluginInput, PluginSpec};
use crate::support::{join_rel, package_name, strings};

/// The `nx` plugin.
pub struct Nx;

/// The plugin's identifier.
pub const ID: PluginId = "nx";

static SPEC: PluginSpec = PluginSpec {
    id: ID,
    enablers: &[
        Enabler::Dependency("nx"),
        Enabler::DependencyPrefix("@nrwl/"),
        Enabler::DependencyPrefix("@nx/"),
    ],
    // The `project.json` of every project below the package is read as well (ADR 0109).
    config: &["nx.json", "project.json"],
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
                out.extend(target_inputs(target, config.base));
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

/// Target options that name a config file of another plugin, relative to the workspace root.
const LINKED_OPTIONS: &[(&str, PluginId)] = &[("tsConfig", "typescript"), ("jestConfig", "jest")];

/// What a target of the project in `project_root` (relative to the package directory) names: its executor's package,
/// its commands, the build entries of `main` and `additionalEntryPoints` and the configs of [`LINKED_OPTIONS`].
fn target_inputs(target: &ConfigValue, project_root: &str) -> Vec<PluginInput> {
    let mut out = Vec::new();
    if let Some(executor) = target.get("executor").and_then(ConfigValue::as_str) {
        out.extend(executor_package(executor));
    }
    let options = target.get("options");
    let mut commands: Vec<&str> = target.get("command").map(strings).unwrap_or_default();
    if let Some(options) = options {
        commands.extend(options.get("command").map(strings).unwrap_or_default());
        if let Some(ConfigValue::Array(list)) = options.get("commands") {
            for command in list {
                match command {
                    ConfigValue::String(script) => commands.push(script),
                    other => commands.extend(other.get("command").map(strings).unwrap_or_default()),
                }
            }
        }
        out.extend(option_inputs(options, project_root));
    }
    // `run-commands` runs its commands in the workspace root, or in `cwd` relative to it.
    let cwd = options
        .and_then(|options| options.get("cwd"))
        .and_then(ConfigValue::as_str)
        .and_then(|cwd| option_path(cwd, project_root));
    for command in commands {
        let script = with_tokens(command, project_root);
        out.push(PluginInput::Script(match &cwd {
            Some(dir) => format!("cd {} && {script}", shell_word(dir)),
            None => script,
        }));
    }
    out
}

/// The entries and linked configs that the `options` of a target name.
fn option_inputs(options: &ConfigValue, project_root: &str) -> Vec<PluginInput> {
    let mut out = Vec::new();
    let mut entries: Vec<&str> = options.get("main").map(strings).unwrap_or_default();
    entries.extend(options.get("additionalEntryPoints").map(strings).unwrap_or_default());
    out.extend(
        entries
            .into_iter()
            .filter_map(|path| option_path(path, project_root))
            .map(PluginInput::ProductionEntry),
    );
    for &(key, plugin) in LINKED_OPTIONS {
        if let Some(path) = options
            .get(key)
            .and_then(ConfigValue::as_str)
            .and_then(|path| option_path(path, project_root))
        {
            out.push(PluginInput::LinkedConfig { plugin, path });
        }
    }
    out
}

/// `text` with Nx's `{workspaceRoot}` and `{projectRoot}` tokens replaced by the workspace root and the project's
/// directory `project_root`, both relative to the package directory, where Nx runs a command by default.
fn with_tokens(text: &str, project_root: &str) -> String {
    let text = text.replace("{workspaceRoot}/", "").replace("{workspaceRoot}", ".");
    if project_root.is_empty() {
        text.replace("{projectRoot}/", "").replace("{projectRoot}", ".")
    } else {
        text.replace("{projectRoot}", project_root)
    }
}

/// A path an option gives relative to the workspace root, relative to the package directory; `None` when it is
/// absolute, leaves the package or holds a token that is not known statically (`{options.outputPath}`).
fn option_path(path: &str, project_root: &str) -> Option<String> {
    let path = join_rel("", with_tokens(path.trim(), project_root).as_str());
    (!path.is_empty() && !path.starts_with('/') && !path.starts_with("..") && !path.contains(['{', '}', '$']))
        .then_some(path)
}

/// `word` quoted for a shell when it holds anything but path characters.
fn shell_word(word: &str) -> String {
    if word
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | '@' | '+'))
    {
        word.to_owned()
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}
