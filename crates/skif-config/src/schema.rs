use std::borrow::Cow;

use crate::model::{Config, IgnoreItem};
use crate::rules::Level;

/// Schema stand-in for a `[rules]` value: a level string or a table of `level` plus options.
/// It only describes the shape for the JSON Schema and is never constructed.
#[allow(dead_code)]
#[derive(schemars::JsonSchema)]
#[serde(untagged)]
pub enum RuleConfigSchema {
    Level(Level),
    Table(RuleTableSchema),
}

/// Table form of a rule entry; the options present depend on the rule.
#[allow(dead_code)]
#[derive(schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub struct RuleTableSchema {
    /// The level of the rule; the key that is written back. Set either `level` or `severity`, not both.
    pub level: Option<Level>,
    /// Alias of `level`.
    pub severity: Option<Level>,
    /// Rule-specific options; their shape is defined by the owning rule.
    #[serde(flatten)]
    pub options: std::collections::BTreeMap<String, serde_json::Value>,
}

/// Schema stand-in for an entry of a `[js]` ignore list: a bare string, or a table of `name` and `reason`.
/// It only describes the shape for the JSON Schema and is never constructed.
#[allow(dead_code)]
#[derive(schemars::JsonSchema)]
#[serde(untagged)]
enum IgnoreItemSchema {
    /// The name, glob or regular expression the list matches.
    Name(String),
    /// The entry with why it exists.
    Table(IgnoreItemTableSchema),
}

/// Table form of an ignore-list entry.
#[allow(dead_code)]
#[derive(schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct IgnoreItemTableSchema {
    /// The name, glob or regular expression the list matches, as the bare string form takes it.
    name: String,
    /// Why the entry exists. `--require-ignore-reasons` and `[lint] require-ignore-reasons` fail a run on an entry
    /// without one.
    reason: Option<String>,
}

impl schemars::JsonSchema for IgnoreItem {
    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("IgnoreItem")
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        <IgnoreItemSchema as schemars::JsonSchema>::json_schema(generator)
    }
}

/// The JSON Schema of `skif.toml`.
#[must_use]
pub fn json_schema() -> serde_json::Value {
    schemars::schema_for!(Config).to_value()
}
