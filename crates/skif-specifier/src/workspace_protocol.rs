// Vendored from syncpack crates/syncpack-specifier/src/workspace_protocol.rs @958d306 (MIT, Copyright (c) 2017-2023 Jamie Mason). Modified for skif; see ATTRIBUTIONS.md.
use {
    crate::{Specifier, semver_range::SemverRange, workspace_specifier::WorkspaceSpecifier},
    std::sync::Arc,
};

#[derive(Debug, PartialEq)]
pub struct WorkspaceProtocol {
    /// The complete raw string
    ///
    /// Examples:
    /// - "workspace:^"
    /// - "workspace:*"
    /// - "workspace:1.2.3"
    pub raw: String,

    /// The version part after "workspace:"
    ///
    /// Examples:
    /// - "^"
    /// - "*"
    /// - "1.2.3"
    pub version_str: String,

    /// Cached inner specifier for delegation
    ///
    /// Examples:
    /// - "workspace:^" -> `RangeOnly(SemverRange::Minor)`
    /// - "workspace:~" -> `RangeOnly(SemverRange::Patch)`
    /// - "workspace:*" -> `RangeOnly(SemverRange::Any)`
    /// - "workspace:1.2.3" -> Resolved(Exact("1.2.3"))
    pub inner_specifier: WorkspaceSpecifier,
}

impl WorkspaceProtocol {
    /// Create a new `WorkspaceProtocol` from a raw string
    #[must_use]
    pub fn new(raw: String) -> Option<Self> {
        let version_str = raw.strip_prefix("workspace:")?.to_string();

        // Check for symbolic workspace references that need resolution
        let inner_specifier = match version_str.as_str() {
            "*" => WorkspaceSpecifier::RangeOnly(SemverRange::Any),
            "^" => WorkspaceSpecifier::RangeOnly(SemverRange::Minor),
            "~" => WorkspaceSpecifier::RangeOnly(SemverRange::Patch),
            _ => {
                // Try to parse as complete specifier (e.g., "1.2.3", "^1.2.3")
                // Use create() instead of new() to avoid nested RefCell borrow
                let spec = Arc::new(Specifier::create(&version_str));
                WorkspaceSpecifier::Resolved(spec)
            }
        };

        Some(Self {
            raw,
            version_str,
            inner_specifier,
        })
    }

    /// Create a `WorkspaceProtocol` as Specifier variant (for compatibility)
    #[must_use]
    pub fn create(raw: &str) -> Specifier {
        // This is the old API - just delegate to new and wrap
        match Self::new(raw.to_string()) {
            Some(wp) => Specifier::WorkspaceProtocol(wp),
            None => Specifier::Unsupported(raw.to_string()),
        }
    }

    /// Check if this workspace protocol needs resolution
    #[must_use]
    pub fn needs_resolution(&self) -> bool {
        self.inner_specifier.needs_resolution()
    }

    /// Get the resolved specifier if available
    #[must_use]
    pub fn as_resolved(&self) -> Option<&Arc<Specifier>> {
        self.inner_specifier.as_resolved()
    }
}
