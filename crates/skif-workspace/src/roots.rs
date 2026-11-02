//! Finding every workspace root and standalone package below a directory.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ignore::Match;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use skif_config::Config;
use skif_core::FileSystem;

use crate::workspace::Workspace;

const MANIFEST: &str = "package.json";
const PNPM_WORKSPACE: &str = "pnpm-workspace.yaml";
const IGNORE_FILES: [&str; 2] = [".gitignore", ".ignore"];

/// A project root found by [`discover_roots`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
pub enum RootKind {
    /// A directory declaring a workspace: a `package.json` with `workspaces`, or a `pnpm-workspace.yaml`.
    Workspace(PathBuf),
    /// A `package.json` that belongs to no workspace found below the start directory.
    Package(PathBuf),
}

impl RootKind {
    /// The root directory.
    #[must_use]
    pub fn path(&self) -> &Path {
        match self {
            Self::Workspace(path) | Self::Package(path) => path,
        }
    }
}

/// Bounds for [`discover_roots`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct DiscoverOptions {
    /// Directories deeper than this below the start directory are not entered.
    pub max_depth: usize,
}

impl Default for DiscoverOptions {
    fn default() -> Self {
        Self { max_depth: 32 }
    }
}

impl DiscoverOptions {
    /// Sets [`max_depth`](Self::max_depth).
    #[must_use]
    pub const fn with_max_depth(mut self, max_depth: usize) -> Self {
        self.max_depth = max_depth;
        self
    }
}

fn declares_workspace(fs: &dyn FileSystem, dir: &Path) -> bool {
    if fs.is_file(&dir.join(PNPM_WORKSPACE)) {
        return true;
    }
    fs.read_to_string(&dir.join(MANIFEST))
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .is_some_and(|manifest| manifest.get("workspaces").is_some())
}

fn ignore_matcher(fs: &dyn FileSystem, dir: &Path) -> Option<Gitignore> {
    let mut builder = GitignoreBuilder::new(dir);
    let mut any = false;
    for name in IGNORE_FILES {
        let path = dir.join(name);
        let Ok(text) = fs.read_to_string(&path) else { continue };
        for line in text.lines() {
            any |= builder.add_line(Some(path.clone()), line).is_ok();
        }
    }
    if any { builder.build().ok() } else { None }
}

fn is_ignored(matchers: &[Arc<Gitignore>], path: &Path) -> bool {
    for matcher in matchers.iter().rev() {
        match matcher.matched(path, true) {
            Match::Ignore(_) => return true,
            Match::Whitelist(_) => return false,
            Match::None => {}
        }
    }
    false
}

#[derive(Default)]
struct Scan {
    packages: BTreeSet<PathBuf>,
    workspaces: BTreeSet<PathBuf>,
}

fn scan(fs: &dyn FileSystem, start: &Path, max_depth: usize) -> Scan {
    let mut found = Scan::default();
    let mut stack: Vec<(PathBuf, usize, Vec<Arc<Gitignore>>)> = vec![(start.to_path_buf(), 0, Vec::new())];
    while let Some((dir, depth, mut matchers)) = stack.pop() {
        if let Some(matcher) = ignore_matcher(fs, &dir) {
            matchers.push(Arc::new(matcher));
        }
        if fs.is_file(&dir.join(MANIFEST)) {
            found.packages.insert(dir.clone());
        }
        if declares_workspace(fs, &dir) {
            found.workspaces.insert(dir.clone());
        }
        if depth >= max_depth {
            continue;
        }
        let Ok(entries) = fs.read_dir(&dir) else { continue };
        for entry in entries {
            let hidden_or_modules = entry
                .path
                .file_name()
                .is_some_and(|name| name == "node_modules" || name.to_string_lossy().starts_with('.'));
            if entry.is_dir && !entry.is_symlink && !hidden_or_modules && !is_ignored(&matchers, &entry.path) {
                stack.push((entry.path, depth + 1, matchers.clone()));
            }
        }
    }
    found
}

/// The member directories of the workspace at `root`; empty when it cannot be discovered (the lint reports why).
fn member_dirs(fs: &dyn FileSystem, root: &Path) -> BTreeSet<PathBuf> {
    Workspace::discover_with_fs(root, &Config::default(), fs)
        .map(|workspace| workspace.packages.iter().map(|package| root.join(&package.dir)).collect())
        .unwrap_or_default()
}

/// Finds every workspace root and standalone package at or below `start`, sorted by path.
///
/// `start` must be a directory path usable through `fs` (absolute for a real file system). The walk skips
/// `node_modules`, hidden directories, symbolic links and directories excluded by a `.gitignore` or `.ignore`
/// found at or below `start`.
///
/// A directory is a [`RootKind::Workspace`] when it declares a workspace and is not a member of an outer workspace
/// found by the walk; a workspace nested in an outer one that does not list it as a member is a root of its own.
/// A directory is a [`RootKind::Package`] when it holds a `package.json`, declares no workspace, is not a member
/// of a found workspace and does not lie below a member. Ancestors of `start` are not consulted.
///
/// # Examples
/// ```
/// use std::path::Path;
/// use skif_workspace::{DiscoverOptions, MemoryFs, RootKind, discover_roots};
///
/// let fs = MemoryFs::new()
///     .with_file("/r/a/package.json", r#"{"workspaces": ["p/*"]}"#)
///     .with_file("/r/a/p/x/package.json", r#"{"name": "x"}"#)
///     .with_file("/r/b/package.json", r#"{"name": "b"}"#);
/// let roots = discover_roots(Path::new("/r"), &fs, &DiscoverOptions::default());
/// assert_eq!(
///     roots,
///     vec![RootKind::Workspace("/r/a".into()), RootKind::Package("/r/b".into())]
/// );
/// ```
#[must_use]
pub fn discover_roots(start: &Path, fs: &dyn FileSystem, options: &DiscoverOptions) -> Vec<RootKind> {
    let found = scan(fs, start, options.max_depth);
    let mut members: BTreeSet<PathBuf> = BTreeSet::new();
    let mut roots = Vec::new();
    // Sorted paths put an outer workspace before everything it contains.
    for workspace in &found.workspaces {
        if members.contains(workspace) {
            continue;
        }
        members.extend(member_dirs(fs, workspace));
        roots.push(RootKind::Workspace(workspace.clone()));
    }
    for package in &found.packages {
        let owned = members.iter().any(|member| package.starts_with(member));
        if !owned && !found.workspaces.contains(package) {
            roots.push(RootKind::Package(package.clone()));
        }
    }
    roots.sort_by(|a, b| a.path().cmp(b.path()));
    roots
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use skif_core::MemoryFs;

    use super::*;

    fn paths(roots: &[RootKind]) -> Vec<String> {
        roots
            .iter()
            .map(|root| {
                let kind = if matches!(root, RootKind::Workspace(_)) { "ws" } else { "pkg" };
                format!("{kind}:{}", root.path().display())
            })
            .collect()
    }

    fn discover(fs: &MemoryFs) -> Vec<String> {
        paths(&discover_roots(Path::new("/r"), fs, &DiscoverOptions::default()))
    }

    #[test]
    fn siblings_are_separate_roots_in_path_order() {
        let fs = MemoryFs::new()
            .with_file("/r/z/package.json", r#"{"name": "z"}"#)
            .with_file("/r/a/pnpm-workspace.yaml", "packages:\n  - 'p/*'\n")
            .with_file("/r/a/package.json", r#"{"name": "a"}"#)
            .with_file("/r/a/p/m/package.json", r#"{"name": "m"}"#)
            .with_file("/r/b/package.json", r#"{"workspaces": ["p/*"]}"#)
            .with_file("/r/b/p/n/package.json", r#"{"name": "n"}"#);
        assert_eq!(discover(&fs), ["ws:/r/a", "ws:/r/b", "pkg:/r/z"]);
    }

    #[test]
    fn members_are_never_roots() {
        let fs = MemoryFs::new()
            .with_file("/r/package.json", r#"{"workspaces": ["packages/*"]}"#)
            .with_file("/r/packages/a/package.json", r#"{"name": "a"}"#)
            .with_file("/r/packages/a/nested/package.json", r#"{"name": "nested"}"#);
        assert_eq!(discover(&fs), ["ws:/r"]);
    }

    #[test]
    fn nested_workspace_outside_the_member_set_is_its_own_root() {
        let fs = MemoryFs::new()
            .with_file("/r/package.json", r#"{"workspaces": ["packages/*"]}"#)
            .with_file("/r/packages/a/package.json", r#"{"name": "a"}"#)
            .with_file("/r/tools/pnpm-workspace.yaml", "packages:\n  - 'p/*'\n")
            .with_file("/r/tools/package.json", r#"{"name": "tools"}"#)
            .with_file("/r/tools/p/t/package.json", r#"{"name": "t"}"#);
        assert_eq!(discover(&fs), ["ws:/r", "ws:/r/tools"]);
    }

    #[test]
    fn workspace_listed_as_a_member_is_not_a_root() {
        let fs = MemoryFs::new()
            .with_file("/r/package.json", r#"{"workspaces": ["packages/*"]}"#)
            .with_file("/r/packages/inner/package.json", r#"{"workspaces": ["x/*"]}"#)
            .with_file("/r/packages/inner/x/y/package.json", r#"{"name": "y"}"#);
        assert_eq!(discover(&fs), ["ws:/r"]);
    }

    #[test]
    fn package_outside_the_members_is_standalone() {
        let fs = MemoryFs::new()
            .with_file("/r/package.json", r#"{"workspaces": ["packages/*"]}"#)
            .with_file("/r/packages/a/package.json", r#"{"name": "a"}"#)
            .with_file("/r/examples/e/package.json", r#"{"name": "e"}"#);
        assert_eq!(discover(&fs), ["ws:/r", "pkg:/r/examples/e"]);
    }

    #[test]
    fn node_modules_hidden_and_gitignored_directories_are_skipped() {
        let fs = MemoryFs::new()
            .with_file("/r/.gitignore", "generated/\n")
            .with_file("/r/app/package.json", r#"{"name": "app"}"#)
            .with_file("/r/app/node_modules/dep/package.json", r#"{"name": "dep"}"#)
            .with_file("/r/.hidden/package.json", r#"{"name": "hidden"}"#)
            .with_file("/r/generated/package.json", r#"{"name": "generated"}"#)
            .with_file("/r/sub/.gitignore", "cache\n")
            .with_file("/r/sub/cache/package.json", r#"{"name": "cache"}"#)
            .with_file("/r/sub/kept/package.json", r#"{"name": "kept"}"#);
        assert_eq!(discover(&fs), ["pkg:/r/app", "pkg:/r/sub/kept"]);
    }

    #[test]
    fn start_itself_can_be_the_root() {
        let fs = MemoryFs::new().with_file("/r/package.json", r#"{"name": "solo"}"#);
        assert_eq!(discover(&fs), ["pkg:/r"]);
    }

    #[test]
    fn nothing_found_is_empty() {
        let fs = MemoryFs::new().with_file("/r/readme.md", "x");
        assert!(discover(&fs).is_empty());
    }

    #[test]
    fn max_depth_bounds_the_walk() {
        let fs = MemoryFs::new().with_file("/r/a/b/c/package.json", r#"{"name": "deep"}"#);
        let shallow = discover_roots(Path::new("/r"), &fs, &DiscoverOptions::default().with_max_depth(2));
        assert!(shallow.is_empty());
        assert_eq!(discover(&fs), ["pkg:/r/a/b/c"]);
    }

    #[test]
    fn broken_workspace_is_still_a_root() {
        let fs = MemoryFs::new().with_file("/r/w/package.json", r#"{"workspaces": ["p/*"]"#);
        let roots = discover_roots(Path::new("/r"), &fs, &DiscoverOptions::default());
        assert_eq!(roots.len(), 1);
    }
}
