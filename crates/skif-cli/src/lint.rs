//! `skif [PATH]`: lint, optionally fix, report, and choose the exit code.

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use clap::ColorChoice;
use serde_json::{Map, Value};
use skif::render::RenderOptions;
use skif::{
    CachedFs, Config, ConfigOverrides, DiscoverOptions, FileSystem, FixPlan, IgnoreEntry, Linter, OsFs, Prefer, Report,
    Selector, Workspace,
};
use skif_core::timing;
use skif_report::{escape_controls, render_json_with};

use crate::args::{Format, LintArgs, Select};
use crate::env::{Env, github_workspace};
use crate::error::{CliError, Kind};
use crate::explain;
use crate::filter::{self, Filter};
use crate::fix::{self, FixRun, RootFix};
use crate::install::{self, InstallStatus};
use crate::output::{self, Rendering, RootView, emit_error, emit_error_with, stderr_line};
use crate::roots::{self, Linters, Resolved, RootError, RootRun};
use crate::trace::{self, Trace};
use crate::write::write_plan;

const CLEAN: u8 = 0;
const PROBLEMS: u8 = 1;
const COULD_NOT_RUN: u8 = 2;

/// Runs the lint command and returns the process exit code.
pub fn run(args: &LintArgs, env: Env) -> u8 {
    let trace = Trace::start(args);
    let result = match &args.trace_dependency {
        Some(name) => explain::trace_dependency(args, name).map(|()| CLEAN),
        None => execute(args, env),
    };
    let code = match result {
        Ok(code) => code,
        Err(error) => run_failed(args, &error),
    };
    trace.finish();
    code
}

/// Reports `error` in the format of the run and returns the exit code of a run that could not start.
pub fn run_failed(args: &LintArgs, error: &CliError) -> u8 {
    emit_error(error, args.format());
    COULD_NOT_RUN
}

/// sherif's `-r` is `--ignore-rule`, skif's is `--recursive`, so `skif -r <rule>` reads the rule as the path. When the
/// path does not exist and names a sherif or skif rule, the run fails with a usage error that says so, instead of an
/// I/O error about a missing directory.
fn rule_given_as_path(path: &Path) -> Result<(), CliError> {
    if path.exists() {
        return Ok(());
    }
    let Some(name) = path.to_str() else {
        return Ok(());
    };
    let id = match skif_migrate::sherif_rule_id(name) {
        Some(id) => id,
        None => match Linter::new(&Config::default())?.rules().find(|meta| meta.id == name) {
            Some(meta) => meta.id,
            None => return Ok(()),
        },
    };
    Err(CliError::new(
        Kind::Usage,
        format!(
            "`{name}` is a rule, not a path: `-r` is `--recursive` in skif, not sherif's `--ignore-rule`; \
             use `--ignore-rule {id}` to turn the rule off, or `--rule {id}=<level>` to set its level"
        ),
    ))
}

/// Whether GitHub annotations accompany the human output: with no `--format`, `GITHUB_ACTIONS=true` selects human
/// output plus annotations (ADR 0012).
fn annotations(args: &LintArgs, env: Env) -> bool {
    args.format.is_empty() && env.github_actions
}

/// A path as the OS resolves it: symlinks and `..` gone, and no Windows `\\?\` prefix on a plain drive path.
pub fn canonical(path: &Path) -> Result<PathBuf, CliError> {
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

pub fn overrides(args: &LintArgs) -> ConfigOverrides {
    ConfigOverrides::default()
        .with_rules(args.rules.iter().cloned())
        .with_ignore_rules(args.ignore_rule.clone())
        .with_ignore_packages(args.ignore_package.clone())
        .with_ignore_dependencies(args.ignore_dependency.clone())
        .with_fail_on_warnings(args.fail_on_warnings.then_some(true))
        .with_production((args.production || args.strict).then_some(true))
        .with_no_js(args.no_js)
        .with_config_file(
            args.config
                .as_ref()
                .map(|file| std::path::absolute(file).unwrap_or_else(|_| file.clone())),
        )
        .with_no_config(args.no_config)
}

/// Fails on the first `[[ignore]]` entry without a `reason`.
fn require_reasons(config: &Config) -> Result<(), CliError> {
    match config.ignore.iter().position(|entry| entry.reason.is_none()) {
        Some(index) => Err(CliError::new(
            Kind::Config,
            format!("ignore[{index}]: no `reason`, which --require-ignore-reasons requires"),
        )),
        None => Ok(()),
    }
}

/// The flags that are not plain configuration overrides, applied over a loaded configuration.
pub(crate) fn adjust(args: &LintArgs, config: &mut Config) -> Result<(), CliError> {
    // Before the flags add entries of their own, which have nowhere to carry a reason.
    if args.require_ignore_reasons {
        require_reasons(config)?;
    }
    for (rule, package) in &args.ignore_rule_for_package {
        config.ignore.push(
            IgnoreEntry::default()
                .with_rules([rule.clone()])
                .with_packages(Selector::new([package.clone()])?),
        );
    }
    if args.strict {
        config.js.isolate_workspaces = true;
    }
    if args.no_gitignore {
        config.js.no_gitignore = true;
    }
    if args.include_entry_exports {
        config.js.include_entry_exports = true;
        for workspace in config.js.workspaces.values_mut() {
            if workspace.include_entry_exports.is_some() {
                workspace.include_entry_exports = Some(true);
            }
        }
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
    filter: &Filter,
) -> Result<(Workspace, Report), CliError> {
    let workspace = Workspace::discover_with_fs(root, config, fs)?;
    let report = roots::lint_loaded(linter, &workspace, config_path)?;
    let report = filter.apply(&workspace, report);
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

/// Whether the rendering of `format` to `file` is coloured: only the rendering of [`LintArgs::format`] can be, and
/// only when it is `human` and goes to stdout or `--output`; a `FORMAT=FILE` rendering never is (ADR 0054).
fn color_enabled(args: &LintArgs, env: Env, format: Format, file: Option<&Path>) -> bool {
    let primary = format == args.format() && (file.is_none() || args.output.is_some());
    primary
        && format == Format::Human
        && match args.color {
            ColorChoice::Always => true,
            ColorChoice::Never => false,
            ColorChoice::Auto => env.stdout_tty && !env.no_color && file.is_none(),
        }
}

/// The exit code of a run that found `report`. Errors beyond `--max-issues` (default 0) are problems;
/// `--max-warnings` bounds the warnings of the whole run, and without it each root applies its own
/// `fail-on-warnings`. `--no-exit-code` makes every outcome clean.
fn run_exit_code(args: &LintArgs, runs: &[RootRun], aggregate: &Report) -> u8 {
    if args.no_exit_code {
        return CLEAN;
    }
    if aggregate.summary.errors > args.max_issues.unwrap_or(0) {
        return PROBLEMS;
    }
    let too_many_warnings = match args.max_warnings {
        Some(max) => aggregate.summary.warnings > max,
        None => runs
            .iter()
            .any(|run| run.config.lint.fail_on_warnings && run.report.summary.warnings > 0),
    };
    if too_many_warnings { PROBLEMS } else { CLEAN }
}

/// Plans, writes and describes the fixes of one root; the second value is the lint of the tree after fixing, or the
/// error that stopped it (the files are already changed by then, so the summary is still owed to the user).
type Relint = Result<(Workspace, Report), CliError>;

fn write_and_relint(
    (args, filter): (&LintArgs, &Filter),
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
    Ok(match lint_once(linter, &root, config, config_path, fs, filter) {
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
/// what was already written. `only` holds the rules `--fix-type` names, if given.
fn fix_roots(
    (args, filter): (&LintArgs, &Filter),
    env: Env,
    linters: &Linters,
    runs: Vec<RootRun>,
    fs: &CachedFs<OsFs>,
    only: Option<&BTreeSet<String>>,
) -> Result<Fixed, CliError> {
    let mut planned = Vec::with_capacity(runs.len());
    for run in runs {
        let selection = fix::selection(args, env, &run.report, run.config.policy.prefer, only)?;
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
            (args, filter),
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
    let fs = CachedFs::new(OsFs);
    let start = canonical(path)?;
    let Resolved {
        base,
        targets,
        multi,
        failures,
        truncated,
        ..
    } = timing::phase("root discovery (roots, configs, workspaces)", || {
        roots::resolve(&start, recursive, &fs, overrides)
    })?;
    let linters = Linters::build(&targets)?;
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
        if !truncated.is_empty() {
            extra.insert("truncated".to_owned(), truncated_json(&truncated));
        }
    }
    let mut buffer = Vec::new();
    render_json_with(&aggregate, &options, extra, &mut buffer)
        .map_err(|error| CliError::new(Kind::Io, format!("cannot render the report: {error}")))?;
    String::from_utf8(buffer).map_err(|error| CliError::new(Kind::Io, format!("the report is not UTF-8: {error}")))
}

/// Lints the root containing `path` and returns the directory the report's paths are relative to, with the report.
/// Prints and writes nothing, like [`lint_to_json`].
pub fn lint_root(path: &Path, overrides: &ConfigOverrides) -> Result<(PathBuf, Report), CliError> {
    let fs = CachedFs::new(OsFs);
    let start = canonical(path)?;
    let Resolved { base, targets, .. } = roots::resolve(&start, false, &fs, overrides)?;
    let linters = Linters::build(&targets)?;
    let runs = roots::lint_targets(&base, targets, &linters)?;
    let reports: Vec<Cow<'_, Report>> = runs
        .iter()
        .map(|run| roots::prefixed(&run.report, &run.prefix))
        .collect();
    let report = roots::aggregate(&reports);
    Ok((base, report))
}

fn errors_json(failures: &[RootError]) -> Value {
    Value::Array(failures.iter().map(RootError::to_json).collect())
}

/// The directories the search for roots did not enter, as the JSON `truncated` array.
fn truncated_json(truncated: &[PathBuf]) -> Value {
    Value::Array(truncated.iter().map(|dir| Value::String(roots::display(dir))).collect())
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
    let fs = CachedFs::new(OsFs);
    let targets_out = args.targets()?;
    if args.fixing() && env.ci {
        return Err(CliError::ci_refusal());
    }
    check_max_show_issues(args, &targets_out)?;
    if args.recursive {
        rule_given_as_path(&args.path)?;
    }
    let start = canonical(&args.path)?;
    let Resolved {
        base,
        mut targets,
        multi,
        failures: root_errors,
        truncated,
    } = timing::phase("root discovery (roots, configs, workspaces)", || {
        roots::resolve(&start, args.recursive, &fs, &overrides(args))
    })?;
    note_truncated(&truncated);
    if args.debug {
        trace::debug_targets(&base, &targets);
    }
    let filter = Filter::new(args)?;
    for target in &mut targets {
        adjust(args, &mut target.config)?;
        filter.configure(&mut target.config)?;
    }
    let linters = Linters::build(&targets)?;
    // An unknown `--fix-type` is a usage error before any root is linted.
    let only = fix::fix_type_rules(&args.fix_type, linters.rules().into_iter().map(|meta| meta.id))?;
    let mut runs = timing::phase("lint roots", || roots::lint_targets(&base, targets, &linters))?;
    filter.apply_runs(&mut runs)?;

    let mut fixes: Vec<FixRun> = Vec::new();
    if args.fixing() {
        match fix_roots((args, &filter), env, &linters, runs, &fs, only.as_ref())? {
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
    let (reports, shown) = match args.max_show_issues {
        Some(max) => {
            let (reports, hidden) = filter::truncate_shown(reports, max);
            if hidden > 0 && args.format() == Format::Human {
                stderr_line(&format!(
                    "note: --max-show-issues {max} left {hidden} finding{} out of the list",
                    if hidden == 1 { "" } else { "s" }
                ));
            }
            let mut shown = roots::aggregate(&reports);
            shown.summary = aggregate.summary;
            (reports, Some(shown))
        }
        None => (reports, None),
    };

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
    let rendering = Rendering {
        format: Format::default(),
        annotations: annotations(args, env),
        color: false,
        root: &base,
        repo_root: None,
        rules: linters.rules(),
        multi,
        errors: root_errors.iter().map(RootError::to_json).collect(),
        truncated: truncated_json(&truncated),
    };
    render_and_write(
        args,
        env,
        &targets_out,
        rendering,
        (&aggregate, shown.as_ref(), &views, &root_fixes),
        &fs,
    )?;
    if !fixes.is_empty() && !matches!(args.format(), Format::Human | Format::Json) {
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

/// `--max-show-issues` shortens the list a person reads: human and JSON output only, and not when fixing, where
/// the list says what is left after the fix. Every other format is rejected rather than silently given the full
/// list: SARIF and GitHub annotations feed code scanning and checks, which must never miss a finding silently, and
/// the allowlist keeps a format added later from ignoring the flag. For the same reason the annotations human
/// output adds under GitHub Actions are rendered from every finding. With several `--format`s, every one must be
/// human or json.
fn check_max_show_issues(args: &LintArgs, targets: &[(Format, Option<PathBuf>)]) -> Result<(), CliError> {
    if args.max_show_issues.is_none() {
        return Ok(());
    }
    let reason = if args.fixing() {
        "cannot be combined with --fix or --fix-unsafe"
    } else if targets
        .iter()
        .any(|(format, _)| !matches!(format, Format::Human | Format::Json))
    {
        "applies to human and json output only; other formats always list every finding"
    } else {
        return Ok(());
    };
    Err(CliError::new(Kind::Usage, format!("--max-show-issues {reason}")))
}

/// Renders every requested format, then writes the files and stdout (ADR 0054). Every rendering is made before
/// anything is written, so a failing one leaves every file untouched; a write that fails partway leaves the files
/// written before it in place. `shown` is the report `--max-show-issues` cut short: the JSON rendering lists it, and
/// the human rendering lists the (equally cut) `views` while its annotations come from the full `report`.
fn render_and_write(
    args: &LintArgs,
    env: Env,
    targets: &[(Format, Option<PathBuf>)],
    mut rendering: Rendering<'_>,
    (report, shown, views, fixes): (&Report, Option<&Report>, &[RootView<'_>], &[RootFix<'_>]),
    fs: &dyn FileSystem,
) -> Result<(), CliError> {
    let needs_repo_root = rendering.annotations
        || targets.iter().any(|(format, _)| {
            matches!(
                format,
                Format::Github | Format::Sarif | Format::Codeclimate | Format::Codeowners
            )
        });
    if needs_repo_root {
        rendering.repo_root = repo_root(rendering.root);
    }
    let mut rendered = Vec::with_capacity(targets.len());
    for (format, file) in targets {
        rendering.format = *format;
        rendering.color = color_enabled(args, env, *format, file.as_deref());
        let listed = shown.filter(|_| *format == Format::Json).unwrap_or(report);
        let bytes = timing::phase("render", || output::render(listed, views, &rendering, fs, fixes))?;
        rendered.push((bytes, file.as_deref()));
    }
    // Files first: a failed write then still reaches a JSON consumer of stdout as the only document.
    for (bytes, file) in &rendered {
        if let Some(file) = file {
            output::emit(bytes, Some(file))?;
        }
    }
    for (bytes, file) in &rendered {
        if file.is_none() {
            output::emit(bytes, None)?;
        }
    }
    Ok(())
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use clap::Parser;

    use super::*;
    use crate::args::Cli;

    fn lint_args(flags: &[&str]) -> LintArgs {
        Cli::try_parse_from(std::iter::once("skif").chain(flags.iter().copied()))
            .unwrap()
            .lint
    }

    const TERMINAL: Env = Env {
        ci: false,
        no_color: false,
        stdin_tty: true,
        stdout_tty: true,
        github_actions: false,
    };

    #[test]
    fn only_the_primary_human_rendering_is_coloured() {
        let file = Some(Path::new("report.txt"));
        let args = lint_args(&["--format", "human", "--format", "human=report.txt"]);
        assert!(color_enabled(&args, TERMINAL, Format::Human, None));
        assert!(!color_enabled(&args, TERMINAL, Format::Human, file));

        let args = lint_args(&["--color", "always", "--format", "json", "--format", "human=report.txt"]);
        assert!(!color_enabled(&args, TERMINAL, Format::Human, file));

        // `--output` is the primary rendering: coloured only when asked for, never by the terminal alone.
        let args = lint_args(&["--format", "human", "--output", "report.txt"]);
        assert!(!color_enabled(&args, TERMINAL, Format::Human, file));
        let args = lint_args(&["--color", "always", "--format", "human", "--output", "report.txt"]);
        assert!(color_enabled(&args, TERMINAL, Format::Human, file));

        let args = lint_args(&["--format", "human"]);
        let no_color = Env {
            no_color: true,
            ..TERMINAL
        };
        assert!(!color_enabled(&args, no_color, Format::Human, None));
        let piped = Env {
            stdout_tty: false,
            ..TERMINAL
        };
        assert!(!color_enabled(&args, piped, Format::Human, None));
        let args = lint_args(&["--color", "never", "--format", "human"]);
        assert!(!color_enabled(&args, TERMINAL, Format::Human, None));
    }

    #[test]
    fn a_missing_path_named_like_a_rule_is_a_usage_error() {
        let sherif = rule_given_as_path(std::path::Path::new("multiple-dependency-versions")).unwrap_err();
        assert_eq!(sherif.kind, Kind::Usage);
        assert!(
            sherif.message.contains("--ignore-rule version-mismatch"),
            "{}",
            sherif.message
        );
        let skif = rule_given_as_path(std::path::Path::new("unused-dependencies")).unwrap_err();
        assert_eq!(skif.kind, Kind::Usage);
        assert!(
            skif.message.contains("--ignore-rule unused-dependencies"),
            "{}",
            skif.message
        );
    }

    #[test]
    fn other_paths_are_left_to_root_discovery() {
        assert!(rule_given_as_path(std::path::Path::new("no-such-directory-here")).is_ok());
        assert!(rule_given_as_path(std::path::Path::new(".")).is_ok());
    }
}
