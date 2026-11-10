//! The plugin model of ADR 0011: a declarative [`PluginSpec`] row, an optional config resolver and the
//! [`PluginOutput`] handed to the module graph.

use std::ops::Range;
use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use skif_config::PluginConfig;
use skif_core::FileSystem;
use skif_js_syntax::{ConfigValue, ModuleLoader};

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
    /// A script of the workspace contains this text (`bun test`).
    Script(&'static str),
    /// The workspace's `package.json` has this key; a `.` separates the keys of a nested object
    /// (`typescript.contentMapper`).
    ManifestKey(&'static str),
}

/// A custom element library whose registrations mark a class as used (ADR 0062).
///
/// A class decorated with `decorator` imported from one of `modules` is registered, and so is a class that extends
/// `base_class` imported from one of them and calls `Class.define(…)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CustomElements {
    /// Specifiers the library is imported from; one that ends with `/` matches every specifier below it.
    pub modules: &'static [&'static str],
    /// The name of the registering decorator as the library exports it (`customElement`).
    pub decorator: &'static str,
    /// The base class whose subclasses register with a static `define`, when the library has one (`FASTElement`).
    pub base_class: Option<&'static str>,
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
    /// Key in `package.json` that holds the tool's config, when the tool supports it; a `.` separates the keys of a
    /// nested object (`config.commitizen`).
    pub package_json_key: Option<&'static str>,
    /// Binaries the tool's packages provide.
    pub binaries: &'static [&'static str],
    /// Globs of files the plugin generates or owns, which must not be reported as unused.
    pub ignore: &'static [&'static str],
    /// The custom element library of the tool, whose registered classes are used without an import.
    pub custom_elements: Option<CustomElements>,
    /// The commands the tool runs (CI steps, Git hooks, task runner tasks) need not run a dependency's binary: one
    /// they run by name may come from the machine, so only a binary reached through a package manager (`pnpm exec
    /// x`, `yarn x`) or a `node_modules/.bin` path is judged by `unlisted-binaries`. Every binary still credits the
    /// dependency that provides it. knip's `optionalBinaries`.
    pub optional_binaries: bool,
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
        custom_elements: None,
        optional_binaries: false,
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
    /// Test files given as a regular expression (Jest's `testRegex`) in the syntax of the `regex` crate, matched
    /// against `/` followed by the path relative to the package directory ([`PluginContext::dir`]); unanchored, it
    /// matches anywhere in the path. It adds entries: the plugin's default entry globs stay.
    EntryRegex(String),
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
    /// A shell script that a source file of the workspace runs (`execa('tsx', ['build.ts'])`), parsed with the script
    /// parser like [`PluginInput::Script`]; the binaries it runs are recorded against `file` rather than the config.
    SourceScript {
        /// The source file, relative to the workspace directory.
        file: String,
        /// The script text.
        script: String,
    },
    /// A package that a compiler option of a `tsconfig` names (`types`, `jsxImportSource`, `plugins`,
    /// `importHelpers`). The option applies to the package reading the config, so that package may list it as well
    /// as the one that owns the config, as knip's `alsoReferenceOwner` has it ([`DepRef::reader_may_list`]).
    CompilerDependency {
        /// The package name, without version or subpath.
        name: String,
        /// The option loads only the package's types (`types`): its `DefinitelyTyped` package serves as well
        /// ([`DepRef::type_only`]).
        type_only: bool,
    },
    /// A package that a file of the tool which is not a config names, such as the `provider` of a Prisma schema's
    /// `generator`: recorded against `file` and judged like a package a config names
    /// ([`DepRef::named_by_config`]).
    FileDependency {
        /// The file, relative to the workspace directory.
        file: String,
        /// The package name, without version or subpath.
        name: String,
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
    /// Finds the modules a config file imports values from (a base config of the project or of a workspace package),
    /// so they are evaluated too; `None` leaves imported values unknown.
    pub modules: Option<&'a dyn ModuleLoader>,
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

    /// The value of a config file that parses but exports nothing the evaluator recognises, read as a build script
    /// that calls the tool's API instead (`await esbuild.build({ entryPoints: [...] })`). `None`, the default, leaves
    /// such a file unparsable, so the package's entries are incomplete.
    fn read_script(&self, _path: &Path, _source: &str) -> Option<ConfigValue> {
        None
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

    /// Where the entries that `config` hides for `cause` can plausibly lie, so that only those files are not judged
    /// unused (ADR 0065). Called for an unknown or partly unknown value ([`IncompleteCause::UnknownValue`],
    /// [`IncompleteCause::UnknownSpread`]) and for computed entry keys ([`IncompleteCause::ComputedEntries`]); a
    /// config that does not parse is always [`IncompleteScope::Package`].
    ///
    /// The default is [`IncompleteScope::Package`]: a plugin that cannot bound what its config may name keeps the
    /// whole package unjudged.
    fn incomplete_scope(&self, _cause: IncompleteCause, _config: &ConfigAst<'_>) -> IncompleteScope {
        IncompleteScope::Package
    }
}

/// Where the unknown entries of an incomplete config can lie; see [`Plugin::incomplete_scope`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum IncompleteScope {
    /// Anywhere in the package: no file of it is judged unused.
    Package,
    /// Only files matching these globs, relative to the workspace directory, may be unknown entries. Every other file
    /// of the package is judged as usual.
    Within(Vec<String>),
}

/// A package the plugin's configuration needs.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "each bool is an independent property of the reference"
)]
pub struct DepRef {
    /// The package name, without version or subpath.
    pub name: String,
    /// The file that references it.
    pub from: PathBuf,
    /// Byte range in `from`, when known; config values carry no spans, so this is usually `None`.
    pub span: Option<Range<usize>>,
    /// The reference does not require the package to be declared (optional peers, on-demand fetches).
    pub optional: bool,
    /// A config file the plugin read names the package itself in a required reference, rather than in an optional
    /// one, in a command the config holds or as a package the plugin assumes without a config: the config needs it
    /// installed, so no manifest listing it is an unlisted dependency at `from`.
    pub named_by_config: bool,
    /// The config reference loads only the package's types (`compilerOptions.types`): the package's `DefinitelyTyped`
    /// package (`@types/…`) satisfies it as well, as an import of types only is satisfied.
    pub type_only: bool,
    /// The package reading the config may list the package the config reference names instead of the package that
    /// owns `from`: a compiler option of a shared base `tsconfig` applies to each package extending it.
    pub reader_may_list: bool,
}

impl DepRef {
    /// A reference to package `name` from `from` without a span; `named_by_config` when a config names it itself,
    /// which only a required reference keeps.
    #[must_use]
    pub fn new(name: String, from: &Path, optional: bool, named_by_config: bool) -> Self {
        Self {
            name,
            from: from.to_path_buf(),
            span: None,
            optional,
            named_by_config: named_by_config && !optional,
            type_only: false,
            reader_may_list: false,
        }
    }

    /// Folds `other`, a reference to the same package from the same file, into this one: it is optional only when
    /// both are, and a config reference when either is a required one, judged as that config reference demands (both
    /// of them when both are config references).
    pub fn merge(&mut self, other: &Self) {
        self.optional &= other.optional;
        if !other.named_by_config {
            return;
        }
        if self.named_by_config {
            self.type_only &= other.type_only;
            self.reader_may_list &= other.reader_may_list;
        } else {
            self.named_by_config = true;
            self.type_only = other.type_only;
            self.reader_may_list = other.reader_may_list;
        }
    }
}

/// A binary that a command read from a file runs; see [`PluginOutput::run_binaries`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct BinaryRef {
    /// The binary's name as the command spells it.
    pub name: String,
    /// The file the command is read from, absolute: a config, a workflow or a source file.
    pub from: PathBuf,
    /// The directory the command runs the binary in after `cd` or `pushd`, relative to the directory of the package
    /// the plugin runs for, where the command starts, `/`-separated; `None` there and when the directory is not a
    /// static relative path (see [`BinaryUse::relocation_dir`](crate::scripts::BinaryUse::relocation_dir)).
    pub dir: Option<String>,
    /// The command runs the binary in `dir` without naming a workspace (`pnpm -C sub exec rspack`): it is credited to
    /// the package there and never judged where the command starts.
    pub filtered: bool,
}

impl BinaryRef {
    /// The binary `name` that a command read from `from` runs, in `dir` (see [`BinaryRef::dir`] and
    /// [`BinaryRef::filtered`]).
    #[must_use]
    pub fn new(name: String, from: &Path, dir: Option<String>, filtered: bool) -> Self {
        Self {
            name,
            from: from.to_path_buf(),
            dir,
            filtered,
        }
    }
}

/// Everything one enabled plugin contributes to a workspace.
///
/// Globs and paths are relative to the workspace directory, except the absolute `config_files`, the `from` of each
/// [`DepRef`] and the `from` of each [`BinaryRef`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct PluginOutput {
    /// Entry files and globs, including the production ones.
    pub entries: Vec<String>,
    /// Project file globs.
    pub project: Vec<String>,
    /// Entries that belong to a production build; a subset of `entries`.
    pub production_entries: Vec<String>,
    /// Entry files given as regular expressions, besides `entries`; see [`PluginInput::EntryRegex`]. Not used in
    /// production mode.
    pub entry_regexes: Vec<String>,
    /// Config files that were found and read.
    pub config_files: Vec<PathBuf>,
    /// Packages the configs reference by name.
    pub referenced_dependencies: Vec<DepRef>,
    /// Binaries the configs invoke (from embedded scripts).
    pub referenced_binaries: Vec<String>,
    /// The binaries of `referenced_binaries` that a command read from a file runs, with that file, for
    /// `unlisted-binaries`. Binaries fetched on demand (`npx x`), run in another workspace (`pnpm --filter web exec
    /// x`), assumed installed globally or ignored by the script parser are left out; so are the binaries a plugin
    /// names without a command (`PluginInput::Binary`).
    pub run_binaries: Vec<BinaryRef>,
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
    /// The custom element library of the tool; see [`PluginSpec::custom_elements`].
    pub custom_elements: Option<CustomElements>,
    /// A config computes entry points the plugin could not evaluate statically (`entry: await glob(…)` with a
    /// computed pattern, a call of an unknown function): `entries` may be missing files. Rules that report files as
    /// unreachable from entries should be conservative for this workspace.
    pub entries_incomplete: bool,
    /// Each config that made the entries incomplete, and why; not empty whenever `entries_incomplete` is set.
    pub incomplete_reasons: Vec<IncompleteEntries>,
}

/// A config that made a plugin's entries incomplete; see [`PluginOutput::incomplete_reasons`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncompleteEntries {
    /// The plugin that reads the config.
    pub plugin: PluginId,
    /// The config file, absolute.
    pub file: PathBuf,
    /// What could not be read.
    pub cause: IncompleteCause,
    /// Where the entries the config hides can lie.
    pub scope: IncompleteScope,
}

/// Why a config leaves a plugin's entries incomplete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum IncompleteCause {
    /// The config file does not parse.
    Unparsable,
    /// The config evaluates to a value that is not known statically, such as the result of an unknown function.
    UnknownValue,
    /// The config spreads or merges in a value that is not known statically, such as an imported base config.
    UnknownSpread,
    /// The keys the plugin reads entries from are computed in a way that cannot be evaluated statically.
    ComputedEntries,
}
