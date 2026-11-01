//! Workspace discovery for skif: root and member manifests with dependency spans, catalogs, package-manager
//! detection and pattern facts.
//!
//! The crate is library-first: no printing, no exiting, no working-directory access. All reads go through
//! [`FileSystem`], so discovery runs on disk ([`OsFs`]) or in memory ([`MemoryFs`]). Discovery reports facts
//! (how many directories each pattern matched, which directories lack a manifest); rules decide what is a problem.
//!
//! Patterns follow pnpm semantics for every package manager: `*` matches direct children, `**` any depth
//! (descending through packages), `!pattern` removes matches after all positive patterns, and `node_modules` and
//! dot-directories are never entered.

mod catalogs;
mod detect;
mod error;
mod fs;
mod manifest;
mod package;
mod pattern;
mod pnpm;
mod workspace;

pub use catalogs::Catalogs;
pub use detect::package_manager_from_manifest;
pub use error::WorkspaceError;
pub use fs::{DirEntry, FileSystem, MemoryFs, OsFs};
pub use manifest::{DependencyEntry, DependencyKind, Manifest};
pub use package::Package;
pub use pnpm::YamlSource;
pub use skif_config::PackageManager;
pub use workspace::{DiscoveryFacts, PatternFact, Workspace, WorkspaceSource};
