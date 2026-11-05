// Vendored from syncpack crates/syncpack-specifier/src/file.rs @958d306 (MIT, Copyright (c) 2017-2023 Jamie Mason). Modified for skif; see ATTRIBUTIONS.md.
use crate::Specifier;

/// A `file:` path.
#[derive(Debug, PartialEq)]
pub struct File {
    /// "<file:path/to/directory>"
    /// "<file:path/to/foo.tar.gz>"
    pub raw: String,
}

impl File {
    /// Parses `raw` into a [`Specifier`].
    #[must_use]
    pub fn create(raw: &str) -> Specifier {
        Specifier::File(Self { raw: raw.to_string() })
    }
}
