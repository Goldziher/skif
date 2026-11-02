//! Displaying paths the same way on every platform.

use std::path::Path;

/// `path` as text with `/` separators on every platform, for messages, titles and machine-readable output.
///
/// Only Windows treats `\` as a separator; elsewhere it is an ordinary file name character and is kept.
#[must_use]
pub fn slash(path: &Path) -> String {
    let text = path.to_string_lossy();
    if cfg!(windows) { text.replace('\\', "/") } else { text.into_owned() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_paths_use_slashes() {
        assert_eq!(slash(Path::new("packages/a/package.json")), "packages/a/package.json");
        assert_eq!(slash(Path::new("")), "");
    }

    #[cfg(windows)]
    #[test]
    fn windows_separators_become_slashes() {
        assert_eq!(slash(Path::new("packages\\a\\package.json")), "packages/a/package.json");
    }
}
