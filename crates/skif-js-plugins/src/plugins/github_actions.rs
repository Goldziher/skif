//! GitHub Actions: `run:` steps of workflows and composite actions are scripts; `uses:` is ignored.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::strings;

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
        steps(config.value, &mut out, 0);
        // A JavaScript action runs files that live next to its manifest.
        if let Some(runs) = config.value.get("runs")
            && runs
                .get("using")
                .and_then(ConfigValue::as_str)
                .is_some_and(|u| u.starts_with("node"))
        {
            for key in ["pre", "main", "post"] {
                if let Some(file) = runs.get(key).and_then(ConfigValue::as_str) {
                    out.push(PluginInput::Entry(config.join(file)));
                }
            }
        }
        out
    }
}

/// Collects the `run` scripts of every `steps` list, wherever it is nested (`jobs.<id>.steps`, `runs.steps`).
fn steps(value: &ConfigValue, out: &mut Vec<PluginInput>, depth: u8) {
    if depth > 12 {
        return;
    }
    match value {
        ConfigValue::Object(entries) => {
            for (key, child) in entries {
                if key == "steps" {
                    out.extend(child.as_array().into_iter().flatten().filter_map(step));
                }
                steps(child, out, depth + 1);
            }
        }
        ConfigValue::Array(items) => items.iter().for_each(|item| steps(item, out, depth + 1)),
        _ => {}
    }
}

/// A `run` step in the workspace root; steps in another `working-directory` belong to another workspace.
fn step(step: &ConfigValue) -> Option<PluginInput> {
    let run = step.get("run").and_then(ConfigValue::as_str)?;
    let in_root = step
        .get("working-directory")
        .map(strings)
        .unwrap_or_default()
        .iter()
        .all(|dir| matches!(*dir, "." | "./"));
    in_root.then(|| {
        let script = ACTION_PATH_VARIABLES
            .iter()
            .fold(run.to_string(), |text, variable| text.replace(variable, "."));
        PluginInput::Script(script)
    })
}
