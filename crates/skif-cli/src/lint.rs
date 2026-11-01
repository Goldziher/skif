//! `skif [PATH]`: lint, optionally fix, report, and choose the exit code.

use std::path::Path;

use clap::ColorChoice;
use skif::{Config, ConfigOverrides, FileSystem, IgnoreEntry, Linter, OsFs, Prefer, Report, Selector, Workspace};

use crate::args::{Format, LintArgs, Select};
use crate::env::Env;
use crate::error::CliError;
use crate::fix::{self, FixRun};
use crate::install::{self, InstallStatus};
use crate::output::{self, emit_error};
use crate::write::write_plan;

const CLEAN: u8 = 0;
const PROBLEMS: u8 = 1;
const COULD_NOT_RUN: u8 = 2;

/// Runs the lint command and returns the process exit code.
pub fn run(args: &LintArgs, env: &Env) -> u8 {
    match execute(args, env) {
        Ok(code) => code,
        Err(error) => {
            emit_error(&error, args.format());
            COULD_NOT_RUN
        }
    }
}

fn overrides(args: &LintArgs) -> ConfigOverrides {
    ConfigOverrides {
        rules: args.rules.iter().cloned().collect(),
        ignore_rules: args.ignore_rule.clone(),
        ignore_packages: args.ignore_package.clone(),
        ignore_dependencies: args.ignore_dependency.clone(),
        fail_on_warnings: args.fail_on_warnings.then_some(true),
        production: args.production.then_some(true),
        no_js: args.no_js,
    }
}

fn configure(args: &LintArgs, root: &Path, fs: &dyn FileSystem) -> Result<Config, CliError> {
    let mut config = skif::load_config(root, fs, &overrides(args))?;
    for (rule, package) in &args.ignore_rule_for_package {
        config.ignore.push(IgnoreEntry {
            rules: vec![rule.clone()],
            packages: Selector::new([package.clone()])?,
            ..IgnoreEntry::default()
        });
    }
    if let Some(select) = args.select {
        config.policy.prefer = match select {
            Select::Highest => Prefer::Highest,
            Select::Lowest => Prefer::Lowest,
        };
    }
    Ok(config)
}

fn lint_once(
    linter: &Linter,
    root: &Path,
    config: &Config,
    fs: &dyn FileSystem,
) -> Result<(Workspace, Report), CliError> {
    let workspace = Workspace::discover_with_fs(root, config, fs)?;
    let report = linter.run(&workspace)?;
    Ok((workspace, report))
}

fn is_manifest_or_catalog(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some("package.json" | "pnpm-workspace.yaml")
    )
}

fn install_step(args: &LintArgs, changed: &[&Path], workspace: &Workspace) -> InstallStatus {
    if args.no_install {
        return InstallStatus::Skipped {
            reason: "--no-install was given",
        };
    }
    if !changed.iter().any(|path| is_manifest_or_catalog(path)) {
        return InstallStatus::Skipped {
            reason: "no manifest or catalog changed",
        };
    }
    match workspace.package_manager {
        Some(manager) => install::install(manager, &workspace.root, args.format() == Format::Json),
        None => InstallStatus::Skipped {
            reason: "no package manager detected",
        },
    }
}

fn color_enabled(args: &LintArgs, env: &Env) -> bool {
    args.format() == Format::Human
        && match args.color {
            ColorChoice::Always => true,
            ColorChoice::Never => false,
            _ => env.stdout_tty && !env.no_color && args.output.is_none(),
        }
}

fn exit_code(report: &Report, max_warnings: Option<usize>) -> u8 {
    if report.summary.errors > 0 || max_warnings.is_some_and(|max| report.summary.warnings > max) {
        PROBLEMS
    } else {
        CLEAN
    }
}

fn execute(args: &LintArgs, env: &Env) -> Result<u8, CliError> {
    let fs = OsFs;
    if args.fixing() && env.ci {
        return Err(CliError::ci_refusal());
    }
    let start = std::path::absolute(&args.path).map_err(|error| CliError::io("cannot resolve", &args.path, &error))?;
    let root = skif::find_workspace_root(&start, &fs)?;
    let config = configure(args, &root, &fs)?;
    let linter = Linter::new(&config)?;
    let (mut workspace, mut report) = lint_once(&linter, &root, &config, &fs)?;

    let mut fix_run = None;
    if args.fixing() {
        let selection = fix::selection(args, env, &report)?;
        let plan = skif::apply_fixes(&workspace, &report, &selection, &fs)?;
        write_plan(&plan, &workspace.root)?;
        if !plan.is_empty() {
            (workspace, report) = lint_once(&linter, &root, &config, &fs)?;
        }
        let changed: Vec<&Path> = plan.paths().collect();
        let install = install_step(args, &changed, &workspace);
        fix_run = Some(FixRun { plan, install });
    }

    let rendered = output::render(
        &report,
        args.format(),
        color_enabled(args, env),
        &workspace.root,
        &fs,
        fix_run.as_ref(),
    )?;
    output::emit(&rendered, args.output.as_deref())?;

    if let Some(InstallStatus::Failed { command, message }) = fix_run.as_ref().map(|run| &run.install) {
        eprintln!("error: install failed ({command}): {message}");
        return Ok(COULD_NOT_RUN);
    }
    let max_warnings = args.max_warnings.or_else(|| config.lint.fail_on_warnings.then_some(0));
    Ok(exit_code(&report, max_warnings))
}
