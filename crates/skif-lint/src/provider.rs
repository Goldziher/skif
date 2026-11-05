use skif_config::{Config, ConfigError};
use skif_core::RuleMeta;
use skif_workspace::map_ordered;

use crate::context::{EnabledRules, LintContext};
use crate::emitter::Emitter;

/// A single lint rule. It detects and reports; policy lives in the [`Emitter`].
///
/// A rule owns its options: configuration keeps the raw `[rules]` table and the rule defines, validates and reads
/// its typed form (see [`parse_options`]).
pub trait Rule: Send + Sync {
    /// The rule's identity, default severity and documentation.
    fn meta(&self) -> &'static RuleMeta;

    /// Looks for the rule's problem across the workspace and reports each one through `out`.
    fn check(&self, cx: &LintContext<'_>, out: &mut Emitter<'_>);

    /// JSON Schema of the rule's options, or `None` when it takes no options.
    fn options_schema(&self) -> Option<serde_json::Value> {
        None
    }

    /// Validates the raw options configured for this rule. The default accepts only an empty table.
    ///
    /// # Errors
    /// [`OptionsError`] naming the offending key when it can be determined.
    fn validate_options(&self, options: &toml::Table) -> Result<(), OptionsError> {
        reject_options(options)
    }
}

/// The options configured for a rule are invalid.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct OptionsError {
    key: Option<String>,
    message: String,
}

impl OptionsError {
    /// An error not attributable to a single key.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            key: None,
            message: message.into(),
        }
    }

    /// An error about option `key`.
    #[must_use]
    pub fn for_key(key: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            key: Some(key.into()),
            message: message.into(),
        }
    }

    /// The offending option key, when known.
    #[must_use]
    pub fn key(&self) -> Option<&str> {
        self.key.as_deref()
    }

    /// What is wrong.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

/// Fails on the first key of `options`; for rules that take no options.
///
/// # Errors
/// [`OptionsError`] naming the first key when `options` is not empty.
pub fn reject_options(options: &toml::Table) -> Result<(), OptionsError> {
    options.keys().next().map_or(Ok(()), |key| {
        Err(OptionsError::for_key(
            key.clone(),
            format!("unknown option `{key}`: this rule takes no options"),
        ))
    })
}

/// Deserializes the raw options of a rule into its typed options.
///
/// Combine with `#[serde(deny_unknown_fields, default)]` on `T` so unknown keys are reported. The offending key is
/// reported when the error names it or when removing exactly one key makes the table valid.
///
/// # Errors
/// [`OptionsError`] describing the deserialization failure.
pub fn parse_options<T: serde::de::DeserializeOwned>(options: &toml::Table) -> Result<T, OptionsError> {
    let parse = |table: &toml::Table| toml::Value::Table(table.clone()).try_into::<T>();
    parse(options).map_err(|err| {
        let message = err.message().to_owned();
        let key = key_in_message(&message).or_else(|| culprit(options, &parse));
        match key {
            Some(key) => OptionsError::for_key(key, message),
            None => OptionsError::new(message),
        }
    })
}

fn key_in_message(message: &str) -> Option<String> {
    let rest = ["unknown field `", "missing field `", "duplicate field `"]
        .iter()
        .find_map(|prefix| message.strip_prefix(prefix))?;
    rest.split_once('`').map(|(key, _)| key.to_owned())
}

fn culprit<T, E>(options: &toml::Table, parse: &impl Fn(&toml::Table) -> Result<T, E>) -> Option<String> {
    let mut fixed = options.keys().filter(|key| {
        let mut without = options.clone();
        without.remove(key.as_str());
        parse(&without).is_ok()
    });
    let first = fixed.next()?;
    fixed.next().is_none().then(|| first.clone())
}

/// A source of rules, such as the manifest rules or a host's own.
pub trait RuleProvider: Send + Sync {
    /// Identifies the provider in errors.
    fn id(&self) -> &'static str;

    /// Metadata of every rule the provider can run, enabled or not.
    fn rules(&self) -> Vec<&'static RuleMeta>;

    /// Validates the raw options configured for rule `rule_id`, which this provider owns. Accepts anything by default.
    ///
    /// # Errors
    /// [`OptionsError`] when the options are invalid.
    fn validate_options(&self, rule_id: &str, options: &toml::Table) -> Result<(), OptionsError> {
        let _ = (rule_id, options);
        Ok(())
    }

    /// Validates the parts of the configuration this provider reads outside `[rules]`, such as the `[js]` table.
    /// Accepts anything by default.
    ///
    /// # Errors
    /// [`ConfigError::Invalid`] naming the offending key.
    fn validate_config(&self, config: &Config) -> Result<(), ConfigError> {
        let _ = config;
        Ok(())
    }

    /// JSON Schema of the options of rule `rule_id`, or `None` when it takes no options.
    fn options_schema(&self, rule_id: &str) -> Option<serde_json::Value> {
        let _ = rule_id;
        None
    }

    /// Runs the rules enabled in `enabled`, reporting through `out`.
    ///
    /// # Errors
    /// [`ProviderError`] when the provider itself fails (as opposed to a rule finding a problem).
    fn run(&self, cx: &LintContext<'_>, enabled: &EnabledRules, out: &mut Emitter<'_>) -> Result<(), ProviderError>;
}

/// A provider failed to run.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct ProviderError {
    message: String,
    #[source]
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl ProviderError {
    /// A failure described by `message` alone.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            source: None,
        }
    }

    /// A failure described by `message` and caused by `source`.
    #[must_use]
    pub fn with_source(message: impl Into<String>, source: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self {
            message: message.into(),
            source: Some(Box::new(source)),
        }
    }
}

/// Adapts a list of [`Rule`]s into a [`RuleProvider`].
///
/// Enabled rules run in parallel on the current rayon pool, or on the calling thread when the context is
/// sequential; their diagnostics are merged in rule order, so the result does not depend on scheduling.
pub struct RulesProvider {
    id: &'static str,
    rules: Vec<Box<dyn Rule>>,
}

impl RulesProvider {
    /// A provider named `id` running `rules`; their diagnostics are merged in this order.
    #[must_use]
    pub fn new(id: &'static str, rules: Vec<Box<dyn Rule>>) -> Self {
        Self { id, rules }
    }

    fn rule(&self, id: &str) -> Option<&dyn Rule> {
        self.rules.iter().map(AsRef::as_ref).find(|rule| rule.meta().id == id)
    }
}

impl RuleProvider for RulesProvider {
    fn id(&self) -> &'static str {
        self.id
    }

    fn rules(&self) -> Vec<&'static RuleMeta> {
        self.rules.iter().map(|rule| rule.meta()).collect()
    }

    fn validate_options(&self, rule_id: &str, options: &toml::Table) -> Result<(), OptionsError> {
        self.rule(rule_id).map_or(Ok(()), |rule| rule.validate_options(options))
    }

    fn options_schema(&self, rule_id: &str) -> Option<serde_json::Value> {
        self.rule(rule_id).and_then(Rule::options_schema)
    }

    fn run(&self, cx: &LintContext<'_>, enabled: &EnabledRules, out: &mut Emitter<'_>) -> Result<(), ProviderError> {
        let active: Vec<&dyn Rule> = self
            .rules
            .iter()
            .map(AsRef::as_ref)
            .filter(|rule| enabled.contains(rule.meta().id))
            .collect();
        let base = &*out;
        let parts: Vec<Emitter<'_>> = map_ordered(&active, cx.sequential, |rule| {
            let mut part = base.fork();
            if !part.is_cancelled() {
                rule.check(cx, &mut part);
            }
            part
        });
        for part in parts {
            out.merge(part);
        }
        Ok(())
    }
}
