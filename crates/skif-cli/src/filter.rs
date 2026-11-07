//! knip's run-time filters: which workspace packages a run reports (`--workspace`).
//!
//! The analysis still covers every package of the root, so a finding in a selected package that depends on the
//! others (an export only another package imports) is the same as in a full run; only the report is narrowed. The
//! filter is applied before fixing, so `--fix` only fixes what is reported.

use std::path::Path;

use skif::{Diagnostic, Package, Report, Selector, Workspace, slash};

use crate::args::LintArgs;
use crate::error::{CliError, Kind};
use crate::roots::RootRun;

/// What a run reports, from the filter flags.
#[derive(Debug, Default)]
pub struct Filter {
    /// `--workspace`: package names or directories (globs, `!` to exclude), or `None` for every package.
    workspaces: Option<Selector>,
}

/// A `--workspace` value as a selector pattern: directories are written relative to the root, with or without a
/// leading `./` and a trailing `/`, and the root itself is `.`.
fn workspace_pattern(value: &str) -> String {
    let (negated, body) = value.strip_prefix('!').map_or((false, value), |body| (true, body));
    let body = body.strip_prefix("./").unwrap_or(body);
    let body = body.trim_end_matches('/');
    let body = if body.is_empty() { "." } else { body };
    if negated { format!("!{body}") } else { body.to_owned() }
}

/// The directory of a package as `--workspace` matches it: `.` for the root.
fn package_dir(package: &Package) -> String {
    if package.dir.as_os_str().is_empty() {
        ".".to_owned()
    } else {
        slash(&package.dir)
    }
}

/// The package a diagnostic belongs to: the one it names, else the innermost package whose directory holds its file.
fn owner<'w>(workspace: &'w Workspace, diagnostic: &Diagnostic) -> Option<&'w Package> {
    if let Some(package) = diagnostic.package_id.and_then(|id| workspace.package(id)) {
        return Some(package);
    }
    let path: &Path = &diagnostic.location.path;
    workspace
        .all_packages()
        .filter(|package| path.starts_with(&package.dir))
        .max_by_key(|package| package.dir.components().count())
}

impl Filter {
    /// The filter the flags ask for.
    ///
    /// # Errors
    /// A usage error when a `--workspace` pattern is not a valid glob.
    pub fn new(args: &LintArgs) -> Result<Self, CliError> {
        let workspaces = if args.workspace.is_empty() {
            None
        } else {
            let patterns = args.workspace.iter().map(|value| workspace_pattern(value));
            Some(Selector::new(patterns).map_err(|error| CliError::new(Kind::Usage, format!("--workspace: {error}")))?)
        };
        Ok(Self { workspaces })
    }

    /// Whether `--workspace` selects `package`.
    fn selects(&self, package: &Package) -> bool {
        self.workspaces.as_ref().is_none_or(|selector| {
            let dir = package_dir(package);
            let names = package.name.as_deref().into_iter().chain(std::iter::once(dir.as_str()));
            selector.matches_any(names, false)
        })
    }

    /// Narrows the report of one root to what the flags select.
    pub fn apply(&self, workspace: &Workspace, report: Report) -> Report {
        if self.workspaces.is_none() {
            return report;
        }
        let packages = workspace.all_packages().filter(|package| self.selects(package)).count();
        let diagnostics = report
            .diagnostics
            .into_iter()
            .filter(|diagnostic| owner(workspace, diagnostic).is_none_or(|package| self.selects(package)))
            .collect();
        Report::new(diagnostics, packages)
    }

    /// Narrows every root's report, after checking that `--workspace` selects a package in at least one of them.
    ///
    /// # Errors
    /// A usage error when `--workspace` was given and selects no package.
    pub fn apply_runs(&self, runs: &mut [RootRun]) -> Result<(), CliError> {
        if let Some(selector) = &self.workspaces {
            let any = runs
                .iter()
                .any(|run| run.workspace.all_packages().any(|package| self.selects(package)));
            if !any && !runs.is_empty() {
                return Err(CliError::new(
                    Kind::Usage,
                    format!(
                        "--workspace {} matches no workspace package (by name or directory)",
                        selector.patterns().join(", ")
                    ),
                ));
            }
        }
        for run in runs {
            let report = std::mem::replace(&mut run.report, Report::new(Vec::new(), 0));
            run.report = self.apply(&run.workspace, report);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::workspace_pattern;

    #[test]
    fn workspace_values_become_patterns() {
        assert_eq!(workspace_pattern("./packages/a/"), "packages/a");
        assert_eq!(workspace_pattern("."), ".");
        assert_eq!(workspace_pattern("./"), ".");
        assert_eq!(workspace_pattern("@scope/a"), "@scope/a");
        assert_eq!(workspace_pattern("!./packages/*"), "!packages/*");
    }
}
