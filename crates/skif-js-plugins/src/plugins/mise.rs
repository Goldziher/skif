//! mise: the `run` commands of the tasks of `mise.toml` and its other config files are scripts, run in the task's
//! `dir` (or `task_config.dir`) relative to the config root.
//!
//! File tasks are read too: the files under `mise-tasks/`, `.mise-tasks/`, `mise/tasks/`, `.mise/tasks/` and
//! `.config/mise/tasks/`, and under the directories `task_config.includes` names. A JavaScript or TypeScript file task
//! is an entry; a shell one (no shebang, or `sh`, `bash`, `zsh`, `dash` or `ksh`) is a script, run in the workspace or
//! in the directory of its `#MISE dir="…"` header. A task for another interpreter, and a file without a shebang whose
//! extension is not a shell's, is not read.

use skif_js_syntax::ConfigValue;

use crate::files::find_files;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{get_in, join_rel, parent_dir, script_in, strings};

/// The `mise` plugin.
pub struct Mise;

static SPEC: PluginSpec = PluginSpec {
    id: "mise",
    enablers: &[Enabler::File("mise.toml"), Enabler::File(".mise.toml")],
    config: &[
        "{mise,.mise}{,.*}.toml",
        "{mise,.mise}/config{,.*}.toml",
        ".config/mise{,.*}.toml",
        ".config/mise/config{,.*}.toml",
        ".config/mise/mise{,.local}.toml",
        "{mise,.mise}/conf.d/[!.]*.toml",
        ".config/mise/conf.d/[!.]*.toml",
    ],
    binaries: &["mise"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Mise {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let root = config_root(config.base, config.file_name());
        let includes: Vec<String> = get_in(config.value, &["task_config", "includes"])
            .map(strings)
            .unwrap_or_default()
            .into_iter()
            .filter(|include| !include.ends_with(".toml"))
            .filter_map(|include| resolve_dir(include, &root))
            .collect();
        let mut out = file_tasks(cx, &includes);
        let Some(ConfigValue::Object(tasks)) = config.value.get("tasks") else {
            return out;
        };
        let default_dir = get_in(config.value, &["task_config", "dir"]).and_then(ConfigValue::as_str);
        for (_, task) in tasks {
            let (runs, dir): (Vec<&str>, Option<&str>) = match task {
                ConfigValue::Object(_) => (
                    ["run", "run_windows"]
                        .iter()
                        .filter_map(|key| task.get(key))
                        .flat_map(strings)
                        .collect(),
                    task.get("dir").and_then(ConfigValue::as_str),
                ),
                other => (strings(other), None),
            };
            let Some(dir) = resolve_dir(dir.or(default_dir).unwrap_or("."), &root) else {
                continue;
            };
            for run in runs.into_iter().filter(|run| !has_template(run)) {
                out.extend(script_in(&dir, run.to_owned()));
            }
        }
        out
    }

    fn resolve_workspace(&self, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let dirs = FILE_TASK_DIRS.map(str::to_owned);
        file_tasks(cx, &dirs)
    }
}

/// The directories mise looks for file tasks in by default, relative to the config root.
const FILE_TASK_DIRS: [&str; 5] = ["mise-tasks", ".mise-tasks", "mise/tasks", ".mise/tasks", ".config/mise/tasks"];

/// Extensions of the file tasks that are JavaScript or TypeScript modules.
const SOURCE_EXTENSIONS: &[&str] = &["js", "mjs", "cjs", "jsx", "ts", "mts", "cts", "tsx"];

/// Interpreters whose file tasks are read as shell scripts, and the extensions of such files.
const SHELLS: &[&str] = &["sh", "bash", "zsh", "dash", "ksh"];

/// The file tasks under `dirs`, directories relative to the workspace, at any depth.
fn file_tasks(cx: &PluginContext<'_>, dirs: &[String]) -> Vec<PluginInput> {
    let patterns: Vec<String> = dirs.iter().map(|dir| join_rel(dir, "**/*")).collect();
    if patterns.is_empty() {
        return Vec::new();
    }
    find_files(cx.fs, cx.dir, &patterns)
        .into_iter()
        .filter_map(|file| file_task(cx, file))
        .collect()
}

/// What one file task contributes: an entry for a JavaScript or TypeScript module, a script for a shell script.
fn file_task(cx: &PluginContext<'_>, file: String) -> Option<PluginInput> {
    let is_source = file
        .rsplit_once('.')
        .is_some_and(|(_, extension)| SOURCE_EXTENSIONS.contains(&extension));
    if is_source {
        return Some(PluginInput::Entry(file));
    }
    let text = cx.fs.read_to_string(&cx.dir.join(&file)).ok()?;
    let is_shell = match text.lines().next().and_then(|line| line.strip_prefix("#!")) {
        Some(shebang) => interpreter(shebang).is_some_and(|name| SHELLS.contains(&name)),
        // Without a shebang, only a file named as a shell script is one (`build`, `build.sh`), not `README.md`.
        None => {
            let name = file.rsplit('/').next().unwrap_or(&file);
            name.rsplit_once('.')
                .is_none_or(|(stem, extension)| stem.is_empty() || SHELLS.contains(&extension))
        }
    };
    if !is_shell {
        return None;
    }
    let dir = match header_dir(&text) {
        Some(dir) => resolve_dir(dir, "")?,
        None => String::new(),
    };
    script_in(&dir, text.to_string())
}

/// The program a shebang line runs: its first word, or the word after `env` and its options.
fn interpreter(shebang: &str) -> Option<&str> {
    let mut words = shebang.split_whitespace();
    let program = words.next()?.rsplit('/').next()?;
    if program == "env" {
        words.find(|word| !word.starts_with('-'))
    } else {
        Some(program)
    }
}

/// The `dir` a file task's `#MISE dir="…"` (or `# [MISE] dir=…`) header sets.
fn header_dir(text: &str) -> Option<&str> {
    text.lines().find_map(|line| {
        let header = line
            .strip_prefix("#MISE")
            .or_else(|| line.strip_prefix("# [MISE]"))
            .or_else(|| line.strip_prefix("#[MISE]"))?;
        let value = header.trim().strip_prefix("dir")?.trim_start().strip_prefix('=')?.trim();
        Some(value.trim_matches(['"', '\'']))
    })
}

/// The directory a config file's paths are relative to: the directory that holds `mise.toml`, `.mise/` or
/// `.config/`.
fn config_root(base: &str, file_name: &str) -> String {
    let name = |dir: &str| dir.rsplit('/').next().unwrap_or(dir).to_owned();
    let is_fragment = name(base) == "conf.d";
    let grouped = if is_fragment { parent_dir(base) } else { base };
    let grouped_name = name(grouped);
    let root =
        if (is_fragment || file_name.starts_with("config.")) && (grouped_name == "mise" || grouped_name == ".mise") {
            parent_dir(grouped)
        } else {
            base
        };
    if name(root) == ".config" {
        parent_dir(root).to_owned()
    } else {
        root.to_owned()
    }
}

fn has_template(value: &str) -> bool {
    value.contains("{{") || value.contains("{%")
}

/// A task directory relative to the workspace: `{{config_root}}` is the config root, a relative path is relative to
/// it; a templated, home-relative or variable path is unknown.
fn resolve_dir(dir: &str, root: &str) -> Option<String> {
    let mut expanded = dir.to_owned();
    for variable in ["{{config_root}}", "{{ config_root }}"] {
        expanded = expanded.replace(variable, ".");
    }
    if has_template(&expanded) || expanded.starts_with('~') || expanded.contains('$') || expanded.starts_with('/') {
        return None;
    }
    Some(join_rel(root, &expanded))
}
