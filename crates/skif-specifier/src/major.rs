// Vendored from syncpack crates/syncpack-specifier/src/major.rs @958d306 (MIT, Copyright (c) 2017-2023 Jamie Mason). Modified for skif; see ATTRIBUTIONS.md.
use {
    crate::{HUGE, Specifier},
    std::sync::Arc,
};

/// A bare major version such as `1`.
#[derive(Debug)]
pub struct Major {
    /// "1"
    pub raw: String,
    /// Padded version, parsed eagerly: "1" -> "1.999999.999999"
    node_version: Arc<nodejs_semver::Version>,
}

impl PartialEq for Major {
    fn eq(&self, other: &Self) -> bool {
        self.raw == other.raw
    }
}

impl Major {
    /// Returns `Specifier::Unsupported` when the number is out of range for a
    /// semver component.
    /// Parses `raw` into a [`Specifier`].
    #[must_use]
    pub fn create(raw: &str) -> Specifier {
        match Self::try_new(raw) {
            Some(major) => Specifier::Major(major),
            None => Specifier::Unsupported(raw.to_string()),
        }
    }

    fn try_new(raw: &str) -> Option<Self> {
        let node_version = Specifier::new_node_version(&format!("{raw}.{HUGE}.{HUGE}"))?;
        Some(Self {
            raw: raw.to_string(),
            node_version,
        })
    }

    #[cfg(test)]
    #[allow(
        clippy::expect_used,
        clippy::missing_panics_doc,
        reason = "test fixtures are valid by construction"
    )]
    #[must_use]
    pub fn create_test(raw: &str) -> Self {
        Self::try_new(raw).expect("valid test fixture")
    }

    /// The padded version the specifier orders by.
    #[must_use]
    pub fn get_node_version(&self) -> Arc<nodejs_semver::Version> {
        self.node_version.clone()
    }
}
