// Vendored from syncpack crates/syncpack-specifier/src/url.rs @958d306 (MIT, Copyright (c) 2017-2023 Jamie Mason). Modified for skif; see ATTRIBUTIONS.md.
use crate::Specifier;

#[derive(Debug, PartialEq)]
pub struct Url {
    /// - "<http://insecure.com/foo.tgz>"
    /// - "<https://server.com/foo.tgz>"
    /// - "<https://server.com/foo.tgz>"
    pub raw: String,
}

impl Url {
    #[must_use]
    pub fn create(raw: &str) -> Specifier {
        Specifier::Url(Self { raw: raw.to_string() })
    }
}
