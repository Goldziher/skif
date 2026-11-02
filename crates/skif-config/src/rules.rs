use indexmap::IndexMap;
use serde::ser::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use skif_core::Severity;

/// The level a rule is configured to: `off`, `info`, `warn` or `error`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Level {
    Off,
    Info,
    Warn,
    Error,
}

impl Level {
    /// The diagnostic severity this level maps to, or `None` when the rule is off.
    #[must_use]
    pub const fn severity(self) -> Option<Severity> {
        match self {
            Self::Off => None,
            Self::Info => Some(Severity::Info),
            Self::Warn => Some(Severity::Warning),
            Self::Error => Some(Severity::Error),
        }
    }

    pub(crate) fn parse(text: &str) -> Option<Self> {
        match text {
            "off" => Some(Self::Off),
            "info" => Some(Self::Info),
            "warn" => Some(Self::Warn),
            "error" => Some(Self::Error),
            _ => None,
        }
    }
}

/// The raw options table of one rule. The rule that owns the options defines and validates their shape.
pub type RuleOptions = toml::Table;

/// Configuration of one rule: an optional level plus options.
///
/// In the table form, `level` is the key that sets the level and is what is written back; `severity` is accepted as
/// an alias. Both names are reserved: they are never passed on as rule options, so a rule cannot define an option
/// called `level` or `severity`, and setting both in one table is an error.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RuleConfig {
    /// `None` keeps the rule's default severity.
    pub severity: Option<Level>,
    /// Rule-specific options, exactly as written in the configuration.
    pub options: RuleOptions,
}

impl RuleConfig {
    #[must_use]
    pub fn level(level: Level) -> Self {
        Self {
            severity: Some(level),
            options: RuleOptions::new(),
        }
    }

    /// Parses the value of `[rules] <id>`: a level string or a table.
    pub(crate) fn from_value(id: &str, value: toml::Value) -> Result<Self, String> {
        match value {
            toml::Value::String(text) => Level::parse(&text).map(Self::level).ok_or_else(|| bad_level(id, &text)),
            toml::Value::Table(mut table) => {
                let severity = table.remove("severity");
                let level = table.remove("level");
                let (key, value) = match (severity, level) {
                    (Some(_), Some(_)) => {
                        return Err(format!(
                            "rule \"{id}\": set either `level` or `severity`, not both; they name the same setting"
                        ));
                    }
                    (Some(value), None) => ("severity", Some(value)),
                    (None, value) => ("level", value),
                };
                let level = match value {
                    None => None,
                    Some(toml::Value::String(text)) => Some(Level::parse(&text).ok_or_else(|| bad_level(id, &text))?),
                    Some(other) => {
                        return Err(format!(
                            "rule \"{id}\": {key} must be a string, found {}",
                            other.type_str()
                        ));
                    }
                };
                Ok(Self {
                    severity: level,
                    options: table,
                })
            }
            other => Err(format!(
                "rule \"{id}\": expected a level string or a table, found {}",
                other.type_str()
            )),
        }
    }

    fn to_value(&self) -> Result<toml::Value, toml::ser::Error> {
        let options = self.options.clone();
        if options.is_empty() {
            if let Some(level) = self.severity {
                return toml::Value::try_from(level);
            }
            return Ok(toml::Value::Table(options));
        }
        let mut table = toml::Table::new();
        if let Some(level) = self.severity {
            table.insert("level".to_string(), toml::Value::try_from(level)?);
        }
        table.extend(options);
        Ok(toml::Value::Table(table))
    }
}

fn bad_level(id: &str, text: &str) -> String {
    format!("rule \"{id}\": invalid level \"{text}\", expected one of \"off\", \"info\", \"warn\", \"error\"")
}

impl Serialize for RuleConfig {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.to_value().map_err(S::Error::custom)?.serialize(serializer)
    }
}

/// Serde adapter for `[rules]`: the option shape depends on the rule ID, so it is resolved per entry.
pub(crate) mod rules_map {
    use super::{Deserialize, Deserializer, IndexMap, RuleConfig, Serialize, Serializer};
    use serde::de::Error as _;

    pub fn serialize<S: Serializer>(map: &IndexMap<String, RuleConfig>, serializer: S) -> Result<S::Ok, S::Error> {
        map.serialize(serializer)
    }

    pub fn deserialize<'de, De: Deserializer<'de>>(
        deserializer: De,
    ) -> Result<IndexMap<String, RuleConfig>, De::Error> {
        let raw = IndexMap::<String, toml::Value>::deserialize(deserializer)?;
        raw.into_iter()
            .map(|(id, value)| match RuleConfig::from_value(&id, value) {
                Ok(config) => Ok((id, config)),
                Err(message) => Err(De::Error::custom(message)),
            })
            .collect()
    }
}
