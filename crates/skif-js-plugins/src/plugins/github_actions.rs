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

/// The spellings of the directory a workflow checks the repository out to, where a step starts unless it sets a
/// `working-directory`: the workspace root. The bare `$GITHUB_WORKSPACE` is replaced apart, since a longer name may
/// start with it.
const WORKSPACE_VARIABLES: &[&str] = &["${{ github.workspace }}", "${{github.workspace}}", "${GITHUB_WORKSPACE}"];

/// The bare spelling of [`WORKSPACE_VARIABLES`].
const WORKSPACE_VARIABLE: &str = "$GITHUB_WORKSPACE";

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
        return Some(PluginInput::Script(in_workspace(&script, ".")));
    }
    if dir.starts_with('/') || dir.contains(['$', '`']) {
        return None;
    }
    let dir = join_rel("", dir);
    if dir.is_empty() || dir == ".." || dir.starts_with("../") {
        return None;
    }
    let root = vec![".."; dir.split('/').count()].join("/");
    let script = in_workspace(&script, &root);
    Some(PluginInput::ScriptAt { dir, script })
}

/// `script` with the workspace directory (see [`WORKSPACE_VARIABLES`]) spelt as `root`, the workspace root relative to
/// where the step starts, so `cd $GITHUB_WORKSPACE/sub` changes to a directory the script parser can place.
fn in_workspace(script: &str, root: &str) -> String {
    let script = WORKSPACE_VARIABLES
        .iter()
        .fold(script.to_owned(), |text, variable| text.replace(variable, root));
    let mut out = String::with_capacity(script.len());
    let mut rest = script.as_str();
    while let Some(found) = rest.find(WORKSPACE_VARIABLE) {
        let after = &rest[found + WORKSPACE_VARIABLE.len()..];
        out.push_str(&rest[..found]);
        let longer_name = after.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_');
        out.push_str(if longer_name { WORKSPACE_VARIABLE } else { root });
        rest = after;
    }
    out.push_str(rest);
    out
}
