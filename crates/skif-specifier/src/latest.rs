use {
    crate::{HUGE, Specifier, semver_range::SemverRange},
    std::sync::Arc,
};

#[derive(Debug)]
pub struct Latest {
    /// "*", "latest", "x"
    pub raw: String,
    /// HUGE version for ordering (999999.999999.999999)
    node_version: Arc<nodejs_semver::Version>,
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
        Specifier::Latest(Self::new(raw))
    }

    fn new(raw: &str) -> Self {
        Self {
            raw: raw.to_string(),
            node_version: Arc::new(nodejs_semver::Version::new(HUGE, HUGE, HUGE, Vec::new(), Vec::new())),
            semver_range: SemverRange::Any,
        }
    }

    #[cfg(test)]
    #[must_use]
    pub fn create_test(raw: &str) -> Self {
        Self::new(raw)
    }

    #[must_use]
    pub fn get_node_version(&self) -> Arc<nodejs_semver::Version> {
        self.node_version.clone()
    }
}
