//! Taskfile: the commands of every task (`cmd`, `cmds`, deferred commands) are scripts; included Taskfiles are read
//! too. A command or include holding a `{{…}}` template is skipped: what it runs is known only to `task`.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};

/// The `taskfile` plugin.
pub struct Taskfile;

static SPEC: PluginSpec = PluginSpec {
    id: "taskfile",
    enablers: &[Enabler::File("{T,t}askfile{,.dist}.{yml,yaml}")],
    root_only: true,
    config: &["{T,t}askfile{,.dist}.{yml,yaml}"],
    binaries: &["task"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Taskfile {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        if let Some(ConfigValue::Object(includes)) = config.value.get("includes") {
            for (_, include) in includes {
                let path = include
                    .as_str()
                    .or_else(|| include.get("taskfile").and_then(ConfigValue::as_str));
                if let Some(path) = path.filter(|path| !has_template(path)) {
                    out.push(PluginInput::ConfigFile(config.join(path)));
                }
            }
        }
        if let Some(ConfigValue::Object(tasks)) = config.value.get("tasks") {
            for (_, task) in tasks {
                for script in task_scripts(task).into_iter().filter(|script| !has_template(script)) {
                    out.push(PluginInput::Script(script.to_owned()));
                }
            }
        }
        out
    }
}

/// Whether `value` holds a Go template (`{{.CLI_ARGS}}`), which `task` expands before running it.
fn has_template(value: &str) -> bool {
    value.contains("{{")
}

/// The commands of a task: a string, a list of strings, or an object with `cmd` or `cmds`.
fn task_scripts(task: &ConfigValue) -> Vec<&str> {
    match task {
        ConfigValue::String(script) => vec![script.as_str()],
        ConfigValue::Array(items) => items.iter().filter_map(ConfigValue::as_str).collect(),
        ConfigValue::Object(_) => {
            let mut scripts: Vec<&str> = task.get("cmd").and_then(ConfigValue::as_str).into_iter().collect();
            match task.get("cmds") {
                Some(ConfigValue::String(script)) => scripts.push(script),
                Some(ConfigValue::Array(commands)) => scripts.extend(commands.iter().flat_map(command_scripts)),
                _ => {}
            }
            scripts
        }
        _ => Vec::new(),
    }
}

/// The scripts of one entry of `cmds`: a string, or an object with `cmd` and a `defer` string or `{ cmd }`.
fn command_scripts(command: &ConfigValue) -> Vec<&str> {
    match command {
        ConfigValue::String(script) => vec![script.as_str()],
        ConfigValue::Object(_) => {
            let mut scripts: Vec<&str> = command.get("cmd").and_then(ConfigValue::as_str).into_iter().collect();
            match command.get("defer") {
                Some(ConfigValue::String(script)) => scripts.push(script),
                Some(defer) => scripts.extend(defer.get("cmd").and_then(ConfigValue::as_str)),
                None => {}
            }
            scripts
        }
        _ => Vec::new(),
    }
}
