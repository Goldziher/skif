//! Taskfile: the commands of every task (`cmd`, `cmds`, deferred commands) are scripts; included Taskfiles are read
//! too. A `{{.NAME}}` template is replaced by the string the task's or the Taskfile's `vars` give `NAME`, and by `.`
//! for `ROOT_DIR` and the Taskfile's directory for `TASKFILE_DIR`; any other template (a dynamic `sh:` variable,
//! `CLI_ARGS`, a function) stands for a word that is not known, as a shell variable does, so the rest of the command
//! is still read. An include whose path holds such a template is skipped.

use skif_js_syntax::{ConfigValue, TEMPLATE_SUBSTITUTION};

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};

/// The `taskfile` plugin.
pub struct Taskfile;

static SPEC: PluginSpec = PluginSpec {
    id: "taskfile",
    enablers: &[Enabler::File("{T,t}askfile{,.dist}.{yml,yaml}")],
    root_only: true,
    config: &["{T,t}askfile{,.dist}.{yml,yaml}"],
    binaries: &["task"],
    optional_binaries: true,
    ..PluginSpec::DEFAULT
};

impl Plugin for Taskfile {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        let taskfile_dir = if config.base.is_empty() { "." } else { config.base };
        let globals = config.value.get("vars");
        if let Some(ConfigValue::Object(includes)) = config.value.get("includes") {
            for (_, include) in includes {
                let path = include
                    .as_str()
                    .or_else(|| include.get("taskfile").and_then(ConfigValue::as_str));
                let Some(path) = path.map(|path| expand(path, &[globals], taskfile_dir)) else {
                    continue;
                };
                if !path.contains(TEMPLATE_SUBSTITUTION) {
                    out.push(PluginInput::ConfigFile(config.join(&path)));
                }
            }
        }
        if let Some(ConfigValue::Object(tasks)) = config.value.get("tasks") {
            for (_, task) in tasks {
                let scopes = [task.get("vars"), globals];
                for script in task_scripts(task) {
                    out.push(PluginInput::Script(expand(script, &scopes, taskfile_dir)));
                }
            }
        }
        out
    }
}

/// `value` with every Go template (`{{.SCRIPT}}`) that `task` expands before running it replaced: a `.NAME` by the
/// first string `NAME` has in `scopes` (`vars` objects, the innermost first) or by the directory of a built-in
/// variable, anything else by [`TEMPLATE_SUBSTITUTION`].
fn expand(value: &str, scopes: &[Option<&ConfigValue>], taskfile_dir: &str) -> String {
    let mut out = String::new();
    let mut rest = value;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            out.push_str(TEMPLATE_SUBSTITUTION);
            return out;
        };
        let inner = after[..end].trim_start_matches('-').trim_end_matches('-').trim();
        let known = inner.strip_prefix('.').and_then(|name| match name {
            "ROOT_DIR" => Some("."),
            "TASKFILE_DIR" => Some(taskfile_dir),
            _ => scopes
                .iter()
                .flatten()
                .find_map(|vars| vars.get(name))
                .and_then(ConfigValue::as_str)
                .filter(|value| !value.contains("{{")),
        });
        rest = &after[end + 2..];
        // `{{.ROOT_DIR}}/scripts/a.js` names `scripts/a.js`, the same path a plain command would.
        if known == Some(".")
            && let Some(path) = rest.strip_prefix('/')
        {
            rest = path;
            continue;
        }
        out.push_str(known.unwrap_or(TEMPLATE_SUBSTITUTION));
    }
    out.push_str(rest);
    out
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
