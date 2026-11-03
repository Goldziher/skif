//! Typed discovery errors.

use std::io;
use std::path::PathBuf;

/// Everything that can stop workspace discovery. Discovery never prints and never exits.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum WorkspaceError {
    /// The workspace root is not a directory.
    #[error("path {0:?} is not a directory")]
    NotADirectory(PathBuf),
    /// The workspace root has no `package.json`.
    #[error("`package.json` not found in {0:?}")]
    MissingRootManifest(PathBuf),
    /// Neither the root `package.json` `workspaces` field nor `pnpm-workspace.yaml` declares workspace patterns.
    #[error("No `workspaces` field in the root `package.json`, or `pnpm-workspace.yaml` file not found in {0:?}")]
    NoWorkspaces(PathBuf),
    /// A `package.json` is not valid JSON. `line` and `column` are 1-based.
    #[error("invalid {path:?} at {line}:{column}: {message}")]
    InvalidManifest {
        /// Workspace-relative path of the manifest.
        path: PathBuf,
        /// What is wrong.
        message: String,
        /// 1-based line of the problem.
        line: u32,
        /// 1-based column of the problem.
        column: u32,
    },
    /// `pnpm-workspace.yaml` could not be parsed or has an unexpected shape.
    #[error("invalid {path:?}: {message}")]
    InvalidPnpmWorkspace {
        /// Workspace-relative path of the file.
        path: PathBuf,
        /// What is wrong.
        message: String,
    },
    /// A workspace pattern cannot be used, for example because it climbs out of the root with `..`.
    #[error("invalid workspace pattern {pattern:?}: {message}")]
    InvalidPattern {
        /// The pattern as written.
        pattern: String,
        /// What is wrong.
        message: String,
    },
    /// A file or directory could not be read.
    #[error("failed to read {path:?}: {source}")]
    Io {
        /// The path that failed.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: io::Error,
    },
    /// The dedicated thread pool for discovery could not be built.
    #[error("failed to build the discovery thread pool: {0}")]
    ThreadPool(String),
}
