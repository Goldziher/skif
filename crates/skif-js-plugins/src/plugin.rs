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
    /// Every workspace: the tool is part of the runtime (`node`).
    Always,
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
    /// A shell script that runs in `dir`, a directory of the workspace other than its root (a workflow step with a
    /// `working-directory`). The files it runs are entries; the binaries and packages it names belong to whichever
    /// package lives in `dir` and are not recorded.
    ScriptAt {
        /// The directory the script runs in, relative to the workspace directory.
        dir: String,
        /// The script text.
        script: String,
    },
    /// A glob of files the config owns.
    Ignore(String),
    /// A module alias (`resolve.alias`, `compilerOptions.paths`): imports of `find` and `find/…` lead to `target`.
    ///
    /// `target` is a directory or file relative to the workspace directory, `None` when the config computes it.
    Alias {
        /// The aliased prefix, without a trailing `/` or `*`.
        find: String,
        /// Where it points.
        target: Option<String>,
    },
    /// Build output and the source directory it comes from (`outDir` and `rootDir`), relative to the workspace.
    BuildDir {
        /// The output directory.
        out_dir: String,
        /// The source directory.
        root_dir: String,
    },
    /// An `exports` condition that selects source files (`customConditions`).
    Condition(String),
    /// A directory bare specifiers are looked up in before packages (`baseUrl`, a `*` entry of `paths`, webpack's
    /// `resolve.modules`), relative to the workspace directory.
    ModuleDir(String),
    /// A file extension bare imports may omit (`resolve.extensions`), with its leading dot.
    Extension(String),
    /// A config file of another plugin, which reads it with its own resolver (an Angular target's `tsConfig` is a
    /// `typescript` config, a Docusaurus project's `babel.config.js` a `babel` one). `path` is relative to the
    /// workspace directory and may be a glob. The file counts as a config file of the plugin that links it.
    LinkedConfig {
        /// The plugin that reads the file.
        plugin: PluginId,
        /// The file or glob, relative to the workspace directory.
        path: String,
    },
    /// A specifier pattern the tool resolves at build time (`@generated/*`, `#imports`): it is no dependency, so it is
    /// never listed as unlisted, unused or unresolved.
    VirtualModule(String),
    /// The config computes its entry points in a way that cannot be evaluated statically, so the entries found are
    /// not all of them. Sets [`PluginOutput::entries_incomplete`].
    EntriesIncomplete,
}

/// A module alias read from a config file; see [`PluginInput::Alias`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathAlias {
    /// The aliased prefix.
    pub find: String,
    /// The target relative to the workspace directory, when statically known.
    pub target: Option<String>,
}

/// Build output and its source directory; see [`PluginInput::BuildDir`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildDir {
    /// The output directory, relative to the workspace directory.
    pub out_dir: String,
    /// The source directory, relative to the workspace directory.
    pub root_dir: String,
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
    /// Names of the dependencies of ancestor workspaces. A plugin whose enabler they satisfy is enabled here too,
    /// as the tool is installed once for the whole repository.
    pub inherited_dependencies: &'a [String],
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

    /// Whether the workspace or one of its ancestors depends on `name`.
    #[must_use]
    pub fn has_dependency_in_ancestors(&self, name: &str) -> bool {
        self.has_dependency(name) || self.inherited_dependencies.iter().any(|d| d == name)
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

    /// Whether the plugin reads anything from its config files. A plugin that does cannot judge a package whose config
    /// is not statically readable, so such a config sets [`PluginOutput::entries_incomplete`] for it. The default is
    /// `true`; a purely declarative plugin returns `false`.
    fn reads_config(&self) -> bool {
        true
    }

    /// Reads the plugin's parsed config file and returns what it references. The default returns
    /// nothing, which is all a purely declarative plugin needs.
    fn resolve_config(&self, _config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        Vec::new()
    }

    /// What the plugin finds in the workspace besides its config files, such as the scripts that invoke the tool or
    /// the constructs in source files that declare entry points. Called for every enabled plugin, whether or not a
    /// config file exists. The default returns nothing.
    fn resolve_workspace(&self, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        Vec::new()
    }

    /// What the plugin knows by convention when it is enabled but no config file exists, such as Vite's
    /// `index.html`. The default returns nothing.
    fn resolve_without_config(&self, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
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
    /// Module aliases the configs declare.
    pub aliases: Vec<PathAlias>,
    /// Output directories and the source directories they are built from.
    pub build_dirs: Vec<BuildDir>,
    /// `exports` conditions that select source files.
    pub conditions: Vec<String>,
    /// Specifier patterns the tool resolves at build time; see [`PluginInput::VirtualModule`].
    pub virtual_modules: Vec<String>,
    /// Directories bare specifiers are looked up in, relative to the workspace directory, in lookup order.
    pub module_dirs: Vec<String>,
    /// File extensions (with the leading dot) imports may omit, from bundler `resolve.extensions`.
    pub extensions: Vec<String>,
    /// A config computes entry points the plugin could not evaluate statically (`entry: await glob(…)` with a
    /// computed pattern, a call of an unknown function): `entries` may be missing files. Rules that report files as
    /// unreachable from entries should be conservative for this workspace.
    pub entries_incomplete: bool,
}
