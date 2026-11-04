//! `skif [PATH]`: lint, optionally fix, report, and choose the exit code.

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::ColorChoice;
use serde_json::{Map, Value};
use skif::render::RenderOptions;
use skif::timing;
use skif::{
    CachedFs, Config, ConfigOverrides, DiscoverOptions, FileSystem, FixPlan, IgnoreEntry, Linter, OsFs, Prefer, Report,
    Selector, Workspace,
};
use skif_report::{escape_controls, render_json_with};

use crate::args::{Format, LintArgs, Select};
use crate::env::{Env, github_workspace};
use crate::error::{CliError, Kind};
use crate::fix::{self, FixRun, RootFix};
use crate::install::{self, InstallStatus};
use crate::output::{self, Rendering, RootView, emit_error, emit_error_with, stderr_line};
use crate::roots::{self, Linters, Resolved, RootError, RootRun};
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
        config_file: args
            .config
            .as_ref()
            .map(|file| std::path::absolute(file).unwrap_or_else(|_| file.clone())),
        no_config: args.no_config,
    }
}

/// The flags that are not plain configuration overrides, applied over a loaded configuration.
fn adjust(args: &LintArgs, config: &mut Config) -> Result<(), CliError> {
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
    Ok(())
}

fn lint_once(
    linter: &Linter,
    root: &Path,
    config: &Config,
    config_path: Option<&Path>,
    fs: &dyn FileSystem,
) -> Result<(Workspace, Report), CliError> {
    let workspace = Workspace::discover_with_fs(root, config, fs)?;
    let report = roots::lint_loaded(linter, &workspace, config_path)?;
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
        Some(manager) => install::install(manager, &workspace.root, args.format() != Format::Human),
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

/// The worst exit code over the roots. `--max-warnings` bounds the warnings of the whole run; without it each root
/// applies its own `fail-on-warnings`.
fn run_exit_code(args: &LintArgs, runs: &[RootRun], aggregate: &Report) -> u8 {
    match args.max_warnings {
        Some(max) => exit_code(aggregate, Some(max)),
        None => runs
            .iter()
            .map(|run| exit_code(&run.report, run.config.lint.fail_on_warnings.then_some(0)))
            .max()
            .unwrap_or(CLEAN),
    }
}

/// Plans, writes and describes the fixes of one root; the second value is the lint of the tree after fixing, or the
/// error that stopped it (the files are already changed by then, so the summary is still owed to the user).
type Relint = Result<(Workspace, Report), CliError>;

fn write_and_relint(
    args: &LintArgs,
    linter: &Linter,
    (config, config_path): (&Config, Option<&Path>),
    state: (Workspace, Report),
    plan: FixPlan,
    fs: &CachedFs<OsFs>,
) -> Result<(FixRun, Relint), CliError> {
    let (workspace, report) = state;
    write_plan(&plan, &workspace.root)?;
    if plan.is_empty() {
        let install = install_step(args, &[], &workspace);
        return Ok((FixRun { plan, install }, Ok((workspace, report))));
    }
    // The written files are stale in the cache; the re-lint must read them from disk.
    fs.clear();
    let root = workspace.root.clone();
    Ok(match lint_once(linter, &root, config, config_path, fs) {
        Ok((workspace, report)) => {
            let changed: Vec<&Path> = plan.paths().collect();
            let install = install_step(args, &changed, &workspace);
            (FixRun { plan, install }, Ok((workspace, report)))
        }
        Err(error) => (FixRun::relint_failed(plan), Err(error)),
    })
}

/// What fixing every root left behind: the re-linted roots with their fix runs, or the exit code of a run that
/// stopped after writing some of the fixes.
enum Fixed {
    All(Vec<RootRun>, Vec<FixRun>),
    Stopped(u8),
}

/// Fixes every root. All plans are made before the first file is written, so a failure while planning (a prompt, a
/// conflicting edit) leaves every file alone. Writing and re-linting then go root by root; a failure there reports
/// what was already written.
fn fix_roots(
    args: &LintArgs,
    env: Env,
    linters: &Linters,
    runs: Vec<RootRun>,
    fs: &CachedFs<OsFs>,
) -> Result<Fixed, CliError> {
    let mut planned = Vec::with_capacity(runs.len());
    for run in runs {
        let selection = fix::selection(args, env, &run.report, run.config.policy.prefer)?;
        let plan = skif::apply_fixes(&run.workspace, &run.report, &selection, fs)?;
        planned.push((run, plan));
    }
    let mut done: Vec<RootRun> = Vec::with_capacity(planned.len());
    let mut fixes: Vec<FixRun> = Vec::with_capacity(planned.len());
    for (run, plan) in planned {
        let RootRun {
            prefix,
            config,
            config_path,
            linter,
            workspace,
            report,
        } = run;
        let written = write_and_relint(
            args,
            linters.get(linter),
            (&config, config_path.as_deref()),
            (workspace, report),
            plan,
            fs,
        );
        let (fix_run, relint) = match written {
            Ok(outcome) => outcome,
            Err(error) if fixes.iter().any(|run| !run.plan.is_empty()) => {
                let prefixes: Vec<&Path> = done.iter().map(|run| run.prefix.as_path()).collect();
                return Ok(Fixed::Stopped(fix_incomplete(
                    args,
                    "fixes were written to earlier roots, but fixing a later root failed",
                    &error,
                    &roots_fix(&prefixes, &fixes),
                )));
            }
            Err(error) => return Err(error),
        };
        fixes.push(fix_run);
        match relint {
            Ok((workspace, report)) => done.push(RootRun {
                prefix,
                config,
                config_path,
                linter,
                workspace,
                report,
            }),
            Err(error) => {
                let prefixes: Vec<&Path> = done
                    .iter()
                    .map(|run| run.prefix.as_path())
                    .chain(std::iter::once(prefix.as_path()))
                    .collect();
                return Ok(Fixed::Stopped(fix_incomplete(
                    args,
                    "fixes were written, but linting again failed",
                    &error,
                    &roots_fix(&prefixes, &fixes),
                )));
            }
        }
    }
    Ok(Fixed::All(done, fixes))
}

/// Lints the workspace containing `path`, or with `recursive` every root at or below it, and returns the report as
/// the JSON document `--format json` prints.
///
/// Nothing is printed, no file is written and the environment is not consulted, so an embedding host (the npm
/// addon) gets the same discovery, configuration and report as the command line, with the same classified errors.
///
/// # Errors
/// A [`CliError`] when `path` cannot be resolved, no workspace is found there, the configuration is invalid or
/// the lint cannot run.
pub fn lint_to_json(path: &Path, overrides: &ConfigOverrides, recursive: bool) -> Result<String, CliError> {
    let fs = Arc::new(CachedFs::new(OsFs));
    let start = canonical(path)?;
    let Resolved {
        base,
        targets,
        multi,
        failures,
        ..
    } = timing::phase("root discovery (roots, configs, workspaces)", || {
        roots::resolve(&start, recursive, fs.as_ref(), overrides)
    })?;
    let linters = Linters::build(&targets, &(Arc::clone(&fs) as Arc<dyn FileSystem>))?;
    let runs = timing::phase("lint roots", || roots::lint_targets(&base, targets, &linters))?;
    let reports: Vec<Cow<'_, Report>> = runs
        .iter()
        .map(|run| roots::prefixed(&run.report, &run.prefix))
        .collect();
    let aggregate = roots::aggregate(&reports);
    let options = RenderOptions::default()
        .with_root(&base)
        .with_tool_version(env!("CARGO_PKG_VERSION"));
    let mut extra = Map::new();
    if multi {
        extra.insert("roots".to_owned(), roots_json(&runs, &reports));
        if !failures.is_empty() {
            extra.insert("errors".to_owned(), errors_json(&failures));
        }
    }
    let mut buffer = Vec::new();
    render_json_with(&aggregate, &options, extra, &mut buffer)
        .map_err(|error| CliError::new(Kind::Io, format!("cannot render the report: {error}")))?;
    String::from_utf8(buffer).map_err(|error| CliError::new(Kind::Io, format!("the report is not UTF-8: {error}")))
}

fn errors_json(failures: &[RootError]) -> Value {
    Value::Array(failures.iter().map(RootError::to_json).collect())
}

/// Tells the user that the search for roots was cut short by the depth limit.
fn note_truncated(truncated: &[PathBuf]) {
    const SHOWN: usize = 5;
    if truncated.is_empty() {
        return;
    }
    let shown: Vec<String> = truncated
        .iter()
        .take(SHOWN)
        .map(|dir| escape_controls(&roots::display(dir)).into_owned())
        .collect();
    let more = truncated.len().saturating_sub(SHOWN);
    let tail = if more > 0 {
        format!(" and {more} more")
    } else {
        String::new()
    };
    stderr_line(&format!(
        "warning: the search for roots stopped {} levels down; nothing below {}{tail} was searched",
        DiscoverOptions::default().max_depth,
        shown.join(", ")
    ));
}

fn roots_json(runs: &[RootRun], reports: &[Cow<'_, Report>]) -> Value {
    Value::Array(
        runs.iter()
            .zip(reports)
            .map(|(run, report)| roots::root_json(run, report))
            .collect(),
    )
}

fn execute(args: &LintArgs, env: Env) -> Result<u8, CliError> {
    let fs = Arc::new(CachedFs::new(OsFs));
    if args.fixing() && env.ci {
        return Err(CliError::ci_refusal());
    }
    let start = canonical(&args.path)?;
    let Resolved {
        base,
        mut targets,
        multi,
        failures: root_errors,
        truncated,
    } = timing::phase("root discovery (roots, configs, workspaces)", || {
        roots::resolve(&start, args.recursive, fs.as_ref(), &overrides(args))
    })?;
    note_truncated(&truncated);
    for target in &mut targets {
        adjust(args, &mut target.config)?;
    }
    let linters = Linters::build(&targets, &(Arc::clone(&fs) as Arc<dyn FileSystem>))?;
    let mut runs = timing::phase("lint roots", || roots::lint_targets(&base, targets, &linters))?;

    let mut fixes: Vec<FixRun> = Vec::new();
    if args.fixing() {
        match fix_roots(args, env, &linters, runs, fs.as_ref())? {
            Fixed::All(fixed_runs, done) => {
                runs = fixed_runs;
                fixes = done;
            }
            Fixed::Stopped(code) => return Ok(code),
        }
    }

    let reports: Vec<Cow<'_, Report>> = runs
        .iter()
        .map(|run| roots::prefixed(&run.report, &run.prefix))
        .collect();
    let aggregate = roots::aggregate(&reports);
    let prefixes: Vec<&Path> = runs.iter().map(|run| run.prefix.as_path()).collect();
    let root_fixes = roots_fix(&prefixes, &fixes);

    let views: Vec<RootView<'_>> = runs
        .iter()
        .zip(&reports)
        .enumerate()
        .map(|(index, (run, report))| RootView {
            prefix: &run.prefix,
            report,
            json: multi.then(|| roots::root_json(run, report)),
            fix: fixes.get(index),
        })
        .collect();
    let (format, annotations) = rendering_format(args, env);
    let rendering = Rendering {
        format,
        annotations,
        color: color_enabled(args, env),
        root: &base,
        repo_root: (matches!(format, Format::Github | Format::Sarif) || annotations)
            .then(|| repo_root(&base))
            .flatten(),
        rules: linters.rules(),
        multi,
        errors: root_errors.iter().map(RootError::to_json).collect(),
    };
    let rendered = timing::phase("render", || {
        output::render(&aggregate, &views, &rendering, fs.as_ref(), &root_fixes)
    })?;
    output::emit(&rendered, args.output.as_deref())?;
    if !fixes.is_empty() && matches!(args.format(), Format::Sarif | Format::Github) {
        write_summary_to_stderr(&root_fixes);
    }

    for root_error in &root_errors {
        stderr_line(&format!("error: {}", escape_controls(&root_error.describe())));
    }
    let install_failures: Vec<&InstallStatus> = fixes
        .iter()
        .map(|run| &run.install)
        .filter(|status| matches!(status, InstallStatus::Failed { .. }))
        .collect();
    for status in &install_failures {
        if let InstallStatus::Failed { command, message } = status {
            stderr_line(&format!(
                "error: install failed ({}): {}",
                escape_controls(command),
                escape_controls(message)
            ));
        }
    }
    if !install_failures.is_empty() || !root_errors.is_empty() {
        return Ok(COULD_NOT_RUN);
    }
    Ok(run_exit_code(args, &runs, &aggregate))
}

fn roots_fix<'a>(prefixes: &[&'a Path], fixes: &'a [FixRun]) -> Vec<RootFix<'a>> {
    prefixes
        .iter()
        .zip(fixes)
        .map(|(&prefix, run)| RootFix { prefix, run })
        .collect()
}

/// The files changed, then the failure: exit 2, with the fix summary on stdout (human) or in the error document
/// (JSON), so the user learns what was written even though the run could not finish.
fn fix_incomplete(args: &LintArgs, headline: &str, error: &CliError, fixes: &[RootFix<'_>]) -> u8 {
    let error = CliError {
        message: format!("{headline}: {}", error.message),
        kind: error.kind,
        path: error.path.clone(),
        line: error.line,
        column: error.column,
    };
    if args.format() == Format::Json {
        let mut extra = Map::new();
        extra.insert("fix".to_owned(), fix::fixes_json(fixes));
        emit_error_with(&error, Format::Json, extra);
    } else {
        // Stdout carries the report in every format but `human`, so the summary goes to stderr there.
        if args.format() == Format::Human {
            let mut summary = Vec::new();
            if fix::write_fixes_human(fixes, &mut summary).is_ok() {
                let _ = output::write_stdout(&summary);
            }
        } else {
            write_summary_to_stderr(fixes);
        }
        emit_error(&error, args.format());
    }
    COULD_NOT_RUN
}

/// The human account of a fix run, for formats whose stdout is a machine-readable report.
fn write_summary_to_stderr(fixes: &[RootFix<'_>]) {
    let mut summary = Vec::new();
    if fix::write_fixes_human(fixes, &mut summary).is_ok() {
        stderr_line(String::from_utf8_lossy(&summary).trim_end());
    }
}
