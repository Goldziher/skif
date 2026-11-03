//! Resolving what to lint (one workspace, or every root below a directory) and linting the roots in parallel.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use rayon::prelude::*;
use serde_json::{Value, json};
use skif::{
    ConfigOverrides, Diagnostic, FileSystem, Linter, Location, Report, RuleMeta, RunOptions, Target, Workspace, slash,
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
    pub fn build(targets: &[Target], fs: &Arc<dyn FileSystem>) -> Result<Self, CliError> {
        let mut linters: Vec<Linter> = Vec::new();
        let mut of_target = Vec::with_capacity(targets.len());
        for target in targets {
            let index = if let Some(index) = linters.iter().position(|linter| linter.config() == &target.config) {
                index
            } else {
                linters.push(Linter::with_fs(&target.config, Arc::clone(fs))?);
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
}

pub fn lint_loaded(linter: &Linter, workspace: &Workspace, config_path: Option<&Path>) -> Result<Report, CliError> {
    let mut options = RunOptions::default();
    if let Some(path) = config_path {
        options = options.with_config_path(path);
    }
    Ok(linter.run_with(workspace, &options)?)
}

/// Lints every target in parallel; the result keeps the targets' (path) order and the first failure in that order
/// wins, so the outcome does not depend on scheduling.
pub fn lint_targets(base: &Path, targets: Vec<Target>, linters: &Linters) -> Result<Vec<RootRun>, CliError> {
    let reports: Vec<Result<Report, CliError>> = targets
        .par_iter()
        .zip(&linters.of_target)
        .map(|(target, &linter)| lint_loaded(linters.get(linter), &target.workspace, target.config_path().as_deref()))
        .collect();
    targets
        .into_iter()
        .zip(reports)
        .zip(&linters.of_target)
        .map(|((target, report), &linter)| {
            Ok(RootRun {
                prefix: prefix_of(base, &target.root),
                config_path: target.config_path(),
                config: target.config,
                linter,
                workspace: target.workspace,
                report: report?,
            })
        })
        .collect()
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
    Cow::Owned(Report::new(diagnostics, report.summary.packages))
}

/// All roots as one report: diagnostics merged in the usual deterministic order, package counts summed.
pub fn aggregate(reports: &[Cow<'_, Report>]) -> Report {
    Report::new(
        reports
            .iter()
            .flat_map(|report| report.diagnostics.iter().cloned())
            .collect(),
        reports.iter().map(|report| report.summary.packages).sum(),
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
    use super::*;

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
}
