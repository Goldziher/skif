//! `skif format [--check]`: a lint with `package-json-format` alone (ADR 0055).

use skif::{Config, Level, Linter};

use crate::args::{FormatArgs, FormatSpec, LintArgs};
use crate::env::Env;
use crate::error::CliError;
use crate::lint;
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
        output: None,
        fix: !args.check,
        fix_unsafe: !args.check && args.allow_unsafe,
        allow_remove_files: false,
        fix_type: Vec::new(),
        select: None,
        no_install: true,
        max_warnings: None,
        fail_on_warnings: false,
        max_issues: None,
        max_show_issues: None,
        no_exit_code: false,
        no_prompt: true,
        rules,
        ignore_rule: Vec::new(),
        ignore_package: Vec::new(),
        ignore_dependency: Vec::new(),
        ignore_rule_for_package: Vec::new(),
        require_ignore_reasons: false,
        no_js: true,
        config: args.config.clone(),
        no_config: args.no_config,
        production: false,
        no_gitignore: false,
        include_entry_exports: false,
        strict: false,
        color: args.color,
        trace_dependency: None,
        debug: false,
        performance: false,
        duration: false,
        memory: false,
        workspace: Vec::new(),
        include: Vec::new(),
        exclude: Vec::new(),
        dependencies: false,
        exports: false,
        files: false,
        cycles: false,
        tags: Vec::new(),
    }
}

/// Runs `skif format` and returns the exit code: a lint's, so `--check` exits 1 when a manifest is not formatted.
pub fn run(args: &FormatArgs, env: Env) -> u8 {
    match levels() {
        Ok(rules) => lint::run(&lint_args(args, rules), env),
        Err(error) => {
            emit_error(&error, args.format.unwrap_or_default());
            2
        }
    }
}
