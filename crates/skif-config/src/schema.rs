use crate::model::Config;
use crate::rules::{ForbidMode, Level};

/// Schema stand-in for a `[rules]` value: a level string or a table of severity plus options.
#[derive(schemars::JsonSchema)]
#[serde(untagged)]
pub enum RuleConfigSchema {
    Level(Level),
    Table(RuleTableSchema),
}

/// Table form of a rule entry; the options present depend on the rule.
#[derive(schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub struct RuleTableSchema {
    pub severity: Option<Level>,
    /// `member-toolchain-fields`
    pub mode: Option<ForbidMode>,
    /// `unused-exports`, `unused-types`
    pub ignore_tags: Option<Vec<String>>,
}

/// The JSON Schema of `skif.toml`.
#[must_use]
pub fn json_schema() -> serde_json::Value {
    schemars::schema_for!(Config).to_value()
}
