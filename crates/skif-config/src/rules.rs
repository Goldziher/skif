use indexmap::IndexMap;
use serde::ser::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use skif_core::Severity;

/// The level a rule is configured to: `off`, `warn` or `error`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Off,
    Warn,
    Error,
}

impl Level {
    /// The diagnostic severity this level maps to, or `None` when the rule is off.
    #[must_use]
    pub const fn severity(self) -> Option<Severity> {
        match self {
            Self::Off => None,
            Self::Warn => Some(Severity::Warning),
            Self::Error => Some(Severity::Error),
        }
    }

    pub(crate) fn parse(text: &str) -> Option<Self> {
        match text {
            "off" => Some(Self::Off),
            "warn" => Some(Self::Warn),
            "error" => Some(Self::Error),
            _ => None,
        }
    }
}

/// How `member-toolchain-fields` treats toolchain fields in member manifests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum ForbidMode {
    /// Members must not declare toolchain fields.
    #[default]
    Forbid,
    /// Members may declare them only when they equal the root's.
    MatchRoot,
}

/// Options of `member-toolchain-fields`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct MemberToolchainFieldsOptions {
    pub mode: ForbidMode,
}

/// Options of `unused-exports` and `unused-types`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct UnusedExportsOptions {
    /// `JSDoc` tags (for example `public`) that exempt an export.
    pub ignore_tags: Vec<String>,
}

/// Typed per-rule options. Rules without declared options keep their raw table in [`RuleOptions::Other`].
#[derive(Debug, Clone, PartialEq, Default)]
pub enum RuleOptions {
    #[default]
    None,
    MemberToolchainFields(MemberToolchainFieldsOptions),
    UnusedExports(UnusedExportsOptions),
    Other(IndexMap<String, toml::Value>),
}

/// Configuration of one rule: an optional level plus options.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RuleConfig {
    /// `None` keeps the rule's default severity.
    pub severity: Option<Level>,
    pub options: RuleOptions,
}

impl RuleConfig {
    #[must_use]
    pub const fn level(level: Level) -> Self {
        Self {
            severity: Some(level),
            options: RuleOptions::None,
        }
    }

    /// Parses the value of `[rules] <id>`: a level string or a table.
    pub(crate) fn from_value(id: &str, value: toml::Value) -> Result<Self, String> {
        match value {
            toml::Value::String(text) => Level::parse(&text).map(Self::level).ok_or_else(|| bad_level(id, &text)),
            toml::Value::Table(mut table) => {
                let severity = table.remove("severity");
                let alias = table.remove("level");
                if severity.is_some() && alias.is_some() {
                    return Err(format!("rule \"{id}\": set either `severity` or `level`, not both"));
                }
                let level = match severity.or(alias) {
                    None => None,
                    Some(toml::Value::String(text)) => Some(Level::parse(&text).ok_or_else(|| bad_level(id, &text))?),
                    Some(other) => {
                        return Err(format!(
                            "rule \"{id}\": severity must be a string, found {}",
                            other.type_str()
                        ));
                    }
                };
                let options = parse_options(id, table)?;
                Ok(Self {
                    severity: level,
                    options,
                })
            }
            other => Err(format!(
                "rule \"{id}\": expected a level string or a table, found {}",
                other.type_str()
            )),
        }
    }

    fn to_value(&self) -> Result<toml::Value, toml::ser::Error> {
        let options = match &self.options {
            RuleOptions::None => toml::Table::new(),
            RuleOptions::MemberToolchainFields(o) => table_of(o)?,
            RuleOptions::UnusedExports(o) => table_of(o)?,
            RuleOptions::Other(map) => map.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        };
        if options.is_empty() {
            if let Some(level) = self.severity {
                return toml::Value::try_from(level);
            }
            return Ok(toml::Value::Table(options));
        }
        let mut table = toml::Table::new();
        if let Some(level) = self.severity {
            table.insert("severity".to_string(), toml::Value::try_from(level)?);
        }
        table.extend(options);
        Ok(toml::Value::Table(table))
    }
}

fn table_of<T: Serialize>(options: &T) -> Result<toml::Table, toml::ser::Error> {
    match toml::Value::try_from(options)? {
        toml::Value::Table(table) => Ok(table),
        _ => Ok(toml::Table::new()),
    }
}

fn bad_level(id: &str, text: &str) -> String {
    format!("rule \"{id}\": invalid level \"{text}\", expected one of \"off\", \"warn\", \"error\"")
}

fn parse_options(id: &str, table: toml::Table) -> Result<RuleOptions, String> {
    if table.is_empty() {
        return Ok(RuleOptions::None);
    }
    let wrap = |err: toml::de::Error| format!("rule \"{id}\": {}", err.message());
    match id {
        "member-toolchain-fields" => table.try_into().map(RuleOptions::MemberToolchainFields).map_err(wrap),
        "unused-exports" | "unused-types" => table.try_into().map(RuleOptions::UnusedExports).map_err(wrap),
        _ => Ok(RuleOptions::Other(table.into_iter().collect())),
    }
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
