//! Workspace discovery for skif: root and member manifests with dependency spans, catalogs, package-manager
//! detection and pattern facts.
//!
//! The crate is library-first: no printing, no exiting, no working-directory access. All reads go through
//! [`FileSystem`], so discovery runs on disk ([`OsFs`]) or in memory ([`MemoryFs`]). Discovery reports facts
//! (how many directories each pattern matched, which directories lack a manifest); rules decide what is a problem.
//!
//! Patterns follow pnpm semantics for every package manager: `*` matches direct children, `**` any depth
//! (descending through packages), `!pattern` removes matches after all positive patterns, and `node_modules` and
//! dot-directories are never entered. A `pnpm-workspace.yaml` without `packages:` selects what pnpm selects then, the
//! root and every package below it except `test` and `tests` directories (`@pnpm/find-packages` defaults to
//! `['.', '**']`); [`DiscoveryFacts::implicit_patterns`] says so. A pattern that cannot be used is listed in
//! [`Workspace::manifest_errors`] and does not stop discovery.

#![warn(missing_docs)]

mod catalogs;
mod detect;
mod error;
mod lerna;
mod manifest;
mod package;
mod parallel;
mod pattern;
mod pnpm;
mod roots;
mod rush;
mod workspace;

pub use catalogs::Catalogs;
pub use detect::package_manager_from_manifest;
pub use error::WorkspaceError;
pub use manifest::{DependencyEntry, FieldSpans, Manifest};
pub use package::{Package, PackageId};
pub use parallel::map_ordered;
pub use pnpm::YamlSource;
pub use roots::{
    Candidates, DiscoverOptions, RootKind, claim_roots, declared_member_dirs, declares_workspace, repository_root,
    scan_candidates,
};
pub use skif_config::PackageManager;
pub use skif_core::{DependencyKind, DirEntry, FileSystem, MemoryFs, OsFs};
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
    let _ = assert_send_sync::<FieldSpans>;
    let _ = assert_send_sync::<YamlSource>;
    let _ = assert_send_sync::<DiscoveryFacts>;
    let _ = assert_send_sync::<PatternFact>;
    let _ = assert_send_sync::<WorkspaceSource>;
    let _ = assert_send_sync::<Candidates>;
    let _ = assert_send_sync::<DiscoverOptions>;
    let _ = assert_send_sync::<RootKind>;
};
