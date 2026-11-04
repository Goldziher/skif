//! Path display normalisation shared by the renderers.

use std::path::Path;

/// Converts a path to a string with `/` separators regardless of platform.
///
/// Only Windows treats `\` as a separator; on other platforms it is an ordinary file name character.
pub(crate) fn normalize(path: &Path) -> String {
    let text = path.to_string_lossy();
    if cfg!(windows) {
        text.replace('\\', "/")
    } else {
        text.into_owned()
    }
}

/// Whether a `/`-separated path string is absolute (`/x`, `C:/x`).
pub(crate) fn is_absolute_text(path: &str) -> bool {
    path.starts_with('/') || matches!(path.as_bytes(), [drive, b':', b'/', ..] if drive.is_ascii_alphabetic())
}

/// Normalises `path` and strips `root` (and a leading `./`) from it when it is a prefix.
pub(crate) fn relativize(path: &Path, root: Option<&Path>) -> String {
    let full = normalize(path);
    let stripped = root
        .map(normalize)
        .map(|root| root.trim_end_matches('/').to_owned())
        .filter(|root| !root.is_empty())
        .and_then(|root| {
            full.strip_prefix(&root)
                .filter(|rest| rest.starts_with('/'))
                .map(|rest| rest.trim_start_matches('/').to_owned())
        })
        .unwrap_or(full);
    stripped
        .strip_prefix("./")
        .map_or_else(|| stripped.clone(), str::to_owned)
}

/// Lexically collapses `.` and `dir/..` components of a relative `/`-separated path, so `web/../skif.toml` reads
/// `skif.toml`; a leading `..` that has nothing to cancel stays. GitHub does not resolve a non-normalised `file=`.
pub(crate) fn collapse_dots(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." if parts.last().is_some_and(|last| *last != "..") => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dots_collapse_lexically() {
        for (path, expected) in [
            ("web/../skif.toml", "skif.toml"),
            ("web/app/../../skif.toml", "skif.toml"),
            ("web/../../skif.toml", "../skif.toml"),
            ("../skif.toml", "../skif.toml"),
            ("./a/./b.json", "a/b.json"),
            ("a//b", "a/b"),
            ("a/b.json", "a/b.json"),
        ] {
            assert_eq!(collapse_dots(path), expected, "{path}");
        }
    }

    #[test]
    fn absolute_text_needs_a_drive_separator() {
        for (path, expected) in [
            ("/x", true),
            ("C:/x", true),
            ("a:b", false),
            ("a:", false),
            ("src/a.json", false),
        ] {
            assert_eq!(is_absolute_text(path), expected, "{path}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_separators_become_slashes() {
        assert_eq!(normalize(Path::new("a\\b\\package.json")), "a/b/package.json");
        assert_eq!(
            relativize(Path::new("C:\\repo\\a\\b.json"), Some(Path::new("C:\\repo"))),
            "a/b.json"
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn backslashes_are_file_name_characters_elsewhere() {
        assert_eq!(normalize(Path::new("a\\b.json")), "a\\b.json");
    }

    #[test]
    fn relativize_table() {
        let cases = [
            ("./a/b.json", None, "a/b.json"),
            ("/repo/a/b.json", Some("/repo"), "a/b.json"),
            ("/repo/a/b.json", Some("/repo/"), "a/b.json"),
            ("/repository/a.json", Some("/repo"), "/repository/a.json"),
            ("C:/repo/a/b.json", Some("C:/repo"), "a/b.json"),
            ("a/b.json", Some("/repo"), "a/b.json"),
            ("a/b.json", Some(""), "a/b.json"),
        ];
        for (path, root, expected) in cases {
            assert_eq!(
                relativize(Path::new(path), root.map(Path::new)),
                expected,
                "{path} {root:?}"
            );
        }
    }
}
