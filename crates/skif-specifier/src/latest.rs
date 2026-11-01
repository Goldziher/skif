// Vendored from syncpack crates/syncpack-specifier/src/latest.rs @958d306 (MIT, Copyright (c) 2017-2023 Jamie Mason). Modified for skif; see ATTRIBUTIONS.md.
use {
    crate::{HUGE, Specifier, semver_range::SemverRange},
    std::sync::{Arc, OnceLock},
};

#[derive(Debug)]
pub struct Latest {
    /// "*", "latest", "x"
    pub raw: String,
    /// Lazily parsed HUGE version for ordering (999999.999999.999999)
    node_version: OnceLock<Arc<nodejs_semver::Version>>,
    /// The semver range is always Any for Latest
    pub semver_range: SemverRange,
}

impl PartialEq for Latest {
    fn eq(&self, other: &Self) -> bool {
        self.raw == other.raw && self.semver_range == other.semver_range
    }
}

impl Latest {
    #[must_use]
    pub fn create(raw: &str) -> Specifier {
        Specifier::Latest(Self {
            raw: raw.to_string(),
            node_version: OnceLock::new(),
            semver_range: SemverRange::Any,
        })
    }

    #[cfg(test)]
    #[must_use]
    pub fn create_test(raw: &str) -> Self {
        Self {
            raw: raw.to_string(),
            node_version: OnceLock::new(),
            semver_range: SemverRange::Any,
        }
    }

    #[allow(
        clippy::expect_used,
        reason = "the raw value was validated by the parser before this variant was built"
    )]
    pub fn get_node_version(&self) -> Arc<nodejs_semver::Version> {
        self.node_version
            .get_or_init(|| {
                let huge = HUGE.to_string();
                let huge_version = format!("{huge}.{huge}.{huge}");
                Specifier::new_node_version(&huge_version).expect("HUGE version is always valid")
            })
            .clone()
    }
}
