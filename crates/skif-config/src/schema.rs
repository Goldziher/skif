use crate::model::Config;
use crate::rules::Level;

/// Schema stand-in for a `[rules]` value: a level string or a table of severity plus options.
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
    pub severity: Option<Level>,
    /// Rule-specific options; their shape is defined by the owning rule.
    #[serde(flatten)]
    pub options: std::collections::BTreeMap<String, serde_json::Value>,
}

/// The JSON Schema of `skif.toml`.
#[must_use]
pub fn json_schema() -> serde_json::Value {
    schemars::schema_for!(Config).to_value()
}
