//! Multi-root discovery: every workspace and standalone package below a directory, each with its own configuration.

use std::path::{Path, PathBuf};

use rayon::prelude::*;
use skif_config::{Config, ConfigOverrides};
use skif_core::FileSystem;
use skif_workspace::{DiscoverOptions, Workspace, discover_roots};

use crate::error::Error;
use crate::root::{config_file, load_config};

/// One root to lint: a workspace or a standalone package, with the configuration that governs it.
#[derive(Debug)]
#[non_exhaustive]
pub struct Target {
    /// Absolute root directory.
    pub root: PathBuf,
    /// The nearest `skif.toml` or `.skif.toml` with the overrides applied, or the defaults when there is none.
    pub config: Config,
    /// Absolute path of the configuration file `config` was read from, when there is one.
    pub config_file: Option<PathBuf>,
    /// The discovered workspace (a standalone package is a workspace of one package).
    pub workspace: Workspace,
}

impl Target {
    /// The configuration file relative to [`root`](Self::root), for
    /// [`RunOptions::with_config_path`](crate::RunOptions::with_config_path): `skif.toml` when it sits in the root,
    /// `../skif.toml` when it was inherited from a parent directory.
    #[must_use]
    pub fn config_path(&self) -> Option<PathBuf> {
        let file = self.config_file.as_ref()?;
        let dir = file.parent()?;
        let up = self.root.strip_prefix(dir).ok()?.components().count();
        let mut path: PathBuf = std::iter::repeat_n("..", up).collect();
        path.push(file.file_name()?);
        Some(path)
    }
}

/// Loads the configuration for `root` from the nearest config file in `root` or a parent up to and including
/// `ceiling`, and discovers the workspace at `root`.
///
/// With `ceiling == root` only the root's own config file counts.
///
/// # Errors
/// [`Error::Config`] for an unreadable, ambiguous or invalid configuration, [`Error::Workspace`] when the root cannot
/// be discovered.
pub fn load_target(
    root: &Path,
    ceiling: &Path,
    fs: &dyn FileSystem,
    overrides: &ConfigOverrides,
) -> Result<Target, Error> {
    let config_dir = root
        .ancestors()
        .take_while(|dir| dir.starts_with(ceiling))
        .find(|dir| config_file(dir, fs).is_some());
    let config = load_config(config_dir.unwrap_or(root), fs, overrides)?;
    let file = config_dir.and_then(|dir| config_file(dir, fs).map(|name| dir.join(name)));
    let workspace = Workspace::discover_with_fs(root, &config, fs)?;
    Ok(Target {
        root: root.to_path_buf(),
        config,
        config_file: file,
        workspace,
    })
}

/// Finds every workspace root and standalone package at or below `start` and loads each as a [`Target`], sorted by
/// path (see [`skif_workspace::discover_roots`] for what counts as a root).
///
/// Each root uses its nearest configuration file, searching from the root up to `start`; a root with none gets the
/// default configuration, not one from above `start`. `start` may be a file, in which case its directory is used,
/// and must be absolute. The result is empty when nothing is found.
///
/// # Errors
/// [`Error::RelativePath`] for a relative `start`, otherwise the first failing root's [`load_target`] error.
///
/// # Examples
/// ```
/// use std::path::Path;
/// use skif::{ConfigOverrides, MemoryFs, discover_all};
///
/// let fs = MemoryFs::new()
///     .with_file("/r/a/package.json", r#"{"name": "a", "version": "1.0.0"}"#)
///     .with_file("/r/b/package.json", r#"{"name": "b", "version": "1.0.0"}"#);
/// let targets = discover_all(Path::new("/r"), &fs, &ConfigOverrides::default())?;
/// assert_eq!(targets.len(), 2);
/// # Ok::<(), skif::Error>(())
/// ```
pub fn discover_all(start: &Path, fs: &dyn FileSystem, overrides: &ConfigOverrides) -> Result<Vec<Target>, Error> {
    if !start.has_root() {
        return Err(Error::RelativePath(start.to_path_buf()));
    }
    let dir = if fs.is_dir(start) {
        start
    } else {
        start.parent().unwrap_or(start)
    };
    discover_roots(dir, fs, &DiscoverOptions::default())
        .par_iter()
        .map(|root| load_target(root.path(), dir, fs, overrides))
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use skif_core::MemoryFs;

    use super::*;

    fn fs() -> MemoryFs {
        MemoryFs::new()
            .with_file("/r/skif.toml", "[lint]\nfail-on-warnings = true\n")
            .with_file("/r/a/package.json", r#"{"name": "a", "version": "1.0.0"}"#)
            .with_file("/r/b/skif.toml", "")
            .with_file("/r/b/package.json", r#"{"name": "b", "version": "1.0.0"}"#)
    }

    #[test]
    fn nearest_config_wins_and_inherits_from_the_start_directory() {
        let targets = discover_all(Path::new("/r"), &fs(), &ConfigOverrides::default()).unwrap();
        let roots: Vec<_> = targets.iter().map(|target| target.root.clone()).collect();
        assert_eq!(roots, [PathBuf::from("/r/a"), PathBuf::from("/r/b")]);
        assert!(targets[0].config.lint.fail_on_warnings);
        assert_eq!(targets[0].config_path(), Some(PathBuf::from("../skif.toml")));
        assert!(!targets[1].config.lint.fail_on_warnings);
        assert_eq!(targets[1].config_path(), Some(PathBuf::from("skif.toml")));
    }

    #[test]
    fn config_above_the_start_directory_is_not_inherited() {
        let targets = discover_all(Path::new("/r/a"), &fs(), &ConfigOverrides::default()).unwrap();
        assert_eq!(targets.len(), 1);
        assert!(!targets[0].config.lint.fail_on_warnings);
        assert_eq!(targets[0].config_path(), None);
    }

    #[test]
    fn relative_start_is_rejected() {
        assert!(matches!(
            discover_all(Path::new("r"), &fs(), &ConfigOverrides::default()),
            Err(Error::RelativePath(_))
        ));
    }
}
