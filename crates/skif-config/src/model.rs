use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use skif_core::{FileSystem, OsFs};
use skif_specifier::CONFIG_IDENTIFIERS;

use crate::dependency_types::{DependencyType, DependencyTypes};
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
#[non_exhaustive]
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
    /// `[workspace]`: package-manager detection and which packages are skipped.
    pub workspace: Workspace,
    /// `[lint]`: how diagnostics are reported.
    pub lint: Lint,
    /// `[rules]`: the level and options of every configured rule, in file order.
    #[serde(with = "rules_map")]
    #[cfg_attr(feature = "schema", schemars(with = "IndexMap<String, RuleConfigSchema>"))]
    pub rules: IndexMap<String, RuleConfig>,
    /// `[[ignore]]`: suppressions for diagnostics.
    pub ignore: Vec<IgnoreEntry>,
    /// `[policy]`: defaults for the version policies.
    pub policy: Policy,
    /// `[[version-groups]]`: how dependency versions must relate, first match wins.
    pub version_groups: Vec<VersionGroup>,
    /// `[[semver-groups]]`: the range type dependencies must use, first match wins.
    pub semver_groups: Vec<SemverGroup>,
    /// `[[custom-types]]`: manifest fields that hold versions and take part in the version policies.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub custom_types: Vec<CustomType>,
    /// `[catalog]`: when catalog entries are suggested and reported.
    pub catalog: Catalog,
    /// `[workspace-protocol]`: how dependencies on workspace packages are written.
    pub workspace_protocol: WorkspaceProtocol,
    /// `[[dependency-families]]`: packages that must share a version.
    pub dependency_families: Vec<DependencyFamily>,
    /// `[update]`: how `skif update` reads the package registry. Linting never reads it.
    pub update: Update,
    /// `[[update-groups]]`: how far `skif update` may move the dependencies it matches, first match wins.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub update_groups: Vec<UpdateGroup>,
    /// `[js]`: the JavaScript and TypeScript analysis.
    pub js: Js,
    /// `[format]`: how `package-json-format` and `skif format` order and lay out `package.json` files.
    pub format: Format,
    /// Scoped rules for packages and files; only package-scoped keys are allowed in them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub overrides: Vec<Override>,
    /// Rule levels set on the command line. They also sit in `rules`; this copy tells the engine to let them beat
    /// the levels of overrides. Set by [`apply`](Self::apply), never read from a file.
    #[serde(skip)]
    #[cfg_attr(feature = "schema", schemars(skip))]
    pub cli_levels: IndexMap<String, Level>,
    /// The instances the version-group policies see, narrowed on the command line (`--dependency`,
    /// `--dependency-types`, `--specifier-types`, `--source`). Never read from a file.
    #[serde(skip)]
    #[cfg_attr(feature = "schema", schemars(skip))]
    pub filter: InstanceFilter,
}

/// Narrows the dependency instances the version-group policies see, as syncpack's command-line filters do: an
/// instance that does not pass is left out before it is assigned to a group, so it is neither reported nor aligned
/// with. Empty selects everything.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct InstanceFilter {
    /// Dependency names or globs.
    pub dependencies: Selector,
    /// Dependency types; a non-empty list also replaces `[policy] dependency-types` as the scope of the default
    /// group, so `["peer"]` governs peer dependencies.
    pub dependency_types: DependencyTypes,
    /// Specifier types (`exact`, `range`, ...), optionally negated with `!`.
    pub specifier_types: Vec<String>,
    /// Globs of the files the instances are declared in, relative to the workspace root (`packages/*/package.json`).
    pub sources: Selector,
}

impl InstanceFilter {
    /// Builds a filter from command-line values.
    ///
    /// # Errors
    /// [`ConfigError::Invalid`] for a glob that does not compile, an empty dependency type or an unknown specifier
    /// type.
    pub fn new(
        dependencies: &[String],
        dependency_types: &[String],
        specifier_types: &[String],
        sources: &[String],
    ) -> Result<Self, ConfigError> {
        if let Some(message) = unknown_specifier_type(specifier_types) {
            return Err(ConfigError::invalid(None, format!("--specifier-types: {message}")));
        }
        Ok(Self {
            dependencies: Selector::new(dependencies.iter().cloned())?,
            dependency_types: DependencyTypes::new(dependency_types)?,
            specifier_types: specifier_types.to_vec(),
            sources: Selector::new(sources.iter().cloned())?,
        })
    }

    /// Whether the filter selects everything.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.dependencies.is_empty()
            && self.dependency_types.is_empty()
            && self.specifier_types.is_empty()
            && self.sources.is_empty()
    }

    /// The first custom type the filter names that `config` does not define.
    #[must_use]
    pub fn unknown_custom_type<'a>(&'a self, config: &Config) -> Option<&'a str> {
        self.dependency_types
            .custom_names()
            .find(|name| config.custom_types.iter().all(|custom| custom.name != *name))
    }
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
    /// Discard the inherited `version-groups`.
    VersionGroups,
    /// Discard the inherited `semver-groups`.
    SemverGroups,
    /// Discard the inherited `ignore` entries.
    Ignore,
    /// Discard the inherited `dependency-families`.
    DependencyFamilies,
    /// Discard the inherited `custom-types`.
    CustomTypes,
    /// Discard the inherited `overrides`.
    Overrides,
    /// Discard the inherited `update-groups`.
    UpdateGroups,
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
            Self::CustomTypes => "custom-types",
            Self::Overrides => "overrides",
            Self::UpdateGroups => "update-groups",
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

    /// Parses and validates the typed form of an already parsed document, leaving `extend-*` keys as written.
    ///
    /// `resolve_references` is whether a `dependency-types` list may only name the custom types this document
    /// defines. A layer of a chain leaves that to the merged configuration, where the types of every layer are known.
    pub(crate) fn from_document(
        document: toml::Spanned<toml::de::DeTable<'_>>,
        source: &str,
        path: Option<&Path>,
        resolve_references: bool,
    ) -> Result<Self, ConfigError> {
        let config = Self::deserialize(toml::de::Deserializer::from(document))
            .map_err(|err| ConfigError::from_toml(&err, source, path))?;
        config.validate_with(path, resolve_references)?;
        Ok(config)
    }

    /// Parses a standalone file: the document is parsed once, its `extend-*` keys are folded into the lists they
    /// extend.
    fn parse(source: &str, path: Option<&Path>) -> Result<Self, ConfigError> {
        let from_toml = |err: &toml::de::Error| ConfigError::from_toml(err, source, path);
        let document = toml::de::DeTable::parse(source).map_err(|err| from_toml(&err))?;
        let config = Self::from_document(document.clone(), source, path, true)?;
        if !config.has_extend_keys() {
            return Ok(config);
        }
        let table = toml::Table::deserialize(toml::de::Deserializer::from(document)).map_err(|err| from_toml(&err))?;
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
            || !self.js.extend_ignore_unresolved.is_empty()
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
        self.validate_with(None, true)
    }

    fn validate_with(&self, path: Option<&Path>, resolve_references: bool) -> Result<(), ConfigError> {
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
            if let Some(message) = group.inconsistency() {
                return invalid(format!("semver-groups[{index}]: {message}"));
            }
            if let Some(message) = unknown_specifier_type(&group.specifier_types) {
                return invalid(format!("semver-groups[{index}]: {message}"));
            }
        }
        for (index, group) in self.update_groups.iter().enumerate() {
            if let Some(message) = group.inconsistency() {
                return invalid(format!("update-groups[{index}]: {message}"));
            }
            if let Some(message) = unknown_specifier_type(&group.specifier_types) {
                return invalid(format!("update-groups[{index}]: {message}"));
            }
        }
        if self.update.max_concurrent_requests == 0 {
            return invalid("update.max-concurrent-requests must be at least 1".to_string());
        }
        if let Some(message) = self.custom_type_problem(resolve_references) {
            return invalid(message);
        }
        for (index, entry) in self.ignore.iter().enumerate() {
            if entry.rules.is_empty() && entry.packages.is_empty() && entry.dependencies.is_empty() {
                return invalid(format!(
                    "ignore[{index}]: set at least one of `rules`, `packages` or `dependencies`; an entry without filters would suppress every diagnostic"
                ));
            }
            if entry.reason.as_deref().is_some_and(|reason| reason.trim().is_empty()) {
                return invalid(format!("ignore[{index}]: `reason` is empty; say why or remove it"));
            }
        }
        if let Some(message) = self.format.inconsistency() {
            return invalid(format!("format.{message}"));
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
        for (index, pattern) in self.workspace.discover_ignore.iter().enumerate() {
            // The gitignore parser reads an unclosed class such as `[bad` literally; a typo like that is reported.
            let glob = pattern.strip_prefix('!').unwrap_or(pattern).trim_start_matches('/');
            let mut builder = ignore::gitignore::GitignoreBuilder::new("");
            let checked = builder
                .add_line(None, pattern)
                .map(|_| ())
                .map_err(|err| err.to_string())
                .and_then(|()| globset::Glob::new(glob).map(|_| ()).map_err(|err| err.to_string()));
            if let Err(err) = checked {
                return invalid(format!("workspace.discover-ignore[{index}]: {err}"));
            }
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

    /// The first problem of `[[custom-types]]`, or, with `resolve_references`, of a `dependency-types` list naming a
    /// custom type that is not defined.
    fn custom_type_problem(&self, resolve_references: bool) -> Option<String> {
        for (index, custom) in self.custom_types.iter().enumerate() {
            if let Some(message) = custom.inconsistency() {
                return Some(format!("custom-types[{index}]: {message}"));
            }
            if self.custom_types[..index]
                .iter()
                .any(|earlier| earlier.name == custom.name)
            {
                return Some(format!(
                    "custom-types[{index}]: the name \"{}\" is defined twice",
                    custom.name
                ));
            }
        }
        if !resolve_references {
            return None;
        }
        let lists = std::iter::once(("policy.dependency-types".to_owned(), &self.policy.dependency_types))
            .chain(self.version_groups.iter().enumerate().map(|(index, group)| {
                (
                    format!("version-groups[{index}].dependency-types"),
                    &group.dependency_types,
                )
            }))
            .chain(self.semver_groups.iter().enumerate().map(|(index, group)| {
                (
                    format!("semver-groups[{index}].dependency-types"),
                    &group.dependency_types,
                )
            }))
            .chain(self.update_groups.iter().enumerate().map(|(index, group)| {
                (
                    format!("update-groups[{index}].dependency-types"),
                    &group.dependency_types,
                )
            }));
        for (place, types) in lists {
            if let Some(name) = types
                .custom_names()
                .find(|name| self.custom_types.iter().all(|custom| custom.name != *name))
            {
                let hint = closest(
                    name,
                    &self
                        .custom_types
                        .iter()
                        .map(|custom| custom.name.as_str())
                        .chain(BUILTIN_TYPE_TOKENS)
                        .collect::<Vec<_>>(),
                )
                .map_or_else(String::new, |s| format!("; did you mean \"{s}\"?"));
                return Some(format!(
                    "{place}: unknown dependency type \"{name}\"{hint} (expected {}, or the name of a `[[custom-types]]` entry)",
                    BUILTIN_TYPE_TOKENS.join(", ")
                ));
            }
        }
        None
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

    /// Fails on the first plugin id (in `[js.plugins]`, a `[js.workspaces]` table or an override) that is not in `known`,
    /// suggesting the closest id.
    ///
    /// # Errors
    /// [`ConfigError::Invalid`] naming the key.
    pub fn validate_plugins(&self, known: &[&str]) -> Result<(), ConfigError> {
        let workspaces = self
            .js
            .workspaces
            .iter()
            .map(|(dir, workspace)| (format!("js.workspaces.\"{dir}\".plugins"), &workspace.plugins));
        let overrides = self
            .overrides
            .iter()
            .enumerate()
            .map(|(index, scoped)| (format!("overrides[{index}].js.plugins"), &scoped.js.plugins));
        let tables = std::iter::once(("js.plugins".to_owned(), &self.js.plugins))
            .chain(workspaces)
            .chain(overrides);
        for (key, table) in tables {
            if let Some(id) = table.keys().find(|id| !known.contains(&id.as_str())) {
                let hint = closest(id, known).map_or_else(String::new, |close| format!("; did you mean `{close}`?"));
                return Err(ConfigError::invalid(
                    None,
                    format!("{key}.{id}: unknown plugin `{id}`{hint}"),
                ));
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
                reason: Some("--ignore-dependency".to_owned()),
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

/// The built-in dependency types, as `dependency-types` names them.
const BUILTIN_TYPE_TOKENS: [&str; 8] = [
    "prod",
    "dev",
    "peer",
    "optional",
    "local",
    "resolutions",
    "overrides",
    "pnpm-overrides",
];

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
    /// npm.
    Npm,
    /// pnpm.
    Pnpm,
    /// Yarn.
    Yarn,
    /// Bun.
    Bun,
}

/// Names of the directories that hold test fixtures. Recursive root discovery skips them by default (they lead
/// [`DEFAULT_DISCOVER_IGNORE`]), and the JavaScript analysis reports nothing about their files that would only hold
/// for production code: their imports are not unresolved or unlisted, and, unless `[js] include-fixtures` is set,
/// their files and exports are not unused.
pub const FIXTURE_DIRECTORIES: [&str; 2] = ["__fixtures__", "fixtures"];

/// Directory names that recursive root discovery skips by default: they hold fixtures and test doubles, not projects.
/// The first entries are [`FIXTURE_DIRECTORIES`].
pub const DEFAULT_DISCOVER_IGNORE: [&str; 4] = ["__fixtures__", "fixtures", "__mocks__", "__snapshots__"];

fn default_discover_ignore() -> Vec<String> {
    DEFAULT_DISCOVER_IGNORE.iter().map(|name| (*name).to_owned()).collect()
}

/// `[workspace]`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
#[non_exhaustive]
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

impl Workspace {
    /// Sets `ignore-packages`.
    #[must_use]
    pub fn with_ignore_packages(mut self, ignore_packages: Selector) -> Self {
        self.ignore_packages = ignore_packages;
        self
    }

    /// Forces the package manager instead of detecting it.
    #[must_use]
    pub const fn with_package_manager(mut self, package_manager: PackageManager) -> Self {
        self.package_manager = Some(package_manager);
        self
    }

    /// Sets `discover-ignore`, replacing the default list.
    #[must_use]
    pub fn with_discover_ignore(mut self, discover_ignore: Vec<String>) -> Self {
        self.discover_ignore = discover_ignore;
        self
    }
}

/// `[lint]`
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
#[non_exhaustive]
pub struct Lint {
    /// Exit non-zero when only warnings were reported.
    pub fail_on_warnings: bool,
    /// Caps the severity of every diagnostic: a rule configured above this level reports at it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_level: Option<Level>,
}

impl Lint {
    /// Sets `fail-on-warnings`.
    #[must_use]
    pub const fn with_fail_on_warnings(mut self, fail_on_warnings: bool) -> Self {
        self.fail_on_warnings = fail_on_warnings;
        self
    }

    /// Caps the severity of every diagnostic at `max_level`.
    #[must_use]
    pub const fn with_max_level(mut self, max_level: Level) -> Self {
        self.max_level = Some(max_level);
        self
    }
}

/// One `[[ignore]]` suppression. An empty list does not narrow that dimension, but at least one list must be set:
/// an entry without any filter is rejected as it would suppress everything.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
#[non_exhaustive]
pub struct IgnoreEntry {
    /// Rule IDs to suppress; an empty list does not narrow the entry by rule.
    pub rules: Vec<String>,
    /// Package names or directories; an empty selector matches every package.
    #[cfg_attr(feature = "schema", schemars(with = "Vec<String>"))]
    pub packages: Selector,
    /// Dependency names; an empty selector matches every dependency.
    #[cfg_attr(feature = "schema", schemars(with = "Vec<String>"))]
    pub dependencies: Selector,
    /// Why the suppression exists. `--require-ignore-reasons` fails a run on an entry without one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl IgnoreEntry {
    /// Limits the entry to the rule IDs `rules`.
    #[must_use]
    pub fn with_rules<I, S>(mut self, rules: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.rules = rules.into_iter().map(Into::into).collect();
        self
    }

    /// Limits the entry to the packages `packages` selects.
    #[must_use]
    pub fn with_packages(mut self, packages: Selector) -> Self {
        self.packages = packages;
        self
    }

    /// Limits the entry to the dependencies `dependencies` selects.
    #[must_use]
    pub fn with_dependencies(mut self, dependencies: Selector) -> Self {
        self.dependencies = dependencies;
        self
    }

    /// Records why the suppression exists.
    #[must_use]
    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }
}

/// Which version `preferred-semver` favours.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum Prefer {
    /// Prefer the highest version in use.
    #[default]
    Highest,
    /// Prefer the lowest version in use.
    Lowest,
}

/// How the override sections (`resolutions`, `overrides`, `pnpm-overrides`) are read as dependency instances.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum OverrideReading {
    /// The dependency of an entry is the package its selector replaces (`parent>child` and `child@<2` are `child`),
    /// npm's nested objects are read, and `pnpm.overrides` of every `package.json` is read.
    #[default]
    Skif,
    /// As syncpack 15 reads them: the key of an entry is the dependency name, so `parent>child` and `child@<2` are
    /// dependencies of their own; only string values count (nested objects are skipped); and `pnpm-overrides` is the
    /// `overrides` of `pnpm-workspace.yaml` only.
    Syncpack,
}

/// `[policy]`
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
#[non_exhaustive]
pub struct Policy {
    /// Which version `preferred-semver` favours.
    pub prefer: Prefer,
    /// Dependency types the policy rules apply to. Empty means production and development dependencies (`dependencies` and `devDependencies`); name `peer` or `optional` to include them.
    #[cfg_attr(feature = "schema", schemars(with = "Vec<String>"))]
    pub dependency_types: DependencyTypes,
    /// How `resolutions`, `overrides` and `pnpm-overrides` entries become dependency instances: `skif` (default) names
    /// the package a selector replaces, `syncpack` reads them as syncpack 15 does.
    pub override_reading: OverrideReading,
}

impl Policy {
    /// Sets which version `preferred-semver` favours.
    #[must_use]
    pub const fn with_prefer(mut self, prefer: Prefer) -> Self {
        self.prefer = prefer;
        self
    }

    /// Sets the dependency types the policy rules apply to.
    #[must_use]
    pub fn with_dependency_types(mut self, dependency_types: DependencyTypes) -> Self {
        self.dependency_types = dependency_types;
        self
    }

    /// Sets how the override sections are read.
    #[must_use]
    pub const fn with_override_reading(mut self, override_reading: OverrideReading) -> Self {
        self.override_reading = override_reading;
        self
    }
}

/// The policy of a version group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum VersionPolicy {
    /// Every matched dependency must use the `pin` version.
    Pinned,
    /// The matched dependencies must not be used.
    Banned,
    /// The matched dependencies are not checked.
    Ignored,
    /// Every range must overlap every other range.
    SameRange,
    /// Every matched dependency must share a major and minor version.
    SameMinor,
    /// Versions must match those declared by the `snap-to` packages.
    SnappedTo,
    /// Versions must match the highest or lowest one in use.
    #[default]
    PreferredSemver,
    /// Versions may differ; only the range the semver groups require is checked (syncpack's `semverRangeOnly`).
    SemverRangeOnly,
}

/// One `[[version-groups]]` entry; first match wins. Selectors accept `!` negation and `$LOCAL`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
#[non_exhaustive]
pub struct VersionGroup {
    /// A name shown in diagnostics.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Dependency names the group covers; empty covers every dependency.
    #[cfg_attr(feature = "schema", schemars(with = "Vec<String>"))]
    pub dependencies: Selector,
    /// Package names the group covers; empty covers every package.
    #[cfg_attr(feature = "schema", schemars(with = "Vec<String>"))]
    pub packages: Selector,
    /// Dependency types the group covers; empty covers the types `[policy] dependency-types` governs (production and development dependencies and a package's own version by default). Peer and optional dependencies, the override sections and custom types are covered only when named.
    #[cfg_attr(feature = "schema", schemars(with = "Vec<String>"))]
    pub dependency_types: DependencyTypes,
    /// Specifier types the group covers (`exact`, `range`, `tag`, ...), optionally negated with `!`; empty covers every type.
    pub specifier_types: Vec<String>,
    /// The policy the group enforces.
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
    /// Sets the label shown in diagnostics.
    #[must_use]
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Selects the dependencies the group covers.
    #[must_use]
    pub fn with_dependencies(mut self, dependencies: Selector) -> Self {
        self.dependencies = dependencies;
        self
    }

    /// Selects the packages the group covers.
    #[must_use]
    pub fn with_packages(mut self, packages: Selector) -> Self {
        self.packages = packages;
        self
    }

    /// Selects the dependency types the group covers.
    #[must_use]
    pub fn with_dependency_types(mut self, dependency_types: DependencyTypes) -> Self {
        self.dependency_types = dependency_types;
        self
    }

    /// Selects the specifier types the group covers.
    #[must_use]
    pub fn with_specifier_types<I, S>(mut self, specifier_types: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.specifier_types = specifier_types.into_iter().map(Into::into).collect();
        self
    }

    /// Sets the policy.
    #[must_use]
    pub const fn with_policy(mut self, policy: VersionPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Sets the version `pinned` requires.
    #[must_use]
    pub fn with_pin(mut self, pin: impl Into<String>) -> Self {
        self.pin = Some(pin.into());
        self
    }

    /// Sets the packages `snapped-to` follows.
    #[must_use]
    pub fn with_snap_to<I, S>(mut self, snap_to: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.snap_to = snap_to.into_iter().map(Into::into).collect();
        self
    }

    /// Overrides `[policy] prefer` for this group.
    #[must_use]
    pub const fn with_prefer(mut self, prefer: Prefer) -> Self {
        self.prefer = Some(prefer);
        self
    }

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
    /// Any version (`*`).
    #[serde(rename = "*")]
    Any,
    /// Compatible versions (`^`).
    #[serde(rename = "^")]
    Caret,
    /// Patch-level changes (`~`).
    #[serde(rename = "~")]
    Tilde,
    /// Greater than (`>`).
    #[serde(rename = ">")]
    Gt,
    /// Greater than or equal (`>=`).
    #[serde(rename = ">=")]
    Gte,
    /// Less than (`<`).
    #[serde(rename = "<")]
    Lt,
    /// Less than or equal (`<=`).
    #[serde(rename = "<=")]
    Lte,
}

/// One `[[semver-groups]]` entry; first match wins. Set `range`, or `ignored = true` to leave the matched dependencies
/// without a range requirement (syncpack's `isIgnored` and `isDisabled`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[non_exhaustive]
pub struct SemverGroup {
    /// A name shown in diagnostics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Dependency names the group covers; empty covers every dependency.
    #[serde(default)]
    #[cfg_attr(feature = "schema", schemars(with = "Vec<String>"))]
    pub dependencies: Selector,
    /// Package names the group covers; empty covers every package.
    #[serde(default)]
    #[cfg_attr(feature = "schema", schemars(with = "Vec<String>"))]
    pub packages: Selector,
    /// Dependency types the group covers; empty covers the types `[policy] dependency-types` governs (production and development dependencies and a package's own version by default). Peer and optional dependencies, the override sections and custom types are covered only when named.
    #[serde(default)]
    #[cfg_attr(feature = "schema", schemars(with = "Vec<String>"))]
    pub dependency_types: DependencyTypes,
    /// Specifier types the group covers, optionally negated with `!`; empty covers every type.
    #[serde(default)]
    pub specifier_types: Vec<String>,
    /// The range type the matched dependencies must use. Required unless `ignored` is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<SemverRange>,
    /// The matched dependencies have no range requirement and no later semver group applies to them. Exclusive with
    /// `range`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub ignored: bool,
}

impl SemverGroup {
    /// A group enforcing `range` on every dependency.
    #[must_use]
    pub fn new(range: SemverRange) -> Self {
        Self {
            label: None,
            dependencies: Selector::default(),
            packages: Selector::default(),
            dependency_types: DependencyTypes::default(),
            specifier_types: Vec::new(),
            range: Some(range),
            ignored: false,
        }
    }

    /// A group that exempts the dependencies it covers from range enforcement.
    #[must_use]
    pub fn ignored() -> Self {
        Self {
            range: None,
            ignored: true,
            ..Self::new(SemverRange::Exact)
        }
    }

    fn inconsistency(&self) -> Option<&'static str> {
        match (self.range, self.ignored) {
            (None, false) => Some("set `range`, or `ignored = true` to exempt the dependencies from a range"),
            (Some(_), true) => Some("`range` and `ignored = true` are mutually exclusive"),
            _ => None,
        }
    }

    /// Sets the label shown in diagnostics.
    #[must_use]
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Selects the dependencies the group covers.
    #[must_use]
    pub fn with_dependencies(mut self, dependencies: Selector) -> Self {
        self.dependencies = dependencies;
        self
    }

    /// Selects the packages the group covers.
    #[must_use]
    pub fn with_packages(mut self, packages: Selector) -> Self {
        self.packages = packages;
        self
    }

    /// Selects the dependency types the group covers.
    #[must_use]
    pub fn with_dependency_types(mut self, dependency_types: DependencyTypes) -> Self {
        self.dependency_types = dependency_types;
        self
    }

    /// Selects the specifier types the group covers.
    #[must_use]
    pub fn with_specifier_types<I, S>(mut self, specifier_types: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.specifier_types = specifier_types.into_iter().map(Into::into).collect();
        self
    }
}

/// How far `skif update` may move a version: syncpack's `--target` and an update group's `target`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum UpdateTarget {
    /// The highest published version, across majors.
    #[default]
    Latest,
    /// The highest version with the same major.
    Minor,
    /// The highest version with the same major and minor.
    Patch,
}

impl UpdateTarget {
    /// The stricter of two targets: `patch` over `minor` over `latest`. The command line's `--target` and an update
    /// group's `target` combine this way, so a group can only narrow what the run allows.
    #[must_use]
    pub const fn stricter(self, other: Self) -> Self {
        match (self, other) {
            (Self::Patch, _) | (_, Self::Patch) => Self::Patch,
            (Self::Minor, _) | (_, Self::Minor) => Self::Minor,
            _ => Self::Latest,
        }
    }

    /// The name as `--target` and `target` write it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Latest => "latest",
            Self::Minor => "minor",
            Self::Patch => "patch",
        }
    }
}

/// The default of `[update] max-concurrent-requests`, syncpack's `maxConcurrentRequests`.
pub const DEFAULT_MAX_CONCURRENT_REQUESTS: usize = 12;

/// `[update]`: how `skif update` reads the package registry (ADR 0058). Linting never reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
#[non_exhaustive]
pub struct Update {
    /// Minutes a version must have been published before `skif update` proposes it; `0` turns the delay off. Unset,
    /// the `minimumReleaseAge` of `pnpm-workspace.yaml` applies, else one day (1440), as in syncpack.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimum_release_age: Option<u64>,
    /// How many registry requests run at once.
    pub max_concurrent_requests: usize,
}

impl Default for Update {
    fn default() -> Self {
        Self {
            minimum_release_age: None,
            max_concurrent_requests: DEFAULT_MAX_CONCURRENT_REQUESTS,
        }
    }
}

impl Update {
    /// Sets the minimum release age in minutes.
    #[must_use]
    pub const fn with_minimum_release_age(mut self, minutes: Option<u64>) -> Self {
        self.minimum_release_age = minutes;
        self
    }

    /// Sets how many registry requests run at once.
    #[must_use]
    pub const fn with_max_concurrent_requests(mut self, requests: usize) -> Self {
        self.max_concurrent_requests = requests;
        self
    }
}

/// One `[[update-groups]]` entry; first match wins. Set `target` to cap how far `skif update` moves the matched
/// dependencies, or `ignored = true` to leave them alone (syncpack's `updateGroups`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[non_exhaustive]
pub struct UpdateGroup {
    /// A name for the group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Dependency names the group covers; empty covers every dependency.
    #[serde(default)]
    #[cfg_attr(feature = "schema", schemars(with = "Vec<String>"))]
    pub dependencies: Selector,
    /// Package names the group covers; empty covers every package. A group that names packages does not cover catalog
    /// entries, which belong to no package.
    #[serde(default)]
    #[cfg_attr(feature = "schema", schemars(with = "Vec<String>"))]
    pub packages: Selector,
    /// Dependency types the group covers; empty covers the types `[policy] dependency-types` governs. A group that
    /// names types does not cover catalog entries.
    #[serde(default)]
    #[cfg_attr(feature = "schema", schemars(with = "Vec<String>"))]
    pub dependency_types: DependencyTypes,
    /// Specifier types the group covers, optionally negated with `!`; empty covers every type.
    #[serde(default)]
    pub specifier_types: Vec<String>,
    /// The furthest the matched dependencies may move; combined with `--target`, the stricter wins. Required unless
    /// `ignored` is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<UpdateTarget>,
    /// `skif update` never changes the matched dependencies. Exclusive with `target`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub ignored: bool,
}

impl UpdateGroup {
    /// A group capping every dependency at `target`.
    #[must_use]
    pub fn new(target: UpdateTarget) -> Self {
        Self {
            label: None,
            dependencies: Selector::default(),
            packages: Selector::default(),
            dependency_types: DependencyTypes::default(),
            specifier_types: Vec::new(),
            target: Some(target),
            ignored: false,
        }
    }

    /// A group whose dependencies `skif update` leaves alone.
    #[must_use]
    pub fn ignored() -> Self {
        Self {
            target: None,
            ignored: true,
            ..Self::new(UpdateTarget::Latest)
        }
    }

    fn inconsistency(&self) -> Option<&'static str> {
        match (self.target, self.ignored) {
            (None, false) => Some("set `target`, or `ignored = true` to leave the dependencies alone"),
            (Some(_), true) => Some("`target` and `ignored = true` are mutually exclusive"),
            _ => None,
        }
    }

    /// Sets the label.
    #[must_use]
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Selects the dependencies the group covers.
    #[must_use]
    pub fn with_dependencies(mut self, dependencies: Selector) -> Self {
        self.dependencies = dependencies;
        self
    }

    /// Selects the packages the group covers.
    #[must_use]
    pub fn with_packages(mut self, packages: Selector) -> Self {
        self.packages = packages;
        self
    }

    /// Selects the dependency types the group covers.
    #[must_use]
    pub fn with_dependency_types(mut self, dependency_types: DependencyTypes) -> Self {
        self.dependency_types = dependency_types;
        self
    }

    /// Selects the specifier types the group covers.
    #[must_use]
    pub fn with_specifier_types<I, S>(mut self, specifier_types: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.specifier_types = specifier_types.into_iter().map(Into::into).collect();
        self
    }
}

/// How a [`CustomType`] reads versions from its `path`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub enum CustomStrategy {
    /// The value is a string `name@version` (`"packageManager": "pnpm@9.0.0"`): the dependency is the name.
    #[serde(rename = "name@version")]
    NameAtVersion,
    /// The value is a version string (`engines.node`): the dependency is named after the custom type.
    #[serde(rename = "version")]
    Version,
    /// The value is an object of `name: version` (`pnpm.peerDependencyRules.allowedVersions`).
    #[serde(rename = "versions-by-name", alias = "versionsByName")]
    VersionsByName,
    /// The value is a version string and the dependency name is the string at `name-path` (how syncpack reads a
    /// package's own `name` and `version`).
    #[serde(rename = "name~version")]
    NameTildeVersion,
}

/// One `[[custom-types]]` entry: a field of `package.json` that holds versions. Name it in a `dependency-types` list to
/// select its instances, like a dependency section (syncpack's `customTypes`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[non_exhaustive]
pub struct CustomType {
    /// The token that selects this type in `dependency-types`. It must not be the name of a built-in type
    /// (`prod`, `dev`, `peer`, `optional`, `local`, `resolutions`, `overrides`, `pnpm-overrides`). With the `version`
    /// strategy it is also the dependency name.
    pub name: String,
    /// Dotted path of the field in `package.json`, e.g. `engines.node` or `packageManager`.
    pub path: String,
    /// How the value at `path` holds versions.
    pub strategy: CustomStrategy,
    /// With the `name~version` strategy: dotted path of the field that holds the dependency name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name_path: Option<String>,
}

impl CustomType {
    /// A custom type reading `strategy` from `path`.
    #[must_use]
    pub fn new(name: impl Into<String>, path: impl Into<String>, strategy: CustomStrategy) -> Self {
        Self {
            name: name.into(),
            path: path.into(),
            strategy,
            name_path: None,
        }
    }

    /// Sets the field that holds the dependency name, for the `name~version` strategy.
    #[must_use]
    pub fn with_name_path(mut self, name_path: impl Into<String>) -> Self {
        self.name_path = Some(name_path.into());
        self
    }

    /// The `name-path` split into its keys; empty when there is none.
    pub fn name_segments(&self) -> impl Iterator<Item = &str> {
        self.name_path.iter().flat_map(|path| path.split('.'))
    }

    /// The `path` split into its keys.
    pub fn segments(&self) -> impl Iterator<Item = &str> {
        self.path.split('.')
    }

    fn inconsistency(&self) -> Option<String> {
        if self.name.is_empty() || self.name.starts_with('!') {
            return Some("`name` must not be empty or start with `!`".to_owned());
        }
        if DependencyType::is_builtin_token(&self.name) {
            return Some(format!(
                "`name` \"{}\" is the name of a built-in dependency type",
                self.name
            ));
        }
        if self.segments().any(str::is_empty) {
            return Some(format!(
                "`path` \"{}\" must be a dotted path of non-empty keys",
                self.path
            ));
        }
        let takes_name_path = self.strategy == CustomStrategy::NameTildeVersion;
        match (takes_name_path, self.name_path.is_some()) {
            (true, false) => return Some("strategy `name~version` requires `name-path`".to_owned()),
            (false, true) => return Some("`name-path` is only valid with strategy `name~version`".to_owned()),
            _ => {}
        }
        if self.name_segments().any(str::is_empty) {
            return Some("`name-path` must be a dotted path of non-empty keys".to_owned());
        }
        None
    }
}

/// `[catalog] mode`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum CatalogMode {
    /// Catalog rules suggest nothing.
    #[default]
    Off,
    /// Suggest moving shared dependencies into a catalog.
    Prefer,
    /// Require shared dependencies to come from a catalog.
    Enforce,
}

/// `[catalog]`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
#[non_exhaustive]
pub struct Catalog {
    /// How strongly catalogs are preferred.
    pub mode: CatalogMode,
    /// Minimum number of packages using a dependency before a catalog entry is suggested.
    pub min_packages: usize,
    /// Dependencies never suggested for a catalog.
    pub exclude: Vec<String>,
    /// Report catalog entries no package references.
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

impl Catalog {
    /// Sets the catalog mode.
    #[must_use]
    pub const fn with_mode(mut self, mode: CatalogMode) -> Self {
        self.mode = mode;
        self
    }

    /// Sets the minimum number of packages using a dependency before an entry is suggested.
    #[must_use]
    pub const fn with_min_packages(mut self, min_packages: usize) -> Self {
        self.min_packages = min_packages;
        self
    }

    /// Sets the dependencies excluded from catalog suggestions.
    #[must_use]
    pub fn with_exclude<I, S>(mut self, exclude: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.exclude = exclude.into_iter().map(Into::into).collect();
        self
    }

    /// Sets whether unused catalog entries are reported.
    #[must_use]
    pub const fn with_report_unused_entries(mut self, report_unused_entries: bool) -> Self {
        self.report_unused_entries = report_unused_entries;
        self
    }
}

/// `[workspace-protocol]`
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
#[non_exhaustive]
pub struct WorkspaceProtocol {
    /// Require `workspace:` for dependencies on other workspace packages.
    pub internal: bool,
    /// Accept `file:` / `link:` paths for internal dependencies.
    pub allow_local_paths: bool,
}

impl WorkspaceProtocol {
    /// Sets whether `workspace:` is required for internal dependencies.
    #[must_use]
    pub const fn with_internal(mut self, internal: bool) -> Self {
        self.internal = internal;
        self
    }

    /// Sets whether `file:` / `link:` paths are accepted for internal dependencies.
    #[must_use]
    pub const fn with_allow_local_paths(mut self, allow_local_paths: bool) -> Self {
        self.allow_local_paths = allow_local_paths;
        self
    }
}

/// `[format]`: what the `package-json-format` rule and `skif format` check, with syncpack's defaults (ADR 0055).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
#[non_exhaustive]
pub struct Format {
    /// Top-level keys that come first, in this order.
    pub sort_first: Vec<String>,
    /// Fields sorted alphabetically one level deep: an object by key, an array of strings by value. `a/b` names a
    /// nested field. For fields whose order means nothing: an array with a `!`-prefixed entry keeps its order.
    pub sort_az: Vec<String>,
    /// Keys that come first, in this order, in every object of `exports`; `default` is always last. The default moves
    /// only `types` and `default`; a key that reorders other conditions makes the fix unsafe.
    pub sort_exports: Vec<String>,
    /// Sort the top-level keys `sort-first` does not name.
    pub sort_packages: bool,
    /// Write a `bugs` object that holds only `url` as that string.
    pub format_bugs: bool,
    /// Write a `repository` object that holds only `url` and a `type` of `git` (or no `type`) as its shorthand.
    pub format_repository: bool,
    /// Check the layout against `JSON.stringify(manifest, null, indent)`: spaces or tabs, for example `"  "` or
    /// `"\t"`. Unset keeps each file's layout.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub indent: Option<String>,
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}

impl Default for Format {
    fn default() -> Self {
        Self {
            sort_first: strings(&["name", "description", "version", "author"]),
            sort_az: strings(&[
                "bin",
                "contributors",
                "dependencies",
                "devDependencies",
                "keywords",
                "peerDependencies",
                "resolutions",
                "scripts",
            ]),
            sort_exports: strings(&["types", "default"]),
            sort_packages: true,
            format_bugs: false,
            format_repository: false,
            indent: None,
        }
    }
}

impl Format {
    /// Sets the keys that come first.
    #[must_use]
    pub fn with_sort_first<I: IntoIterator<Item = S>, S: Into<String>>(mut self, keys: I) -> Self {
        self.sort_first = keys.into_iter().map(Into::into).collect();
        self
    }

    /// Sets the fields sorted alphabetically.
    #[must_use]
    pub fn with_sort_az<I: IntoIterator<Item = S>, S: Into<String>>(mut self, keys: I) -> Self {
        self.sort_az = keys.into_iter().map(Into::into).collect();
        self
    }

    /// Sets the order of the `exports` conditions.
    #[must_use]
    pub fn with_sort_exports<I: IntoIterator<Item = S>, S: Into<String>>(mut self, keys: I) -> Self {
        self.sort_exports = keys.into_iter().map(Into::into).collect();
        self
    }

    /// Sets whether the remaining top-level keys are sorted.
    #[must_use]
    pub const fn with_sort_packages(mut self, sort: bool) -> Self {
        self.sort_packages = sort;
        self
    }

    /// Sets whether `bugs` is collapsed to its URL.
    #[must_use]
    pub const fn with_format_bugs(mut self, format: bool) -> Self {
        self.format_bugs = format;
        self
    }

    /// Sets whether `repository` is collapsed to its shorthand.
    #[must_use]
    pub const fn with_format_repository(mut self, format: bool) -> Self {
        self.format_repository = format;
        self
    }

    /// Sets the indent the layout is checked against.
    #[must_use]
    pub fn with_indent(mut self, indent: impl Into<String>) -> Self {
        self.indent = Some(indent.into());
        self
    }

    /// The indent with a written `\t` read as a tab, as syncpack reads it.
    #[must_use]
    pub fn indent_text(&self) -> Option<String> {
        self.indent.as_ref().map(|indent| indent.replace("\\t", "\t"))
    }

    fn inconsistency(&self) -> Option<String> {
        let indent = self.indent_text()?;
        (indent.is_empty() || indent.chars().any(|c| c != ' ' && c != '\t')).then(|| {
            format!(
                "indent: {:?} is not a run of spaces or tabs",
                self.indent.as_deref().unwrap_or_default()
            )
        })
    }
}

/// One `[[dependency-families]]` entry: packages that must share a version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[non_exhaustive]
pub struct DependencyFamily {
    /// The family name; a built-in family of the same name is replaced.
    pub name: String,
    /// Package names or globs that must share a version.
    #[serde(default)]
    pub packages: Vec<String>,
    /// `false` disables a built-in family of the same name.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl DependencyFamily {
    /// An enabled family called `name` whose members are `packages`.
    #[must_use]
    pub fn new<I, S>(name: impl Into<String>, packages: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            name: name.into(),
            packages: packages.into_iter().map(Into::into).collect(),
            enabled: true,
        }
    }

    /// Enables or disables the family; `false` disables a built-in family of the same name.
    #[must_use]
    pub const fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
}

const fn default_true() -> bool {
    true
}

/// `[js] discovery`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum JsDiscovery {
    /// The git index, checked against the file system: a directory modified since the index was written is listed
    /// from disk, so untracked and deleted files are found as the walk finds them. The directory walk when the
    /// workspace is not in a git work tree with a usable index.
    #[default]
    Auto,
    /// The git index alone, without checking the file system: faster, but blind to files created or deleted since the
    /// last `git add`. The directory walk when there is no usable index.
    Git,
    /// Always walk the directories.
    Walk,
}

/// `[js.cycles]`: knip's `cycles` options of the `circular-imports` rule.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
#[non_exhaustive]
pub struct JsCycles {
    /// Cycles that are not reported, each the list of its files relative to the workspace root. A cycle is allowed
    /// when its files are exactly the files of one entry, in any order.
    pub allow: Vec<Vec<String>>,
    /// Count dynamic `import()` as an edge of a cycle. Off by default: a dynamic import runs after the module graph
    /// is loaded, so it cannot cause an initialisation-order problem.
    pub dynamic_imports: bool,
}

/// `[js]`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
#[non_exhaustive]
#[allow(
    clippy::struct_excessive_bools,
    reason = "each bool is an independent switch of the analysis"
)]
pub struct Js {
    /// Run the JavaScript analysis.
    pub enabled: bool,
    /// Entry-point globs, added to the ones plugins and `package.json` provide.
    pub entry: Vec<String>,
    /// Globs of the files that belong to the project.
    pub project: Vec<String>,
    /// Globs of files that are not analysed.
    pub ignore: Vec<String>,
    /// Globs of files, relative to the workspace root, never reported by `unused-files`; other rules still analyse
    /// them (knip's `ignoreFiles`).
    pub ignore_files: Vec<String>,
    /// Dependencies never reported as unused or unlisted.
    pub ignore_dependencies: Vec<String>,
    /// Binaries never reported as unlisted.
    pub ignore_binaries: Vec<String>,
    /// Never report the system and shell tools scripts run (`rm`, `curl`, `git`, `docker`) as unlisted binaries,
    /// since they are installed globally rather than from npm (knip's `ignoreGlobalBinaries`). On by default; turn it
    /// off to have `unlisted-binaries` report them unless a dependency provides them.
    pub ignore_global_binaries: bool,
    /// Import specifiers never reported as unresolved. Entries are exact specifiers, globs or regular expressions
    /// (`/^@org\/.+$/`), see the `[js]` documentation.
    pub ignore_unresolved: Vec<String>,
    /// Workspace directories (globs) the JavaScript analysis skips. Manifest and policy rules still check them.
    pub ignore_workspaces: Vec<String>,
    /// Exports that other code in the same file uses are not reported: `true` for every kind, or a table of kinds.
    pub ignore_exports_used_in_file: IgnoreExportsUsedInFile,
    /// Enum and namespace member names (globs or regular expressions) never reported as unused.
    pub ignore_members: Vec<String>,
    /// Analyse production code only.
    pub production: bool,
    /// knip's strict mode. Each workspace package uses only the dependencies its own manifest lists: an import is
    /// not satisfied by a containing package or the root. Production code must import from `dependencies` or
    /// `peerDependencies`, type-only imports use no package, and required peer dependencies are checked like
    /// `dependencies`. Set by `skif --strict`; not a configuration key, so a file never sets it.
    #[serde(skip)]
    #[cfg_attr(feature = "schema", schemars(skip))]
    pub isolate_workspaces: bool,
    /// Find source files that `.gitignore` files hide (the git index is not used then). Set by
    /// `skif --no-gitignore`; not a configuration key.
    #[serde(skip)]
    #[cfg_attr(feature = "schema", schemars(skip))]
    pub no_gitignore: bool,
    /// Report unused exports of entry files too.
    pub include_entry_exports: bool,
    /// The TypeScript configuration of the root package, relative to the workspace root and inside it, used instead
    /// of `tsconfig.json` for its path aliases and compiler options (knip's `--tsConfig`). Other packages keep their
    /// own `tsconfig.json`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tsconfig: Option<String>,
    /// The project files of a package with a TypeScript configuration are the files its `files` names and those its
    /// `include` selects minus its `exclude`, instead of the `project` globs (knip's `--use-tsconfig-files`).
    pub use_tsconfig_files: bool,
    /// Report files in `fixtures` and `__fixtures__` directories as unused, and their unused exports, types and
    /// members, like any other file. Off by default: those directories hold test data that tests load by path.
    pub include_fixtures: bool,
    /// How source files are found: `auto` lists them from the git index and re-lists from disk every directory
    /// modified since the index was written, which gives the walk's result without walking every directory; `git`
    /// trusts the index alone (faster, blind to untracked and deleted files); `walk` always walks. Both index modes
    /// fall back to the walk when the workspace is not in a git work tree with a usable SHA-1 index.
    pub discovery: JsDiscovery,
    /// What `circular-imports` reports.
    pub cycles: JsCycles,
    /// Per-workspace overrides keyed by directory. The key `"."` is the root workspace.
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
    /// Appended to `ignore-unresolved` instead of replacing it. Folded away when the configuration is resolved.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub extend_ignore_unresolved: Vec<String>,
}

impl Js {
    /// Sets the entry globs.
    #[must_use]
    pub fn with_entry<I, S>(mut self, entry: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.entry = entry.into_iter().map(Into::into).collect();
        self
    }

    /// Sets the project globs.
    #[must_use]
    pub fn with_project<I, S>(mut self, project: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.project = project.into_iter().map(Into::into).collect();
        self
    }

    /// Enables or disables the JavaScript analysis.
    #[must_use]
    pub const fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Restricts the analysis to production code.
    #[must_use]
    pub const fn with_production(mut self, production: bool) -> Self {
        self.production = production;
        self
    }

    /// Makes each workspace package use only the dependencies its own manifest lists, as knip's `--strict` does.
    #[must_use]
    pub const fn with_isolate_workspaces(mut self, isolate: bool) -> Self {
        self.isolate_workspaces = isolate;
        self
    }

    /// Uses `path`, relative to the workspace root, as the TypeScript configuration of the root package.
    #[must_use]
    pub fn with_tsconfig(mut self, path: impl Into<String>) -> Self {
        self.tsconfig = Some(path.into());
        self
    }

    /// Takes the project files of each package with a TypeScript configuration from its `files`, `include` and
    /// `exclude`.
    #[must_use]
    pub const fn with_use_tsconfig_files(mut self, enabled: bool) -> Self {
        self.use_tsconfig_files = enabled;
        self
    }

    /// Switches a plugin on or off, or configures it.
    #[must_use]
    pub fn with_plugin(mut self, name: impl Into<String>, plugin: PluginConfig) -> Self {
        self.plugins.insert(name.into(), plugin);
        self
    }
}

impl Default for Js {
    fn default() -> Self {
        Self {
            enabled: true,
            entry: Vec::new(),
            project: Vec::new(),
            ignore: Vec::new(),
            ignore_files: Vec::new(),
            ignore_dependencies: Vec::new(),
            ignore_binaries: Vec::new(),
            ignore_global_binaries: true,
            ignore_unresolved: Vec::new(),
            ignore_workspaces: Vec::new(),
            ignore_exports_used_in_file: IgnoreExportsUsedInFile::default(),
            ignore_members: Vec::new(),
            production: false,
            isolate_workspaces: false,
            no_gitignore: false,
            include_entry_exports: false,
            tsconfig: None,
            use_tsconfig_files: false,
            include_fixtures: false,
            discovery: JsDiscovery::Auto,
            cycles: JsCycles::default(),
            workspaces: IndexMap::new(),
            plugins: IndexMap::new(),
            extend_ignore: Vec::new(),
            extend_ignore_dependencies: Vec::new(),
            extend_ignore_binaries: Vec::new(),
            extend_ignore_unresolved: Vec::new(),
        }
    }
}

/// `ignore-exports-used-in-file`: `true`, or a table naming the kinds of export it applies to.
///
/// It is read by hand rather than as an untagged enum, so that a mistyped kind is reported by name (`unknown field
/// "fucntion"`) instead of serde's "data did not match any variant".
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
#[non_exhaustive]
pub enum IgnoreExportsUsedInFile {
    /// `true` ignores every kind of export, `false` none.
    All(bool),
    /// Ignores the kinds set to `true`.
    Kinds(ExportKinds),
}

impl<'de> Deserialize<'de> for IgnoreExportsUsedInFile {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;

        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = IgnoreExportsUsedInFile;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("`true`, `false` or a table of export kinds")
            }

            fn visit_bool<E: serde::de::Error>(self, value: bool) -> Result<Self::Value, E> {
                Ok(IgnoreExportsUsedInFile::All(value))
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                ExportKinds::deserialize(serde::de::value::MapAccessDeserializer::new(map))
                    .map(IgnoreExportsUsedInFile::Kinds)
            }
        }

        deserializer.deserialize_any(Visitor)
    }
}

impl Default for IgnoreExportsUsedInFile {
    fn default() -> Self {
        Self::All(false)
    }
}

/// The kinds of export `ignore-exports-used-in-file` can name, as in knip.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
#[non_exhaustive]
#[allow(clippy::struct_excessive_bools)]
pub struct ExportKinds {
    /// `const`, `let` and `var` exports.
    pub variable: bool,
    /// `function` exports.
    pub function: bool,
    /// `class` exports.
    pub class: bool,
    /// `enum` exports.
    #[serde(rename = "enum")]
    pub enumeration: bool,
    /// TypeScript `namespace` exports.
    pub namespace: bool,
    /// Type alias exports. skif does not tell type aliases from interfaces, so this and `interface` are the same.
    #[serde(rename = "type")]
    pub type_alias: bool,
    /// Interface exports, see `type`.
    pub interface: bool,
    /// Enum and namespace members. skif always counts a member read in its own file as used, so this key is
    /// accepted for compatibility and has no further effect.
    pub member: bool,
}

/// `[js.workspaces."<dir>"]`
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
#[non_exhaustive]
pub struct JsWorkspace {
    /// Entry-point globs relative to the workspace directory.
    pub entry: Vec<String>,
    /// Project file globs relative to the workspace directory.
    pub project: Vec<String>,
    /// Globs of files that are not analysed.
    pub ignore: Vec<String>,
    /// Globs of files, relative to the workspace directory, never reported by `unused-files`.
    pub ignore_files: Vec<String>,
    /// Dependencies never reported as unused or unlisted.
    pub ignore_dependencies: Vec<String>,
    /// Binaries never reported as unlisted.
    pub ignore_binaries: Vec<String>,
    /// Import specifiers never reported as unresolved in this workspace.
    pub ignore_unresolved: Vec<String>,
    /// Report unused exports of entry files of this workspace; unset inherits `[js] include-entry-exports`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_entry_exports: Option<bool>,
    /// Exports used in their own file that are not reported in this workspace; unset inherits `[js]`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ignore_exports_used_in_file: Option<IgnoreExportsUsedInFile>,
    /// Enum and namespace member names never reported as unused in this workspace.
    pub ignore_members: Vec<String>,
    /// Module aliases as in a `tsconfig` `paths` map: `{ "alias/*" = ["./target/*"] }`, targets relative to the workspace.
    pub paths: IndexMap<String, Vec<String>>,
    /// Plugin switches and options for this workspace, applied over `[js.plugins]`.
    pub plugins: IndexMap<String, PluginConfig>,
}

/// `[js.plugins] <name> = true | { ... }`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
#[non_exhaustive]
pub enum PluginConfig {
    /// A bare switch: `true` forces the plugin on, `false` off.
    Enabled(bool),
    /// The table form with options.
    Options(PluginOptions),
}

/// Table form of a plugin entry.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
#[non_exhaustive]
pub struct PluginOptions {
    /// Forces the plugin on or off; unset keeps its detection.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Config-file globs the plugin reads.
    pub config: Vec<String>,
    /// Entry-point globs the plugin contributes.
    pub entry: Vec<String>,
    /// Project file globs the plugin contributes.
    pub project: Vec<String>,
}

impl PluginOptions {
    /// Options with nothing set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Switches the plugin on or off explicitly.
    #[must_use]
    pub const fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = Some(enabled);
        self
    }

    /// Sets the config-file globs.
    #[must_use]
    pub fn with_config<I, S>(mut self, config: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.config = config.into_iter().map(Into::into).collect();
        self
    }

    /// Sets the entry globs.
    #[must_use]
    pub fn with_entry<I, S>(mut self, entry: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.entry = entry.into_iter().map(Into::into).collect();
        self
    }

    /// Sets the project globs.
    #[must_use]
    pub fn with_project<I, S>(mut self, project: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.project = project.into_iter().map(Into::into).collect();
        self
    }
}

/// `[js]` keys an override can set for the packages it selects.
///
/// Globs are relative to each selected package. The `extend-*` keys append to what the package inherits; the others
/// replace it, and set to an empty list they clear it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
#[non_exhaustive]
pub struct PackageJs {
    /// Replaces the entry globs; an empty list clears what the package inherits.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry: Option<Vec<String>>,
    /// Replaces the project globs; an empty list clears what the package inherits.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<Vec<String>>,
    /// Replaces the ignore globs; an empty list clears what the package inherits.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ignore: Option<Vec<String>>,
    /// Replaces the ignored dependencies; an empty list clears what the package inherits.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ignore_dependencies: Option<Vec<String>>,
    /// Replaces the ignored binaries; an empty list clears what the package inherits.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ignore_binaries: Option<Vec<String>>,
    /// Appended to the ignore globs.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub extend_ignore: Vec<String>,
    /// Appended to the ignored dependencies.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub extend_ignore_dependencies: Vec<String>,
    /// Appended to the ignored binaries.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub extend_ignore_binaries: Vec<String>,
    /// Replaces the ignored unresolved specifiers; an empty list clears what the package inherits.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ignore_unresolved: Option<Vec<String>>,
    /// Appended to the ignored unresolved specifiers.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub extend_ignore_unresolved: Vec<String>,
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
#[non_exhaustive]
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

    /// As [`applies_to`](Self::applies_to) for directories and files already written with `/` separators (see
    /// `skif_core::slash`), which a caller checking many overrides converts once instead of once per override.
    #[must_use]
    pub fn applies_to_slashed(&self, package: Option<(&str, &str)>, file: Option<&str>) -> bool {
        self.scope
            .as_ref()
            .is_some_and(|scope| scope.applies_to_slashed(package, file))
    }
}

/// Where an [`Override`] was declared, with its patterns resolved to absolute ones.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
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
        let package = package.map(|(name, dir)| (name, skif_core::slash(dir)));
        let file = file.map(skif_core::slash);
        self.applies_to_slashed(
            package.as_ref().map(|(name, dir)| (*name, dir.as_str())),
            file.as_deref(),
        )
    }

    fn applies_to_slashed(&self, package: Option<(&str, &str)>, file: Option<&str>) -> bool {
        let package_ok =
            !self.has_packages || package.is_some_and(|(name, dir)| self.packages.matches_any([name, dir], true));
        let file_ok = !self.has_files || file.is_some_and(|file| self.files.matches(file, false));
        package_ok && file_ok
    }
}
