//! Azure Pipelines: the `script` and `bash` steps of `azure-pipelines.yml` and of the templates it includes are
//! scripts. A step's `workingDirectory` is honoured as GitHub Actions' `working-directory` is. knip has no counterpart.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::join_rel;

/// The `azure-pipelines` plugin.
pub struct AzurePipelines;

static SPEC: PluginSpec = PluginSpec {
    id: "azure-pipelines",
    enablers: &[
        Enabler::File("azure-pipelines.{yml,yaml}"),
        Enabler::File(".azure-pipelines/**/*.{yml,yaml}"),
    ],
    root_only: true,
    config: &["azure-pipelines.{yml,yaml}", ".azure-pipelines/**/*.{yml,yaml}"],
    optional_binaries: true,
    ..PluginSpec::DEFAULT
};

/// The step keys whose value is a POSIX shell script.
const SCRIPT_KEYS: &[&str] = &["script", "bash"];

impl Plugin for AzurePipelines {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        walk(config.value, config, &mut out, 0);
        out
    }
}

/// Collects the script steps and template references of every object, however deeply nested (`stages`, `jobs`,
/// `steps`, conditional insertions).
fn walk(value: &ConfigValue, config: &ConfigAst<'_>, out: &mut Vec<PluginInput>, depth: u8) {
    if depth > 16 {
        return;
    }
    match value {
        ConfigValue::Object(entries) => {
            if let Some(template) = value.get("template").and_then(ConfigValue::as_str)
                && !template.contains(['@', '$'])
            {
                let template = template.trim();
                // A leading `/` is the repository root; anything else is relative to the including file.
                let path = match template.strip_prefix('/') {
                    Some(rooted) => join_rel("", rooted),
                    None => config.join(template),
                };
                out.push(PluginInput::ConfigFile(path));
            }
            for key in SCRIPT_KEYS {
                if let Some(script) = value.get(key).and_then(ConfigValue::as_str) {
                    out.extend(step(
                        script,
                        value.get("workingDirectory").and_then(ConfigValue::as_str),
                    ));
                }
            }
            for (_, child) in entries {
                walk(child, config, out, depth + 1);
            }
        }
        ConfigValue::Array(items) => items.iter().for_each(|item| walk(item, config, out, depth + 1)),
        _ => {}
    }
}

/// A script step: a script of the repository root, or of the directory its `workingDirectory` names. A directory that
/// is computed (`$(Build.SourcesDirectory)/web`) or outside the repository drops the step.
fn step(script: &str, dir: Option<&str>) -> Option<PluginInput> {
    let dir = dir.map_or("", str::trim);
    if matches!(dir, "" | "." | "./") {
        return Some(PluginInput::Script(script.to_owned()));
    }
    if dir.starts_with('/') || dir.contains(['$', '`']) {
        return None;
    }
    let dir = join_rel("", dir);
    (!dir.is_empty() && dir != ".." && !dir.starts_with("../")).then(|| PluginInput::ScriptAt {
        dir,
        script: script.to_owned(),
    })
}
