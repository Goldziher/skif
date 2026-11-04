use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use skif_core::{FileSystem, OsFs};
use skif_specifier::CONFIG_IDENTIFIERS;

use crate::dependency_types::DependencyTypes;
use crate::error::ConfigError;
use crate::overrides::ConfigOverrides;
use crate::rules::{Level, RuleConfig, rules_map};
use crate::selector::{Selector, anchor_patterns};

#[cfg(feature = "schema")]
use crate::schema::RuleConfigSchema;

/// The complete `skif.toml` model.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct Config {
    /// Stops the upward search for configuration files: no file above this one contributes. Only valid in a file
    /// found by discovery, not in one reached through `extends`.
    #[serde(skip_serializing_if = "is_false")]
    pub root: bool,
    /// Configuration files to inherit, relative to this file. They apply before this file does, in the order listed.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub extends: Vec<String>,
    /// Inherited lists to discard before this file applies.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub reset: Vec<ResetKey>,
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
    /// Scoped rules for packages and files; only package-scoped keys are allowed in them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub overrides: Vec<Override>,
    /// Rule levels set on the command line. They also sit in `rules`; this copy tells the engine to let them beat
    /// the levels of overrides. Set by [`apply`](Self::apply), never read from a file.
    #[serde(skip)]
    #[cfg_attr(feature = "schema", schemars(skip))]
    pub cli_levels: IndexMap<String, Level>,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_false(value: &bool) -> bool {
    !*value
}

/// The inherited lists that `reset` can discard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum ResetKey {
    VersionGroups,
    SemverGroups,
    Ignore,
    DependencyFamilies,
    Overrides,
}

impl ResetKey {
    /// The key as written in `skif.toml`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::VersionGroups => "version-groups",
            Self::SemverGroups => "semver-groups",
            Self::Ignore => "ignore",
            Self::DependencyFamilies => "dependency-families",
            Self::Overrides => "overrides",
        }
    }
}

/// Why `patterns` are not a valid selector, when they are not.
fn selector_problem(patterns: &[String]) -> Option<String> {
    match Selector::new(patterns.iter().cloned()) {
        Err(ConfigError::Invalid { message, .. }) => Some(message),
        _ => None,
    }
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
        Self::from_toml_file_with_fs(path, &OsFs)
    }

    /// Reads and parses a configuration file through `fs`; errors carry `path`.
    ///
    /// # Errors
    /// [`ConfigError::Io`] when the file cannot be read, otherwise as [`Config::from_toml_str`].
    pub fn from_toml_file_with_fs(path: &Path, fs: &dyn FileSystem) -> Result<Self, ConfigError> {
        let source = fs.read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Self::parse(&source, Some(path))
    }

    /// Parses and validates one file on its own, leaving `extend-*` keys as written.
    pub(crate) fn parse_file(source: &str, path: Option<&Path>) -> Result<Self, ConfigError> {
        let config: Self = toml::from_str(source).map_err(|err| ConfigError::from_toml(&err, source, path))?;
        config.validate_at(path)?;
        Ok(config)
    }

    /// Parses a standalone file: like [`Config::parse_file`], with `extend-*` keys folded into the lists they extend.
    fn parse(source: &str, path: Option<&Path>) -> Result<Self, ConfigError> {
        let config = Self::parse_file(source, path)?;
        if !config.has_extend_keys() {
            return Ok(config);
        }
        let table: toml::Table = toml::from_str(source).map_err(|err| ConfigError::from_toml(&err, source, path))?;
        let mut merger = crate::merge::Merger::default();
        merger.apply(0, &table, &std::collections::HashMap::new());
        let folded = merger.into_table();
        toml::Value::Table(folded)
            .try_into()
            .map_err(|err: toml::de::Error| ConfigError::invalid(path, err.message()))
    }

    fn has_extend_keys(&self) -> bool {
        !self.workspace.extend_ignore_packages.is_empty()
            || !self.js.extend_ignore.is_empty()
            || !self.js.extend_ignore_dependencies.is_empty()
            || !self.js.extend_ignore_binaries.is_empty()
            || !self.catalog.extend_exclude.is_empty()
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

    pub(crate) fn validate_at(&self, path: Option<&Path>) -> Result<(), ConfigError> {
        let invalid = |message: String| Err(ConfigError::invalid(path, message));
        for (index, group) in self.version_groups.iter().enumerate() {
            if let Some(message) = group.inconsistency() {
                return invalid(format!("version-groups[{index}]: {message}"));
            }
            if let Some(message) = unknown_specifier_type(&group.specifier_types) {
                return invalid(format!("version-groups[{index}]: {message}"));
            }
        }
        for (index, group) in self.semver_groups.iter().enumerate() {
            if let Some(message) = unknown_specifier_type(&group.specifier_types) {
                return invalid(format!("semver-groups[{index}]: {message}"));
            }
        }
        for (index, entry) in self.ignore.iter().enumerate() {
            if entry.rules.is_empty() && entry.packages.is_empty() && entry.dependencies.is_empty() {
                return invalid(format!(
                    "ignore[{index}]: set at least one of `rules`, `packages` or `dependencies`; an entry without filters would suppress every diagnostic"
                ));
            }
        }
        for (index, scoped) in self.overrides.iter().enumerate() {
            if let Some(message) = scoped.inconsistency() {
                return invalid(format!("overrides[{index}]: {message}"));
            }
        }
        if self.catalog.min_packages == 0 {
            return invalid("catalog.min-packages must be at least 1".to_string());
        }
        if let Some(message) = selector_problem(&self.catalog.exclude) {
            return invalid(format!("catalog.exclude: {message}"));
        }
        for (index, family) in self.dependency_families.iter().enumerate() {
            if family.name.is_empty() {
                return invalid(format!("dependency-families[{index}]: `name` must not be empty"));
            }
            if family.enabled && family.packages.is_empty() {
                return invalid(format!("dependency-families[{index}]: `packages` must not be empty"));
            }
            if let Some(message) = selector_problem(&family.packages) {
                return invalid(format!("dependency-families[{index}]: {message}"));
            }
        }
        Ok(())
    }

    /// Fails on the first rule ID (in `[rules]`, `[[ignore]]` or an override) that is not in `known`, suggesting the closest ID.
    ///
    /// # Errors
    /// [`ConfigError::UnknownRule`].
    pub fn validate_rules(&self, known: &[&str]) -> Result<(), ConfigError> {
        let configured = self.rules.keys();
        let ignored = self.ignore.iter().flat_map(|entry| entry.rules.iter());
        let scoped = self.overrides.iter().flat_map(|scoped| {
            scoped
                .rules
                .keys()
                .chain(scoped.ignore.iter().flat_map(|entry| entry.rules.iter()))
        });
        for id in configured.chain(ignored).chain(scoped) {
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
            self.cli_levels.insert(id.clone(), *level);
        }
        for id in &overrides.ignore_rules {
            self.rules.entry(id.clone()).or_default().severity = Some(Level::Off);
            self.cli_levels.insert(id.clone(), Level::Off);
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

/// Describes the first entry of `specifier_types` (optionally negated with `!`) that is not a specifier type.
fn unknown_specifier_type(specifier_types: &[String]) -> Option<String> {
    specifier_types.iter().find_map(|raw| {
        let name = raw.strip_prefix('!').unwrap_or(raw);
        if CONFIG_IDENTIFIERS.contains(&name) {
            return None;
        }
        let hint = closest(name, &CONFIG_IDENTIFIERS).map_or_else(String::new, |s| format!("; did you mean \"{s}\"?"));
        Some(format!(
            "unknown specifier type \"{name}\"{hint} (expected one of: {})",
            CONFIG_IDENTIFIERS.join(", ")
        ))
    })
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

/// Directory names that recursive root discovery skips by default: they hold fixtures and test doubles, not projects.
pub const DEFAULT_DISCOVER_IGNORE: [&str; 4] = ["__fixtures__", "fixtures", "__mocks__", "__snapshots__"];

fn default_discover_ignore() -> Vec<String> {
    DEFAULT_DISCOVER_IGNORE.iter().map(|name| (*name).to_owned()).collect()
}

/// `[workspace]`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct Workspace {
    /// Package names or globs excluded from every check.
    #[cfg_attr(feature = "schema", schemars(with = "Vec<String>"))]
    pub ignore_packages: Selector,
    /// Overrides package-manager detection.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_manager: Option<PackageManager>,
    /// Gitignore-style patterns, relative to the directory a recursive run starts in, of directories that root
    /// discovery (`-r`) does not enter, so no project below them is found. A bare name matches at any depth; a
    /// pattern with a slash is anchored at the start directory. Replaces the default list
    /// (`__fixtures__`, `fixtures`, `__mocks__`, `__snapshots__`); `[]` skips nothing. Read from the configuration
    /// file that governs the start directory.
    pub discover_ignore: Vec<String>,
    /// Appended to `ignore-packages` instead of replacing it. Folded away when the configuration is resolved.
    #[serde(skip_serializing_if = "Selector::is_empty")]
    #[cfg_attr(feature = "schema", schemars(with = "Vec<String>"))]
    pub extend_ignore_packages: Selector,
}

impl Default for Workspace {
    fn default() -> Self {
        Self {
            ignore_packages: Selector::default(),
            extend_ignore_packages: Selector::default(),
            package_manager: None,
            discover_ignore: default_discover_ignore(),
        }
    }
}

/// `[lint]`
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct Lint {
    /// Exit non-zero when only warnings were reported.
    pub fail_on_warnings: bool,
    /// Caps the severity of every diagnostic: a rule configured above this level reports at it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_level: Option<Level>,
}

/// One `[[ignore]]` suppression. An empty list does not narrow that dimension, but at least one list must be set:
/// an entry without any filter is rejected as it would suppress everything.
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
#[non_exhaustive]
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
    /// Dependency types the policy rules apply to. Empty means production and development dependencies (`dependencies` and `devDependencies`); name `peer` or `optional` to include them.
    #[cfg_attr(feature = "schema", schemars(with = "Vec<String>"))]
    pub dependency_types: DependencyTypes,
}

/// The policy of a version group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
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
#[non_exhaustive]
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
#[non_exhaustive]
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
    /// Appended to `exclude` instead of replacing it. Folded away when the configuration is resolved.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub extend_exclude: Vec<String>,
}

impl Default for Catalog {
    fn default() -> Self {
        Self {
            mode: CatalogMode::Off,
            min_packages: 2,
            exclude: Vec::new(),
            report_unused_entries: false,
            extend_exclude: Vec::new(),
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
    /// Appended to `ignore` instead of replacing it. Folded away when the configuration is resolved.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub extend_ignore: Vec<String>,
    /// Appended to `ignore-dependencies` instead of replacing it. Folded away when the configuration is resolved.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub extend_ignore_dependencies: Vec<String>,
    /// Appended to `ignore-binaries` instead of replacing it. Folded away when the configuration is resolved.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub extend_ignore_binaries: Vec<String>,
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
            extend_ignore: Vec::new(),
            extend_ignore_dependencies: Vec::new(),
            extend_ignore_binaries: Vec::new(),
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

/// `[js]` keys an override can set for the packages it selects.
///
/// Globs are relative to each selected package. The `extend-*` keys append to what the package inherits; the others
/// replace it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct PackageJs {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub entry: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub project: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ignore: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ignore_dependencies: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ignore_binaries: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub extend_ignore: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub extend_ignore_dependencies: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub extend_ignore_binaries: Vec<String>,
    /// Plugin switches and options for the selected packages.
    #[serde(skip_serializing_if = "IndexMap::is_empty")]
    pub plugins: IndexMap<String, PluginConfig>,
}

impl PackageJs {
    /// Whether no key is set.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

/// One `[[overrides]]` entry: settings for the packages and files it selects.
///
/// Patterns are anchored at the directory of the file that declares them: a package pattern starting with `./` is a
/// directory below it, any other is a package name (or a `$LOCAL`-style token); a file pattern is always a path
/// below it. A `$ROOT/` prefix anchors a pattern at the root being linted instead, and a path that would leave the
/// anchor never matches.
///
/// Only package-scoped keys are allowed. An override never changes what a workspace-wide analysis computes; it
/// changes how the diagnostics located in the selected packages or files are reported, and the JavaScript inputs of
/// the selected packages.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct Override {
    /// Package names, or directories (`./apps/old/**`). At least one of `packages` and `files` must be set.
    #[serde(skip_serializing_if = "Selector::is_empty")]
    #[cfg_attr(feature = "schema", schemars(with = "Vec<String>"))]
    pub packages: Selector,
    /// File globs. When both lists are set, a diagnostic must match both.
    #[serde(skip_serializing_if = "Selector::is_empty")]
    #[cfg_attr(feature = "schema", schemars(with = "Vec<String>"))]
    pub files: Selector,
    /// Caps the severity of every matched diagnostic.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_level: Option<Level>,
    /// Rule levels; options are root-scoped and not allowed here.
    #[serde(with = "rules_map", skip_serializing_if = "IndexMap::is_empty")]
    #[cfg_attr(feature = "schema", schemars(with = "IndexMap<String, RuleConfigSchema>"))]
    pub rules: IndexMap<String, RuleConfig>,
    /// Suppressions limited to the selected packages and files; an entry may omit every filter.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ignore: Vec<IgnoreEntry>,
    /// JavaScript inputs for the selected packages; needs `packages` and no `files`.
    #[serde(skip_serializing_if = "PackageJs::is_empty")]
    pub js: PackageJs,
    /// Where the override was declared. Set on a resolved configuration only.
    #[serde(skip)]
    pub scope: Option<OverrideScope>,
}

impl Override {
    pub(crate) fn inconsistency(&self) -> Option<&'static str> {
        if self.packages.is_empty() && self.files.is_empty() {
            return Some(
                "set at least one of `packages` and `files`; an override without a selector would apply everywhere",
            );
        }
        if !self.js.is_empty() && (self.packages.is_empty() || !self.files.is_empty()) {
            return Some("`js` needs `packages` and cannot be combined with `files`");
        }
        if self.rules.values().any(|rule| !rule.options.is_empty()) {
            return Some("rule options are root-scoped; an override can only set rule levels");
        }
        None
    }

    /// Whether this override covers a diagnostic located in `package` (its name and absolute directory) and/or in
    /// `file` (absolute). When both selectors are set both must match; a selector that is set but has nothing to
    /// match against (a diagnostic without a package or file) does not match.
    ///
    /// False for an override without a [`scope`](Self::scope), which only a resolved configuration has.
    #[must_use]
    pub fn applies_to(&self, package: Option<(&str, &Path)>, file: Option<&Path>) -> bool {
        self.scope.as_ref().is_some_and(|scope| scope.applies_to(package, file))
    }
}

/// Where an [`Override`] was declared, with its patterns resolved to absolute ones.
#[derive(Debug, Clone, PartialEq)]
pub struct OverrideScope {
    /// The directory of the declaring file.
    pub declared_in: PathBuf,
    /// Index of the declaring file in the resolved configuration's sources.
    pub source: usize,
    has_packages: bool,
    has_files: bool,
    packages: Selector,
    files: Selector,
}

impl OverrideScope {
    /// Anchors the patterns of `scoped`, declared by the file `source` in `declared_in`, for the root `root`.
    ///
    /// # Errors
    /// [`ConfigError::Invalid`] when an anchored pattern is not a valid glob.
    pub fn new(scoped: &Override, declared_in: &Path, source: usize, root: &Path) -> Result<Self, ConfigError> {
        Ok(Self {
            declared_in: declared_in.to_path_buf(),
            source,
            has_packages: !scoped.packages.is_empty(),
            has_files: !scoped.files.is_empty(),
            packages: anchor_patterns(scoped.packages.patterns(), declared_in, root, false)?,
            files: anchor_patterns(scoped.files.patterns(), declared_in, root, true)?,
        })
    }

    fn applies_to(&self, package: Option<(&str, &Path)>, file: Option<&Path>) -> bool {
        let package_ok = !self.has_packages
            || package
                .is_some_and(|(name, dir)| self.packages.matches_any([name, skif_core::slash(dir).as_str()], true));
        let file_ok = !self.has_files || file.is_some_and(|file| self.files.matches(&skif_core::slash(file), false));
        package_ok && file_ok
    }
}
