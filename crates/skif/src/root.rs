//! Locating the workspace root and loading its configuration.

use std::path::{Path, PathBuf};

use skif_config::{CONFIG_FILE_NAMES, Config, ConfigError, ConfigOverrides};
use skif_core::FileSystem;

use crate::error::Error;

const MANIFEST: &str = "package.json";
const PNPM_WORKSPACE: &str = "pnpm-workspace.yaml";

fn declares_workspace(dir: &Path, fs: &dyn FileSystem) -> bool {
    if fs.is_file(&dir.join(PNPM_WORKSPACE)) {
        return true;
    }
    fs.read_to_string(&dir.join(MANIFEST))
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .is_some_and(|manifest| manifest.get("workspaces").is_some())
}

/// Finds the nearest ancestor of `start` (inclusive) that declares a workspace: a `package.json` with a
/// `workspaces` field, or a `pnpm-workspace.yaml`.
///
/// `start` may be a file or a directory and must be rooted (absolute); the search never consults the working
/// directory, so a relative path is rejected rather than silently searching only its own lexical ancestors. When no ancestor declares a workspace but `start` itself holds a
/// `package.json`, `start` is returned so that discovery can report precisely what is missing.
///
/// # Errors
/// [`Error::RelativePath`] when `start` is relative, [`Error::NoWorkspace`] when nothing qualifies.
///
/// # Examples
/// ```
/// use std::path::Path;
/// use skif::{MemoryFs, find_workspace_root};
///
/// let fs = MemoryFs::new()
///     .with_file("/repo/package.json", r#"{"workspaces": ["packages/*"]}"#)
///     .with_file("/repo/packages/a/package.json", r#"{"name": "a"}"#);
/// let root = find_workspace_root(Path::new("/repo/packages/a"), &fs)?;
/// assert_eq!(root, Path::new("/repo"));
/// # Ok::<(), skif::Error>(())
/// ```
pub fn find_workspace_root(start: &Path, fs: &dyn FileSystem) -> Result<PathBuf, Error> {
    if !start.has_root() {
        return Err(Error::RelativePath(start.to_path_buf()));
    }
    let first = if fs.is_dir(start) {
        start
    } else {
        start.parent().unwrap_or(start)
    };
    first
        .ancestors()
        .find(|dir| declares_workspace(dir, fs))
        .map(Path::to_path_buf)
        .or_else(|| fs.is_file(&first.join(MANIFEST)).then(|| first.to_path_buf()))
        .ok_or_else(|| Error::NoWorkspace(start.to_path_buf()))
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
