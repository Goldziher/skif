//! Catalog rules: `catalog-not-used`, `catalog-mismatch`, `catalog-missing-entry`, `catalog-version-divergence`,
//! `unresolved-catalog-references` and `unused-catalog-entries` (ADR 0009, ADR 0048).
//!
//! Catalogs come from `pnpm-workspace.yaml`, the root `package.json`, bun's `workspaces` object and Yarn's
//! `.yarnrc.yml` ([`skif_workspace::Catalogs`]), each read only for the package manager that uses it. A fix that moves an instance onto an existing entry holding its
//! own specifier, or a compatible upgrade of it, is safe; every other fix is unsafe (ADR 0051).

mod analysis;
mod mismatch;
mod missing_entry;
pub(crate) mod names;
mod not_used;
mod references;
pub(crate) mod support;
pub(crate) mod target;
mod unresolved_references;
mod unused_entries;
mod version_divergence;

pub(crate) use analysis::UsageIndex;
pub(crate) use analysis::{is_catalogable, owned_dependencies, usages};
pub use mismatch::CatalogMismatch;
pub use missing_entry::CatalogMissingEntry;
pub use not_used::CatalogNotUsed;
use skif_config::CatalogMode;
use skif_lint::Rule;
pub use unresolved_references::UnresolvedCatalogReferences;
pub use unused_entries::UnusedCatalogEntries;
pub use version_divergence::CatalogVersionDivergence;

/// The rule that reports the plain instances of the dependencies a catalog owns (see [`owned_dependencies`]):
/// `catalog-mismatch` while `[catalog] mode` is `off`, `catalog-not-used` otherwise.
pub(crate) fn owning_rule(mode: CatalogMode) -> &'static str {
    match mode {
        CatalogMode::Off => mismatch::META.id,
        _ => not_used::META.id,
    }
}

/// The catalog rules, in the order their diagnostics are merged.
pub(crate) fn rules() -> Vec<Box<dyn Rule>> {
    vec![
        Box::new(CatalogNotUsed),
        Box::new(CatalogMismatch),
        Box::new(CatalogMissingEntry),
        Box::new(CatalogVersionDivergence),
        Box::new(UnresolvedCatalogReferences),
        Box::new(UnusedCatalogEntries),
    ]
}
