//! Path helpers shared by the facade and its hosts.

use std::path::{Component, Path, PathBuf};

/// `base/path` with every `..` segment resolved against the segment before it, lexically and without reading the file
/// system, so the paths by which different roots reach one file compare equal.
///
/// A `..` with nothing left to remove is kept, so a relative `base` may resolve to a path above it.
///
/// ```
/// use std::path::Path;
/// use skif::join_lexical;
///
/// assert_eq!(join_lexical(Path::new("/r/tools/a"), Path::new("../../skif.toml")), Path::new("/r/skif.toml"));
/// assert_eq!(join_lexical(Path::new(""), Path::new("../skif.toml")), Path::new("../skif.toml"));
/// ```
#[must_use]
pub fn join_lexical(base: &Path, path: &Path) -> PathBuf {
    let mut joined = PathBuf::new();
    for component in base.components().chain(path.components()) {
        match component {
            Component::ParentDir if joined.pop() => {}
            other => joined.push(other),
        }
    }
    joined
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_lexical_resolves_parent_segments() {
        let cases = [
            ("/r/tools/a", "../../../etc/skif.toml", "/etc/skif.toml"),
            ("/r", "../etc/skif.toml", "/etc/skif.toml"),
            ("/r/tools/a", "", "/r/tools/a"),
            ("a/b", "package.json", "a/b/package.json"),
            ("a", "../skif.toml", "skif.toml"),
            ("", "x", "x"),
            ("", "../skif.toml", "../skif.toml"),
        ];
        for (base, path, expected) in cases {
            assert_eq!(
                join_lexical(Path::new(base), Path::new(path)),
                Path::new(expected),
                "{base} + {path}"
            );
        }
    }
}
