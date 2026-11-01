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
mod manifest;
mod package;
mod pattern;
mod pnpm;
mod workspace;

pub use catalogs::Catalogs;
pub use detect::package_manager_from_manifest;
pub use error::WorkspaceError;
pub use skif_core::{DependencyKind, DirEntry, FileSystem, MemoryFs, OsFs};
pub use manifest::{DependencyEntry, Manifest};
pub use package::{Package, PackageId};
pub use pnpm::YamlSource;
pub use skif_config::PackageManager;
pub use workspace::{DiscoveryFacts, PatternFact, Workspace, WorkspaceSource};

const _: () = {
    fn assert_send_sync<T: Send + Sync>() {}
    let _ = assert_send_sync::<Workspace>;
    let _ = assert_send_sync::<Package>;
    let _ = assert_send_sync::<PackageId>;
    let _ = assert_send_sync::<Manifest>;
    let _ = assert_send_sync::<DependencyEntry>;
    let _ = assert_send_sync::<Catalogs>;
    let _ = assert_send_sync::<WorkspaceError>;
};
