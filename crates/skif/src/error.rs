//! The facade's error type.

use std::path::PathBuf;

use skif_config::ConfigError;
use skif_lint::{LintError, RegistryError};
use skif_workspace::WorkspaceError;

/// Anything that can stop discovery, configuration or a lint run.
///
/// The variants keep the typed errors of the underlying crates so a host can inspect positions and paths.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The configuration file or an override is invalid.
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// The workspace could not be discovered.
    #[error(transparent)]
    Workspace(#[from] WorkspaceError),
    /// Two rules share an identifier.
    #[error(transparent)]
    Registry(#[from] RegistryError),
    /// A lint run failed or was cancelled.
    #[error(transparent)]
    Lint(#[from] LintError),
    /// No ancestor of the start path declares a workspace.
    #[error("no workspace found at or above {}", .0.display())]
    NoWorkspace(PathBuf),
    /// A path that must be absolute was relative; the library never consults the working directory.
    #[error("{} is relative; pass an absolute path", .0.display())]
    RelativePath(PathBuf),
    /// `RunOptions::rules` names a rule that is not registered.
    #[error("unknown rule \"{0}\"")]
    UnknownRule(String),
    /// The requested thread pool could not be built.
    #[error("cannot build the thread pool: {0}")]
    ThreadPool(String),
}
