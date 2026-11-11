//! The process environment the CLI is allowed to look at, gathered in one place.

use std::ffi::OsStr;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use crate::error::CliError;

/// Facts about the invoking environment that change behaviour.
#[derive(Debug, Clone, Copy)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent yes/no facts about the environment"
)]
pub struct Env {
    /// `CI` is truthy: set, non-empty after trimming, and not `0`, `false`, `no` or `off` in any case.
    pub ci: bool,
    /// `NO_COLOR` is set to a non-empty value.
    pub no_color: bool,
    pub stdin_tty: bool,
    pub stdout_tty: bool,
    /// `GITHUB_ACTIONS` is `true`.
    pub github_actions: bool,
}

impl Env {
    pub fn detect() -> Self {
        let ci = std::env::var_os("CI").map(|value| value.to_string_lossy().into_owned());
        let no_color = std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty());
        Self {
            ci: is_truthy(ci.as_deref()),
            no_color,
            stdin_tty: std::io::stdin().is_terminal(),
            stdout_tty: std::io::stdout().is_terminal(),
            github_actions: std::env::var_os("GITHUB_ACTIONS").is_some_and(|value| value == "true"),
        }
    }
}

/// The variable that switches the phase timings on.
const TIMINGS_VAR: &str = "SKIF_TIMINGS";

/// Turns the phase timings on, to stderr, when `SKIF_TIMINGS` is set to anything but empty or `0`. The library
/// never looks at the variable; this is the only place that does.
pub fn enable_timings() {
    if std::env::var_os(TIMINGS_VAR).is_some_and(|value| !value.is_empty() && value != "0") {
        let _ = skif_core::timing::enable_stderr();
    }
}

/// `GITHUB_WORKSPACE`, the directory GitHub checked the repository out to.
pub fn github_workspace() -> Option<PathBuf> {
    std::env::var_os("GITHUB_WORKSPACE")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Under `bazel run`, makes the directory the user ran Bazel from the current directory, so the default `.` and every
/// relative path on the command line name the source tree rather than the runfiles tree Bazel starts the binary in.
/// Outside `bazel run` (no `BUILD_WORKSPACE_DIRECTORY`) the current directory is left alone.
pub fn enter_bazel_run_directory() -> Result<(), CliError> {
    let Some(dir) = bazel_run_directory(
        std::env::var_os("BUILD_WORKSPACE_DIRECTORY").as_deref(),
        std::env::var_os("BUILD_WORKING_DIRECTORY").as_deref(),
    ) else {
        return Ok(());
    };
    std::env::set_current_dir(&dir)
        .map_err(|error| CliError::io("cannot enter the `bazel run` directory", &dir, &error))
}

/// The directory a `bazel run` invocation started in, from `BUILD_WORKSPACE_DIRECTORY` (the workspace root) and
/// `BUILD_WORKING_DIRECTORY` (the user's directory inside it): the working directory when set, else the workspace
/// root. `None` when the workspace root is unset or empty, which means this is not `bazel run`; a working directory
/// alone is ignored. Empty values count as unset, and a relative working directory is taken below the workspace root.
pub fn bazel_run_directory(workspace: Option<&OsStr>, working: Option<&OsStr>) -> Option<PathBuf> {
    let workspace = Path::new(workspace.filter(|value| !value.is_empty())?);
    Some(
        working
            .filter(|value| !value.is_empty())
            .map_or_else(|| workspace.to_path_buf(), |working| workspace.join(working)),
    )
}

/// Whether a `CI` value means "this is CI".
pub fn is_truthy(value: Option<&str>) -> bool {
    match value.map(str::trim) {
        None | Some("") => false,
        Some(text) => !["0", "false", "no", "off"]
            .iter()
            .any(|falsy| text.eq_ignore_ascii_case(falsy)),
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::path::PathBuf;

    use super::{bazel_run_directory, is_truthy};

    #[test]
    fn the_bazel_run_directory_is_the_working_directory_else_the_workspace() {
        let workspace = std::env::temp_dir().join("workspace");
        let packages = workspace.join("packages");
        let elsewhere = std::env::temp_dir().join("elsewhere");
        let root = Some(workspace.as_os_str());
        let cases: [(Option<&OsStr>, Option<&OsStr>, Option<PathBuf>); 8] = [
            (None, None, None),
            (None, Some(elsewhere.as_os_str()), None),
            (Some(OsStr::new("")), Some(elsewhere.as_os_str()), None),
            (root, None, Some(workspace.clone())),
            (root, Some(OsStr::new("")), Some(workspace.clone())),
            (root, Some(packages.as_os_str()), Some(packages.clone())),
            (root, Some(elsewhere.as_os_str()), Some(elsewhere.clone())),
            (root, Some(OsStr::new("packages")), Some(packages.clone())),
        ];
        for (workspace, working, expected) in cases {
            assert_eq!(
                bazel_run_directory(workspace, working),
                expected,
                "{workspace:?} {working:?}"
            );
        }
    }

    #[test]
    fn ci_truthiness_table() {
        let cases = [
            (None, false),
            (Some(""), false),
            (Some("  "), false),
            (Some("0"), false),
            (Some("false"), false),
            (Some("FALSE"), false),
            (Some("No"), false),
            (Some("off"), false),
            (Some("1"), true),
            (Some("true"), true),
            (Some("yes"), true),
            (Some("anything"), true),
        ];
        for (value, expected) in cases {
            assert_eq!(is_truthy(value), expected, "{value:?}");
        }
    }
}
