// Vendored from syncpack crates/syncpack-specifier/src/link.rs @958d306 (MIT, Copyright (c) 2017-2023 Jamie Mason). Modified for skif; see ATTRIBUTIONS.md.
use crate::Specifier;

#[derive(Debug, PartialEq)]
pub struct Link {
    /// "link:../path/to/workspace"
    /// "link:./relative/path"
    pub raw: String,
}

impl Link {
    #[must_use]
    pub fn create(raw: &str) -> Specifier {
        Specifier::Link(Self { raw: raw.to_string() })
    }
}
