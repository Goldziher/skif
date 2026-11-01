use std::path::{Path, PathBuf};

use crate::error::ConfigError;
use crate::model::Config;

/// Accepted configuration file names, in lookup order.
pub const CONFIG_FILE_NAMES: [&str; 2] = ["skif.toml", ".skif.toml"];

/// Looks for `skif.toml` or `.skif.toml` directly in `root`.
///
/// # Errors
/// [`ConfigError::Ambiguous`] when both exist, plus any error from loading the file.
pub fn discover(root: &Path) -> Result<Option<(PathBuf, Config)>, ConfigError> {
    let found: Vec<PathBuf> = CONFIG_FILE_NAMES.iter().map(|name| root.join(name)).filter(|path| path.is_file()).collect();
    match found.as_slice() {
        [] => Ok(None),
        [path] => Ok(Some((path.clone(), Config::from_toml_file(path)?))),
        [first, second, ..] => Err(ConfigError::Ambiguous { first: first.clone(), second: second.clone() }),
    }
}
