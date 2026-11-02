//! Resolving what to lint (one workspace, or every root below a directory) and linting the roots in parallel.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use rayon::prelude::*;
use serde_json::{Value, json};
use skif::{ConfigOverrides, Diagnostic, FileSystem, Linter, Location, Report, RuleMeta, RunOptions, Target, Workspace, slash};

use crate::error::{CliError, Kind};

/// The roots to lint and the directory their diagnostic paths are relative to.
pub struct Resolved {
    /// The workspace root for a single-workspace run; the invocation directory for a multi-root run.
    pub base: PathBuf,
    pub targets: Vec<Target>,
    /// Whether this is a multi-root run: it reports `roots` and groups human output by root.
    pub multi: bool,
}

fn directory_of(start: &Path, fs: &dyn FileSystem) -> PathBuf {
    if fs.is_dir(start) {
        start.to_path_buf()
    } else {
        start.parent().unwrap_or(start).to_path_buf()
    }
}

/// Decides between the workspace containing `start` and every root below it.
///
/// A multi-root run is chosen by `recursive`, and also when neither `start` nor an ancestor holds a workspace or a
/// `package.json`, so `skif .` at the top of a repository of independent projects lints each of them.
pub fn resolve(
    start: &Path,
    recursive: bool,
    fs: &dyn FileSystem,
    overrides: &ConfigOverrides,
) -> Result<Resolved, CliError> {
    let found = if recursive {
        None
    } else {
        match skif::find_workspace_root(start, fs) {
            Ok(root) => Some(root),
            Err(skif::Error::NoWorkspace(_)) => None,
            Err(error) => return Err(error.into()),
        }
    };
    if let Some(root) = found {
        let target = skif::load_target(&root, &root, fs, overrides)?;
        return Ok(Resolved {
            base: root,
            targets: vec![target],
            multi: false,
        });
    }
    let targets = skif::discover_all(start, fs, overrides)?;
    if targets.is_empty() {
        return Err(CliError::new(
            Kind::Workspace,
            format!("no workspace or package found at or below {}", start.display()),
        ));
    }
    Ok(Resolved {
        base: directory_of(start, fs),
        targets,
        multi: true,
    })
}

/// The distinct linters of a run: roots with equal configuration share one.
pub struct Linters {
    linters: Vec<Linter>,
    of_target: Vec<usize>,
}

impl Linters {
    pub fn build(targets: &[Target]) -> Result<Self, CliError> {
        let mut linters: Vec<Linter> = Vec::new();
        let mut of_target = Vec::with_capacity(targets.len());
        for target in targets {
            let index = match linters.iter().position(|linter| linter.config() == &target.config) {
                Some(index) => index,
                None => {
                    linters.push(Linter::new(&target.config)?);
                    linters.len() - 1
                }
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
        .map(|(target, &linter)| {
            lint_loaded(linters.get(linter), &target.workspace, target.config_path().as_deref())
        })
        .collect();
    targets
        .into_iter()
        .zip(reports)
        .zip(&linters.of_target)
        .map(|((target, report), &linter)| {
            Ok(RootRun {
                prefix: target.root.strip_prefix(base).unwrap_or(Path::new("")).to_path_buf(),
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

/// The report with every path relative to the run's base instead of the root.
pub fn prefixed(report: &Report, prefix: &Path) -> Report {
    if prefix.as_os_str().is_empty() {
        return report.clone();
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
    Report::new(diagnostics, report.summary.packages)
}

/// All roots as one report: diagnostics merged in the usual deterministic order, package counts summed.
pub fn aggregate(reports: &[Report]) -> Report {
    Report::new(
        reports.iter().flat_map(|report| report.diagnostics.iter().cloned()).collect(),
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
        assert_eq!(join(Path::new("a/b"), Path::new("package.json")), Path::new("a/b/package.json"));
        assert_eq!(join(Path::new("a"), Path::new("../skif.toml")), Path::new("skif.toml"));
        assert_eq!(join(Path::new(""), Path::new("x")), Path::new("x"));
    }

    #[test]
    fn display_names_the_base_as_a_dot() {
        assert_eq!(display(Path::new("")), ".");
        assert_eq!(display(Path::new("a/b")), "a/b");
    }
}
