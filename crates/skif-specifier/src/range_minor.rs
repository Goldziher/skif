// Vendored from syncpack crates/syncpack-specifier/src/range_minor.rs @958d306 (MIT, Copyright (c) 2017-2023 Jamie Mason). Modified for skif; see ATTRIBUTIONS.md.
use {
    crate::{HUGE, Specifier, semver_range::SemverRange, strip_semver_range},
    std::sync::{Arc, OnceLock},
};

#[derive(Debug)]
pub struct RangeMinor {
    /// "^1.2"
    pub raw: String,
    /// Lazily parsed range: "^1.2.999999"
    node_range: OnceLock<Arc<nodejs_semver::Range>>,
    /// Lazily parsed padded version: "1.2.999999"
    node_version: OnceLock<Arc<nodejs_semver::Version>>,
    /// The raw semver specifier without range characters: "1.2"
    pub semver_number: String,
    /// The semver range characters used in this specifier
    pub semver_range: SemverRange,
}

impl PartialEq for RangeMinor {
    fn eq(&self, other: &Self) -> bool {
        self.raw == other.raw
    }
}

impl RangeMinor {
    #[must_use]
    pub fn create(raw: &str) -> Specifier {
        let semver_range = SemverRange::parse(raw);
        let semver_number = strip_semver_range(raw).to_string();
        Specifier::RangeMinor(Self {
            raw: raw.to_string(),
            node_range: OnceLock::new(),
            node_version: OnceLock::new(),
            semver_number,
            semver_range,
        })
    }

    #[cfg(test)]
    #[must_use]
    pub fn create_test(raw: &str) -> Self {
        let semver_range = SemverRange::parse(raw);
        let semver_number = strip_semver_range(raw).to_string();
        Self {
            raw: raw.to_string(),
            node_range: OnceLock::new(),
            node_version: OnceLock::new(),
            semver_number,
            semver_range,
        }
    }

    #[allow(
        clippy::expect_used,
        reason = "the raw value was validated by the parser before this variant was built"
    )]
    pub fn get_node_version(&self) -> Arc<nodejs_semver::Version> {
        self.node_version
            .get_or_init(|| {
                let padded = format!("{}.{}", self.semver_number, HUGE);
                Specifier::new_node_version(&padded).expect("pre-validated range minor version")
            })
            .clone()
    }

    #[allow(
        clippy::expect_used,
        reason = "the raw value was validated by the parser before this variant was built"
    )]
    pub fn get_node_range(&self) -> Arc<nodejs_semver::Range> {
        self.node_range
            .get_or_init(|| Specifier::new_node_range(&self.raw).expect("pre-validated range minor"))
            .clone()
    }
}
