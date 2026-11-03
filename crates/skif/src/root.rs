//! Locating the workspace root and loading its configuration.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use skif_config::{CONFIG_FILE_NAMES, Config, ConfigError, ConfigOverrides};
use skif_core::FileSystem;
use skif_workspace::{Workspace, declared_member_dirs, declares_workspace, repository_root};

use crate::error::Error;

const MANIFEST: &str = "package.json";

/// The directory the search for a configuration file of `root` stops at: the root of the git repository holding it,
/// and the root itself outside a repository.
pub(crate) fn config_ceiling(root: &Path, fs: &dyn FileSystem) -> PathBuf {
    repository_root(fs, root).unwrap_or_else(|| root.to_path_buf())
}

/// The nearest directory from `root` up to and including `ceiling` that holds a configuration file.
pub(crate) fn config_dir<'a>(root: &'a Path, ceiling: &Path, fs: &dyn FileSystem) -> Option<&'a Path> {
    root.ancestors()
        .take_while(|dir| dir.starts_with(ceiling))
        .find(|dir| config_file(dir, fs).is_some())
}

/// The project that contains a start path, and the workspaces above it that could claim it.
pub(crate) struct Located {
    /// The nearest directory at or above the start with a `package.json` or a workspace declaration.
    pub project: PathBuf,
    /// The directories above `project` that declare a workspace, nearest first.
    pub ancestors: Vec<PathBuf>,
}

/// Locates the project of `start`, climbing no higher than the root of the git repository holding it.
pub(crate) fn locate(start: &Path, fs: &dyn FileSystem) -> Result<Located, Error> {
    if !start.has_root() {
        return Err(Error::RelativePath(start.to_path_buf()));
    }
    let first = if fs.is_dir(start) {
        start
    } else {
        start.parent().unwrap_or(start)
    };
    let ceiling = repository_root(fs, first);
    let within = |dir: &Path| ceiling.as_deref().is_none_or(|top| dir.starts_with(top));
    let project = first
        .ancestors()
        .take_while(|dir| within(dir))
        .find(|dir| fs.is_file(&dir.join(MANIFEST)) || declares_workspace(fs, dir))
        .ok_or_else(|| Error::NoWorkspace(start.to_path_buf()))?;
    let ancestors = project
        .ancestors()
        .skip(1)
        .take_while(|dir| within(dir))
        .filter(|dir| declares_workspace(fs, dir))
        .map(Path::to_path_buf)
        .collect();
    Ok(Located {
        project: project.to_path_buf(),
        ancestors,
    })
}

/// The member directories of the workspace at `root`, discovered with its own configuration; the declared
/// patterns when it cannot be discovered.
fn workspace_members(root: &Path, fs: &dyn FileSystem) -> BTreeSet<PathBuf> {
    let ceiling = config_ceiling(root, fs);
    let dir = config_dir(root, &ceiling, fs).unwrap_or(root);
    let config = load_config(dir, fs, &ConfigOverrides::default()).unwrap_or_default();
    Workspace::discover_with_fs(root, &config, fs)
        .map_or_else(|_| declared_member_dirs(fs, root), |workspace| workspace.member_dirs())
}

/// Finds the workspace that `start` belongs to.
///
/// The project of `start` is its nearest ancestor (inclusive) with a `package.json` or a workspace declaration. When
/// the project declares a workspace itself, it is the root. Otherwise the nearest ancestor workspace that lists the
/// project as a member (after negations) is the root, and when none does, the project is its own root as a single
/// package: an ancestor workspace never claims a project it does not include.
///
/// A workspace that declares itself is its own root even when an outer workspace lists it as a member, as it is for
/// [`discover_all`](crate::discover_all). Ancestor workspaces are read with their own configuration, so their
/// package-manager choice decides which patterns count.
///
/// The search never climbs above the root of the git repository holding `start` (the nearest directory with a
/// `.git`), so a stray `package.json` in a parent of the repository is never taken for its workspace.
///
/// `start` may be a file or a directory and must be rooted (absolute); the search never consults the working
/// directory, so a relative path is rejected rather than silently searching only its own lexical ancestors.
///
/// # Errors
/// [`Error::RelativePath`] when `start` is relative, [`Error::NoWorkspace`] when `start` and its ancestors hold no
/// `package.json` and declare no workspace.
///
/// # Examples
/// ```
/// use std::path::Path;
/// use skif::{MemoryFs, find_workspace_root};
///
/// let fs = MemoryFs::new()
///     .with_file("/repo/package.json", r#"{"workspaces": ["packages/*"]}"#)
///     .with_file("/repo/packages/a/package.json", r#"{"name": "a"}"#)
///     .with_file("/repo/other/package.json", r#"{"name": "other"}"#);
/// assert_eq!(find_workspace_root(Path::new("/repo/packages/a"), &fs)?, Path::new("/repo"));
/// assert_eq!(find_workspace_root(Path::new("/repo/other"), &fs)?, Path::new("/repo/other"));
/// # Ok::<(), skif::Error>(())
/// ```
pub fn find_workspace_root(start: &Path, fs: &dyn FileSystem) -> Result<PathBuf, Error> {
    let Located { project, ancestors } = locate(start, fs)?;
    if declares_workspace(fs, &project) {
        return Ok(project);
    }
    Ok(ancestors
        .into_iter()
        .find(|dir| workspace_members(dir, fs).contains(&project))
        .unwrap_or(project))
}

/// The name of the configuration file in `root` (`skif.toml` or `.skif.toml`), relative to `root`, when there is one.
///
/// Pass it to [`RunOptions::with_config_path`](crate::RunOptions::with_config_path) so diagnostics about the
/// configuration point at the right file. When both files exist [`load_config`] fails, and this returns the first.
///
/// # Examples
/// ```
/// use std::path::Path;
/// use skif::{MemoryFs, config_file};
///
/// let fs = MemoryFs::new().with_file("/repo/.skif.toml", "");
/// assert_eq!(config_file(Path::new("/repo"), &fs), Some(".skif.toml"));
/// ```
#[must_use]
pub fn config_file(root: &Path, fs: &dyn FileSystem) -> Option<&'static str> {
    CONFIG_FILE_NAMES
        .iter()
        .copied()
        .find(|name| fs.is_file(&root.join(name)))
}

fn with_path(mut error: ConfigError, file: &Path) -> ConfigError {
    if let ConfigError::Parse { path, .. } | ConfigError::Invalid { path, .. } = &mut error {
        *path = Some(file.to_path_buf());
    }
    error
}

fn read_config(path: &Path, fs: &dyn FileSystem) -> Result<Config, ConfigError> {
    let text = fs.read_to_string(path).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Config::from_toml_str(&text).map_err(|error| with_path(error, path))
}

/// Loads `skif.toml` or `.skif.toml` from `root` through `fs` and layers `overrides` over it (defaults < file < overrides).
///
/// No file means the default configuration.
///
/// # Errors
/// [`ConfigError::Ambiguous`] when both files exist, [`ConfigError::Io`] when one cannot be read,
/// [`ConfigError::Parse`] / [`ConfigError::Invalid`] (carrying the file, line and column) for bad contents, and
/// [`ConfigError::Invalid`] for an override that is not a valid selector.
///
/// # Examples
/// ```
/// use std::path::Path;
/// use skif::{ConfigOverrides, MemoryFs, load_config};
///
/// let fs = MemoryFs::new().with_file("/repo/skif.toml", "[lint]\nfail-on-warnings = true\n");
/// let config = load_config(Path::new("/repo"), &fs, &ConfigOverrides::default())?;
/// assert!(config.lint.fail_on_warnings);
/// # Ok::<(), skif::ConfigError>(())
/// ```
pub fn load_config(root: &Path, fs: &dyn FileSystem, overrides: &ConfigOverrides) -> Result<Config, ConfigError> {
    let found: Vec<PathBuf> = CONFIG_FILE_NAMES
        .iter()
        .map(|name| root.join(name))
        .filter(|path| fs.is_file(path))
        .collect();
    let mut config = match found.as_slice() {
        [] => Config::default(),
        [path] => read_config(path, fs)?,
        [first, second, ..] => {
            return Err(ConfigError::Ambiguous {
                first: first.clone(),
                second: second.clone(),
            });
        }
    };
    config.apply(overrides)?;
    Ok(config)
}
