//! mise: the `run` commands of the tasks of `mise.toml` and its other config files are scripts, run in the task's
//! `dir` (or `task_config.dir`) relative to the config root.

use skif_js_syntax::ConfigValue;

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

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let Some(ConfigValue::Object(tasks)) = config.value.get("tasks") else {
            return Vec::new();
        };
        let root = config_root(config.base, config.file_name());
        let default_dir = get_in(config.value, &["task_config", "dir"]).and_then(ConfigValue::as_str);
        let mut out = Vec::new();
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
