//! `skif [PATH]`: lint, optionally fix, report, and choose the exit code.

use std::path::{Path, PathBuf};

use clap::ColorChoice;
use serde_json::Map;
use skif::{
    Config, ConfigOverrides, FileSystem, IgnoreEntry, Linter, OsFs, Prefer, Report, RunOptions, Selector, Workspace,
};

use crate::args::{Format, LintArgs, Select};
use crate::env::{Env, github_workspace};
use crate::error::CliError;
use crate::fix::{self, FixRun};
use crate::install::{self, InstallStatus};
use crate::output::{self, Rendering, emit_error, emit_error_with};
use crate::write::write_plan;

const CLEAN: u8 = 0;
const PROBLEMS: u8 = 1;
const COULD_NOT_RUN: u8 = 2;

/// Runs the lint command and returns the process exit code.
pub fn run(args: &LintArgs, env: Env) -> u8 {
    match execute(args, env) {
        Ok(code) => code,
        Err(error) => {
            emit_error(&error, args.format());
            COULD_NOT_RUN
        }
    }
}

/// The format to render and whether GitHub annotations accompany it: with no `--format`, `GITHUB_ACTIONS=true`
/// selects human output plus annotations (ADR 0012).
fn rendering_format(args: &LintArgs, env: Env) -> (Format, bool) {
    (args.format(), args.format.is_none() && env.github_actions)
}

/// A path as the OS resolves it: symlinks and `..` gone, and no Windows `\\?\` prefix on a plain drive path.
fn canonical(path: &Path) -> Result<PathBuf, CliError> {
    let resolved = std::fs::canonicalize(path).map_err(|error| CliError::io("cannot resolve", path, &error))?;
    Ok(simplified(resolved))
}

#[cfg(windows)]
fn simplified(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => PathBuf::from(rest),
        _ => path,
    }
}

#[cfg(not(windows))]
const fn simplified(path: PathBuf) -> PathBuf {
    path
}

/// The repository top-level for annotation paths: `GITHUB_WORKSPACE` when it contains the workspace root, else the
/// nearest ancestor holding a `.git`.
fn repo_root(root: &Path) -> Option<PathBuf> {
    github_workspace()
        .map(|workspace| simplified(std::fs::canonicalize(&workspace).unwrap_or(workspace)))
        .filter(|workspace| root.starts_with(workspace))
        .or_else(|| {
            root.ancestors()
                .find(|dir| dir.join(".git").exists())
                .map(Path::to_path_buf)
        })
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
    let mut options = RunOptions::default();
    let config_path = skif::config_file(root, fs).map(Path::new);
    if let Some(path) = config_path {
        options = options.with_config_path(path);
    }
    let report = linter.run_with(&workspace, &options)?;
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

fn color_enabled(args: &LintArgs, env: Env) -> bool {
    args.format() == Format::Human
        && match args.color {
            ColorChoice::Always => true,
            ColorChoice::Never => false,
            ColorChoice::Auto => env.stdout_tty && !env.no_color && args.output.is_none(),
        }
}

fn exit_code(report: &Report, max_warnings: Option<usize>) -> u8 {
    if report.summary.errors > 0 || max_warnings.is_some_and(|max| report.summary.warnings > max) {
        PROBLEMS
    } else {
        CLEAN
    }
}

fn rendering<'a>(args: &LintArgs, env: Env, linter: &Linter, root: &'a Path) -> Rendering<'a> {
    let (format, annotations) = rendering_format(args, env);
    Rendering {
        format,
        annotations,
        color: color_enabled(args, env),
        root,
        repo_root: (format == Format::Github || annotations)
            .then(|| repo_root(root))
            .flatten(),
        rules: linter.rules().collect(),
    }
}

/// Plans, writes and describes the fixes; the second value is the lint of the tree after fixing, or the error that
/// stopped it (the files are already changed by then, so the summary is still owed to the user).
type Relint = Result<(Workspace, Report), CliError>;

fn fixed(
    args: &LintArgs,
    env: Env,
    linter: &Linter,
    config: &Config,
    state: (Workspace, Report),
    fs: &dyn FileSystem,
) -> Result<(FixRun, Relint), CliError> {
    let (workspace, report) = state;
    let selection = fix::selection(args, env, &report)?;
    let plan = skif::apply_fixes(&workspace, &report, &selection, fs)?;
    write_plan(&plan, &workspace.root)?;
    if plan.is_empty() {
        let install = install_step(args, &[], &workspace);
        return Ok((FixRun { plan, install }, Ok((workspace, report))));
    }
    let root = workspace.root.clone();
    Ok(match lint_once(linter, &root, config, fs) {
        Ok((workspace, report)) => {
            let changed: Vec<&Path> = plan.paths().collect();
            let install = install_step(args, &changed, &workspace);
            (FixRun { plan, install }, Ok((workspace, report)))
        }
        Err(error) => (FixRun::relint_failed(plan), Err(error)),
    })
}

fn execute(args: &LintArgs, env: Env) -> Result<u8, CliError> {
    let fs = OsFs;
    if args.fixing() && env.ci {
        return Err(CliError::ci_refusal());
    }
    let start = canonical(&args.path)?;
    let root = skif::find_workspace_root(&start, &fs)?;
    let config = configure(args, &root, &fs)?;
    let linter = Linter::new(&config)?;
    let state = lint_once(&linter, &root, &config, &fs)?;

    let (workspace, report, fix_run) = if args.fixing() {
        let (run, relint) = fixed(args, env, &linter, &config, state, &fs)?;
        match relint {
            Ok((workspace, report)) => (workspace, report, Some(run)),
            Err(error) => return Ok(relint_failed(args, &error, &run)),
        }
    } else {
        (state.0, state.1, None)
    };

    let rendered = output::render(
        &report,
        &rendering(args, env, &linter, &workspace.root),
        &fs,
        fix_run.as_ref(),
    )?;
    output::emit(&rendered, args.output.as_deref())?;
    if let Some(run) = &fix_run
        && matches!(args.format(), Format::Sarif | Format::Github)
    {
        let mut summary = Vec::new();
        if run.write_human(&mut summary).is_ok() {
            eprint!("{}", String::from_utf8_lossy(&summary));
        }
    }

    if let Some(InstallStatus::Failed { command, message }) = fix_run.as_ref().map(|run| &run.install) {
        eprintln!("error: install failed ({command}): {message}");
        return Ok(COULD_NOT_RUN);
    }
    let max_warnings = args.max_warnings.or_else(|| config.lint.fail_on_warnings.then_some(0));
    Ok(exit_code(&report, max_warnings))
}

/// The files changed, then the failure: exit 2, with the fix summary on stdout (human) or in the error document
/// (JSON), so the user learns what was written even though the re-lint could not run.
fn relint_failed(args: &LintArgs, error: &CliError, run: &FixRun) -> u8 {
    let error = CliError {
        message: format!("fixes were written, but linting again failed: {}", error.message),
        kind: error.kind,
        path: error.path.clone(),
        line: error.line,
        column: error.column,
    };
    if args.format() == Format::Json {
        let mut extra = Map::new();
        extra.insert("fix".to_owned(), run.to_json());
        emit_error_with(&error, Format::Json, extra);
    } else {
        let mut summary = Vec::new();
        if run.write_human(&mut summary).is_ok() {
            let _ = output::write_stdout(&summary);
        }
        emit_error(&error, args.format());
    }
    COULD_NOT_RUN
}
