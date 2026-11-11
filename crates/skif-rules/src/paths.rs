//! Workspace-relative paths written with `/` separators, `""` for the root.

/// `name` in the directory `dir`.
pub(crate) fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    }
}

/// The directory of `path`, `""` at the root.
pub(crate) fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

/// The last segment of `path`.
pub(crate) fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_join_and_split_at_slashes() {
        assert_eq!(join("", "a"), "a");
        assert_eq!(join("libs/a", "pnpm-lock.yaml"), "libs/a/pnpm-lock.yaml");
        assert_eq!(parent("libs/a/pnpm-lock.yaml"), "libs/a");
        assert_eq!(parent("pnpm-lock.yaml"), "");
        assert_eq!(file_name("libs/a/pnpm-lock.yaml"), "pnpm-lock.yaml");
        assert_eq!(file_name("BUILD"), "BUILD");
    }
}
