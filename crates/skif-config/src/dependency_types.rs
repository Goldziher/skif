//! Typed selection of dependency sections (`prod`, `dev`, `peer`, `optional`).

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use skif_core::DependencyKind;

use crate::error::ConfigError;

/// A list of dependency kinds with `!` negation, e.g. `["prod", "!peer"]`.
///
/// A kind is selected when no negated entry names it and, if there are plain entries, one of them names it. An
/// empty list selects every kind. Tokens are [`DependencyKind::token`]s; the field names (`devDependencies`) are
/// accepted too and written back as tokens.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DependencyTypes {
    entries: Vec<(DependencyKind, bool)>,
}

impl DependencyTypes {
    /// Parses `tokens`.
    ///
    /// # Errors
    /// [`ConfigError::Invalid`] naming the first unknown token.
    pub fn new<I, S>(tokens: I) -> Result<Self, ConfigError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        Self::parse(tokens).map_err(|message| ConfigError::invalid(None, message))
    }

    fn parse<I, S>(tokens: I) -> Result<Self, String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let entries = tokens
            .into_iter()
            .map(|token| {
                let token = token.as_ref();
                let (negated, name) = token.strip_prefix('!').map_or((false, token), |name| (true, name));
                DependencyKind::from_token(name).map(|kind| (kind, negated)).ok_or_else(|| {
                    format!("unknown dependency type \"{token}\"; expected prod, dev, peer or optional, optionally prefixed with `!`")
                })
            })
            .collect::<Result<_, _>>()?;
        Ok(Self { entries })
    }

    /// Whether the list is empty, which selects every kind.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Whether `kind` is selected.
    #[must_use]
    pub fn matches(&self, kind: DependencyKind) -> bool {
        let named = |negated: bool| self.entries.iter().any(|&(entry, entry_negated)| entry == kind && entry_negated == negated);
        let has_plain = self.entries.iter().any(|&(_, negated)| !negated);
        !named(true) && (!has_plain || named(false))
    }

    /// The entries as configuration tokens, e.g. `["prod", "!peer"]`.
    #[must_use]
    pub fn tokens(&self) -> Vec<String> {
        self.entries
            .iter()
            .map(|&(kind, negated)| format!("{}{}", if negated { "!" } else { "" }, kind.token()))
            .collect()
    }
}

impl<const N: usize> PartialEq<[&str; N]> for DependencyTypes {
    fn eq(&self, other: &[&str; N]) -> bool {
        self.tokens().iter().map(String::as_str).eq(other.iter().copied())
    }
}

impl Serialize for DependencyTypes {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.tokens().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for DependencyTypes {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let tokens = Vec::<String>::deserialize(deserializer)?;
        Self::parse(&tokens).map_err(D::Error::custom)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn kinds(tokens: &[&str]) -> Vec<bool> {
        let types = DependencyTypes::new(tokens.iter().copied()).unwrap();
        DependencyKind::ALL.into_iter().map(|kind| types.matches(kind)).collect()
    }

    #[test]
    fn selection_table() {
        // Order of the flags: prod, dev, peer, optional.
        let cases: &[(&[&str], [bool; 4])] = &[
            (&[], [true, true, true, true]),
            (&["prod", "dev"], [true, true, false, false]),
            (&["prod", "!peer"], [true, false, false, false]),
            (&["!peer"], [true, true, false, true]),
            (&["devDependencies", "optional"], [false, true, false, true]),
            (&["dev", "!dev"], [false, false, false, false]),
        ];
        for &(tokens, expected) in cases {
            assert_eq!(kinds(tokens), expected, "{tokens:?}");
        }
    }

    #[test]
    fn unknown_tokens_are_errors_naming_the_token() {
        for token in ["bundled", "!bundled", "", "Dev"] {
            let error = DependencyTypes::new([token]).unwrap_err();
            assert!(error.to_string().contains(&format!("\"{token}\"")), "{token}: {error}");
        }
    }

    #[test]
    fn serde_writes_canonical_tokens() {
        let parsed: DependencyTypes = serde_json::from_str(r#"["devDependencies","!peer"]"#).unwrap();
        assert_eq!(serde_json::to_string(&parsed).unwrap(), r#"["dev","!peer"]"#);
        assert_eq!(parsed, ["dev", "!peer"]);
        assert!(serde_json::from_str::<DependencyTypes>(r#"["nope"]"#).is_err());
    }
}
