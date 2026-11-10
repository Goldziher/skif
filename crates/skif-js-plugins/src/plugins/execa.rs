//! execa: the commands a source file runs with `execa('tsx', ['build.ts'])`, `execaCommand('…')`, `execaNode(…)` and
//! `` $`…` `` are scripts, parsed like the scripts of `package.json`; so are the default export of execa 5 and earlier
//! (`execa(…)`, `execa.sync(…)`, `execa.command(…)`, `execa.node(…)`). Only literal commands count: a computed argument
//! is left out, and a call with a literal argument that is not a plain word is skipped. A call with a `cwd` option runs
//! its command in that directory, which the script parser places only when the file's own location computes it (see
//! [`command_in_call_dir`]).

use skif_js_syntax::{
    ConfigValue, ImportedCall, command_in_call_dir, extract_command_calls, file_call_options, literal_command,
};

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
            for call in extract_command_calls(&text, &path, &["execa"], &[]) {
                out.extend(execa_script(&call).map(|script| PluginInput::SourceScript {
                    file: source.clone(),
                    script,
                }));
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
    let arguments = &call.arguments;
    match export {
        "execa" | "execaSync" | "$sync" | "default" | "sync" => safe_command(arguments),
        "execaNode" | "node" => literal_command(arguments)
            .map(|command| command_in_call_dir(format!("node {command}"), file_call_options(arguments))),
        "execaCommand" | "execaCommandSync" | "command" | "commandSync" => {
            let command = arguments.first().and_then(ConfigValue::as_str)?;
            Some(command_in_call_dir(command.to_owned(), arguments.get(1)))
        }
        _ => None,
    }
}

/// The command of a `(file, [arguments], options)` call, in the directory its `cwd` option names; see
/// [`literal_command`] and [`command_in_call_dir`].
pub(crate) fn safe_command(arguments: &[ConfigValue]) -> Option<String> {
    let command = literal_command(arguments)?;
    Some(command_in_call_dir(command, file_call_options(arguments)))
}
