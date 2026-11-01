//! The ways a run can fail to happen at all (exit code 2), with the `kind` JSON mode reports.

use std::fmt;
use std::path::Path;

use skif::{ConfigError, LintError, WorkspaceError};

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
            message: message.into(),
            path: None,
            line: None,
            column: None,
        }
    }

    pub fn io(context: &str, path: &Path, error: &std::io::Error) -> Self {
        Self {
            path: Some(display(path)),
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

/// A path as displayed to users and machines in messages.
pub fn display(path: &Path) -> String {
    path.display().to_string()
}

/// A workspace-relative path with `/` separators on every platform, for JSON.
pub fn slash(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn position(value: Option<usize>) -> Option<u32> {
    value.and_then(|value| u32::try_from(value).ok())
}

impl From<ConfigError> for CliError {
    fn from(error: ConfigError) -> Self {
        let mut cli = Self::new(Kind::Config, error.to_string());
        match &error {
            ConfigError::Parse {
                path, line, column, ..
            } => {
                cli.path = path.as_deref().map(display);
                cli.line = position(*line);
                cli.column = position(*column);
            }
            ConfigError::Invalid { path, .. } => cli.path = path.as_deref().map(display),
            ConfigError::Io { path, .. } => {
                cli.kind = Kind::Io;
                cli.path = Some(display(path));
            }
            ConfigError::Ambiguous { first, .. } => cli.path = Some(display(first)),
            _ => {}
        }
        cli
    }
}

impl From<WorkspaceError> for CliError {
    fn from(error: WorkspaceError) -> Self {
        let mut cli = Self::new(Kind::Workspace, error.to_string());
        match &error {
            WorkspaceError::InvalidManifest {
                path, line, column, ..
            } => {
                cli.path = Some(display(path));
                cli.line = Some(*line);
                cli.column = Some(*column);
            }
            WorkspaceError::InvalidPnpmWorkspace { path, .. } => cli.path = Some(display(path)),
            WorkspaceError::Io { path, .. } => {
                cli.kind = Kind::Io;
                cli.path = Some(display(path));
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
