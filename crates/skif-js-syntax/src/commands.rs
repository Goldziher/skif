//! Commands a file runs through `node:child_process`: `execSync('tsc -b')` and `exec(…)` run a command line,
//! `spawn('eslint', ['.'])`, `spawnSync`, `execFile` and `execFileSync` a file with arguments. Only literal commands
//! count, as for the `execa` plugin; `fork` runs a module, not a command. A `cwd` option runs the command in the
//! directory it names, which the script parser places only when the file's own location computes it (see
//! [`command_in_call_dir`]).

use oxc_ast::ast::Program;

use crate::config::{
    ImportedCall, command_in_call_dir, command_line_call, file_call_options, imported_calls_in, literal_command,
};
use crate::model::Import;

const MODULES: &[&str] = &["child_process", "node:child_process"];

/// The name the file evaluates under (`__filename`, `import.meta.url`). A `cwd` the file's own location computes is
/// placed by the file's directory alone, so any name serves, but an empty one would make the file its own directory
/// and put `path.dirname(fileURLToPath(import.meta.url))` one level above it.
const FILE_NAME: &str = "file.js";

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
    for command in imported_calls_in(program, FILE_NAME, MODULES, &[], true)
        .iter()
        .filter_map(command)
    {
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
        "exec" | "execSync" => command_line_call(arguments),
        "spawn" | "spawnSync" | "execFile" | "execFileSync" => {
            literal_command(arguments).map(|command| command_in_call_dir(command, file_call_options(arguments)))
        }
        _ => None,
    }
}
