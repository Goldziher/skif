//! Path display normalisation shared by the renderers.

use std::path::Path;

/// Converts a path to a string with `/` separators regardless of platform.
pub(crate) fn normalize(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relativize_table() {
        let cases = [
            ("a\\b\\package.json", None, "a/b/package.json"),
            ("./a/b.json", None, "a/b.json"),
            ("/repo/a/b.json", Some("/repo"), "a/b.json"),
            ("/repo/a/b.json", Some("/repo/"), "a/b.json"),
            ("/repository/a.json", Some("/repo"), "/repository/a.json"),
            ("C:\\repo\\a\\b.json", Some("C:\\repo"), "a/b.json"),
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
