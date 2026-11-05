//! Typed selection of the places a dependency version is declared: the four dependency sections, a package's own
//! version, the override sections and the custom types of `[[custom-types]]`.

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use skif_core::DependencyKind;

use crate::error::ConfigError;

/// A place where a dependency version is declared, as a configuration token names it.
///
/// `prod`, `dev`, `peer` and `optional` are the dependency sections; `local` is a package's own `version`;
/// `resolutions` (yarn), `overrides` (npm) and `pnpm-overrides` (`pnpm.overrides` of `package.json` and `overrides`
/// of `pnpm-workspace.yaml`) are the override sections; any other token names a `[[custom-types]]` entry.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum DependencyType {
    /// A dependency section of `package.json`.
    Section(DependencyKind),
    /// The package's own `version`.
    Local,
    /// `resolutions` of `package.json`.
    Resolutions,
    /// `overrides` of `package.json`.
    Overrides,
    /// `pnpm.overrides` of `package.json` and `overrides` of `pnpm-workspace.yaml`.
    PnpmOverrides,
    /// A `[[custom-types]]` entry, by name.
    Custom(String),
}

impl DependencyType {
    /// The type a configuration token names; a token that is no built-in type names a custom type. `None` for an
    /// empty token.
    #[must_use]
    pub fn parse(token: &str) -> Option<Self> {
        if token.is_empty() {
            return None;
        }
        Some(match token {
            "local" => Self::Local,
            "resolutions" => Self::Resolutions,
            "overrides" => Self::Overrides,
            "pnpm-overrides" | "pnpmOverrides" => Self::PnpmOverrides,
            other => DependencyKind::from_token(other).map_or_else(|| Self::Custom(other.to_owned()), Self::Section),
        })
    }

    /// Whether `token` is the name of a built-in type, which a custom type cannot take.
    #[must_use]
    pub fn is_builtin_token(token: &str) -> bool {
        !matches!(Self::parse(token), Some(Self::Custom(_)) | None)
    }

    /// The configuration token: `dev`, `local`, `pnpm-overrides` or the name of the custom type.
    #[must_use]
    pub fn token(&self) -> &str {
        match self {
            Self::Section(kind) => kind.token(),
            Self::Local => "local",
            Self::Resolutions => "resolutions",
            Self::Overrides => "overrides",
            Self::PnpmOverrides => "pnpm-overrides",
            Self::Custom(name) => name.as_str(),
        }
    }

    /// What a message calls the place: the field name of a section (`devDependencies`), `version` for a package's own
    /// version, the token otherwise.
    #[must_use]
    pub fn label(&self) -> &str {
        match self {
            Self::Section(kind) => kind.field(),
            Self::Local => "version",
            other => other.token(),
        }
    }
}

impl From<DependencyKind> for DependencyType {
    fn from(kind: DependencyKind) -> Self {
        Self::Section(kind)
    }
}

/// A list of dependency types with `!` negation, e.g. `["prod", "!peer"]`.
///
/// A type is selected when no negated entry names it and, if there are plain entries, one of them names it. An
/// empty list selects every type. Tokens are [`DependencyType::token`]s; the field names (`devDependencies`) are
/// accepted too and written back as tokens. A token that names no built-in type is a custom type: that it exists is
/// checked when the configuration is validated.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DependencyTypes {
    entries: Vec<(DependencyType, bool)>,
}

impl DependencyTypes {
    /// Parses `tokens`.
    ///
    /// # Errors
    /// [`ConfigError::Invalid`] naming the first empty token.
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
                DependencyType::parse(name).map(|ty| (ty, negated)).ok_or_else(|| {
                    format!("empty dependency type \"{token}\"; expected prod, dev, peer, optional, local, resolutions, overrides, pnpm-overrides or the name of a custom type, optionally prefixed with `!`")
                })
            })
            .collect::<Result<_, _>>()?;
        Ok(Self { entries })
    }

    /// Whether the list is empty, which selects every type.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Whether `ty` is selected.
    #[must_use]
    pub fn matches(&self, ty: &DependencyType) -> bool {
        let named = |negated: bool| {
            self.entries
                .iter()
                .any(|(entry, entry_negated)| entry == ty && *entry_negated == negated)
        };
        let has_plain = self.entries.iter().any(|&(_, negated)| !negated);
        !named(true) && (!has_plain || named(false))
    }

    /// Whether `kind` is selected.
    #[must_use]
    pub fn matches_kind(&self, kind: DependencyKind) -> bool {
        self.matches(&DependencyType::Section(kind))
    }

    /// Whether a plain (not negated) entry names `ty`.
    #[must_use]
    pub fn names(&self, ty: &DependencyType) -> bool {
        self.entries.iter().any(|(entry, negated)| entry == ty && !negated)
    }

    /// The names of the custom types the list mentions, negated or not.
    pub fn custom_names(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().filter_map(|(ty, _)| match ty {
            DependencyType::Custom(name) => Some(name.as_str()),
            _ => None,
        })
    }

    /// The entries as configuration tokens, e.g. `["prod", "!peer"]`.
    #[must_use]
    pub fn tokens(&self) -> Vec<String> {
        self.entries
            .iter()
            .map(|(ty, negated)| format!("{}{}", if *negated { "!" } else { "" }, ty.token()))
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
        DependencyKind::ALL
            .into_iter()
            .map(|kind| types.matches_kind(kind))
            .collect()
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
    fn empty_tokens_are_errors_naming_the_token() {
        for token in ["", "!"] {
            let error = DependencyTypes::new([token]).unwrap_err();
            assert!(error.to_string().contains(&format!("\"{token}\"")), "{token}: {error}");
        }
    }

    #[test]
    fn tokens_name_builtin_and_custom_types() {
        let cases = [
            ("prod", DependencyType::Section(DependencyKind::Dependencies)),
            (
                "devDependencies",
                DependencyType::Section(DependencyKind::DevDependencies),
            ),
            ("local", DependencyType::Local),
            ("resolutions", DependencyType::Resolutions),
            ("overrides", DependencyType::Overrides),
            ("pnpm-overrides", DependencyType::PnpmOverrides),
            ("pnpmOverrides", DependencyType::PnpmOverrides),
            ("engines-node", DependencyType::Custom("engines-node".to_owned())),
        ];
        for (token, expected) in cases {
            assert_eq!(DependencyType::parse(token), Some(expected), "{token}");
        }
        assert!(DependencyType::is_builtin_token("local") && !DependencyType::is_builtin_token("engines-node"));
        assert_eq!(DependencyType::parse(""), None);
    }

    #[test]
    fn local_and_custom_types_follow_the_same_selection_rules() {
        let local = DependencyType::Local;
        let custom = DependencyType::Custom("node".to_owned());
        let select =
            |tokens: &[&str], ty: &DependencyType| DependencyTypes::new(tokens.iter().copied()).unwrap().matches(ty);
        assert!(select(&[], &local) && select(&["!dev"], &local) && select(&["local"], &local));
        assert!(!select(&["!local"], &local) && !select(&["prod"], &local));
        assert!(select(&["node"], &custom) && !select(&["prod"], &custom) && !select(&["!node"], &custom));
        let types = DependencyTypes::new(["prod", "!node"]).unwrap();
        assert_eq!(types.custom_names().collect::<Vec<_>>(), ["node"]);
        assert!(types.names(&DependencyType::Section(DependencyKind::Dependencies)) && !types.names(&custom));
    }

    #[test]
    fn serde_writes_canonical_tokens() {
        let parsed: DependencyTypes =
            serde_json::from_str(r#"["devDependencies","!peer","pnpmOverrides","node"]"#).unwrap();
        assert_eq!(
            serde_json::to_string(&parsed).unwrap(),
            r#"["dev","!peer","pnpm-overrides","node"]"#
        );
        assert_eq!(parsed, ["dev", "!peer", "pnpm-overrides", "node"]);
        assert!(serde_json::from_str::<DependencyTypes>(r#"[""]"#).is_err());
    }

    #[test]
    fn labels_name_the_place() {
        assert_eq!(
            DependencyType::Section(DependencyKind::DevDependencies).label(),
            "devDependencies"
        );
        assert_eq!(DependencyType::Local.label(), "version");
        assert_eq!(DependencyType::Resolutions.label(), "resolutions");
        assert_eq!(DependencyType::Custom("node".to_owned()).label(), "node");
    }
}
