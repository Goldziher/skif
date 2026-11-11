//! The seam through which tool knowledge (plugins) feeds the graph.
//!
//! The graph never knows which tools a repository uses. A host registers any number of [`InputSource`]s; for every
//! workspace package each one returns a [`WorkspaceInputs`] with the entry and project globs, ignore lists and
//! references the tool implies. The plugin table of `skif-js-plugins` implements this trait.

use std::path::PathBuf;
use std::sync::OnceLock;

use skif_core::FileSystem;
pub use skif_core::{IncompleteCause, IncompleteEntries, IncompleteScope};
use skif_workspace::{Package, Workspace};

/// A module alias a tool configuration declares: imports of `find` (and `find/…`) point at `target`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathAlias {
    /// The aliased prefix, without a trailing `/` or `*`.
    pub find: String,
    /// The directory or file it stands for, relative to the package directory with `/` separators. `None` when the
    /// configuration computes it, in which case imports of the alias are known not to be packages but cannot be
    /// followed.
    pub target: Option<String>,
}

/// A binary that a command read from a file runs: a step of a workflow, a hook, a task of a tool config, or a source
/// file calling `execa` or `zx`. See [`WorkspaceInputs::run_binaries`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RunBinary {
    /// The binary's name as the command spells it.
    pub name: String,
    /// The file the command is read from, absolute.
    pub file: PathBuf,
    /// The directory the command runs the binary in after `cd` or `pushd`, relative to the package directory where
    /// the command starts, `/`-separated; `None` there and when the directory is not a static relative path.
    pub dir: Option<String>,
    /// The command runs the binary in `dir` without naming a workspace (`pnpm -C sub exec rspack`): it is credited to
    /// the package there and never judged where the command starts.
    pub filtered: bool,
}

impl RunBinary {
    /// The binary `name` that a command read from `file` runs, in `dir` (see [`RunBinary::dir`] and
    /// [`RunBinary::filtered`]).
    #[must_use]
    pub fn new(name: String, file: PathBuf, dir: Option<String>, filtered: bool) -> Self {
        Self {
            name,
            file,
            dir,
            filtered,
        }
    }
}

/// A package that a tool's config file names itself (a `tsconfig` `extends`, an eslint plugin, a Prisma generator):
/// the config needs it installed. See [`WorkspaceInputs::config_dependencies`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigDependency {
    /// The package name, without version or subpath.
    pub name: String,
    /// The file that names it, absolute.
    pub file: PathBuf,
    /// The reference loads only the package's types (`compilerOptions.types`): its `DefinitelyTyped` package
    /// (`@types/…`) satisfies it as well.
    pub type_only: bool,
    /// The package reading the config may list it instead of the package that owns `file`: a compiler option of a
    /// shared base `tsconfig` applies to each package extending it.
    pub reader_may_list: bool,
}

/// Build output that maps back to source: `dist` to `src` for a `tsconfig` with `outDir` and `rootDir`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildDir {
    /// The output directory, relative to the package directory.
    pub out_dir: String,
    /// The source directory, relative to the package directory.
    pub root_dir: String,
}

/// A custom element library: a class it registers is used by the browser through its tag name, so it is no unused
/// export (ADR 0062). `customElements.define('x-y', Class)` counts without one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomElementLibrary {
    /// Specifiers the library is imported from; one that ends with `/` matches every specifier below it.
    pub modules: Vec<String>,
    /// The name of the registering decorator as the library exports it (`customElement`).
    pub decorator: String,
    /// The base class whose subclasses register with a static `define(…)` (`FASTElement`), when there is one.
    pub base_class: Option<String>,
}

impl CustomElementLibrary {
    /// Whether `specifier` imports this library.
    #[must_use]
    pub fn is_module(&self, specifier: &str) -> bool {
        self.modules
            .iter()
            .any(|module| module == specifier || (module.ends_with('/') && specifier.starts_with(module.as_str())))
    }
}

/// What an [`InputSource`] contributes for one workspace package.
///
/// Globs are relative to the package directory and use `/` separators. A leading `!` excludes. A source starts from
/// [`WorkspaceInputs::default`] and fills in the fields it knows, as fields may be added.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct WorkspaceInputs {
    /// Entry files in every mode, such as test files or framework routes.
    pub entry: Vec<String>,
    /// Entry files that are part of the shipped product; the only plugin entries used in production mode.
    pub production_entry: Vec<String>,
    /// Entry files given as regular expressions in the syntax of the `regex` crate (Jest's `testRegex`), matched
    /// against `/` followed by the path relative to the package directory; besides `entry`, and like it not used in
    /// production mode.
    pub entry_regex: Vec<String>,
    /// Entry expressions of the package's Nx project scopes (ADR 0109): each selects files below its scope, matched
    /// like `entry_regex` against `/` followed by the path relative to the scope directory.
    pub scoped_entry_regex: Vec<ScopedEntryRegex>,
    /// Files that belong to the project, added to the default or configured project globs.
    pub project: Vec<String>,
    /// Files whose issues are never reported, relative to the package directory.
    pub ignore: Vec<String>,
    /// Package names (globs allowed) that are never reported as unused, unlisted or optional peers.
    pub ignore_dependencies: Vec<String>,
    /// Binary names (globs allowed) that are never reported as unlisted.
    pub ignore_binaries: Vec<String>,
    /// Specifier patterns (globs allowed) the tool resolves at build time, such as `@generated/*` or `#imports`. They
    /// are no dependencies and no file backs them, so they are never reported as unlisted or unresolved.
    pub virtual_modules: Vec<String>,
    /// Packages the tool uses without importing them, such as an `ESLint` parser named in a config.
    pub dependencies: Vec<String>,
    /// The packages of `dependencies` that a config file names itself and needs installed, with that file:
    /// `unlisted-dependencies` reports the ones no manifest lists at the file. Packages that a command in a config
    /// names, optional ones and those a tool assumes without a config are not among them.
    pub config_dependencies: Vec<ConfigDependency>,
    /// Binaries the tool runs without a script naming them.
    pub binaries: Vec<String>,
    /// The binaries of `binaries` that a command read from a file runs, with that file: `unlisted-binaries` reports
    /// the ones no dependency provides. Binaries fetched on demand, run in another workspace or installed globally
    /// are not among them.
    pub run_binaries: Vec<RunBinary>,
    /// Configuration files of the tools, absolute. They are entries in every mode so what they import counts.
    pub config_files: Vec<PathBuf>,
    /// Module aliases declared by bundler and compiler configuration.
    pub aliases: Vec<PathAlias>,
    /// Output directories and the source directories they are built from.
    pub build_dirs: Vec<BuildDir>,
    /// Extra `exports` conditions that select source files (`customConditions`).
    pub conditions: Vec<String>,
    /// Directories bare specifiers are looked up in (`baseUrl`, `paths` wildcards, `resolve.modules`), relative to the
    /// package directory with `/` separators, in lookup order.
    pub module_dirs: Vec<String>,
    /// File extensions (with the leading dot) imports may omit, from bundler `resolve.extensions`.
    pub extensions: Vec<String>,
    /// Custom element libraries the package uses, whose registered classes are used exports.
    pub custom_elements: Vec<CustomElementLibrary>,
    /// Whether a tool's entries could only be partly read (a computed `entry`), so unused files and dependencies of
    /// the package cannot be told apart from ones reached through the unknown entries.
    pub entries_incomplete: bool,
    /// Why the entries are incomplete, when `entries_incomplete` is set and the source knows.
    pub incomplete_reasons: Vec<IncompleteEntries>,
    /// Where the entries the source could not read can lie, when `entries_incomplete` is set: only files there are
    /// left unjudged by `unused-files` (ADR 0065).
    pub incomplete_scope: IncompleteScope,
}

impl WorkspaceInputs {
    /// Appends everything in `other`.
    pub fn extend(&mut self, other: Self) {
        self.entry.extend(other.entry);
        self.production_entry.extend(other.production_entry);
        self.entry_regex.extend(other.entry_regex);
        self.scoped_entry_regex.extend(other.scoped_entry_regex);
        self.project.extend(other.project);
        self.ignore.extend(other.ignore);
        self.ignore_dependencies.extend(other.ignore_dependencies);
        self.ignore_binaries.extend(other.ignore_binaries);
        self.virtual_modules.extend(other.virtual_modules);
        self.dependencies.extend(other.dependencies);
        self.config_dependencies.extend(other.config_dependencies);
        self.binaries.extend(other.binaries);
        self.run_binaries.extend(other.run_binaries);
        self.config_files.extend(other.config_files);
        self.aliases.extend(other.aliases);
        self.build_dirs.extend(other.build_dirs);
        self.conditions.extend(other.conditions);
        self.module_dirs.extend(other.module_dirs);
        self.extensions.extend(other.extensions);
        self.custom_elements.extend(other.custom_elements);
        if other.entries_incomplete {
            self.incomplete_scope = if self.entries_incomplete {
                std::mem::take(&mut self.incomplete_scope).union(other.incomplete_scope)
            } else {
                other.incomplete_scope
            };
        }
        self.entries_incomplete |= other.entries_incomplete;
        self.incomplete_reasons.extend(other.incomplete_reasons);
    }
}

/// An entry expression that selects files below a directory of the package; see
/// [`WorkspaceInputs::scoped_entry_regex`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopedEntryRegex {
    /// The directory, relative to the package directory with `/` separators.
    pub scope: String,
    /// The expression, in the syntax of the `regex` crate.
    pub regex: String,
}

/// Listings of the workspace that every [`InputRequest`] of one run shares, each made on first use, so the input
/// sources of the packages do not walk the same directories again.
#[derive(Debug)]
pub struct WorkspaceListing {
    gitignore: bool,
    sequential: bool,
    nx_projects: OnceLock<Vec<(String, String)>>,
}

impl WorkspaceListing {
    /// An empty listing, whose walks honour `.gitignore` files unless `gitignore` is off (`--no-gitignore`) and run
    /// on the calling thread with `sequential`.
    #[must_use]
    pub fn new(gitignore: bool, sequential: bool) -> Self {
        Self {
            gitignore,
            sequential,
            nx_projects: OnceLock::new(),
        }
    }

    /// The directories of the `project.json` files that the source walk reaches in `workspace`, relative to its root
    /// and sorted, each with the directory of the package that owns it (`""` for the root): the Nx projects of ADR
    /// 0109, below no build output, hidden or gitignored directory. One walk serves the whole run.
    pub fn nx_projects(&self, fs: &dyn FileSystem, workspace: &Workspace) -> &[(String, String)] {
        // The walk runs on rayon while other packages' setups ask for the same listing.
        skif_core::get_or_compute(&self.nx_projects, || {
            crate::discover::nx_project_dirs(fs, workspace, self.sequential, self.gitignore)
        })
    }
}

/// The package an [`InputSource`] is asked about.
#[derive(Clone, Copy)]
pub struct InputRequest<'a> {
    /// The whole workspace.
    pub workspace: &'a Workspace,
    /// The package whose inputs are wanted.
    pub package: &'a Package,
    /// File access, for reading tool configuration files.
    pub fs: &'a dyn FileSystem,
    /// The dependencies listed by the `package.json` files above the workspace root (a nested root sees the tools
    /// of the project that contains it), sorted and without duplicates.
    pub ancestor_dependencies: &'a [String],
    /// The directory of `package` as the `[js.workspaces]` keys select it: `.` for the root package and its directory
    /// otherwise, both joined to the workspace's [`workspace_keys_anchor`](Workspace::workspace_keys_anchor) when it
    /// has one (`tools/sub` for the root package of the root in `tools/sub` of a run with an explicit configuration).
    pub workspace_key: &'a str,
    /// The listings every request of the run shares.
    pub listing: &'a WorkspaceListing,
}

impl std::fmt::Debug for InputRequest<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InputRequest")
            .field("package", &self.package.display_name())
            .finish_non_exhaustive()
    }
}

/// A provider of tool-specific entries, project files, ignores and references.
pub trait InputSource: Send + Sync {
    /// The inputs for `request.package`; an empty value when the tool is not used there.
    fn inputs(&self, request: &InputRequest<'_>) -> WorkspaceInputs;
}

const _: () = {
    fn assert_send_sync<T: Send + Sync>() {}
    let _ = assert_send_sync::<WorkspaceInputs>;
};

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::io;
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    use skif_config::Config;
    use skif_core::{DirEntry, MemoryFs};

    use super::*;

    /// A file system that records the directories it lists.
    struct Counting {
        inner: MemoryFs,
        listed: Mutex<Vec<PathBuf>>,
    }

    impl FileSystem for Counting {
        fn read_to_string(&self, path: &Path) -> io::Result<Arc<str>> {
            self.inner.read_to_string(path)
        }

        fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
            self.listed.lock().unwrap().push(path.to_path_buf());
            self.inner.read_dir(path)
        }

        fn is_file(&self, path: &Path) -> bool {
            self.inner.is_file(path)
        }

        fn is_dir(&self, path: &Path) -> bool {
            self.inner.is_dir(path)
        }
    }

    /// The Nx projects of a run are listed by one walk, however many packages ask for them: each directory is listed
    /// once.
    #[test]
    fn the_nx_projects_are_listed_once_per_run() {
        let memory = MemoryFs::new()
            .with_file("/ws/package.json", r#"{"name":"r","workspaces":["packages/*"]}"#)
            .with_file("/ws/apps/web/project.json", "{}")
            .with_file("/ws/packages/b/package.json", r#"{"name":"b"}"#)
            .with_file("/ws/packages/b/project.json", "{}");
        let config = Config::from_toml_str("").unwrap();
        let workspace = Workspace::discover_with_fs(Path::new("/ws"), &config, &memory).unwrap();
        let fs = Counting {
            inner: memory,
            listed: Mutex::new(Vec::new()),
        };
        let listing = WorkspaceListing::new(true, true);
        let expected = [
            ("apps/web".to_owned(), String::new()),
            ("packages/b".to_owned(), "packages/b".to_owned()),
        ];
        assert_eq!(listing.nx_projects(&fs, &workspace), expected);
        let first_walk = fs.listed.lock().unwrap().clone();
        assert_eq!(listing.nx_projects(&fs, &workspace), expected);
        let listed = fs.listed.lock().unwrap().clone();
        assert_eq!(listed, first_walk);
        let apps = listed.iter().filter(|dir| *dir == Path::new("/ws/apps")).count();
        assert_eq!(apps, 1, "{listed:?}");
    }
}
