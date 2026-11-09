//! GitHub Actions: `run:` steps of workflows and composite actions are scripts; `uses:` is ignored, and so is a step
//! whose `shell` (or the `defaults.run.shell` of its job or workflow) is not a POSIX shell (`pwsh`, `cmd`, `python`).

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::join_rel;

/// The `github-actions` plugin.
pub struct GithubActions;

static SPEC: PluginSpec = PluginSpec {
    id: "github-actions",
    enablers: &[Enabler::File(".github/workflows/*.{yml,yaml}")],
    root_only: true,
    config: &[".github/workflows/*.{yml,yaml}", ".github/**/action.{yml,yaml}"],
    optional_binaries: true,
    ..PluginSpec::DEFAULT
};

const ACTION_PATH_VARIABLES: &[&str] = &[
    "${{ github.action_path }}",
    "${{github.action_path}}",
    "${GITHUB_ACTION_PATH}",
    "$GITHUB_ACTION_PATH",
];

/// The shells whose scripts the script parser reads.
const POSIX_SHELLS: &[&str] = &["ash", "bash", "dash", "ksh", "sh", "zsh"];

/// The `defaults.run` of the enclosing workflow and job.
#[derive(Clone, Copy, Default)]
struct RunDefaults<'a> {
    working_directory: Option<&'a str>,
    shell: Option<&'a str>,
}

impl Plugin for GithubActions {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        steps(config.value, RunDefaults::default(), &mut out, 0);
        out.extend(
            node_action_files(config.value)
                .into_iter()
                .map(|file| PluginInput::Entry(config.join(file))),
        );
        out
    }
}

/// The files a JavaScript action runs (`runs.pre`, `runs.main` and `runs.post` of an `action.yml` whose `runs.using`
/// is a Node.js version), relative to the manifest.
pub(crate) fn node_action_files(manifest: &ConfigValue) -> Vec<&str> {
    let Some(runs) = manifest.get("runs").filter(|runs| {
        runs.get("using")
            .and_then(ConfigValue::as_str)
            .is_some_and(|using| using.starts_with("node"))
    }) else {
        return Vec::new();
    };
    ["pre", "main", "post"]
        .into_iter()
        .filter_map(|key| runs.get(key).and_then(ConfigValue::as_str))
        .collect()
}

/// Collects the `run` scripts of every `steps` list, wherever it is nested (`jobs.<id>.steps`, `runs.steps`).
/// `defaults` is the `defaults.run` of the enclosing workflow or job.
fn steps<'a>(value: &'a ConfigValue, defaults: RunDefaults<'a>, out: &mut Vec<PluginInput>, depth: u8) {
    if depth > 12 {
        return;
    }
    match value {
        ConfigValue::Object(entries) => {
            let run = value.get("defaults").and_then(|defaults| defaults.get("run"));
            let field = |key: &str| run.and_then(|run| run.get(key)).and_then(ConfigValue::as_str);
            let defaults = RunDefaults {
                working_directory: field("working-directory").or(defaults.working_directory),
                shell: field("shell").or(defaults.shell),
            };
            for (key, child) in entries {
                if key == "steps" {
                    out.extend(
                        child
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|item| step(item, defaults)),
                    );
                }
                steps(child, defaults, out, depth + 1);
            }
        }
        ConfigValue::Array(items) => items.iter().for_each(|item| steps(item, defaults, out, depth + 1)),
        _ => {}
    }
}

/// Whether a step's `shell` runs its script as a POSIX shell script: `bash`, `sh` or a custom command line of one
/// (`bash --noprofile --norc -eo pipefail {0}`, `/bin/sh -e {0}`). A shell an expression computes is assumed to be one.
fn is_posix_shell(shell: &str) -> bool {
    if shell.contains("${{") {
        return true;
    }
    let program = shell.split_whitespace().next().unwrap_or("");
    let program = program.rsplit('/').next().unwrap_or(program);
    program.is_empty() || POSIX_SHELLS.contains(&program)
}

/// A `run` step: a script of the workspace root, or of the directory its `working-directory` names. A directory that
/// is computed or outside the workspace drops the step, and so does a shell that is not POSIX.
fn step(step: &ConfigValue, defaults: RunDefaults<'_>) -> Option<PluginInput> {
    let run = step.get("run").and_then(ConfigValue::as_str)?;
    let shell = step.get("shell").and_then(ConfigValue::as_str).or(defaults.shell);
    if !shell.is_none_or(is_posix_shell) {
        return None;
    }
    let dir = step
        .get("working-directory")
        .and_then(ConfigValue::as_str)
        .or(defaults.working_directory)
        .map_or("", str::trim);
    let script = ACTION_PATH_VARIABLES
        .iter()
        .fold(run.to_string(), |text, variable| text.replace(variable, "."));
    if matches!(dir, "" | "." | "./") {
        return Some(PluginInput::Script(script));
    }
    if dir.starts_with('/') || dir.contains(['$', '`']) {
        return None;
    }
    let dir = join_rel("", dir);
    (!dir.is_empty() && dir != ".." && !dir.starts_with("../")).then_some(PluginInput::ScriptAt { dir, script })
}
