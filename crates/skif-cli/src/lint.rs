//! `skif [PATH]`: lint, optionally fix, report, and choose the exit code.

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::ColorChoice;
use serde_json::{Map, Value};
use skif::render::RenderOptions;
use skif::{
    CachedFs, Config, ConfigOverrides, DiscoverOptions, FileSystem, FixPlan, IgnoreEntry, Linter, ModuleCache, OsFs,
    Prefer, Report, Selector, Target, Workspace, relative_tsconfig,
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
use crate::module_cache::{CacheHost, Persistent};
use crate::output::{self, Rendering, RootView, emit_error, emit_error_with, stderr_line};
use crate::roots::{self, Linters, Resolved, RootError, RootRun};
use crate::trace::{self, Trace};
use crate::write::write_plan;

const CLEAN: u8 = 0;
const PROBLEMS: u8 = 1;
const COULD_NOT_RUN: u8 = 2;

/// What happens to the memory of a run when [`run`] returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Teardown {
    /// Free it, as a host that keeps running must.
    Free,
    /// Leave it to the operating system: only for a process that exits next (the `skif` binary).
    Leak,
}

/// Runs the lint command and returns the process exit code. With `--cache`, the run reads and writes the store of
/// parse results.
pub fn run(args: &LintArgs, env: Env, teardown: Teardown) -> u8 {
    let persistent = args.cache.then(|| Persistent::new(args));
    run_with_cache(
        args,
        env,
        teardown,
        persistent.as_ref().map(|host| host as &dyn CacheHost),
    )
}

/// Like [`run`], parsing through the module cache of `modules`, which is told when the run has finished, whatever
/// its outcome: `--watch` keeps one host for every round.
pub fn run_with_cache(args: &LintArgs, env: Env, teardown: Teardown, modules: Option<&dyn CacheHost>) -> u8 {
    let trace = Trace::start(args);
    let result = match &args.trace_dependency {
        Some(name) => explain::trace_dependency(args, name).map(|()| CLEAN),
        None => execute(args, env, teardown, modules),
    };
    let code = match result {
        Ok(code) => code,
        Err(error) => run_failed(args, &error),
    };
    if let Some(modules) = modules {
        timing::phase("module cache", || modules.finish());
    }
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
pub(crate) fn simplified(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => PathBuf::from(rest),
        _ => path,
    }
}

#[cfg(not(windows))]
pub(crate) const fn simplified(path: PathBuf) -> PathBuf {
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

/// Fails on the first suppression without a `reason` (an `[[ignore]]` entry or an entry of a `[js]` ignore list) when
/// `--require-ignore-reasons` or `[lint] require-ignore-reasons` asks for reasons; the flag is named when both do.
fn require_reasons(args: &LintArgs, config: &Config) -> Result<(), CliError> {
    let required_by = if args.require_ignore_reasons {
        "--require-ignore-reasons"
    } else if config.lint.require_ignore_reasons {
        "[lint] require-ignore-reasons"
    } else {
        return Ok(());
    };
    match config.missing_ignore_reason() {
        Some(entry) => Err(CliError::new(
            Kind::Config,
            format!(
                "{entry}: no `reason`, which {required_by} requires (the position is in the list merged from every \
                 config file and its `extend-*` keys)"
            ),
        )),
        None => Ok(()),
    }
}

/// The flags that are not plain configuration overrides, applied over a loaded configuration.
pub(crate) fn adjust(args: &LintArgs, config: &mut Config) -> Result<(), CliError> {
    // Before the flags add entries of their own, which have nowhere to carry a reason.
    require_reasons(args, config)?;
    for (rule, package) in &args.ignore_rule_for_package {
        config.ignore.push(
            IgnoreEntry::default()
                .with_rules([rule.clone()])
                .with_packages(Selector::new([package.clone()])?),
        );
    }
    crate::instances::apply_filter(&args.filter, config)?;
    if args.strict {
        config.js.isolate_workspaces = true;
    }
    if args.no_gitignore {
        config.js.no_gitignore = true;
    }
    if args.use_tsconfig_files {
        config.js.use_tsconfig_files = true;
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

/// Whether `path` names an existing file under `root`, read the way the JS analysis reads it ([`relative_tsconfig`]).
/// `None` for an empty, absolute or escaping path: the configuration check rejects those, naming the setting.
fn tsconfig_exists(root: &Path, path: &str) -> Option<bool> {
    relative_tsconfig(path).map(|relative| root.join(relative).is_file())
}

/// Applies `--tsconfig` and checks that every root's TypeScript configuration exists.
///
/// The flag names a file relative to each root; in a multi-root run it applies to the roots that have that file and
/// the others keep their own `[js] tsconfig`. A `[js] tsconfig` must exist in the root whose configuration sets it.
/// Roots without JavaScript analysis (`--no-js`, `[js] enabled = false`) read no tsconfig, so they are skipped.
///
/// # Errors
/// A configuration error naming the path when `--tsconfig` exists in no root that runs the JavaScript analysis, or
/// a root's `[js] tsconfig` does not exist there.
pub(crate) fn apply_tsconfig(args: &LintArgs, targets: &mut [Target]) -> Result<(), CliError> {
    if let Some(path) = &args.tsconfig {
        let mut found = false;
        let mut searched = Vec::new();
        for target in targets.iter_mut().filter(|target| target.config.js.enabled) {
            if tsconfig_exists(&target.root, path) != Some(false) {
                target.config.js.tsconfig = Some(path.clone());
                found = true;
            }
            searched.push(target.root.display().to_string());
        }
        if !found && !searched.is_empty() {
            return Err(CliError::new(
                Kind::Config,
                format!("--tsconfig: `{path}` does not exist in {}", searched.join(", ")),
            ));
        }
    }
    for target in targets.iter().filter(|target| target.config.js.enabled) {
        if let Some(path) = &target.config.js.tsconfig
            && tsconfig_exists(&target.root, path) == Some(false)
        {
            return Err(CliError::new(
                Kind::Config,
                format!("[js] tsconfig: `{path}` does not exist in {}", target.root.display()),
            ));
        }
    }
    Ok(())
}

/// Warns on stderr about the packages whose TypeScript configuration selects none of their files under
/// `--use-tsconfig-files`, since nothing of them can then be reported unused.
fn warn_empty_tsconfig_projects(targets: &[Target], fs: &dyn FileSystem) {
    for target in targets {
        for config in skif::empty_tsconfig_projects(&target.workspace, &target.config, fs) {
            stderr_line(&format!(
                "warning: {}: selects no file of its package (`files`, `include`), so --use-tsconfig-files leaves \
                 that package without project files",
                escape_controls(&crate::error::slashes(target.root.join(config).display().to_string()))
            ));
        }
    }
}

/// Lints the root of `previous` again from disk. What root discovery decided about it (a standalone package nested
/// below another root, members another root lints) is carried over, so the re-lint judges the root as the first
/// lint did.
fn lint_once(
    linter: &Linter,
    previous: &Workspace,
    (config, config_path, extra_entries): (&Config, Option<&Path>, &[PathBuf]),
    fs: &dyn FileSystem,
    filter: &Filter,
) -> Result<(Workspace, Report), CliError> {
    let mut workspace = Workspace::discover_with_fs(&previous.root, config, fs)?;
    workspace.set_nested(previous.is_nested());
    let ignored: BTreeSet<PathBuf> = previous
        .package_ids()
        .filter(|id| previous.is_ignored(*id))
        .filter_map(|id| previous.package(id))
        .map(|package| previous.root.join(&package.dir))
        .collect();
    workspace.ignore_dirs(&ignored);
    let report = roots::lint_loaded(linter, &workspace, config_path, extra_entries)?;
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
    (config, config_path, extra_entries): (&Config, Option<&Path>, &[PathBuf]),
    state: (Workspace, Report),
    plan: FixPlan,
    fs: &CachedFs<OsFs>,
) -> Result<(FixRun, Relint), CliError> {
    let (workspace, report) = state;
    write_plan(&plan, &workspace.root)?;
    if plan.is_empty() {
        let install = install_step(args, &[], &workspace);
        return Ok((
            FixRun {
                plan,
                install,
                preview: None,
            },
            Ok((workspace, report)),
        ));
    }
    // The written files are stale in the cache; the re-lint must read them from disk.
    fs.clear();
    Ok(
        match lint_once(linter, &workspace, (config, config_path, extra_entries), fs, filter) {
            Ok((workspace, report)) => {
                let changed: Vec<&Path> = plan.paths().collect();
                let install = install_step(args, &changed, &workspace);
                (
                    FixRun {
                        plan,
                        install,
                        preview: None,
                    },
                    Ok((workspace, report)),
                )
            }
            Err(error) => (FixRun::relint_failed(plan), Err(error)),
        },
    )
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
            extra_entries,
        } = run;
        let written = write_and_relint(
            (args, filter),
            linters.get(linter),
            (&config, config_path.as_deref(), &extra_entries),
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
                extra_entries,
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

/// Plans the fixes of every root and describes them without writing anything: what `--dry-run` shows.
fn plan_roots(
    args: &LintArgs,
    env: Env,
    runs: &[RootRun],
    fs: &CachedFs<OsFs>,
    only: Option<&BTreeSet<String>>,
) -> Result<Vec<FixRun>, CliError> {
    runs.iter()
        .map(|run| {
            let selection = fix::selection(args, env, &run.report, run.config.policy.prefer, only)?;
            let plan = skif::apply_fixes(&run.workspace, &run.report, &selection, fs)?;
            let preview = fix::preview(&plan, &run.workspace.root, fs)?;
            Ok(FixRun::dry_run(plan, preview))
        })
        .collect()
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
    lint_to_json_with(path, overrides, recursive, None)
}

/// [`lint_to_json`], parsing through `modules` when given (`skif mcp` keeps one per path).
pub(crate) fn lint_to_json_with(
    path: &Path,
    overrides: &ConfigOverrides,
    recursive: bool,
    modules: Option<&Arc<dyn ModuleCache>>,
) -> Result<String, CliError> {
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
    let linters = Linters::build_with(&targets, None, modules)?;
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

/// Lints the root containing `path`, reading every file through `fs`, and returns the directory the report's paths
/// are relative to, with the report and the runs it was aggregated from: the language server plans code actions from
/// a run's workspace and report. Source files are parsed through the cache `modules` gives for that directory.
/// Prints and writes nothing, like [`lint_to_json`].
pub fn lint_root(
    path: &Path,
    overrides: &ConfigOverrides,
    fs: &Arc<dyn FileSystem>,
    modules: Option<&dyn CacheHost>,
) -> Result<(PathBuf, Report, Vec<RootRun>), CliError> {
    let start = canonical(path)?;
    let Resolved { base, targets, .. } = roots::resolve(&start, false, &CachedFs::new(Arc::clone(fs)), overrides)?;
    let modules = modules.map(|host| host.modules(&base));
    let linters = Linters::build_with(&targets, Some(fs), modules.as_ref())?;
    let runs = roots::lint_targets(&base, targets, &linters)?;
    let reports: Vec<Cow<'_, Report>> = runs
        .iter()
        .map(|run| roots::prefixed(&run.report, &run.prefix))
        .collect();
    let report = roots::aggregate(&reports).into_owned();
    Ok((base, report, runs))
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

fn execute(args: &LintArgs, env: Env, teardown: Teardown, modules: Option<&dyn CacheHost>) -> Result<u8, CliError> {
    let fs = CachedFs::new(OsFs);
    let targets_out = args.targets()?;
    if args.dry_run && !args.fixing() {
        return Err(CliError::new(Kind::Usage, "--dry-run needs --fix or --fix-unsafe"));
    }
    // A dry run writes nothing, so it is allowed where fixing is not.
    if args.fixing() && !args.dry_run() && env.ci {
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
    apply_tsconfig(args, &mut targets)?;
    warn_empty_tsconfig_projects(&targets, &fs);
    let modules = modules.map(|host| host.modules(&base));
    let linters = Linters::build_with(&targets, None, modules.as_ref())?;
    // An unknown `--fix-type` is a usage error before any root is linted.
    let only = fix::fix_type_rules(&args.fix_type, linters.rules().into_iter().map(|meta| meta.id))?;
    let mut runs = timing::phase("lint roots", || roots::lint_targets(&base, targets, &linters))?;
    filter.apply_runs(&mut runs)?;

    let mut fixes: Vec<FixRun> = Vec::new();
    if args.dry_run() {
        fixes = plan_roots(args, env, &runs, &fs, only.as_ref())?;
    } else if args.fixing() {
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
    // `--max-show-issues` cuts a copy of the per-root reports (cheap: they are borrowed), so the full `aggregate`
    // keeps borrowing the originals.
    let cut: Option<Vec<Cow<'_, Report>>> = args.max_show_issues.map(|max| {
        let (cut, hidden) = filter::truncate_shown(reports.clone(), max);
        if hidden > 0 && args.format() == Format::Human {
            stderr_line(&format!(
                "note: --max-show-issues {max} left {hidden} finding{} out of the list",
                if hidden == 1 { "" } else { "s" }
            ));
        }
        cut
    });
    let shown = cut.as_ref().map(|cut| {
        let mut shown = roots::aggregate(cut).into_owned();
        shown.summary = aggregate.summary;
        shown
    });
    let listed = cut.as_ref().unwrap_or(&reports);

    let views: Vec<RootView<'_>> = runs
        .iter()
        .zip(listed)
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
    let code = run_exit_code(args, &runs, &aggregate);
    if teardown == Teardown::Leak {
        // The process exits next: freeing every diagnostic, the workspace model and the file caches one allocation at
        // a time is a measurable part of a large run, and the operating system reclaims the memory at once.
        std::mem::forget(views);
        std::mem::forget(aggregate);
        std::mem::forget(shown);
        std::mem::forget(cut);
        std::mem::forget(reports);
        std::mem::forget(runs);
        std::mem::forget(linters);
        std::mem::forget(fs);
    }
    Ok(code)
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
/// written before it in place. A run with stdout as its only output streams the report instead (see
/// [`output::render_to_stdout`]), so a renderer error there can leave a partial report on stdout.
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
    // A run whose only output is stdout streams it: nothing else is written, so there is no file to keep untouched.
    if let [(format, None)] = targets {
        rendering.format = *format;
        rendering.color = color_enabled(args, env, *format, None);
        let listed = shown.filter(|_| *format == Format::Json).unwrap_or(report);
        return timing::phase("render", || {
            output::render_to_stdout(listed, views, &rendering, fs, fixes)
        });
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
