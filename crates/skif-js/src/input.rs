//! The seam through which tool knowledge (plugins) feeds the graph.
//!
//! The graph never knows which tools a repository uses. A host registers any number of [`InputSource`]s; for every
//! workspace package each one returns a [`WorkspaceInputs`] with the entry and project globs, ignore lists and
//! references the tool implies. The plugin table of `skif-js-plugins` implements this trait.

use std::path::PathBuf;

use skif_core::FileSystem;
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

/// Build output that maps back to source: `dist` to `src` for a `tsconfig` with `outDir` and `rootDir`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildDir {
    /// The output directory, relative to the package directory.
    pub out_dir: String,
    /// The source directory, relative to the package directory.
    pub root_dir: String,
}

/// What an [`InputSource`] contributes for one workspace package.
///
/// Globs are relative to the package directory and use `/` separators. A leading `!` excludes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkspaceInputs {
    /// Entry files in every mode, such as test files or framework routes.
    pub entry: Vec<String>,
    /// Entry files that are part of the shipped product; the only plugin entries used in production mode.
    pub production_entry: Vec<String>,
    /// Files that belong to the project, added to the default or configured project globs.
    pub project: Vec<String>,
    /// Files whose issues are never reported, relative to the package directory.
    pub ignore: Vec<String>,
    /// Package names (globs allowed) that are never reported as unused, unlisted or optional peers.
    pub ignore_dependencies: Vec<String>,
    /// Binary names (globs allowed) that are never reported as unlisted.
    pub ignore_binaries: Vec<String>,
    /// Packages the tool uses without importing them, such as an `ESLint` parser named in a config.
    pub dependencies: Vec<String>,
    /// Binaries the tool runs without a script naming them.
    pub binaries: Vec<String>,
    /// Configuration files of the tools, absolute. They are entries in every mode so what they import counts.
    pub config_files: Vec<PathBuf>,
    /// Module aliases declared by bundler and compiler configuration.
    pub aliases: Vec<PathAlias>,
    /// Output directories and the source directories they are built from.
    pub build_dirs: Vec<BuildDir>,
    /// Extra `exports` conditions that select source files (`customConditions`).
    pub conditions: Vec<String>,
}

impl WorkspaceInputs {
    /// Appends everything in `other`.
    pub fn extend(&mut self, other: Self) {
        self.entry.extend(other.entry);
        self.production_entry.extend(other.production_entry);
        self.project.extend(other.project);
        self.ignore.extend(other.ignore);
        self.ignore_dependencies.extend(other.ignore_dependencies);
        self.ignore_binaries.extend(other.ignore_binaries);
        self.dependencies.extend(other.dependencies);
        self.binaries.extend(other.binaries);
        self.config_files.extend(other.config_files);
        self.aliases.extend(other.aliases);
        self.build_dirs.extend(other.build_dirs);
        self.conditions.extend(other.conditions);
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
