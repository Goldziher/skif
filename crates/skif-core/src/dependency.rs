//! The dependency sections of a `package.json`.

use serde::{Deserialize, Serialize};

/// The dependency sections of a `package.json`.
///
/// Serialized as the `package.json` field name (`devDependencies`). Configuration selects kinds with the short
/// tokens of [`DependencyKind::token`] (`dev`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub enum DependencyKind {
    /// `dependencies`
    Dependencies,
    /// `devDependencies`
    DevDependencies,
    /// `peerDependencies`
    PeerDependencies,
    /// `optionalDependencies`
    OptionalDependencies,
}

impl DependencyKind {
    /// Every kind, in the order manifests conventionally list them.
    pub const ALL: [Self; 4] = [
        Self::Dependencies,
        Self::DevDependencies,
        Self::PeerDependencies,
        Self::OptionalDependencies,
    ];

    /// The `package.json` field name, e.g. `"devDependencies"`.
    #[must_use]
    pub const fn field(self) -> &'static str {
        match self {
            Self::Dependencies => "dependencies",
            Self::DevDependencies => "devDependencies",
            Self::PeerDependencies => "peerDependencies",
            Self::OptionalDependencies => "optionalDependencies",
        }
    }

    /// The short name used in configuration: `prod`, `dev`, `peer` or `optional`.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Dependencies => "prod",
            Self::DevDependencies => "dev",
            Self::PeerDependencies => "peer",
            Self::OptionalDependencies => "optional",
        }
    }

    /// The kind whose [`field`](Self::field) is `field`.
    #[must_use]
    pub fn from_field(field: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.field() == field)
    }

    /// The kind named by a configuration token, accepting the short [`token`](Self::token) or the field name.
    #[must_use]
    pub fn from_token(token: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.token() == token || kind.field() == token)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip_for_every_kind() {
        for kind in DependencyKind::ALL {
            assert_eq!(DependencyKind::from_field(kind.field()), Some(kind));
            assert_eq!(DependencyKind::from_token(kind.token()), Some(kind));
            assert_eq!(DependencyKind::from_token(kind.field()), Some(kind));
            assert_eq!(serde_json::to_value(kind).unwrap(), kind.field());
        }
    }

    #[test]
    fn unknown_names_are_rejected() {
        for name in ["", "Dev", "devDeps", "bundled", "!dev"] {
            assert_eq!(DependencyKind::from_field(name), None, "{name}");
            assert_eq!(DependencyKind::from_token(name), None, "{name}");
        }
    }
}
