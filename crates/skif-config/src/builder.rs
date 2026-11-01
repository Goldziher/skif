use crate::error::ConfigError;
use crate::model::{
    Catalog, Config, DependencyFamily, IgnoreEntry, Js, Lint, Policy, SemverGroup, VersionGroup, Workspace,
    WorkspaceProtocol,
};
use crate::rules::{Level, RuleConfig};

/// Builds a [`Config`] programmatically, starting from [`Config::default`].
#[derive(Debug, Clone, Default)]
pub struct ConfigBuilder {
    config: Config,
}

impl ConfigBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn workspace(mut self, workspace: Workspace) -> Self {
        self.config.workspace = workspace;
        self
    }

    #[must_use]
    pub fn lint(mut self, lint: Lint) -> Self {
        self.config.lint = lint;
        self
    }

    #[must_use]
    pub const fn fail_on_warnings(mut self, value: bool) -> Self {
        self.config.lint.fail_on_warnings = value;
        self
    }

    #[must_use]
    pub fn rule(mut self, id: impl Into<String>, rule: RuleConfig) -> Self {
        self.config.rules.insert(id.into(), rule);
        self
    }

    #[must_use]
    pub fn rule_level(self, id: impl Into<String>, level: Level) -> Self {
        self.rule(id, RuleConfig::level(level))
    }

    #[must_use]
    pub fn ignore(mut self, entry: IgnoreEntry) -> Self {
        self.config.ignore.push(entry);
        self
    }

    #[must_use]
    pub fn policy(mut self, policy: Policy) -> Self {
        self.config.policy = policy;
        self
    }

    #[must_use]
    pub fn version_group(mut self, group: VersionGroup) -> Self {
        self.config.version_groups.push(group);
        self
    }

    #[must_use]
    pub fn semver_group(mut self, group: SemverGroup) -> Self {
        self.config.semver_groups.push(group);
        self
    }

    #[must_use]
    pub fn catalog(mut self, catalog: Catalog) -> Self {
        self.config.catalog = catalog;
        self
    }

    #[must_use]
    pub const fn workspace_protocol(mut self, protocol: WorkspaceProtocol) -> Self {
        self.config.workspace_protocol = protocol;
        self
    }

    #[must_use]
    pub fn dependency_family(mut self, family: DependencyFamily) -> Self {
        self.config.dependency_families.push(family);
        self
    }

    #[must_use]
    pub fn js(mut self, js: Js) -> Self {
        self.config.js = js;
        self
    }

    /// Validates and returns the configuration.
    ///
    /// # Errors
    /// [`ConfigError::Invalid`] when a group is inconsistent.
    pub fn build(self) -> Result<Config, ConfigError> {
        self.config.validate()?;
        Ok(self.config)
    }
}
