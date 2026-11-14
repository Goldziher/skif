//! moonrepo: the `command` of every task in `moon.yml` and `.moon/tasks*.yml` is a script.

use skif_js_syntax::{ConfigValue, shell_word};

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};

/// The `moonrepo` plugin.
pub struct Moonrepo;

static SPEC: PluginSpec = PluginSpec {
    id: "moonrepo",
    enablers: &[Enabler::Dependency("@moonrepo/cli")],
    root_only: true,
    config: &["moon.yml", ".moon/tasks.yml", ".moon/tasks/*.yml"],
    binaries: &["moon"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Moonrepo {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    /// `$workspaceRoot` and `$projectRoot` are the workspace itself, as the plugin is enabled at the root only.
    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let Some(ConfigValue::Object(tasks)) = config.value.get("tasks") else {
            return Vec::new();
        };
        let expand = |word: &str| {
            ["$workspaceRoot", "$projectRoot"]
                .iter()
                .fold(word.to_owned(), |word, variable| {
                    word.replace(&format!("{variable}/"), "").replace(variable, ".")
                })
        };
        tasks
            .iter()
            .filter_map(|(_, task)| match task.get("command")? {
                ConfigValue::String(command) => Some(expand(command)),
                ConfigValue::Array(words) => Some(
                    words
                        .iter()
                        .filter_map(ConfigValue::as_str)
                        .map(|word| shell_word(&expand(word)))
                        .collect::<Vec<_>>()
                        .join(" "),
                ),
                _ => None,
            })
            .filter(|command| !command.trim().is_empty())
            .map(PluginInput::Script)
            .collect()
    }
}
