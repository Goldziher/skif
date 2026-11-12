//! Resolving what to lint (one workspace, or every root below a directory) and linting the roots in parallel.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::hash::BuildHasher;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rayon::prelude::*;
use serde_json::{Value, json};
use skif::{
    ConfigOverrides, Diagnostic, FileSystem, Linter, Location, ModuleCache, OsFs, Report, RuleMeta, RunOptions, Target,
    Workspace, join_lexical, slash,
};
use skif_core::{HashMap, HashState};

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
    /// Builds one linter per distinct configuration, reading the disk. The cache a linter keeps over its file system
    /// serves all the roots it lints. The linters of a multi-root run share the module cache
    /// [`Linter::shared_modules`] builds, which lives as long as they do.
    pub fn build(targets: &[Target]) -> Result<Self, CliError> {
        Self::build_with(targets, None, None)
    }

    /// Like [`Linters::build`], reading through `fs` instead of the disk and parsing every source file through
    /// `modules` when given (`--cache`, or the cache of a long-lived host) instead of the in-memory cache of a
    /// multi-root run.
    ///
    /// `fs` is the language server's unsaved buffers, or the [`CachedFs`](skif::CachedFs) a command found its roots
    /// through, so the linters do not read the listings and manifests discovery already read again. Such a cache
    /// lives for one command and is cleared after the command writes files, before it lints again.
    pub fn build_with(
        targets: &[Target],
        fs: Option<&Arc<dyn FileSystem>>,
        modules: Option<&Arc<dyn ModuleCache>>,
    ) -> Result<Self, CliError> {
        let shared = Linter::shared_modules(targets, modules.cloned());
        let (firsts, of_target) = distinct_configs(targets.iter().map(|target| &target.config));
        let linters = firsts
            .into_iter()
            .map(|first| {
                let target = &targets[first];
                let fs = fs.map_or_else(|| Arc::new(OsFs) as Arc<dyn FileSystem>, Arc::clone);
                let linter = match &shared {
                    Some(modules) => Linter::with_shared_modules(&target.config, fs, modules),
                    None => Linter::with_fs(&target.config, fs),
                };
                linter.map_err(|error| target.locate(error))
            })
            .collect::<Result<Vec<_>, _>>()?;
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

/// Groups `configs` by equality: the position of the first of each distinct configuration, in first-seen order, and
/// for every configuration the index of its group. A configuration is compared only with the distinct ones sharing
/// its [`config_key`], not with every one seen so far.
fn distinct_configs<'a>(configs: impl IntoIterator<Item = &'a skif::Config>) -> (Vec<usize>, Vec<usize>) {
    let mut firsts: Vec<(usize, &skif::Config)> = Vec::new();
    let mut by_key: HashMap<u64, Vec<usize>> = HashMap::default();
    let mut of_config = Vec::new();
    for (position, config) in configs.into_iter().enumerate() {
        let candidates = by_key.entry(config_key(config)).or_default();
        let group = if let Some(&group) = candidates.iter().find(|&&group| firsts[group].1 == config) {
            group
        } else {
            let group = firsts.len();
            firsts.push((position, config));
            candidates.push(group);
            group
        };
        of_config.push(group);
    }
    (firsts.into_iter().map(|(position, _)| position).collect(), of_config)
}

/// A hash that equal configurations share, over the parts that are cheap to hash: the rule IDs, summed so their
/// order does not count (`==` compares the rules as a map), what the configuration extends, and the lengths of its
/// lists.
fn config_key(config: &skif::Config) -> u64 {
    let rules = config.rules.keys().fold(0_u64, |sum, id| sum.wrapping_add(HashState.hash_one(id)));
    HashState.hash_one((
        rules,
        &config.extends,
        config.ignore.len(),
        config.overrides.len(),
        config.version_groups.len(),
        config.semver_groups.len(),
        config.custom_types.len(),
        config.dependency_families.len(),
        config.update_groups.len(),
    ))
}

/// One linted root. The report is relative to the root, which is what fixing needs.
pub struct RootRun {
    /// The root relative to the run's base; empty for a single-workspace run.
    pub prefix: PathBuf,
    pub config: skif::Config,
    pub config_path: Option<PathBuf>,
    /// The absolute configuration files `config` was merged from: a root whose file a fix wrote is linted again.
    pub config_sources: Vec<PathBuf>,
    pub linter: usize,
    pub workspace: Workspace,
    pub report: Report,
    /// The files of this root that enclosing roots of the run import or run, relative to the root; a re-lint after
    /// fixing passes them again.
    pub extra_entries: Vec<PathBuf>,
    /// What the code of other roots of the run uses of this root; a re-lint after fixing credits it again.
    pub other_root_uses: OtherRootUses,
}

/// What the code of other roots of a multi-root run uses of a root, which the root's own lint cannot see.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OtherRootUses {
    /// Dependencies of the root's manifests that other roots use: those the code of roots nested in it imports or
    /// runs without listing them, those the files of this root in `files` import, and those providing a binary that a
    /// command of an enclosing root runs after `cd` into this root. The manifest path relative to the root, and the
    /// dependency name.
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

/// Decides the `stale-js-config` findings about configuration files several roots of the run read
/// ([`skif::settle_shared_config`], ADR 0098). Called once, on the reports the run shows: after the lint after fixing
/// and after the flags narrowed them.
pub fn settle_shared_config(runs: &mut [RootRun]) {
    skif::settle_shared_config(
        runs.iter_mut()
            .map(|run| (run.workspace.root.as_path(), &mut run.report)),
    );
}

/// Hands the dependencies that the report of root `from` uses through manifests outside it (above it, or of the files
/// of other roots it reaches) to the innermost root holding each manifest, as the manifest path relative to that root.
fn route_ancestor_dependency_uses(
    (targets, roots): (&[Target], &RootIndex<'_>),
    from: usize,
    report: &Report,
    uses: &mut [OtherRootUses],
) {
    for (manifest_dir, name) in &report.ancestor_dependency_uses {
        let Some(owner) = roots.owner(manifest_dir).filter(|owner| *owner != from) else {
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

/// The roots of a run by directory, so the roots around a path are found by walking its ancestors instead of testing
/// every root.
struct RootIndex<'a> {
    /// How many targets have each root, and the last of them by index.
    roots: HashMap<&'a Path, (usize, usize)>,
}

impl<'a> RootIndex<'a> {
    /// The index of `roots`, the root directories of the targets in order.
    fn new(roots: impl IntoIterator<Item = &'a Path>) -> Self {
        let mut index: HashMap<&'a Path, (usize, usize)> = HashMap::default();
        for (position, root) in roots.into_iter().enumerate() {
            let entry = index.entry(root).or_insert((0, position));
            *entry = (entry.0 + 1, position);
        }
        Self { roots: index }
    }

    /// The index of the roots of `targets`.
    fn of(targets: &'a [Target]) -> Self {
        Self::new(targets.iter().map(|target| target.root.as_path()))
    }

    /// How many other roots enclose the root `root`: a root nested in no other is at depth 0.
    fn depth(&self, root: &Path) -> usize {
        distinct_ancestors(root)
            .skip(1)
            .filter_map(|dir| self.roots.get(dir))
            .map(|(count, _)| count)
            .sum()
    }

    /// The innermost root that holds the absolute file `file`, by index.
    fn owner(&self, file: &Path) -> Option<usize> {
        file.ancestors()
            .find_map(|dir| self.roots.get(dir))
            .map(|(_, index)| *index)
    }
}

/// The ancestors of `path`, itself first, without the repeats a `.` component makes (`/a/.` is `/a`).
fn distinct_ancestors(path: &Path) -> impl Iterator<Item = &Path> {
    let mut previous: Option<&Path> = None;
    path.ancestors().filter(move |dir| {
        let repeat = previous == Some(*dir);
        previous = Some(*dir);
        !repeat
    })
}

/// Hands the files of other projects that the report of root `from` lists (absolute) to the innermost root holding
/// each, as paths relative to that root. A file below `from` belongs to a root nested in it, which is linted later and
/// takes it as an entry; a file outside `from` is counted as used by its root once every root is linted, as that root
/// may be linted already.
fn route_other_project_files(
    (targets, roots): (&[Target], &RootIndex<'_>),
    (from, report): (usize, &Report),
    extra: &mut [BTreeSet<PathBuf>],
    uses: &mut [OtherRootUses],
) {
    let base = &targets[from].root;
    for abs in &report.other_project_files {
        let Some(owner) = roots.owner(abs).filter(|owner| *owner != from) else {
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
/// that owns such a file or manifest no longer reports them unused. The `stale-js-config` findings about a
/// configuration file several roots read are left to [`settle_shared_config`]: each report holds what its root found.
pub fn lint_targets(base: &Path, targets: Vec<Target>, linters: &Linters) -> Result<Vec<RootRun>, CliError> {
    let roots = RootIndex::of(&targets);
    let depths: Vec<usize> = targets.iter().map(|target| roots.depth(&target.root)).collect();
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
                route_other_project_files((&targets, &roots), (index, report), &mut extra, &mut other_root_uses);
            }
            reports[index] = Some(report);
        }
    }
    for (index, report) in reports.iter().enumerate() {
        if let Some(Ok(report)) = report {
            route_ancestor_dependency_uses((&targets, &roots), index, report, &mut other_root_uses);
        }
    }
    drop(roots);
    targets
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
                config_sources: target.sources.into_iter().map(|source| source.path).collect(),
                config: target.config,
                linter,
                workspace: target.workspace,
                report,
                extra_entries: extra_entries.into_iter().collect(),
                other_root_uses,
            })
        })
        .collect()
}

fn relocate(location: &mut Location, prefix: &Path) {
    location.path = join_lexical(prefix, &location.path);
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
            package.dir = join_lexical(prefix, &package.dir);
            for reason in &mut package.reasons {
                reason.file = reason.file.take().map(|file| join_lexical(prefix, &file));
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
        .map(|package| display(&join_lexical(&run.prefix, &package.dir)))
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
    use super::*;

    #[test]
    fn equal_configurations_share_a_linter_in_first_seen_order() {
        let with_rules = |ids: &[&str]| {
            let mut config = skif::Config::default();
            for id in ids {
                config.rules.insert((*id).to_owned(), skif::RuleConfig::level(skif::Level::Error));
            }
            config
        };
        let ab = with_rules(&["a", "b"]);
        let ba = with_rules(&["b", "a"]);
        let plain = skif::Config::default();
        let mut root = plain.clone();
        root.root = true;
        assert_eq!(ab, ba);
        assert_eq!(config_key(&ab), config_key(&ba));
        assert_eq!(config_key(&root), config_key(&plain));
        assert_ne!(root, plain);

        let configs = [&root, &ab, &plain, &ba, &root, &plain];
        assert_eq!(distinct_configs(configs), (vec![0, 1, 2], vec![0, 1, 2, 1, 0, 2]));
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

    /// How many roots of `roots` other than `root` enclose it, testing every root.
    fn depth_by_scan(roots: &[&Path], root: &Path) -> usize {
        roots
            .iter()
            .filter(|other| **other != root && root.starts_with(other))
            .count()
    }

    /// The innermost root of `roots` holding `file`, testing every root.
    fn owner_by_scan(roots: &[&Path], file: &Path) -> Option<usize> {
        roots
            .iter()
            .enumerate()
            .filter(|(_, root)| file.starts_with(root))
            .max_by_key(|(_, root)| root.components().count())
            .map(|(index, _)| index)
    }

    #[test]
    fn the_root_index_finds_what_testing_every_root_finds() {
        let roots: Vec<&Path> = ["/r", "/r/a", "/r/a/b/c", "/r/ab", "/s", "/r/a/b/c/d/e"]
            .into_iter()
            .map(Path::new)
            .collect();
        let index = RootIndex::new(roots.iter().copied());
        let depths: Vec<usize> = roots.iter().map(|root| index.depth(root)).collect();
        assert_eq!(depths, [0, 1, 2, 1, 0, 3]);
        for root in &roots {
            assert_eq!(index.depth(root), depth_by_scan(&roots, root), "{}", root.display());
        }
        let files = [
            ("/r/x.js", Some(0)),
            ("/r/a/x.js", Some(1)),
            ("/r/a/b/x.js", Some(1)),
            ("/r/a/b/c/x.js", Some(2)),
            ("/r/ab/x.js", Some(3)),
            ("/r/abc/x.js", Some(0)),
            ("/r/a/b/c/d/e", Some(5)),
            ("/s", Some(4)),
            ("/t/x.js", None),
            ("/r/a/./b/c/x.js", Some(2)),
        ];
        for (file, owner) in files {
            assert_eq!(index.owner(Path::new(file)), owner, "{file}");
            assert_eq!(owner_by_scan(&roots, Path::new(file)), owner, "{file}");
        }
    }

    #[test]
    fn a_dot_component_does_not_count_an_enclosing_root_twice() {
        let roots = [Path::new("/r"), Path::new("/r/./a")];
        let index = RootIndex::new(roots);
        assert_eq!(index.depth(Path::new("/r/./a")), 1);
        assert_eq!(depth_by_scan(&roots, Path::new("/r/./a")), 1);
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
                vec![
                    IncompleteEntries::new(IncompleteCause::UnknownSpread)
                        .with_plugin("vite")
                        .with_file(file),
                ],
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
