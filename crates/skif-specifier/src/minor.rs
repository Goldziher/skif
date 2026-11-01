// Vendored from syncpack crates/syncpack-specifier/src/minor.rs @958d306 (MIT, Copyright (c) 2017-2023 Jamie Mason). Modified for skif; see ATTRIBUTIONS.md.
use {
    crate::{HUGE, Specifier},
    std::sync::{Arc, OnceLock},
};

#[derive(Debug)]
pub struct Minor {
    /// "1.2"
    pub raw: String,
    /// Lazily parsed padded version: "1.2" -> "1.2.999999"
    node_version: OnceLock<Arc<nodejs_semver::Version>>,
}

impl PartialEq for Minor {
    fn eq(&self, other: &Self) -> bool {
        self.raw == other.raw
    }
}

impl Minor {
    #[must_use]
    pub fn create(raw: &str) -> Specifier {
        Specifier::Minor(Self {
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
                let padded = format!("{}.{}", self.raw, HUGE);
                Specifier::new_node_version(&padded).expect("pre-validated minor version")
            })
            .clone()
    }
}
