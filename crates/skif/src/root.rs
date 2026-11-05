//! Locating the workspace root and loading its configuration.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use skif_config::{
    CONFIG_FILE_NAMES, Ceiling, Config, ConfigCache, ConfigError, ConfigLoader, ConfigOverrides, ResolvedConfig,
};
use skif_core::FileSystem;
use skif_workspace::{Workspace, declared_member_dirs, declares_workspace, repository_root};

use crate::error::Error;

const MANIFEST: &str = "package.json";

/// The directory the search for a configuration file of `root` stops at: the root of the git repository holding it,
/// and the root itself outside a repository.
pub(crate) fn config_ceiling(root: &Path, fs: &dyn FileSystem) -> PathBuf {
    repository_root(fs, root).unwrap_or_else(|| root.to_path_buf())
}

/// Whether a directory strictly above `root`, up to and including `ceiling`, holds a `package.json` or a workspace
/// declaration: a standalone package there is nested in another project.
pub(crate) fn is_enclosed(root: &Path, ceiling: &Path, fs: &dyn FileSystem) -> bool {
    root.ancestors()
        .skip(1)
        .take_while(|dir| dir.starts_with(ceiling))
        .any(|dir| fs.is_file(&dir.join(MANIFEST)) || declares_workspace(fs, dir))
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
fn workspace_members(root: &Path, fs: &dyn FileSystem, cache: &ConfigCache) -> BTreeSet<PathBuf> {
    let config = resolve_config(root, &Ceiling::Vcs, fs, &ConfigOverrides::default(), cache)
        .map(|resolved| resolved.config)
        .unwrap_or_default();
    Workspace::discover_with_fs(root, &config, fs)
        .map_or_else(|_| declared_member_dirs(fs, root), |workspace| workspace.claimed_dirs())
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
    let cached = crate::cache::per_call(fs);
    let fs: &dyn FileSystem = &cached;
    let Located { project, ancestors } = locate(start, fs)?;
    if declares_workspace(fs, &project) {
        return Ok(project);
    }
    let cache = ConfigCache::new();
    Ok(ancestors
        .into_iter()
        .find(|dir| workspace_members(dir, fs, &cache).contains(&project))
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

/// Resolves the configuration of `root` through `fs`: every `skif.toml` or `.skif.toml` from `root` up to `ceiling`
/// (stopping after a file with `root = true`), with what they `extend`, merged farthest first, then `overrides` on top
/// (defaults < files < overrides). Returns the merged configuration with the files it came from and where each key
/// was set.
///
/// `cache` remembers which directories hold a configuration file and every file parsed, so pass one cache to all
/// the roots of a run, in parallel if you like: each file is read once.
///
/// # Errors
/// [`ConfigError::Ambiguous`] when a directory of the chain has both files, [`ConfigError::Io`] when a file cannot be
/// read, [`ConfigError::Parse`], [`ConfigError::Invalid`] and [`ConfigError::Scope`] (carrying the file, line and
/// column) for bad contents, [`ConfigError::ExtendsCycle`] for files that extend each other, and
/// [`ConfigError::Invalid`] for an override that is not a valid selector.
pub fn resolve_config(
    root: &Path,
    ceiling: &Ceiling,
    fs: &dyn FileSystem,
    overrides: &ConfigOverrides,
    cache: &ConfigCache,
) -> Result<ResolvedConfig, ConfigError> {
    ConfigLoader::new(fs, cache).load(root, ceiling, overrides)
}

/// Loads the configuration of `root` through `fs` and layers `overrides` over it (defaults < files < overrides).
///
/// The files are `root`'s own `skif.toml` or `.skif.toml` and those of its parent directories up to the root of the
/// git repository (`root` alone outside one), stopping after a file with `root = true`; each file also brings in the
/// files it `extends`. No file means the default configuration. See [`resolve_config`] for the other ceilings and
/// for where each setting came from.
///
/// # Errors
/// [`ConfigError::Ambiguous`] when both files exist in a directory, [`ConfigError::Io`] when one cannot be read,
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
    Ok(resolve_config(root, &Ceiling::Vcs, fs, overrides, &ConfigCache::new())?.config)
}
