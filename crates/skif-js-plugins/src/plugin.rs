//! The plugin model of ADR 0011: a declarative [`PluginSpec`] row, an optional config resolver and the
//! [`PluginOutput`] handed to the module graph.

use std::ops::Range;
use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use skif_config::PluginConfig;
use skif_core::FileSystem;
use skif_js_syntax::ConfigValue;

/// The knip commit every plugin row was last checked against.
pub const KNIP_COMMIT: &str = "da07b6e9887f9a1eb08545da29755b7f9d309bca";

/// A plugin's stable identifier, the key of `[js.plugins]` (`"vitest"`).
pub type PluginId = &'static str;

/// Why a plugin turns on for a workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Enabler {
    /// The workspace depends on exactly this package.
    Dependency(&'static str),
    /// The workspace depends on a package whose name starts with this prefix (`@storybook/`).
    DependencyPrefix(&'static str),
    /// The workspace's own package name starts with this prefix.
    ManifestNamePrefix(&'static str),
    /// The workspace's own package name contains this text.
    ManifestNameContains(&'static str),
    /// A file matching this glob exists, relative to the workspace directory.
    File(&'static str),
}

/// The declarative row of a plugin.
#[derive(Debug, Clone, Copy)]
pub struct PluginSpec {
    /// Stable identifier, for example `"vitest"`.
    pub id: PluginId,
    /// Any matching enabler turns the plugin on.
    pub enablers: &'static [Enabler],
    /// Only the repository root workspace can enable the plugin by detection.
    pub root_only: bool,
    /// Config file globs, relative to the workspace directory.
    pub config: &'static [&'static str],
    /// Entry file globs.
    pub entry: &'static [&'static str],
    /// Entry globs that are also entries of a production build.
    pub production: &'static [&'static str],
    /// Project file globs.
    pub project: &'static [&'static str],
    /// Key in `package.json` that holds the tool's config, when the tool supports it.
    pub package_json_key: Option<&'static str>,
    /// Binaries the tool's packages provide.
    pub binaries: &'static [&'static str],
    /// Globs of files the plugin generates or owns, which must not be reported as unused.
    pub ignore: &'static [&'static str],
    /// The knip commit the row was checked against.
    pub knip_commit: &'static str,
}

impl PluginSpec {
    /// A row with every list empty; plugins override what they need with struct update syntax.
    pub const DEFAULT: Self = Self {
        id: "",
        enablers: &[],
        root_only: false,
        config: &[],
        entry: &[],
        production: &[],
        project: &[],
        package_json_key: None,
        binaries: &[],
        ignore: &[],
        knip_commit: KNIP_COMMIT,
    };
}

/// What a plugin's config resolver found in a config file.
///
/// Paths and globs are relative to the workspace directory.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PluginInput {
    /// An entry file or glob.
    Entry(String),
    /// An entry that is also part of a production build.
    ProductionEntry(String),
    /// A package the config references.
    Dependency(String),
    /// A package the config references that may legitimately be absent.
    OptionalDependency(String),
    /// A binary the config invokes.
    Binary(String),
    /// A further config file of the same plugin (`extends`, project references).
    ConfigFile(String),
    /// A shell script, parsed with the script parser.
    Script(String),
    /// A glob of files the config owns.
    Ignore(String),
}

/// The statically extracted content of a plugin's config file.
///
/// The value comes from the static extraction of ADR 0010 for JavaScript and TypeScript, and from the JSON, YAML or TOML parsers otherwise. It holds no oxc type.
#[derive(Debug, Clone, Copy)]
pub struct ConfigAst<'a> {
    /// Absolute path of the config file (`package.json` when the config is a key of it).
    pub path: &'a Path,
    /// Directory of the config file relative to the workspace directory, `""` for the workspace root.
    pub base: &'a str,
    /// The extracted value.
    pub value: &'a ConfigValue,
    /// The raw file text; empty for a `package.json` key.
    pub source: &'a str,
}

impl ConfigAst<'_> {
    /// The file name of the config file.
    #[must_use]
    pub fn file_name(&self) -> &str {
        self.path.file_name().and_then(|n| n.to_str()).unwrap_or_default()
    }

    /// Resolves a path written in the config against the config file's directory, relative to the workspace.
    #[must_use]
    pub fn join(&self, relative: &str) -> String {
        crate::support::join_rel(self.base, relative)
    }
}

/// What a workspace looks like to the plugins.
#[derive(Clone, Copy)]
pub struct PluginContext<'a> {
    /// The workspace directory.
    pub dir: &'a Path,
    /// Whether the workspace is the repository root.
    pub is_root: bool,
    /// The `name` of the workspace's `package.json`.
    pub package_name: Option<&'a str>,
    /// Names of every dependency of the workspace, all kinds.
    pub dependencies: &'a [String],
    /// The workspace's scripts.
    pub scripts: &'a IndexMap<String, String>,
    /// File access for config files.
    pub fs: &'a dyn FileSystem,
    /// `[js.plugins]` switches and overrides.
    pub plugins: &'a IndexMap<String, PluginConfig>,
}

impl PluginContext<'_> {
    /// Whether the workspace depends on `name`.
    #[must_use]
    pub fn has_dependency(&self, name: &str) -> bool {
        self.dependencies.iter().any(|d| d == name)
    }
}

impl std::fmt::Debug for PluginContext<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginContext")
            .field("dir", &self.dir)
            .field("is_root", &self.is_root)
            .field("package_name", &self.package_name)
            .finish_non_exhaustive()
    }
}

/// A plugin: a declarative row and an optional config resolver.
pub trait Plugin: Send + Sync {
    /// The declarative row: enablers, config, entry, production and project patterns.
    fn spec(&self) -> &'static PluginSpec;

    /// Reads the plugin's parsed config file and returns what it references. The default returns
    /// nothing, which is all a purely declarative plugin needs.
    fn resolve_config(&self, _config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        Vec::new()
    }
}

/// A package the plugin's configuration needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepRef {
    /// The package name, without version or subpath.
    pub name: String,
    /// The file that references it.
    pub from: PathBuf,
    /// Byte range in `from`, when known; config values carry no spans, so this is usually `None`.
    pub span: Option<Range<usize>>,
    /// The reference does not require the package to be declared (optional peers, on-demand fetches).
    pub optional: bool,
}

/// Everything one enabled plugin contributes to a workspace.
///
/// Globs and paths are relative to the workspace directory, except the absolute `config_files` and
/// the `from` of each [`DepRef`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginOutput {
    /// Entry files and globs, including the production ones.
    pub entries: Vec<String>,
    /// Project file globs.
    pub project: Vec<String>,
    /// Entries that belong to a production build; a subset of `entries`.
    pub production_entries: Vec<String>,
    /// Config files that were found and read.
    pub config_files: Vec<PathBuf>,
    /// Packages the configs reference by name.
    pub referenced_dependencies: Vec<DepRef>,
    /// Binaries the configs invoke (from embedded scripts).
    pub referenced_binaries: Vec<String>,
    /// Binaries the tool itself provides.
    pub provided_binaries: Vec<String>,
    /// Globs of files that must not be reported as unused.
    pub ignore: Vec<String>,
}
