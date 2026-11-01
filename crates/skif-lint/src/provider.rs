use rayon::prelude::*;
use skif_core::RuleMeta;

use crate::context::{EnabledRules, LintContext};
use crate::emitter::Emitter;

/// A single lint rule. It detects and reports; policy lives in the [`Emitter`].
pub trait Rule: Send + Sync {
    fn meta(&self) -> &'static RuleMeta;
    fn check(&self, cx: &LintContext<'_>, out: &mut Emitter<'_>);
}

/// A source of rules, such as the manifest rules or a host's own.
pub trait RuleProvider: Send + Sync {
    /// Identifies the provider in errors.
    fn id(&self) -> &'static str;

    /// Metadata of every rule the provider can run, enabled or not.
    fn rules(&self) -> Vec<&'static RuleMeta>;

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
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            source: None,
        }
    }

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
/// Enabled rules run in parallel on the current rayon pool; their diagnostics are merged in
/// rule order, so the result does not depend on scheduling.
pub struct RulesProvider {
    id: &'static str,
    rules: Vec<Box<dyn Rule>>,
}

impl RulesProvider {
    #[must_use]
    pub fn new(id: &'static str, rules: Vec<Box<dyn Rule>>) -> Self {
        Self { id, rules }
    }
}

impl RuleProvider for RulesProvider {
    fn id(&self) -> &'static str {
        self.id
    }

    fn rules(&self) -> Vec<&'static RuleMeta> {
        self.rules.iter().map(|rule| rule.meta()).collect()
    }

    fn run(&self, cx: &LintContext<'_>, enabled: &EnabledRules, out: &mut Emitter<'_>) -> Result<(), ProviderError> {
        let active: Vec<&dyn Rule> = self
            .rules
            .iter()
            .map(AsRef::as_ref)
            .filter(|rule| enabled.contains(rule.meta().id))
            .collect();
        let base = &*out;
        let parts: Vec<Emitter<'_>> = active
            .par_iter()
            .map(|rule| {
                let mut part = base.fork();
                if !part.is_cancelled() {
                    rule.check(cx, &mut part);
                }
                part
            })
            .collect();
        for part in parts {
            out.merge(part);
        }
        Ok(())
    }
}
