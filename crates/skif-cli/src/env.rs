//! The process environment the CLI is allowed to look at, gathered in one place.

use std::io::IsTerminal;
use std::path::PathBuf;

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

/// `GITHUB_WORKSPACE`, the directory GitHub checked the repository out to.
pub fn github_workspace() -> Option<PathBuf> {
    std::env::var_os("GITHUB_WORKSPACE")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
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
    use super::is_truthy;

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
