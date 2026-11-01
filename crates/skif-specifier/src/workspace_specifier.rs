// Vendored from syncpack crates/syncpack-specifier/src/workspace_specifier.rs @958d306 (MIT, Copyright (c) 2017-2023 Jamie Mason). Modified for skif; see ATTRIBUTIONS.md.
use {
    crate::{Specifier, semver_range::SemverRange},
    std::sync::Arc,
};

/// Specifier type for workspace protocol dependencies
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceSpecifier {
    /// Resolved to a complete specifier
    ///
    /// Examples:
    /// - workspace:1.2.3 -> Resolved(Exact("1.2.3"))
    /// - workspace:^1.2.3 -> Resolved(Major(...))
    Resolved(Arc<Specifier>),

    /// Unresolved range prefix (requires local package version)
    ///
    /// Examples:
    /// - workspace:^ -> `RangeOnly(SemverRange::Minor)`
    /// - workspace:~ -> `RangeOnly(SemverRange::Patch)`
    /// - workspace:* -> `RangeOnly(SemverRange::Any)`
    RangeOnly(SemverRange),
}

impl WorkspaceSpecifier {
    /// Check if this requires resolution
    #[must_use]
    pub fn needs_resolution(&self) -> bool {
        matches!(self, Self::RangeOnly(_))
    }

    /// Get the underlying Specifier if resolved
    #[must_use]
    pub fn as_resolved(&self) -> Option<&Arc<Specifier>> {
        match self {
            Self::Resolved(spec) => Some(spec),
            Self::RangeOnly(_) => None,
        }
    }
}
