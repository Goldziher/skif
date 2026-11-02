//! The JS rules, one per file.

mod circular_imports;
mod duplicate_exports;
mod members;
mod support;
mod unlisted_binaries;
mod unlisted_dependencies;
mod unparseable_files;
mod unresolved_imports;
mod unused_dependencies;
mod unused_dev_dependencies;
mod unused_enum_members;
mod unused_exports;
mod unused_files;
mod unused_namespace_members;
mod unused_optional_peer_dependencies;
mod unused_types;

use skif_core::RuleMeta;
use skif_lint::Emitter;

pub use circular_imports::CircularImports;
pub use duplicate_exports::DuplicateExports;
pub use unlisted_binaries::UnlistedBinaries;
pub use unlisted_dependencies::UnlistedDependencies;
pub use unparseable_files::UnparseableFiles;
pub use unresolved_imports::UnresolvedImports;
pub use unused_dependencies::UnusedDependencies;
pub use unused_dev_dependencies::UnusedDevDependencies;
pub use unused_enum_members::UnusedEnumMembers;
pub use unused_exports::UnusedExports;
pub use unused_files::UnusedFiles;
pub use unused_namespace_members::UnusedNamespaceMembers;
pub use unused_optional_peer_dependencies::UnusedOptionalPeerDependencies;
pub use unused_types::UnusedTypes;

use crate::analysis::JsAnalysis;

/// A JS rule. Unlike a manifest rule it reads the shared [`JsAnalysis`], which is built once per run.
pub trait JsRule: Send + Sync {
    /// Identifier, default severity and summary.
    fn meta(&self) -> &'static RuleMeta;

    /// Reports every finding to `out`.
    fn check(&self, analysis: &JsAnalysis<'_>, out: &mut Emitter<'_>);
}

/// Every JS rule, in the order their diagnostics are merged.
#[must_use]
pub fn rules() -> Vec<Box<dyn JsRule>> {
    vec![
        Box::new(UnusedDependencies),
        Box::new(UnusedDevDependencies),
        Box::new(UnlistedDependencies),
        Box::new(UnresolvedImports),
        Box::new(UnlistedBinaries),
        Box::new(UnusedOptionalPeerDependencies),
        Box::new(UnusedFiles),
        Box::new(UnusedExports),
        Box::new(UnusedTypes),
        Box::new(UnusedEnumMembers),
        Box::new(UnusedNamespaceMembers),
        Box::new(DuplicateExports),
        Box::new(CircularImports),
        Box::new(UnparseableFiles),
    ]
}
