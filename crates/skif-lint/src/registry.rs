use std::collections::{HashMap, HashSet};

use skif_config::{Config, ConfigError};
use skif_core::RuleMeta;

use crate::context::EnabledRules;
use crate::provider::RuleProvider;

/// Two rules share an ID.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum RegistryError {
    #[error("rule id \"{id}\" is registered more than once")]
    DuplicateRule { id: String },
}

/// The rules of every registered provider; the single source for rule listings and validation.
pub struct Registry {
    pub(crate) providers: Vec<Box<dyn RuleProvider>>,
    metas: Vec<&'static RuleMeta>,
    owners: HashMap<&'static str, usize>,
}

impl Registry {
    /// Builds a registry from `providers`.
    ///
    /// # Errors
    /// [`RegistryError::DuplicateRule`] when a rule ID appears twice, within or across providers.
    pub fn new(providers: Vec<Box<dyn RuleProvider>>) -> Result<Self, RegistryError> {
        let mut seen = HashSet::new();
        let mut metas = Vec::new();
        let mut owners = HashMap::new();
        for (index, provider) in providers.iter().enumerate() {
            for meta in provider.rules() {
                if !seen.insert(meta.id) {
                    return Err(RegistryError::DuplicateRule { id: meta.id.to_owned() });
                }
                owners.insert(meta.id, index);
                metas.push(meta);
            }
        }
        Ok(Self {
            providers,
            metas,
            owners,
        })
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

    /// JSON Schema of the options of rule `id`, or `None` when it is unknown or takes no options.
    #[must_use]
    pub fn options_schema(&self, id: &str) -> Option<serde_json::Value> {
        self.owner(id)?.options_schema(id)
    }

    /// Checks that every rule ID in `[rules]` and `[[ignore]]` is registered and that the options of each configured
    /// rule are valid according to the rule that owns them.
    ///
    /// # Errors
    /// [`ConfigError::UnknownRule`] for the first unknown ID, [`ConfigError::InvalidRuleOptions`] for the first rule
    /// whose options are rejected.
    pub fn validate_config(&self, config: &Config) -> Result<(), ConfigError> {
        let ids: Vec<&str> = self.rules().map(|meta| meta.id).collect();
        config.validate_rules(&ids)?;
        for (id, rule) in &config.rules {
            let Some(owner) = self.owner(id) else { continue };
            owner
                .validate_options(id, &rule.options)
                .map_err(|err| ConfigError::InvalidRuleOptions {
                    rule: id.clone(),
                    key: err.key().map(str::to_owned),
                    message: err.message().to_owned(),
                })?;
        }
        Ok(())
    }

    fn owner(&self, id: &str) -> Option<&dyn RuleProvider> {
        let index = *self.owners.get(id)?;
        self.providers.get(index).map(AsRef::as_ref)
    }
}
