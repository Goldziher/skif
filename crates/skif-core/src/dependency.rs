//! The dependency sections of a `package.json` and the names of the packages they list.

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

/// The longest package name npm accepts, scope included (`validate-npm-package-name`).
const MAX_PACKAGE_NAME_LEN: usize = 214;

/// Whether `name` is a name the npm registry can hold: `^(@[a-z0-9-~][a-z0-9-._~]*/)?[a-z0-9-~][a-z0-9-._~]*$`,
/// ignoring case because the registry still serves legacy names with capitals. Only such a name is put in a registry
/// URL, so a name from a manifest cannot add a path segment, a query or a fragment to it. npm also caps a name at 214
/// characters, scope included.
#[must_use]
pub fn is_valid_package_name(name: &str) -> bool {
    fn segment(text: &str) -> bool {
        let mut bytes = text.bytes();
        bytes
            .next()
            .is_some_and(|first| first.is_ascii_alphanumeric() || matches!(first, b'-' | b'~'))
            && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~'))
    }
    if name.len() > MAX_PACKAGE_NAME_LEN {
        return false;
    }
    match name.strip_prefix('@') {
        Some(scoped) => scoped
            .split_once('/')
            .is_some_and(|(scope, rest)| segment(scope) && segment(rest)),
        None => segment(name),
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
