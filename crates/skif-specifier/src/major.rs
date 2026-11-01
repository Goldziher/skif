// Vendored from syncpack crates/syncpack-specifier/src/major.rs @958d306 (MIT, Copyright (c) 2017-2023 Jamie Mason). Modified for skif; see ATTRIBUTIONS.md.
use {
    crate::{HUGE, Specifier},
    std::sync::{Arc, OnceLock},
};

#[derive(Debug)]
pub struct Major {
    /// "1"
    pub raw: String,
    /// Lazily parsed padded version: "1" -> "1.999999.999999"
    node_version: OnceLock<Arc<nodejs_semver::Version>>,
}

impl PartialEq for Major {
    fn eq(&self, other: &Self) -> bool {
        self.raw == other.raw
    }
}

impl Major {
    #[must_use]
    pub fn create(raw: &str) -> Specifier {
        Specifier::Major(Self {
            raw: raw.to_string(),
            node_version: OnceLock::new(),
        })
    }

    #[cfg(test)]
    #[must_use]
    pub fn create_test(raw: &str) -> Self {
        Self {
            raw: raw.to_string(),
            node_version: OnceLock::new(),
        }
    }

    #[allow(
        clippy::expect_used,
        reason = "the raw value was validated by the parser before this variant was built"
    )]
    pub fn get_node_version(&self) -> Arc<nodejs_semver::Version> {
        self.node_version
            .get_or_init(|| {
                let padded = format!("{}.{}.{}", self.raw, HUGE, HUGE);
                Specifier::new_node_version(&padded).expect("pre-validated major version")
            })
            .clone()
    }
}
