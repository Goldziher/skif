//! Resolving what to lint (one workspace, or every root below a directory) and linting the roots in parallel.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use rayon::prelude::*;
use serde_json::{Value, json};
use skif::{
    ConfigJudgement, ConfigOverrides, Diagnostic, FileSystem, Linter, Location, ModuleCache, OsFs, Report, RuleMeta,
    RunOptions, Target, Workspace, slash,
};

use crate::error::{CliError, Kind};

/// A root of a multi-root run that could not be loaded; the other roots are still linted.
pub struct RootError {
    /// The root relative to the run's base.
    pub prefix: PathBuf,
    pub error: CliError,
}

impl RootError {
    /// The root and the reason, for the terminal.
    pub fn describe(&self) -> String {
        format!("{}: {}", display(&self.prefix), self.error.message)
    }

    /// An entry of the JSON `errors` array.
    pub fn to_json(&self) -> Value {
        json!({
            "path": display(&self.prefix),
            "kind": self.error.kind.as_str(),
            "message": self.error.message,
        })
    }
}

/// The roots to lint and the directory their diagnostic paths are relative to.
pub struct Resolved {
    /// The workspace root for a single-workspace run; the invocation directory for a multi-root run.
    pub base: PathBuf,
    pub targets: Vec<Target>,
    /// Whether this is a multi-root run: it reports `roots` and groups human output by root.
    pub multi: bool,
    /// Roots found by a multi-root run that could not be loaded.
    pub failures: Vec<RootError>,
    /// Directories (relative to the base) the walk of a multi-root run did not enter because of the depth limit.
    pub truncated: Vec<PathBuf>,
}

fn directory_of(start: &Path, fs: &dyn FileSystem) -> PathBuf {
    if fs.is_dir(start) {
        start.to_path_buf()
    } else {
        start.parent().unwrap_or(start).to_path_buf()
    }
}

/// `root` relative to `base`. A root that is not below the base (the two differ in form, as `\\?\` paths do on
/// Windows) keeps its full path, so roots never collapse into the base and their diagnostics stay apart.
fn prefix_of(base: &Path, root: &Path) -> PathBuf {
    root.strip_prefix(base).unwrap_or(root).to_path_buf()
}

/// Decides between the workspace containing `start` and every root below it.
///
/// A multi-root run is chosen by `recursive`, and also when neither `start` nor an ancestor up to the root of its git
/// repository holds a workspace or a `package.json`, so `skif .` at the top of a repository of independent projects
/// lints each of them.
pub fn resolve(
    start: &Path,
    recursive: bool,
    fs: &dyn FileSystem,
    overrides: &ConfigOverrides,
) -> Result<Resolved, CliError> {
    let found = if recursive {
        None
    } else {
        match skif::find_target(start, fs, overrides) {
            Ok(target) => Some(target),
            Err(skif::Error::NoWorkspace(_)) => None,
            Err(error) => return Err(error.into()),
        }
    };
    if let Some(target) = found {
        return Ok(Resolved {
            base: target.root.clone(),
            targets: vec![target],
            multi: false,
            failures: Vec::new(),
            truncated: Vec::new(),
        });
    }
    let base = directory_of(start, fs);
    let discovery = skif::discover_targets(start, fs, overrides)?;
    let mut failures = discovery.failures.into_iter();
    if discovery.targets.is_empty() {
        return Err(match failures.next() {
            Some(failure) => {
                let mut error = CliError::from(failure.error);
                error.message = format!("{}: {}", display(&prefix_of(&base, &failure.root)), error.message);
                error
            }
            None => CliError::new(
                Kind::Workspace,
                format!("no workspace or package found at or below {}", start.display()),
            ),
        });
    }
    Ok(Resolved {
        failures: failures
            .map(|failure| RootError {
                prefix: prefix_of(&base, &failure.root),
                error: failure.error.into(),
            })
            .collect(),
        truncated: discovery.truncated.iter().map(|dir| prefix_of(&base, dir)).collect(),
        base,
        targets: discovery.targets,
        multi: true,
    })
}

/// The distinct linters of a run: roots with equal configuration share one.
pub struct Linters {
    linters: Vec<Linter>,
    of_target: Vec<usize>,
}

impl Linters {
    /// Builds one linter per distinct configuration. They read the disk themselves: the cache a linter keeps over its
    /// file system serves all the roots it lints, and a second layer under it would outlive a run.
    pub fn build(targets: &[Target]) -> Result<Self, CliError> {
        Self::build_with(targets, None, None)
    }

    /// Like [`Linters::build`], reading through `fs` instead of the disk (the language server's unsaved buffers) and
    /// parsing source files through `modules` when given.
    pub fn build_with(
        targets: &[Target],
        fs: Option<&Arc<dyn FileSystem>>,
        modules: Option<&Arc<dyn ModuleCache>>,
    ) -> Result<Self, CliError> {
        let mut linters: Vec<Linter> = Vec::new();
        let mut of_target = Vec::with_capacity(targets.len());
        for target in targets {
            let index = if let Some(index) = linters.iter().position(|linter| linter.config() == &target.config) {
                index
            } else {
                let fs = fs.map_or_else(|| Arc::new(OsFs) as Arc<dyn FileSystem>, Arc::clone);
                linters.push(match modules {
                    Some(modules) => Linter::with_module_cache(&target.config, fs, Arc::clone(modules))?,
                    None => Linter::with_fs(&target.config, fs)?,
                });
                linters.len() - 1
            };
            of_target.push(index);
        }
        Ok(Self { linters, of_target })
    }

    pub fn get(&self, index: usize) -> &Linter {
        &self.linters[index]
    }

    /// The rules of every linter, once each, for SARIF.
    pub fn rules(&self) -> Vec<&'static RuleMeta> {
        let mut rules: BTreeMap<&'static str, &'static RuleMeta> = BTreeMap::new();
        for meta in self.linters.iter().flat_map(Linter::rules) {
            rules.entry(meta.id).or_insert(meta);
        }
        rules.into_values().collect()
    }
}

/// One linted root. The report is relative to the root, which is what fixing needs.
pub struct RootRun {
    /// The root relative to the run's base; empty for a single-workspace run.
    pub prefix: PathBuf,
    pub config: skif::Config,
    pub config_path: Option<PathBuf>,
    pub linter: usize,
    pub workspace: Workspace,
    pub report: Report,
    /// The files of this root that enclosing roots of the run import or run, relative to the root; a re-lint after
    /// fixing passes them again.
    pub extra_entries: Vec<PathBuf>,
    /// What the code of other roots of the run uses of this root; a re-lint after fixing credits it again.
    pub other_root_uses: OtherRootUses,
    /// The `stale-js-config` findings of the report that the decision across roots took out of it, because another
    /// root reports them or used the setting they call stale (see [`settle_shared_config_findings`]).
    pub withheld: Vec<Diagnostic>,
}

/// What the code of other roots of a multi-root run uses of a root, which the root's own lint cannot see.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OtherRootUses {
    /// Dependencies of the root's manifests that other roots use: those the code of roots nested in it imports or
    /// runs without listing them, and those the files of this root in `files` import. The manifest path relative to
    /// the root, and the dependency name.
    pub dependencies: BTreeSet<(PathBuf, String)>,
    /// Files of the root, relative to it, that the code of a root outside whose directory they lie reaches (a nested
    /// root requiring `../tools/detect.js`, a sibling root), with everything those files import in turn.
    pub files: BTreeSet<PathBuf>,
}

impl OtherRootUses {
    fn is_empty(&self) -> bool {
        self.dependencies.is_empty() && self.files.is_empty()
    }
}

/// The rules that a use by a nested root answers.
const UNUSED_DEPENDENCY_RULES: [&str; 2] = ["unused-dependencies", "unused-dev-dependencies"];

/// The rule whose findings about a configuration file several roots read are decided across those roots.
const STALE_JS_CONFIG: &str = "stale-js-config";

/// The report without the findings that `uses` answers: the unused-dependency findings of dependencies a nested root
/// imports or runs without listing them (Node.js resolves them from this root's `node_modules`) or that a file of
/// this root another root's code reaches imports, and the `unused-files` findings of those files.
pub fn credit_other_root_uses(report: Report, uses: &OtherRootUses) -> Report {
    if uses.is_empty() {
        return report;
    }
    report.retain(|diagnostic| {
        let used_dependency = UNUSED_DEPENDENCY_RULES.contains(&diagnostic.rule.as_str())
            && diagnostic.dependency.as_ref().is_some_and(|name| {
                uses.dependencies
                    .contains(&(diagnostic.location.path.clone(), name.clone()))
            });
        let used_file = diagnostic.rule.as_str() == "unused-files" && uses.files.contains(&diagnostic.location.path);
        !(used_dependency || used_file)
    })
}

/// The `stale-js-config` findings of each root, as the location path relative to the root and the message, that the
/// decision across roots takes out of its report (ADR 0099). An entry of a configuration list that two or more roots
/// judged, by the absolute file, the key as written and the entry ([`Report::config_judgements`]), is stale only when
/// every one of them found it stale: a root that used it drops the finding everywhere, and a root that could not
/// judge it has no say. A stale one is kept by the first root in `roots` (path order) that reports it, and taken out
/// of the others. Any other finding at the same absolute path with the same message is kept by the first root that
/// reports it.
fn shared_config_drops(roots: &[(&Path, &Report)]) -> Vec<BTreeSet<(PathBuf, String)>> {
    let mut drops: Vec<BTreeSet<(PathBuf, String)>> = vec![BTreeSet::new(); roots.len()];
    let mut settings: BTreeMap<(PathBuf, &str, &str), Vec<(usize, &ConfigJudgement)>> = BTreeMap::new();
    for (index, (root, report)) in roots.iter().enumerate() {
        for judgement in &report.config_judgements {
            settings
                .entry((join(root, &judgement.file), judgement.key.as_str(), judgement.entry.as_str()))
                .or_default()
                .push((index, judgement));
        }
    }
    let reports = |index: usize, judgement: &ConfigJudgement| {
        roots[index].1.diagnostics.iter().any(|diagnostic| {
            diagnostic.rule.as_str() == STALE_JS_CONFIG
                && diagnostic.location.path == judgement.file
                && diagnostic.message == judgement.message
        })
    };
    for judgements in settings.values() {
        let judging: BTreeSet<usize> = judgements.iter().map(|(index, _)| *index).collect();
        if judging.len() < 2 {
            continue;
        }
        let used = judgements.iter().any(|(_, judgement)| !judgement.stale);
        let keeper = judgements
            .iter()
            .find(|(index, judgement)| !used && judgement.stale && reports(*index, *judgement))
            .map(|(index, _)| *index);
        for (index, judgement) in judgements {
            if judgement.stale && keeper != Some(*index) {
                drops[*index].insert((judgement.file.clone(), judgement.message.clone()));
            }
        }
    }
    let mut first_reporter: BTreeMap<(PathBuf, &str), usize> = BTreeMap::new();
    for (index, (root, report)) in roots.iter().enumerate() {
        let judged: BTreeSet<(&Path, &str)> = report
            .config_judgements
            .iter()
            .map(|judgement| (judgement.file.as_path(), judgement.message.as_str()))
            .collect();
        for diagnostic in report
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.rule.as_str() == STALE_JS_CONFIG)
            .filter(|diagnostic| !judged.contains(&(diagnostic.location.path.as_path(), diagnostic.message.as_str())))
        {
            let first = *first_reporter
                .entry((join(root, &diagnostic.location.path), diagnostic.message.as_str()))
                .or_insert(index);
            if first != index {
                drops[index].insert((diagnostic.location.path.clone(), diagnostic.message.clone()));
            }
        }
    }
    drops
}

/// Decides the `stale-js-config` findings about configuration files several roots of the run read, once every root
/// is linted, and again after fixing (see [`shared_config_drops`]). The findings a decision takes out of a report are
/// kept in [`RootRun::withheld`], and the next decision starts from the full reports again, so a root that was not
/// linted again keeps its say.
pub fn settle_shared_config_findings(runs: &mut [RootRun]) {
    for run in runs.iter_mut() {
        let withheld = std::mem::take(&mut run.withheld);
        let report = std::mem::replace(&mut run.report, Report::new(Vec::new(), 0));
        run.report = report.extended(withheld);
    }
    let drops = {
        let roots: Vec<(&Path, &Report)> = runs
            .iter()
            .map(|run| (run.workspace.root.as_path(), &run.report))
            .collect();
        shared_config_drops(&roots)
    };
    for (run, dropped) in runs.iter_mut().zip(drops) {
        if dropped.is_empty() {
            continue;
        }
        let mut withheld = Vec::new();
        let report = std::mem::replace(&mut run.report, Report::new(Vec::new(), 0));
        run.report = report.retain(|diagnostic| {
            let decided = diagnostic.rule.as_str() == STALE_JS_CONFIG
                && dropped.contains(&(diagnostic.location.path.clone(), diagnostic.message.clone()));
            if decided {
                withheld.push(diagnostic.clone());
            }
            !decided
        });
        run.withheld = withheld;
    }
}

/// Hands the dependencies that the report of root `from` uses through manifests outside it (above it, or of the files
/// of other roots it reaches) to the innermost root holding each manifest, as the manifest path relative to that root.
fn route_ancestor_dependency_uses(targets: &[Target], from: usize, report: &Report, uses: &mut [OtherRootUses]) {
    for (manifest_dir, name) in &report.ancestor_dependency_uses {
        let Some(owner) = owning_root(targets, manifest_dir).filter(|owner| *owner != from) else {
            continue;
        };
        if let Ok(rel) = manifest_dir.strip_prefix(&targets[owner].root) {
            uses[owner]
                .dependencies
                .insert((rel.join("package.json"), name.clone()));
        }
    }
}

pub fn lint_loaded(
    linter: &Linter,
    workspace: &Workspace,
    config_path: Option<&Path>,
    extra_entries: &[PathBuf],
) -> Result<Report, CliError> {
    let mut options = RunOptions::default().with_extra_entries(extra_entries);
    if let Some(path) = config_path {
        options = options.with_config_path(path);
    }
    Ok(linter.run_with(workspace, &options)?)
}

/// How many other roots of the run enclose each root: a root nested in no other is at depth 0.
fn nesting_depths(targets: &[Target]) -> Vec<usize> {
    targets
        .iter()
        .map(|target| {
            targets
                .iter()
                .filter(|other| other.root != target.root && target.root.starts_with(&other.root))
                .count()
        })
        .collect()
}

/// The innermost root of `targets` that holds the absolute file `file`, by index.
fn owning_root(targets: &[Target], file: &Path) -> Option<usize> {
    targets
        .iter()
        .enumerate()
        .filter(|(_, target)| file.starts_with(&target.root))
        .max_by_key(|(_, target)| target.root.components().count())
        .map(|(index, _)| index)
}

/// Hands the files of other projects that the report of root `from` lists (absolute) to the innermost root holding
/// each, as paths relative to that root. A file below `from` belongs to a root nested in it, which is linted later and
/// takes it as an entry; a file outside `from` is counted as used by its root once every root is linted, as that root
/// may be linted already.
fn route_other_project_files(
    targets: &[Target],
    (from, report): (usize, &Report),
    extra: &mut [BTreeSet<PathBuf>],
    uses: &mut [OtherRootUses],
) {
    let base = &targets[from].root;
    for abs in &report.other_project_files {
        let Some(owner) = owning_root(targets, abs).filter(|owner| *owner != from) else {
            continue;
        };
        let Ok(rel) = abs.strip_prefix(&targets[owner].root) else {
            continue;
        };
        if abs.starts_with(base) {
            extra[owner].insert(rel.to_path_buf());
        } else {
            uses[owner].files.insert(rel.to_path_buf());
        }
    }
}

/// Lints every target, in parallel, and keeps the targets' (path) order; the first failure in that order wins, so the
/// outcome does not depend on scheduling.
///
/// Roots nested in other roots of the run are linted after them, in waves by nesting depth (all roots of one depth in
/// parallel): a file of a nested root that only an enclosing root imports (`build/lib/x.ts` of a `build` project,
/// imported from the outer `src`) is then an entry of the nested root instead of an unused file. Each report lists
/// the files of other projects its code imports or runs; each goes to the innermost root that holds it. A run without
/// nested roots is a single wave, as before. Each report also lists the files outside its root that its code reaches
/// and the dependencies its code uses that only a manifest above its root lists; once every root is linted, the root
/// that owns such a file or manifest no longer reports them unused. A `stale-js-config` finding about a configuration
/// file several roots read is reported once, and an entry they judged only when each of them found it stale.
pub fn lint_targets(base: &Path, targets: Vec<Target>, linters: &Linters) -> Result<Vec<RootRun>, CliError> {
    let depths = nesting_depths(&targets);
    let mut extra: Vec<BTreeSet<PathBuf>> = vec![BTreeSet::new(); targets.len()];
    let mut other_root_uses: Vec<OtherRootUses> = vec![OtherRootUses::default(); targets.len()];
    let mut reports: Vec<Option<Result<Report, CliError>>> = (0..targets.len()).map(|_| None).collect();
    for depth in 0..=depths.iter().copied().max().unwrap_or(0) {
        let wave: Vec<usize> = (0..targets.len()).filter(|index| depths[*index] == depth).collect();
        let linted: Vec<(usize, Result<Report, CliError>)> = wave
            .par_iter()
            .map(|&index| {
                let target = &targets[index];
                let entries: Vec<PathBuf> = extra[index].iter().cloned().collect();
                let linter = linters.get(linters.of_target[index]);
                (
                    index,
                    lint_loaded(linter, &target.workspace, target.config_path().as_deref(), &entries),
                )
            })
            .collect();
        for (index, report) in linted {
            if let Ok(report) = &report {
                route_other_project_files(&targets, (index, report), &mut extra, &mut other_root_uses);
            }
            reports[index] = Some(report);
        }
    }
    for (index, report) in reports.iter().enumerate() {
        if let Some(Ok(report)) = report {
            route_ancestor_dependency_uses(&targets, index, report, &mut other_root_uses);
        }
    }
    let mut runs = targets
        .into_iter()
        .zip(reports)
        .zip(extra)
        .zip(other_root_uses)
        .zip(&linters.of_target)
        .map(|((((target, report), extra_entries), other_root_uses), &linter)| {
            let report = report.unwrap_or_else(|| Err(CliError::new(Kind::Internal, "a root was not linted")))?;
            let report = credit_other_root_uses(report, &other_root_uses);
            Ok(RootRun {
                prefix: prefix_of(base, &target.root),
                config_path: target.config_path(),
                config: target.config,
                linter,
                workspace: target.workspace,
                report,
                extra_entries: extra_entries.into_iter().collect(),
                other_root_uses,
                withheld: Vec::new(),
            })
        })
        .collect::<Result<Vec<RootRun>, CliError>>()?;
    settle_shared_config_findings(&mut runs);
    Ok(runs)
}

/// `prefix/path` with `..` segments resolved, so a config file inherited from a parent directory stays inside the base.
fn join(prefix: &Path, path: &Path) -> PathBuf {
    let mut joined = PathBuf::new();
    for component in prefix.components().chain(path.components()) {
        match component {
            Component::ParentDir if joined.pop() => {}
            other => joined.push(other),
        }
    }
    joined
}

fn relocate(location: &mut Location, prefix: &Path) {
    location.path = join(prefix, &location.path);
}

/// The report with every path relative to the run's base instead of the root; borrowed when the root is the base.
pub fn prefixed<'a>(report: &'a Report, prefix: &Path) -> Cow<'a, Report> {
    if prefix.as_os_str().is_empty() {
        return Cow::Borrowed(report);
    }
    let diagnostics: Vec<Diagnostic> = report
        .diagnostics
        .iter()
        .cloned()
        .map(|mut diagnostic| {
            relocate(&mut diagnostic.location, prefix);
            for related in &mut diagnostic.related {
                relocate(&mut related.location, prefix);
            }
            diagnostic
        })
        .collect();
    let incomplete = report
        .incomplete_packages
        .iter()
        .cloned()
        .map(|mut package| {
            package.dir = join(prefix, &package.dir);
            for reason in &mut package.reasons {
                reason.file = reason.file.take().map(|file| join(prefix, &file));
            }
            package
        })
        .collect();
    Cow::Owned(Report::new(diagnostics, report.summary.packages).with_incomplete_packages(incomplete))
}

/// All roots as one report: diagnostics and incomplete packages merged in the usual deterministic order, package
/// counts summed.
///
/// A single root's report is borrowed: [`Report::new`] already put it in that order, and copying thousands of
/// diagnostics only to sort them again is a measurable part of a large run.
pub fn aggregate<'a>(reports: &'a [Cow<'_, Report>]) -> Cow<'a, Report> {
    if let [report] = reports {
        return Cow::Borrowed(report);
    }
    Cow::Owned(
        Report::new(
            reports
                .iter()
                .flat_map(|report| report.diagnostics.iter().cloned())
                .collect(),
            reports.iter().map(|report| report.summary.packages).sum(),
        )
        .with_incomplete_packages(
            reports
                .iter()
                .flat_map(|report| report.incomplete_packages.iter().cloned())
                .collect(),
        ),
    )
}

/// The path of a root for display: `.` for the base itself.
pub fn display(prefix: &Path) -> String {
    if prefix.as_os_str().is_empty() {
        ".".to_owned()
    } else {
        slash(prefix)
    }
}

/// An entry of the JSON `roots` array.
pub fn root_json(run: &RootRun, report: &Report) -> Value {
    let packages: Vec<String> = run
        .workspace
        .all_packages()
        .filter(|package| !run.workspace.is_ignored(package.id))
        .map(|package| display(&join(&run.prefix, &package.dir)))
        .collect();
    json!({
        "path": display(&run.prefix),
        "packages": packages,
        "summary": serde_json::to_value(report.summary).unwrap_or(Value::Null),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use skif::{MemoryFs, RuleId, Severity};

    use super::*;

    const OUTER: &str = "../etc/skif.toml";
    const INNER: &str = "../../../etc/skif.toml";
    const ROOTS: [&str; 3] = ["/r", "/r/tools/a", "/r/tools/b"];
    const ENTRY_KEY: &str = "js.workspaces.\"tools/*\".entry";
    const ENTRY: &str = "`src/extra.js` in `js.workspaces.\"tools/*\".entry` matched nothing";
    const DEPENDENCY: &str = "`left-pad` in `js.ignore-dependencies` matched nothing";
    const HINT: &str = "`src/main.js` in `js.entry` is already an entry of a plugin, `package.json` or a script";

    fn stale(path: &str, message: &str) -> Diagnostic {
        Diagnostic::new(
            RuleId::from_static(STALE_JS_CONFIG),
            Severity::Warning,
            message,
            Location::new(path, None),
        )
    }

    /// A report with the findings `reported` and the judgements `judged`, as `(file, key, entry, stale, message)`.
    fn report(reported: &[(&str, &str)], judged: &[(&str, &str, &str, bool, &str)]) -> Report {
        let diagnostics = reported.iter().map(|(path, message)| stale(path, message)).collect();
        let judgements = judged
            .iter()
            .map(|&(file, key, entry, is_stale, message)| ConfigJudgement::new(file, key, entry, is_stale, message))
            .collect();
        Report::new(diagnostics, 1).with_config_judgements(judgements)
    }

    /// The findings [`shared_config_drops`] takes out of each of `reports`, linted at [`ROOTS`] in order.
    fn drops(reports: &[Report]) -> Vec<Vec<(String, String)>> {
        let roots: Vec<(&Path, &Report)> = ROOTS.map(Path::new).into_iter().zip(reports).collect();
        shared_config_drops(&roots)
            .into_iter()
            .map(|dropped| {
                dropped
                    .into_iter()
                    .map(|(path, message)| (slash(&path), message))
                    .collect()
            })
            .collect()
    }

    fn dropped(path: &str, message: &str) -> Vec<(String, String)> {
        vec![(path.to_owned(), message.to_owned())]
    }

    #[test]
    fn an_entry_of_a_glob_table_one_root_uses_is_stale_nowhere() {
        let reports = [
            report(&[], &[]),
            report(&[], &[(INNER, ENTRY_KEY, "src/extra.js", false, ENTRY)]),
            report(&[(INNER, ENTRY)], &[(INNER, ENTRY_KEY, "src/extra.js", true, ENTRY)]),
        ];
        assert_eq!(drops(&reports), [vec![], vec![], dropped(INNER, ENTRY)]);
    }

    #[test]
    fn a_root_that_could_not_judge_an_entry_does_not_hide_it() {
        let reports = [
            report(
                &[(OUTER, DEPENDENCY)],
                &[(OUTER, "js.ignore-dependencies", "left-pad", true, DEPENDENCY)],
            ),
            report(&[], &[]),
            report(&[], &[]),
        ];
        assert!(drops(&reports).iter().all(Vec::is_empty));
    }

    #[test]
    fn an_entry_stale_in_every_root_that_judged_it_is_reported_by_the_first() {
        let judged = |path| (path, "js.ignore-dependencies", "left-pad", true, DEPENDENCY);
        let reports = [
            report(&[(OUTER, DEPENDENCY)], &[judged(OUTER)]),
            report(&[(INNER, DEPENDENCY)], &[judged(INNER)]),
            report(&[(INNER, DEPENDENCY)], &[judged(INNER)]),
        ];
        assert_eq!(
            drops(&reports),
            [vec![], dropped(INNER, DEPENDENCY), dropped(INNER, DEPENDENCY)]
        );
    }

    #[test]
    fn a_stale_entry_is_kept_by_the_first_root_that_reports_it() {
        // The outer root judged the entry stale but does not report it (an `[[ignore]]` entry silences it there).
        let judged = |path| (path, "js.ignore-dependencies", "left-pad", true, DEPENDENCY);
        let reports = [
            report(&[], &[judged(OUTER)]),
            report(&[(INNER, DEPENDENCY)], &[judged(INNER)]),
            report(&[], &[]),
        ];
        assert_eq!(drops(&reports)[1], Vec::<(String, String)>::new());
    }

    #[test]
    fn other_findings_at_a_shared_file_are_reported_by_the_first_root_alone() {
        let reports = [
            report(&[(OUTER, HINT), (OUTER, HINT)], &[]),
            report(&[(INNER, HINT)], &[]),
            report(&[("package.json", HINT)], &[]),
        ];
        assert_eq!(drops(&reports), [vec![], dropped(INNER, HINT), vec![]]);
    }

    fn run_at(root: &str, report: Report) -> RootRun {
        let fs = MemoryFs::new().with_file(
            Path::new(root).join("package.json"),
            r#"{"name": "app", "version": "1.0.0"}"#,
        );
        let config = skif::Config::default();
        RootRun {
            prefix: PathBuf::new(),
            workspace: Workspace::discover_with_fs(Path::new(root), &config, &fs).unwrap(),
            config,
            config_path: None,
            linter: 0,
            report,
            extra_entries: Vec::new(),
            other_root_uses: OtherRootUses::default(),
            withheld: Vec::new(),
        }
    }

    fn messages(run: &RootRun) -> Vec<&str> {
        run.report
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect()
    }

    #[test]
    fn settling_again_decides_from_the_findings_an_earlier_decision_withheld() {
        let judged = |path| (path, "js.ignore-dependencies", "left-pad", true, DEPENDENCY);
        let mut runs = vec![
            run_at(ROOTS[0], report(&[(OUTER, DEPENDENCY)], &[judged(OUTER)])),
            run_at(ROOTS[1], report(&[(INNER, DEPENDENCY)], &[judged(INNER)])),
        ];
        settle_shared_config_findings(&mut runs);
        assert_eq!((messages(&runs[0]), messages(&runs[1])), (vec![DEPENDENCY], vec![]));
        assert_eq!(runs[1].withheld.len(), 1);
        // Linted again after fixing, the outer root no longer judges the entry, so the nested root's finding stands.
        runs[0].report = report(&[], &[]);
        settle_shared_config_findings(&mut runs);
        assert_eq!((messages(&runs[0]), messages(&runs[1])), (vec![], vec![DEPENDENCY]));
        assert!(runs[1].withheld.is_empty());
        assert_eq!(runs[1].report.summary.warnings, 1);
    }

    #[test]
    fn join_resolves_parent_segments() {
        assert_eq!(
            join(Path::new("a/b"), Path::new("package.json")),
            Path::new("a/b/package.json")
        );
        assert_eq!(join(Path::new("a"), Path::new("../skif.toml")), Path::new("skif.toml"));
        assert_eq!(join(Path::new(""), Path::new("x")), Path::new("x"));
    }

    #[test]
    fn display_names_the_base_as_a_dot() {
        assert_eq!(display(Path::new("")), ".");
        assert_eq!(display(Path::new("a/b")), "a/b");
    }

    #[test]
    fn a_root_outside_the_base_keeps_its_full_path() {
        assert_eq!(prefix_of(Path::new("/a"), Path::new("/a/b/c")), Path::new("b/c"));
        assert_eq!(prefix_of(Path::new("/a"), Path::new("/a")), Path::new(""));
        assert_eq!(prefix_of(Path::new("/a"), Path::new("/z/b")), Path::new("/z/b"));
    }

    #[test]
    fn prefixing_with_the_base_borrows_and_otherwise_relocates() {
        let report = Report::new(Vec::new(), 3);
        assert!(matches!(prefixed(&report, Path::new("")), Cow::Borrowed(_)));
        assert!(matches!(prefixed(&report, Path::new("a")), Cow::Owned(_)));
        let both = [prefixed(&report, Path::new("a")), prefixed(&report, Path::new(""))];
        assert_eq!(aggregate(&both).summary.packages, 6);
    }

    #[test]
    fn a_root_error_names_the_root() {
        let error = RootError {
            prefix: PathBuf::from("pkgs/a"),
            error: CliError::new(Kind::Workspace, "boom"),
        };
        assert_eq!(error.describe(), "pkgs/a: boom");
        assert_eq!(error.to_json()["path"], "pkgs/a");
        assert_eq!(error.to_json()["kind"], "workspace");
    }

    #[test]
    fn prefixing_and_aggregating_relocate_incomplete_packages() {
        use skif::{IncompleteCause, IncompleteEntries, IncompletePackage};

        let package = |name: &str, dir: &str, file: &str| {
            IncompletePackage::new(
                name.to_owned(),
                dir.into(),
                vec![IncompleteEntries {
                    cause: IncompleteCause::UnknownSpread,
                    plugin: Some("vite".to_owned()),
                    file: Some(file.into()),
                }],
            )
        };
        let nested = Report::new(Vec::new(), 1).with_incomplete_packages(vec![package(".", "", "vite.config.ts")]);
        let base = Report::new(Vec::new(), 1).with_incomplete_packages(vec![package("z", "z", "z/vite.config.ts")]);
        let reports = [prefixed(&base, Path::new("")), prefixed(&nested, Path::new("apps/web"))];
        let merged = aggregate(&reports);
        let shown: Vec<(PathBuf, Option<PathBuf>)> = merged
            .incomplete_packages
            .iter()
            .map(|package| (package.dir.clone(), package.reasons[0].file.clone()))
            .collect();
        assert_eq!(
            shown,
            [
                (
                    PathBuf::from("apps/web"),
                    Some(PathBuf::from("apps/web/vite.config.ts"))
                ),
                (PathBuf::from("z"), Some(PathBuf::from("z/vite.config.ts"))),
            ]
        );
    }
}
