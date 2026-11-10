//! Commands a file runs through `node:child_process`: `execSync('tsc -b')` and `exec(…)` run a command line,
//! `spawn('eslint', ['.'])`, `spawnSync`, `execFile` and `execFileSync` a file with arguments. Only literal commands
//! count, as for the `execa` plugin; `fork` runs a module, not a command. A `cwd` option runs the command in the
//! directory it names, which the script parser places only when the file's own location computes it (see
//! [`command_in_call_dir`]).

use oxc_ast::ast::Program;

use crate::config::{
    ConfigValue, ImportedCall, command_in_call_dir, file_call_options, imported_calls_in, literal_command,
};
use crate::model::Import;

const MODULES: &[&str] = &["child_process", "node:child_process"];

/// The command lines of the `child_process` calls in `program`, in source order; empty unless one of `imports` reads
/// the module, so files that do not use it are not walked again.
pub(crate) fn child_process_commands(program: &Program<'_>, imports: &[Import]) -> Vec<String> {
    if !imports
        .iter()
        .any(|import| MODULES.contains(&import.specifier.as_str()))
    {
        return Vec::new();
    }
    let mut commands: Vec<String> = Vec::new();
    for command in imported_calls_in(program, "", MODULES, &[]).iter().filter_map(command) {
        if !commands.contains(&command) {
            commands.push(command);
        }
    }
    commands
}

fn command(call: &ImportedCall) -> Option<String> {
    if call.template.is_some() {
        return None;
    }
    let arguments = &call.arguments;
    match call.export.as_str() {
        "exec" | "execSync" => {
            let command = arguments.first().and_then(ConfigValue::as_str)?;
            Some(command_in_call_dir(command.to_owned(), arguments.get(1)))
        }
        "spawn" | "spawnSync" | "execFile" | "execFileSync" => {
            literal_command(arguments).map(|command| command_in_call_dir(command, file_call_options(arguments)))
        }
        _ => None,
    }
}
