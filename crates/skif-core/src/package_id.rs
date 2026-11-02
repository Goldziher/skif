//! Identity of a package within one workspace.

use std::fmt;

/// Identifies a package within one workspace: its index among all packages, the root first.
///
/// Ids are only meaningful for the workspace that issued them; they are cheap to copy, hash and compare, so
/// per-package facts live in vectors indexed by id instead of maps keyed by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PackageId(u32);

impl PackageId {
    /// The id of the root package.
    pub const ROOT: Self = Self(0);

    /// Position of the package among all packages, the root being 0.
    #[must_use]
    pub const fn index(self) -> usize {
        self.0 as usize
    }

    /// The id at position `index` (saturating at `u32::MAX`); only the issuing workspace should call this.
    #[must_use]
    pub fn from_index(index: usize) -> Self {
        Self(u32::try_from(index).unwrap_or(u32::MAX))
    }
}

impl fmt::Display for PackageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_round_trips_and_saturates() {
        assert_eq!(PackageId::from_index(7).index(), 7);
        assert_eq!(PackageId::from_index(0), PackageId::ROOT);
        assert_eq!(PackageId::from_index(usize::MAX).index(), u32::MAX as usize);
        assert_eq!(PackageId::from_index(3).to_string(), "#3");
        assert!(PackageId::ROOT < PackageId::from_index(1));
    }
}
