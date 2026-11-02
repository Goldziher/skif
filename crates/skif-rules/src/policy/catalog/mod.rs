//! Catalog rules: `catalog-not-used`, `catalog-mismatch`, `catalog-missing-entry`, `unresolved-catalog-references`
//! and `unused-catalog-entries` (ADR 0009).
//!
//! Catalogs come from `pnpm-workspace.yaml`, the root `package.json` and bun's `workspaces` object
//! ([`skif_workspace::Catalogs`]). Fixes are unsafe: they change which version is installed.

mod analysis;
mod mismatch;
mod missing_entry;
mod names;
mod not_used;
pub(crate) mod support;
mod target;
mod unresolved_references;
mod unused_entries;

pub(crate) use analysis::{is_catalogable, owned_dependencies};
pub use mismatch::CatalogMismatch;
pub use missing_entry::CatalogMissingEntry;
pub use not_used::CatalogNotUsed;
use skif_lint::Rule;
pub use unresolved_references::UnresolvedCatalogReferences;
pub use unused_entries::UnusedCatalogEntries;

/// The catalog rules, in the order their diagnostics are merged.
pub(crate) fn rules() -> Vec<Box<dyn Rule>> {
    vec![
        Box::new(CatalogNotUsed),
        Box::new(CatalogMismatch),
        Box::new(CatalogMissingEntry),
        Box::new(UnresolvedCatalogReferences),
        Box::new(UnusedCatalogEntries),
    ]
}
