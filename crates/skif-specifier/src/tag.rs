// Vendored from syncpack crates/syncpack-specifier/src/tag.rs @958d306 (MIT, Copyright (c) 2017-2023 Jamie Mason). Modified for skif; see ATTRIBUTIONS.md.
use crate::Specifier;

/// A dist-tag such as `beta`.
#[derive(Debug, PartialEq)]
pub struct Tag {
    /// "alpha"
    pub raw: String,
}

impl Tag {
    /// Parses `raw` into a [`Specifier`].
    #[must_use]
    pub fn create(raw: &str) -> Specifier {
        Specifier::Tag(Self { raw: raw.to_string() })
    }
}
