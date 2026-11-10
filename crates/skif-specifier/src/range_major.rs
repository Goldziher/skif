// Vendored from syncpack crates/syncpack-specifier/src/range_major.rs @958d306 (MIT, Copyright (c) 2017-2023 Jamie Mason). Modified for skif; see ATTRIBUTIONS.md.
use {
    crate::{HUGE, Specifier, semver_range::SemverRange, strip_semver_range},
    std::sync::Arc,
};

/// A range over a bare major version such as `^1`.
#[derive(Debug)]
pub struct RangeMajor {
    /// "^1"
    pub raw: String,
    /// Range, parsed eagerly
    node_range: Arc<nodejs_semver::Range>,
    /// Padded version, parsed eagerly
    node_version: Arc<nodejs_semver::Version>,
    /// The raw semver specifier without range characters
    pub semver_number: String,
    /// The semver range characters used in this specifier
    pub semver_range: SemverRange,
}

impl PartialEq for RangeMajor {
    fn eq(&self, other: &Self) -> bool {
        self.raw == other.raw
    }
}

impl RangeMajor {
    /// Returns `Specifier::Unsupported` when a number is out of range for a
    /// semver component.
    /// Parses `raw` into a [`Specifier`].
    #[must_use]
    pub fn create(raw: &str) -> Specifier {
        match Self::try_new(raw) {
            Some(parsed) => Specifier::RangeMajor(parsed),
            None => Specifier::Unsupported(raw.to_string()),
        }
    }

    fn try_new(raw: &str) -> Option<Self> {
        let semver_range = SemverRange::parse(raw);
        let semver_number = strip_semver_range(raw).to_string();
        let node_version = Specifier::new_node_version(&format!("{semver_number}.{HUGE}.{HUGE}"))?;
        // The range comes from the raw text so `^2` covers `2.3.0`; only the
        // ordering version is padded.
        let node_range = Specifier::new_node_range(raw)?;
        Some(Self {
            raw: raw.to_string(),
            node_range,
            node_version,
            semver_number,
            semver_range,
        })
    }

    #[cfg(test)]
    #[allow(
        clippy::expect_used,
        clippy::missing_panics_doc,
        reason = "test fixtures are valid by construction"
    )]
    #[must_use]
    /// Builds the specifier from `raw`, panicking when it is not of this kind; a test helper, not API.
    #[doc(hidden)]
    pub fn create_test(raw: &str) -> Self {
        Self::try_new(raw).expect("valid test fixture")
    }

    /// The padded version the specifier orders by.
    #[must_use]
    pub fn get_node_version(&self) -> Arc<nodejs_semver::Version> {
        self.node_version.clone()
    }

    /// The range the specifier stands for.
    #[must_use]
    pub fn get_node_range(&self) -> Arc<nodejs_semver::Range> {
        self.node_range.clone()
    }
}
