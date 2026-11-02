//! Displaying paths the same way on every platform.

use std::path::Path;

use serde::Serializer;

/// `path` as text with `/` separators on every platform, for messages, titles and machine-readable output.
///
/// Only Windows treats `\` as a separator; elsewhere it is an ordinary file name character and is kept.
#[must_use]
pub fn slash(path: &Path) -> String {
    let text = path.to_string_lossy();
    if cfg!(windows) { text.replace('\\', "/") } else { text.into_owned() }
}

/// A `serde` serializer for workspace-relative paths, so serialized reports and plans read the same on every
/// platform.
///
/// # Errors
/// Whatever `serializer` reports.
pub fn serialize_path<S: Serializer>(path: &Path, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&slash(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_paths_use_slashes() {
        assert_eq!(slash(Path::new("packages/a/package.json")), "packages/a/package.json");
        assert_eq!(slash(Path::new("")), "");
    }

    #[test]
    fn serialized_paths_use_slashes() {
        #[derive(serde::Serialize)]
        struct Holder {
            #[serde(serialize_with = "serialize_path")]
            path: std::path::PathBuf,
        }
        let json = serde_json::to_string(&Holder {
            path: Path::new("packages").join("a").join("package.json"),
        })
        .unwrap();
        assert_eq!(json, r#"{"path":"packages/a/package.json"}"#);
    }

    #[cfg(windows)]
    #[test]
    fn windows_separators_become_slashes() {
        assert_eq!(slash(Path::new("packages\\a\\package.json")), "packages/a/package.json");
    }
}
