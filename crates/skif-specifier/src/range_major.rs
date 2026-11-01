// Vendored from syncpack crates/syncpack-specifier/src/range_major.rs @958d306 (MIT, Copyright (c) 2017-2023 Jamie Mason). Modified for skif; see ATTRIBUTIONS.md.
use {
    crate::{HUGE, Specifier, semver_range::SemverRange, strip_semver_range},
    std::sync::{Arc, OnceLock},
};

#[derive(Debug)]
pub struct RangeMajor {
    /// "^1"
    pub raw: String,
    /// Lazily parsed range: "^1.999999.999999"
    node_range: OnceLock<Arc<nodejs_semver::Range>>,
    /// Lazily parsed padded version: "1.999999.999999"
    node_version: OnceLock<Arc<nodejs_semver::Version>>,
    /// The raw semver specifier without range characters: "1"
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
    #[must_use]
    pub fn create(raw: &str) -> Specifier {
        let semver_range = SemverRange::parse(raw);
        let semver_number = strip_semver_range(raw).to_string();
        Specifier::RangeMajor(Self {
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
                let padded = format!("{}.{}.{}", self.semver_number, HUGE, HUGE);
                Specifier::new_node_version(&padded).expect("pre-validated range major version")
            })
            .clone()
    }

    #[allow(
        clippy::expect_used,
        reason = "the raw value was validated by the parser before this variant was built"
    )]
    pub fn get_node_range(&self) -> Arc<nodejs_semver::Range> {
        self.node_range
            .get_or_init(|| {
                let padded = format!("{}.{}.{}", self.semver_number, HUGE, HUGE);
                let range_str = format!("{}{}", self.semver_range.unwrap(), padded);
                Specifier::new_node_range(&range_str).expect("pre-validated range major")
            })
            .clone()
    }
}
