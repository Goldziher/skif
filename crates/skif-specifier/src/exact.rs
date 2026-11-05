// Vendored from syncpack crates/syncpack-specifier/src/exact.rs @958d306 (MIT, Copyright (c) 2017-2023 Jamie Mason). Modified for skif; see ATTRIBUTIONS.md.
use {crate::Specifier, std::sync::Arc};

/// An exact version such as `1.2.3`.
#[derive(Debug, PartialEq)]
pub struct Exact {
    /// "1.2.3"
    pub raw: String,
    /// Used for ordering and comparison, semver range characters are NOT
    /// included, for example:
    ///
    /// - "1.2.3" → "1.2.3"
    /// - "^1.2.3" → "1.2.3"
    pub node_version: Arc<nodejs_semver::Version>,
    /// Range representation of exact version
    pub node_range: Arc<nodejs_semver::Range>,
}

impl Exact {
    /// Parses `raw` into a [`Specifier`].
    #[must_use]
    pub fn create(raw: &str) -> Specifier {
        // Strip leading = if present (npm allows =1.2.3 as equivalent to 1.2.3)
        let version_without_equals = raw.strip_prefix('=').unwrap_or(raw);

        match Specifier::new_node_version(version_without_equals) {
            Some(node_version) => {
                // A string that parses as a version always parses as a range
                match Specifier::new_node_range(version_without_equals) {
                    Some(node_range) => Specifier::Exact(Self {
                        raw: raw.to_string(),
                        node_version,
                        node_range,
                    }),
                    None => Specifier::Unsupported(raw.to_string()),
                }
            }
            None => Specifier::Unsupported(raw.to_string()),
        }
    }
}
