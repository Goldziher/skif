//! Rule identity and static rule metadata.

use std::borrow::Cow;
use std::fmt;

use serde::{Deserialize, Serialize};

/// Stable kebab-case identifier of a rule, e.g. `unused-dependency`.
///
/// Deserializing validates the identifier, exactly like [`RuleId::new`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(try_from = "String")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "schema", schemars(with = "String"))]
pub struct RuleId(Cow<'static, str>);

impl Serialize for RuleId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl TryFrom<String> for RuleId {
    type Error = InvalidRuleId;

    fn try_from(id: String) -> Result<Self, Self::Error> {
        Self::new(id)
    }
}

/// Error returned by [`RuleId::new`] when the identifier is not kebab-case.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid rule id {0:?}: expected kebab-case `[a-z0-9]+(-[a-z0-9]+)*`")]
pub struct InvalidRuleId(pub String);

impl RuleId {
    /// Wraps a static identifier without validation; intended for `const` rule tables.
    #[must_use]
    pub const fn from_static(id: &'static str) -> Self {
        Self(Cow::Borrowed(id))
    }

    /// Creates a rule id, rejecting anything that is not `[a-z0-9]+(-[a-z0-9]+)*`.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidRuleId`] when `id` is empty or contains characters or separators outside kebab-case.
    pub fn new(id: impl Into<String>) -> Result<Self, InvalidRuleId> {
        let id = id.into();
        if is_kebab_case(&id) {
            Ok(Self(Cow::Owned(id)))
        } else {
            Err(InvalidRuleId(id))
        }
    }

    /// The identifier as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RuleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

fn is_kebab_case(id: &str) -> bool {
    id.split('-')
        .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()))
}

/// How serious a diagnostic is. Ordered `Info < Warning < Error`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub enum Severity {
    /// Informational finding.
    Info,
    /// Should be fixed but does not fail the run by default.
    Warning,
    /// Fails the run.
    Error,
}

/// Area of the workspace a rule inspects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub enum Category {
    /// Workspace layout and configuration.
    Workspace,
    /// Individual `package.json` manifests.
    Manifest,
    /// Cross-package policies such as version consistency.
    Policy,
    /// Catalog entries.
    Catalog,
    /// JavaScript and TypeScript sources.
    Js,
}

/// Whether, and how safely, a rule can repair its findings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub enum Fixability {
    /// No fix is available.
    None,
    /// Fixes are always safe.
    Safe,
    /// Fixes may change behavior.
    Unsafe,
    /// Depends on the individual finding.
    Sometimes,
}

/// Static description of a rule, used for documentation and listings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct RuleMeta {
    /// Rule identifier.
    pub id: &'static str,
    /// Area of the workspace the rule inspects.
    pub category: Category,
    /// Severity applied unless configured otherwise; `None` means the rule is off by default.
    pub default: Option<Severity>,
    /// Fix availability.
    pub fixable: Fixability,
    /// One-line description.
    pub summary: &'static str,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn rule_id_validation_table() {
        let cases = [
            ("a", true),
            ("unused-dependency", true),
            ("a1", true),
            ("1abc", true),
            ("a-1-b", true),
            ("", false),
            ("-a", false),
            ("a-", false),
            ("a--b", false),
            ("A", false),
            ("aB", false),
            ("a_b", false),
            ("a b", false),
            ("a.b", false),
            ("é", false),
        ];
        for (id, valid) in cases {
            assert_eq!(RuleId::new(id).is_ok(), valid, "id {id:?}");
        }
    }

    #[test]
    fn invalid_rule_id_reports_input() {
        let err = RuleId::new("Bad_Id").unwrap_err();
        assert_eq!(err, InvalidRuleId("Bad_Id".to_owned()));
        assert!(err.to_string().contains("Bad_Id"));
    }

    #[test]
    fn rule_id_accessors_agree() {
        const ID: RuleId = RuleId::from_static("no-op");
        assert_eq!(ID.as_str(), "no-op");
        assert_eq!(ID.to_string(), "no-op");
        assert_eq!(ID, RuleId::new("no-op").unwrap());
    }

    #[test]
    fn rule_id_serde_is_transparent() {
        let id = RuleId::from_static("unused-dependency");
        assert_eq!(
            serde_json::to_value(&id).unwrap(),
            serde_json::json!("unused-dependency")
        );
        let back: RuleId = serde_json::from_str("\"unused-dependency\"").unwrap();
        assert_eq!(back, id);
    }

    #[test]
    fn rule_id_deserialization_validates() {
        for bad in ["\"Bad_Id\"", "\"\"", "\"a--b\"", "\"-a\"", "\"é\""] {
            let error = serde_json::from_str::<RuleId>(bad).unwrap_err();
            assert!(error.to_string().contains("invalid rule id"), "{bad}: {error}");
        }
        assert!(serde_json::from_str::<RuleId>("3").is_err());
        let ids: Vec<RuleId> = serde_json::from_str("[\"a\", \"b-1\"]").unwrap();
        assert_eq!(ids, [RuleId::from_static("a"), RuleId::from_static("b-1")]);
    }

    #[test]
    fn severity_orders_info_below_error() {
        assert!(Severity::Info < Severity::Warning);
        assert!(Severity::Warning < Severity::Error);
        assert_eq!(
            serde_json::to_value(Severity::Warning).unwrap(),
            serde_json::json!("warning")
        );
    }

    #[test]
    fn rule_meta_serializes_kebab_case() {
        let meta = RuleMeta {
            id: "unused-dependency",
            category: Category::Manifest,
            default: Some(Severity::Error),
            fixable: Fixability::Sometimes,
            summary: "Unused dependency.",
        };
        assert_eq!(
            serde_json::to_value(meta).unwrap(),
            serde_json::json!({
                "id": "unused-dependency",
                "category": "manifest",
                "default": "error",
                "fixable": "sometimes",
                "summary": "Unused dependency.",
            })
        );
    }
}
