//! execa: the commands a source file runs with `execa('tsx', ['build.ts'])`, `execaCommand('…')`, `execaNode(…)` and
//! `` $`…` `` are scripts, parsed like the scripts of `package.json`. Only literal commands count: a computed argument is
//! left out.

use skif_js_syntax::{ConfigValue, ImportedCall, extract_imported_calls};

use crate::plugin::{Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::sources_containing;

/// The `execa` plugin.
pub struct Execa;

static SPEC: PluginSpec = PluginSpec {
    id: "execa",
    enablers: &[Enabler::Dependency("execa")],
    ..PluginSpec::DEFAULT
};

impl Plugin for Execa {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn reads_config(&self) -> bool {
        false
    }

    fn resolve_workspace(&self, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        for (source, text) in sources_containing(cx, "execa") {
            let path = cx.dir.join(&source);
            for call in extract_imported_calls(&text, &path, &["execa"], &[]) {
                out.extend(execa_script(&call).map(PluginInput::Script));
            }
        }
        out
    }
}

/// The command line of one call or tagged template of execa.
fn execa_script(call: &ImportedCall) -> Option<String> {
    let export = call.export.as_str();
    if let Some(template) = &call.template {
        return match export {
            "$" | "$sync" => Some(template.clone()),
            "execaNode" => Some(format!("node {template}")),
            _ => None,
        };
    }
    match export {
        "execa" | "execaSync" | "$sync" => safe_command(&call.arguments),
        "execaNode" => safe_command(&call.arguments).map(|command| format!("node {command}")),
        "execaCommand" | "execaCommandSync" => call.arguments.first().and_then(ConfigValue::as_str).map(str::to_owned),
        _ => None,
    }
}

/// The command of a `(file, [arguments])` call: the file and the arguments that are plain literal words. `None` when the
/// file is not one.
pub(crate) fn safe_command(arguments: &[ConfigValue]) -> Option<String> {
    let is_safe = |word: &str| {
        !word.is_empty()
            && word
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '_' | '@' | '.' | ',' | ':' | '/' | '=' | '+' | '~' | '-'))
    };
    let file = arguments.first()?.as_str().filter(|file| is_safe(file))?;
    let mut words = vec![file];
    if let Some(ConfigValue::Array(items)) = arguments.get(1) {
        words.extend(
            items
                .iter()
                .filter_map(ConfigValue::as_str)
                .filter(|word| is_safe(word)),
        );
    }
    Some(words.join(" "))
}
