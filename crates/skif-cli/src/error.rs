//! The ways a run can fail to happen at all (exit code 2), with the `kind` JSON mode reports.

use std::fmt;
use std::path::Path;

use skif::{ConfigError, LintError, WorkspaceError, slash};

/// Machine-readable category of a failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Config,
    Workspace,
    Io,
    Usage,
    CiRefusal,
}

impl Kind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Config => "config",
            Self::Workspace => "workspace",
            Self::Io => "io",
            Self::Usage => "usage",
            Self::CiRefusal => "ci-refusal",
        }
    }
}

/// A failure that stops skif from checking the repository.
#[derive(Debug)]
pub struct CliError {
    pub kind: Kind,
    pub message: String,
    pub path: Option<String>,
    pub line: Option<u32>,
    pub column: Option<u32>,
}

impl CliError {
    pub fn new(kind: Kind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: slashes(message.into()),
            path: None,
            line: None,
            column: None,
        }
    }

    pub fn io(context: &str, path: &Path, error: &std::io::Error) -> Self {
        Self {
            path: Some(slash(path)),
            ..Self::new(Kind::Io, format!("{context} {}: {error}", path.display()))
        }
    }

    pub fn ci_refusal() -> Self {
        Self::new(
            Kind::CiRefusal,
            "refusing to fix: the CI environment variable is set; run --fix locally or unset CI",
        )
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

/// Native separators rewritten to `/` on Windows, so messages and snapshots read the same on every platform.
fn slashes(text: String) -> String {
    if cfg!(windows) { text.replace('\\', "/") } else { text }
}

fn position(value: Option<usize>) -> Option<u32> {
    value.and_then(|value| u32::try_from(value).ok())
}

impl From<ConfigError> for CliError {
    fn from(error: ConfigError) -> Self {
        let mut cli = Self::new(Kind::Config, error.to_string());
        match &error {
            ConfigError::Parse { path, line, column, .. } => {
                cli.path = path.as_deref().map(slash);
                cli.line = position(*line);
                cli.column = position(*column);
            }
            ConfigError::Invalid { path, .. } => cli.path = path.as_deref().map(slash),
            ConfigError::Io { path, .. } => {
                cli.kind = Kind::Io;
                cli.path = Some(slash(path));
            }
            ConfigError::Ambiguous { first, .. } => cli.path = Some(slash(first)),
            _ => {}
        }
        cli
    }
}

impl From<WorkspaceError> for CliError {
    fn from(error: WorkspaceError) -> Self {
        let mut cli = Self::new(Kind::Workspace, error.to_string());
        match &error {
            WorkspaceError::InvalidManifest { path, line, column, .. } => {
                cli.path = Some(slash(path));
                cli.line = Some(*line);
                cli.column = Some(*column);
            }
            WorkspaceError::InvalidPnpmWorkspace { path, .. } => cli.path = Some(slash(path)),
            WorkspaceError::Io { path, .. } => {
                cli.kind = Kind::Io;
                cli.path = Some(slash(path));
            }
            _ => {}
        }
        cli
    }
}

impl From<skif::Error> for CliError {
    fn from(error: skif::Error) -> Self {
        let message = error.to_string();
        match error {
            skif::Error::Config(error) | skif::Error::Lint(LintError::Config(error)) => error.into(),
            skif::Error::Workspace(error) => error.into(),
            skif::Error::NoWorkspace(_) => Self::new(Kind::Workspace, message),
            skif::Error::UnknownRule(_) | skif::Error::Registry(_) => Self::new(Kind::Config, message),
            _ => Self::new(Kind::Io, message),
        }
    }
}

impl From<skif::FixError> for CliError {
    fn from(error: skif::FixError) -> Self {
        Self::new(Kind::Io, error.to_string())
    }
}
