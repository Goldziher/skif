//! GitHub Actions: `run:` steps of workflows and composite actions are scripts; `uses:` is ignored.

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
    ..PluginSpec::DEFAULT
};

const ACTION_PATH_VARIABLES: &[&str] = &[
    "${{ github.action_path }}",
    "${{github.action_path}}",
    "${GITHUB_ACTION_PATH}",
    "$GITHUB_ACTION_PATH",
];

impl Plugin for GithubActions {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        steps(config.value, None, &mut out, 0);
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
/// `default_dir` is the `defaults.run.working-directory` of the enclosing workflow or job.
fn steps(value: &ConfigValue, default_dir: Option<&str>, out: &mut Vec<PluginInput>, depth: u8) {
    if depth > 12 {
        return;
    }
    match value {
        ConfigValue::Object(entries) => {
            let default_dir = value
                .get("defaults")
                .and_then(|defaults| defaults.get("run"))
                .and_then(|run| run.get("working-directory"))
                .and_then(ConfigValue::as_str)
                .or(default_dir);
            for (key, child) in entries {
                if key == "steps" {
                    out.extend(
                        child
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|item| step(item, default_dir)),
                    );
                }
                steps(child, default_dir, out, depth + 1);
            }
        }
        ConfigValue::Array(items) => items.iter().for_each(|item| steps(item, default_dir, out, depth + 1)),
        _ => {}
    }
}

/// A `run` step: a script of the workspace root, or of the directory its `working-directory` names. A directory that
/// is computed or outside the workspace drops the step.
fn step(step: &ConfigValue, default_dir: Option<&str>) -> Option<PluginInput> {
    let run = step.get("run").and_then(ConfigValue::as_str)?;
    let dir = step
        .get("working-directory")
        .and_then(ConfigValue::as_str)
        .or(default_dir)
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
