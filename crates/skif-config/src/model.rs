use std::path::Path;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::dependency_types::DependencyTypes;
use crate::error::ConfigError;
use crate::overrides::ConfigOverrides;
use crate::rules::{Level, RuleConfig, rules_map};
use crate::selector::Selector;

#[cfg(feature = "schema")]
use crate::schema::RuleConfigSchema;

/// The complete `skif.toml` model.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct Config {
    pub workspace: Workspace,
    pub lint: Lint,
    #[serde(with = "rules_map")]
    #[cfg_attr(feature = "schema", schemars(with = "IndexMap<String, RuleConfigSchema>"))]
    pub rules: IndexMap<String, RuleConfig>,
    pub ignore: Vec<IgnoreEntry>,
    pub policy: Policy,
    pub version_groups: Vec<VersionGroup>,
    pub semver_groups: Vec<SemverGroup>,
    pub catalog: Catalog,
    pub workspace_protocol: WorkspaceProtocol,
    pub dependency_families: Vec<DependencyFamily>,
    pub js: Js,
}

impl Config {
    /// Parses `skif.toml` contents.
    ///
    /// # Errors
    /// [`ConfigError::Parse`] for malformed TOML, unknown keys or wrong types, and
    /// [`ConfigError::Invalid`] for inconsistent group definitions.
    pub fn from_toml_str(source: &str) -> Result<Self, ConfigError> {
        Self::parse(source, None)
    }

    /// Reads and parses a configuration file; errors carry `path`.
    ///
    /// # Errors
    /// [`ConfigError::Io`] when the file cannot be read, otherwise as [`Config::from_toml_str`].
    pub fn from_toml_file(path: &Path) -> Result<Self, ConfigError> {
        let source = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Self::parse(&source, Some(path))
    }

    fn parse(source: &str, path: Option<&Path>) -> Result<Self, ConfigError> {
        let config: Self = toml::from_str(source).map_err(|err| ConfigError::from_toml(&err, source, path))?;
        config.validate_at(path)?;
        Ok(config)
    }

    /// Serializes back to TOML.
    ///
    /// # Errors
    /// Fails only when a rule option holds a value TOML cannot represent.
    pub fn to_toml_string(&self) -> Result<String, toml::ser::Error> {
        toml::to_string(self)
    }

    /// Checks cross-field consistency of groups, catalog and families.
    ///
    /// # Errors
    /// [`ConfigError::Invalid`] naming the offending entry.
    pub fn validate(&self) -> Result<(), ConfigError> {
        self.validate_at(None)
    }

    fn validate_at(&self, path: Option<&Path>) -> Result<(), ConfigError> {
        let invalid = |message: String| Err(ConfigError::invalid(path, message));
        for (index, group) in self.version_groups.iter().enumerate() {
            if let Some(message) = group.inconsistency() {
                return invalid(format!("version-groups[{index}]: {message}"));
            }
        }
        if self.catalog.min_packages == 0 {
            return invalid("catalog.min-packages must be at least 1".to_string());
        }
        for (index, family) in self.dependency_families.iter().enumerate() {
            if family.name.is_empty() {
                return invalid(format!("dependency-families[{index}]: `name` must not be empty"));
            }
            if family.enabled && family.packages.is_empty() {
                return invalid(format!("dependency-families[{index}]: `packages` must not be empty"));
            }
        }
        Ok(())
    }

    /// Fails on the first rule ID (in `[rules]` or `[[ignore]]`) that is not in `known`, suggesting the closest ID.
    ///
    /// # Errors
    /// [`ConfigError::UnknownRule`].
    pub fn validate_rules(&self, known: &[&str]) -> Result<(), ConfigError> {
        let configured = self.rules.keys();
        let ignored = self.ignore.iter().flat_map(|entry| entry.rules.iter());
        for id in configured.chain(ignored) {
            if !known.contains(&id.as_str()) {
                return Err(ConfigError::UnknownRule {
                    rule: id.clone(),
                    suggestion: closest(id, known),
                });
            }
        }
        Ok(())
    }

    /// The configuration of rule `id`, if it appears in `[rules]`.
    #[must_use]
    pub fn rule(&self, id: &str) -> Option<&RuleConfig> {
        self.rules.get(id)
    }

    /// Layers CLI overrides over this configuration (defaults < file < CLI).
    ///
    /// # Errors
    /// [`ConfigError::Invalid`] when an ignore pattern in `overrides` is not a valid selector; the configuration is
    /// left unchanged.
    pub fn apply(&mut self, overrides: &ConfigOverrides) -> Result<(), ConfigError> {
        let ignore_packages = Selector::new(overrides.ignore_packages.iter().cloned())?;
        let ignore_dependencies = Selector::new(overrides.ignore_dependencies.iter().cloned())?;
        for (id, level) in &overrides.rules {
            self.rules.entry(id.clone()).or_default().severity = Some(*level);
        }
        for id in &overrides.ignore_rules {
            self.rules.entry(id.clone()).or_default().severity = Some(Level::Off);
        }
        if !ignore_packages.is_empty() {
            self.workspace.ignore_packages = ignore_packages;
        }
        if !ignore_dependencies.is_empty() {
            self.ignore.push(IgnoreEntry {
                dependencies: ignore_dependencies,
                ..IgnoreEntry::default()
            });
        }
        if let Some(value) = overrides.fail_on_warnings {
            self.lint.fail_on_warnings = value;
        }
        if let Some(value) = overrides.production {
            self.js.production = value;
        }
        if overrides.no_js {
            self.js.enabled = false;
        }
        Ok(())
    }
}

fn closest(id: &str, known: &[&str]) -> Option<String> {
    let limit = (id.chars().count() / 3).clamp(2, 4);
    known
        .iter()
        .map(|candidate| (strsim::levenshtein(id, candidate), candidate))
        .filter(|(distance, _)| *distance <= limit)
        .min_by_key(|(distance, _)| *distance)
        .map(|(_, candidate)| (*candidate).to_string())
}

/// Package managers that can be forced with `[workspace] package-manager`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum PackageManager {
    Npm,
    Pnpm,
    Yarn,
    Bun,
}

/// `[workspace]`
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct Workspace {
    /// Package names or globs excluded from every check.
    #[cfg_attr(feature = "schema", schemars(with = "Vec<String>"))]
    pub ignore_packages: Selector,
    /// Overrides package-manager detection.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_manager: Option<PackageManager>,
}

/// `[lint]`
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct Lint {
    /// Exit non-zero when only warnings were reported.
    pub fail_on_warnings: bool,
}

/// One `[[ignore]]` suppression. Empty lists match everything.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct IgnoreEntry {
    pub rules: Vec<String>,
    /// Package names or directories; an empty selector matches every package.
    #[cfg_attr(feature = "schema", schemars(with = "Vec<String>"))]
    pub packages: Selector,
    /// Dependency names; an empty selector matches every dependency.
    #[cfg_attr(feature = "schema", schemars(with = "Vec<String>"))]
    pub dependencies: Selector,
}

/// Which version `preferred-semver` favours.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum Prefer {
    #[default]
    Highest,
    Lowest,
}

/// `[policy]`
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct Policy {
    pub prefer: Prefer,
    /// Dependency types the policies apply to; empty means all.
    #[cfg_attr(feature = "schema", schemars(with = "Vec<String>"))]
    pub dependency_types: DependencyTypes,
}

/// The policy of a version group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum VersionPolicy {
    Pinned,
    Banned,
    Ignored,
    SameRange,
    SameMinor,
    SnappedTo,
    #[default]
    PreferredSemver,
}

/// One `[[version-groups]]` entry; first match wins. Selectors accept `!` negation and `$LOCAL`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct VersionGroup {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[cfg_attr(feature = "schema", schemars(with = "Vec<String>"))]
    pub dependencies: Selector,
    #[cfg_attr(feature = "schema", schemars(with = "Vec<String>"))]
    pub packages: Selector,
    #[cfg_attr(feature = "schema", schemars(with = "Vec<String>"))]
    pub dependency_types: DependencyTypes,
    pub specifier_types: Vec<String>,
    pub policy: VersionPolicy,
    /// Required by `pinned`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pin: Option<String>,
    /// Required by `snapped-to`: packages whose version the group follows.
    pub snap_to: Vec<String>,
    /// Allowed with `preferred-semver`; overrides `[policy] prefer`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefer: Option<Prefer>,
}

impl VersionGroup {
    fn inconsistency(&self) -> Option<&'static str> {
        match self.policy {
            VersionPolicy::Pinned if self.pin.is_none() => return Some("policy `pinned` requires `pin`"),
            VersionPolicy::SnappedTo if self.snap_to.is_empty() => {
                return Some("policy `snapped-to` requires `snap-to`");
            }
            _ => {}
        }
        if self.pin.is_some() && self.policy != VersionPolicy::Pinned {
            return Some("`pin` is only valid with policy `pinned`");
        }
        if !self.snap_to.is_empty() && self.policy != VersionPolicy::SnappedTo {
            return Some("`snap-to` is only valid with policy `snapped-to`");
        }
        if self.prefer.is_some() && self.policy != VersionPolicy::PreferredSemver {
            return Some("`prefer` is only valid with policy `preferred-semver`");
        }
        None
    }
}

/// The range type a semver group enforces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum SemverRange {
    /// No range operator: an exact version.
    #[serde(rename = "")]
    Exact,
    #[serde(rename = "*")]
    Any,
    #[serde(rename = "^")]
    Caret,
    #[serde(rename = "~")]
    Tilde,
    #[serde(rename = ">")]
    Gt,
    #[serde(rename = ">=")]
    Gte,
    #[serde(rename = "<")]
    Lt,
    #[serde(rename = "<=")]
    Lte,
}

/// One `[[semver-groups]]` entry; first match wins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct SemverGroup {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default)]
    #[cfg_attr(feature = "schema", schemars(with = "Vec<String>"))]
    pub dependencies: Selector,
    #[serde(default)]
    #[cfg_attr(feature = "schema", schemars(with = "Vec<String>"))]
    pub packages: Selector,
    #[serde(default)]
    #[cfg_attr(feature = "schema", schemars(with = "Vec<String>"))]
    pub dependency_types: DependencyTypes,
    #[serde(default)]
    pub specifier_types: Vec<String>,
    pub range: SemverRange,
}

/// `[catalog] mode`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum CatalogMode {
    #[default]
    Off,
    Prefer,
    Enforce,
}

/// `[catalog]`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct Catalog {
    pub mode: CatalogMode,
    /// Minimum number of packages using a dependency before a catalog entry is suggested.
    pub min_packages: usize,
    pub exclude: Vec<String>,
    pub report_unused_entries: bool,
}

impl Default for Catalog {
    fn default() -> Self {
        Self {
            mode: CatalogMode::Off,
            min_packages: 2,
            exclude: Vec::new(),
            report_unused_entries: false,
        }
    }
}

/// `[workspace-protocol]`
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct WorkspaceProtocol {
    /// Require `workspace:` for dependencies on other workspace packages.
    pub internal: bool,
    /// Accept `file:` / `link:` paths for internal dependencies.
    pub allow_local_paths: bool,
}

/// One `[[dependency-families]]` entry: packages that must share a version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct DependencyFamily {
    pub name: String,
    #[serde(default)]
    pub packages: Vec<String>,
    /// `false` disables a built-in family of the same name.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

const fn default_true() -> bool {
    true
}

/// `[js]`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct Js {
    pub enabled: bool,
    pub entry: Vec<String>,
    pub project: Vec<String>,
    pub ignore: Vec<String>,
    pub ignore_dependencies: Vec<String>,
    pub ignore_binaries: Vec<String>,
    pub production: bool,
    pub include_entry_exports: bool,
    /// Per-workspace overrides keyed by directory.
    pub workspaces: IndexMap<String, JsWorkspace>,
    /// Plugin switches keyed by plugin name.
    pub plugins: IndexMap<String, PluginConfig>,
}

impl Default for Js {
    fn default() -> Self {
        Self {
            enabled: true,
            entry: Vec::new(),
            project: Vec::new(),
            ignore: Vec::new(),
            ignore_dependencies: Vec::new(),
            ignore_binaries: Vec::new(),
            production: false,
            include_entry_exports: false,
            workspaces: IndexMap::new(),
            plugins: IndexMap::new(),
        }
    }
}

/// `[js.workspaces."<dir>"]`
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct JsWorkspace {
    pub entry: Vec<String>,
    pub project: Vec<String>,
    pub ignore: Vec<String>,
    pub ignore_dependencies: Vec<String>,
    pub ignore_binaries: Vec<String>,
}

/// `[js.plugins] <name> = true | { ... }`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
pub enum PluginConfig {
    Enabled(bool),
    Options(PluginOptions),
}

/// Table form of a plugin entry.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct PluginOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    pub config: Vec<String>,
    pub entry: Vec<String>,
    pub project: Vec<String>,
}
