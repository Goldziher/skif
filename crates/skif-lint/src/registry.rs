use std::collections::HashSet;

use skif_config::{Config, ConfigError};
use skif_core::RuleMeta;

use crate::context::EnabledRules;
use crate::provider::RuleProvider;

/// Two rules share an ID.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegistryError {
    #[error("rule id \"{id}\" is registered more than once")]
    DuplicateRule { id: String },
}

/// The rules of every registered provider; the single source for rule listings and validation.
pub struct Registry {
    pub(crate) providers: Vec<Box<dyn RuleProvider>>,
    metas: Vec<&'static RuleMeta>,
}

impl Registry {
    /// Builds a registry from `providers`.
    ///
    /// # Errors
    /// [`RegistryError::DuplicateRule`] when a rule ID appears twice, within or across providers.
    pub fn new(providers: Vec<Box<dyn RuleProvider>>) -> Result<Self, RegistryError> {
        let mut seen = HashSet::new();
        let mut metas = Vec::new();
        for provider in &providers {
            for meta in provider.rules() {
                if !seen.insert(meta.id) {
                    return Err(RegistryError::DuplicateRule { id: meta.id.to_owned() });
                }
                metas.push(meta);
            }
        }
        Ok(Self { providers, metas })
    }

    /// Every registered rule, in provider order.
    pub fn rules(&self) -> impl Iterator<Item = &'static RuleMeta> + '_ {
        self.metas.iter().copied()
    }

    /// Metadata of rule `id`.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&'static RuleMeta> {
        self.rules().find(|meta| meta.id == id)
    }

    /// Resolves which rules run, and at which severity, under `config`.
    #[must_use]
    pub fn enabled(&self, config: &Config) -> EnabledRules {
        EnabledRules::resolve(self.rules(), config)
    }

    /// Checks that every rule ID in `[rules]` and `[[ignore]]` is registered.
    ///
    /// # Errors
    /// [`ConfigError::UnknownRule`] for the first unknown ID.
    pub fn validate_config(&self, config: &Config) -> Result<(), ConfigError> {
        let ids: Vec<&str> = self.rules().map(|meta| meta.id).collect();
        config.validate_rules(&ids)
    }
}
