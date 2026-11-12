//! `skif format [--check]`: a lint with `package-json-format` alone (ADR 0009).

use skif::{Config, Level, Linter};

use crate::args::{FormatArgs, FormatSpec, LintArgs};
use crate::env::Env;
use crate::error::CliError;
use crate::lint::{self, Teardown};
use crate::output::emit_error;

/// The rule `skif format` runs.
const RULE: &str = "package-json-format";

/// Every registered rule off except `package-json-format`, which is an error.
fn levels() -> Result<Vec<(String, Level)>, CliError> {
    let linter = Linter::new(&Config::default())?;
    Ok(linter
        .rules()
        .map(|meta| {
            let level = if meta.id == RULE { Level::Error } else { Level::Off };
            (meta.id.to_owned(), level)
        })
        .collect())
}

/// The lint that `args` stands for: check only, or fix without installing.
fn lint_args(args: &FormatArgs, rules: Vec<(String, Level)>) -> LintArgs {
    LintArgs {
        path: args.path.clone(),
        recursive: args.recursive,
        format: args
            .format
            .map(|format| FormatSpec { format, file: None })
            .into_iter()
            .collect(),
        fix: !args.check,
        fix_unsafe: !args.check && args.allow_unsafe,
        no_install: true,
        no_prompt: true,
        rules,
        no_js: true,
        config: args.config.clone(),
        no_config: args.no_config,
        color: args.color,
        ..LintArgs::default()
    }
}

/// Runs `skif format` and returns the exit code: a lint's, so `--check` exits 1 when a manifest is not formatted.
pub fn run(args: &FormatArgs, env: Env, teardown: Teardown) -> u8 {
    match levels() {
        Ok(rules) => lint::run(&lint_args(args, rules), env, teardown),
        Err(error) => {
            emit_error(&error, args.format.unwrap_or_default());
            2
        }
    }
}
