use crate::error::ConfigError;
use crate::model::{
    Catalog, Config, CustomType, DependencyFamily, IgnoreEntry, Js, Lint, Policy, SemverGroup, VersionGroup, Workspace,
    WorkspaceProtocol,
};
use crate::rules::{Level, RuleConfig};

/// Builds a [`Config`] programmatically, starting from [`Config::default`].
///
/// Every model struct is `#[non_exhaustive]`: build one from [`Default`], its constructor and its `with_*` methods.
///
/// ```
/// use skif_config::{ConfigBuilder, DependencyFamily, Selector, SemverGroup, SemverRange};
///
/// # fn main() -> Result<(), skif_config::ConfigError> {
/// let config = ConfigBuilder::new()
///     .semver_group(SemverGroup::new(SemverRange::Caret).with_dependencies(Selector::new(["lodash"])?))
///     .dependency_family(DependencyFamily::new("react", ["react", "react-dom"]))
///     .build()?;
/// assert_eq!(config.semver_groups.len(), 1);
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Default)]
pub struct ConfigBuilder {
    config: Config,
}

impl ConfigBuilder {
    /// An empty builder: the default configuration.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the `[workspace]` section.
    #[must_use]
    pub fn workspace(mut self, workspace: Workspace) -> Self {
        self.config.workspace = workspace;
        self
    }

    /// Sets the `[lint]` section.
    #[must_use]
    pub fn lint(mut self, lint: Lint) -> Self {
        self.config.lint = lint;
        self
    }

    /// Sets `[lint] fail-on-warnings`.
    #[must_use]
    pub const fn fail_on_warnings(mut self, value: bool) -> Self {
        self.config.lint.fail_on_warnings = value;
        self
    }

    /// Configures rule `id`.
    #[must_use]
    pub fn rule(mut self, id: impl Into<String>, rule: RuleConfig) -> Self {
        self.config.rules.insert(id.into(), rule);
        self
    }

    /// Sets the level of rule `id`, leaving its options alone.
    #[must_use]
    pub fn rule_level(self, id: impl Into<String>, level: Level) -> Self {
        self.rule(id, RuleConfig::level(level))
    }

    /// Appends an `[[ignore]]` entry.
    #[must_use]
    pub fn ignore(mut self, entry: IgnoreEntry) -> Self {
        self.config.ignore.push(entry);
        self
    }

    /// Sets the `[policy]` section.
    #[must_use]
    pub fn policy(mut self, policy: Policy) -> Self {
        self.config.policy = policy;
        self
    }

    /// Appends a `[[version-groups]]` entry.
    #[must_use]
    pub fn version_group(mut self, group: VersionGroup) -> Self {
        self.config.version_groups.push(group);
        self
    }

    /// Appends a `[[semver-groups]]` entry.
    #[must_use]
    pub fn semver_group(mut self, group: SemverGroup) -> Self {
        self.config.semver_groups.push(group);
        self
    }

    /// Sets the `[catalog]` section.
    #[must_use]
    pub fn catalog(mut self, catalog: Catalog) -> Self {
        self.config.catalog = catalog;
        self
    }

    /// Sets the `[workspace-protocol]` section.
    #[must_use]
    pub const fn workspace_protocol(mut self, protocol: WorkspaceProtocol) -> Self {
        self.config.workspace_protocol = protocol;
        self
    }

    /// Appends a `[[custom-types]]` entry.
    #[must_use]
    pub fn custom_type(mut self, custom: CustomType) -> Self {
        self.config.custom_types.push(custom);
        self
    }

    /// Appends a `[[dependency-families]]` entry.
    #[must_use]
    pub fn dependency_family(mut self, family: DependencyFamily) -> Self {
        self.config.dependency_families.push(family);
        self
    }

    /// Sets the `[js]` section.
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
