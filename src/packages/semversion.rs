use anyhow::{Result, anyhow};
use semver::{BuildMetadata, Op, Prerelease, Version, VersionReq};
use std::{cmp::Ordering, fmt::Display};

#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub enum SemVersion {
    Exact(Version),
    Range(VersionReq),
}

impl Display for SemVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Exact(version) => f.write_str(&version.to_string()),
            Self::Range(version) => f.write_str(&version.to_string()),
        }
    }
}

impl SemVersion {
    pub fn parse(version: &str) -> Result<Self> {
        if let Ok(version) = Version::parse(version) {
            return Ok(Self::Exact(version));
        }

        if let Ok(version) = VersionReq::parse(version) {
            return Ok(Self::Range(version));
        }

        Err(anyhow!("Invalid version: {version}"))
    }

    pub fn patch(&self) -> u64 {
        match self {
            Self::Exact(version) => version.patch,
            Self::Range(version) => version
                .comparators
                .first()
                .map_or(0, |comparator| comparator.patch.unwrap_or(0)),
        }
    }

    pub fn minor(&self) -> u64 {
        match self {
            Self::Exact(version) => version.minor,
            Self::Range(version) => version
                .comparators
                .first()
                .map_or(0, |comparator| comparator.minor.unwrap_or(0)),
        }
    }

    pub fn major(&self) -> u64 {
        match self {
            Self::Exact(version) => version.major,
            Self::Range(version) => version
                .comparators
                .first()
                .map_or(0, |comparator| comparator.major),
        }
    }

    pub fn prerelease(&self) -> Prerelease {
        match self {
            Self::Exact(version) => version.pre.clone(),
            Self::Range(version) => version
                .comparators
                .first()
                .map_or(Prerelease::EMPTY, |comparator| comparator.pre.clone()),
        }
    }

    pub fn cmp(&self, other: &Self) -> Ordering {
        let mut ordering = self.patch().cmp(&other.patch());

        ordering = match self.minor().cmp(&other.minor()) {
            Ordering::Equal => ordering,
            new_ordering => new_ordering,
        };

        ordering = match self.major().cmp(&other.major()) {
            Ordering::Equal => ordering,
            new_ordering => new_ordering,
        };

        match self.prerelease().cmp(&other.prerelease()) {
            Ordering::Equal => ordering,
            new_ordering => new_ordering,
        }
    }

    pub fn is_valid(&self) -> bool {
        match self {
            Self::Exact(_) => true,
            Self::Range(version) => !version.comparators.is_empty(),
        }
    }

    /// Whether the lowest version allowed by `self` satisfies the `peer` range.
    /// Returns `None` when this cannot be determined, e.g. for upper-bounded ranges.
    pub fn satisfies(&self, peer: &Self) -> Option<bool> {
        let requirement = match peer {
            Self::Exact(version) => {
                let mut version = version.clone();
                version.build = BuildMetadata::EMPTY;

                VersionReq::parse(&format!("={version}")).ok()?
            }
            Self::Range(requirement) => requirement.clone(),
        };

        Some(requirement.matches(&self.lowest_version()?))
    }

    fn lowest_version(&self) -> Option<Version> {
        match self {
            Self::Exact(version) => Some(version.clone()),
            Self::Range(requirement) => {
                let comparator = requirement.comparators.first()?;

                match comparator.op {
                    Op::Exact | Op::Caret | Op::Tilde | Op::GreaterEq | Op::Wildcard => {
                        let mut version = Version::new(
                            comparator.major,
                            comparator.minor.unwrap_or(0),
                            comparator.patch.unwrap_or(0),
                        );
                        version.pre = comparator.pre.clone();

                        Some(version)
                    }
                    _ => None,
                }
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn satisfies(version: &str, peer: &str) -> Option<bool> {
        SemVersion::parse(version)
            .unwrap()
            .satisfies(&SemVersion::parse(peer).unwrap())
    }

    #[test]
    fn satisfies_exact_versions() {
        assert_eq!(satisfies("3.45.1-build2", "3.45.1-build2"), Some(true));
        assert_eq!(satisfies("3.45.3-build2", "3.45.1-build2"), Some(false));
        assert_eq!(satisfies("1.2.4", "1.2.3"), Some(false));
    }

    #[test]
    fn satisfies_peer_ranges() {
        assert_eq!(satisfies("18.2.0", "^18.0.0"), Some(true));
        assert_eq!(satisfies("17.0.2", "^18.0.0"), Some(false));
        assert_eq!(satisfies("16.8.0", ">=16.8.0"), Some(true));
        assert_eq!(satisfies("1.2.3", "*"), Some(true));
    }

    #[test]
    fn satisfies_dev_ranges_by_lowest_version() {
        assert_eq!(satisfies("^18.2.0", "^18.0.0"), Some(true));
        assert_eq!(satisfies("~17.0.0", "^18.0.0"), Some(false));
        assert_eq!(satisfies(">=16.8.0", ">=16.8.0"), Some(true));
    }

    #[test]
    fn satisfies_is_unknown_without_a_lower_bound() {
        assert_eq!(satisfies("<2.0.0", "^1.0.0"), None);
        assert_eq!(satisfies("*", "^1.0.0"), None);
    }
}
