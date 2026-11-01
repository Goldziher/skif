//! Finds the source files of the workspace and assigns each to the package that owns it.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use skif_core::FileSystem;
use skif_workspace::{PackageId, Workspace};

use crate::resolve::is_code_file;

/// Directories that never hold analysed sources.
const SKIPPED_DIRS: &[&str] = &["node_modules", ".git", "dist", "build", "coverage"];

const MAX_DEPTH: usize = 48;

/// A source file on disk.
#[derive(Debug, Clone)]
pub(crate) struct Candidate {
    pub abs: PathBuf,
    /// Relative to the workspace root, `/`-separated.
    pub rel: String,
    /// The package whose directory is the closest ancestor.
    pub owner: PackageId,
}

/// The path relative to `base`, `/`-separated; `None` when `path` is not below `base`.
pub(crate) fn relative(path: &Path, base: &Path) -> Option<String> {
    let rest = path.strip_prefix(base).ok()?;
    Some(rest.to_string_lossy().replace('\\', "/"))
}

/// Lists every `.js`/`.ts`-family file below the workspace root, sorted by relative path.
///
/// Files of a nested package belong to that package; packages excluded by `ignore-packages` contribute nothing.
pub(crate) fn discover(fs: &dyn FileSystem, workspace: &Workspace) -> Vec<Candidate> {
    let mut owners: HashMap<PathBuf, PackageId> = HashMap::new();
    let mut skipped: HashSet<PathBuf> = HashSet::new();
    for (id, package) in workspace.iter().filter(|(_, package)| !package.is_root) {
        let dir = workspace.root.join(&package.dir);
        if workspace.is_ignored(id) {
            skipped.insert(dir);
        } else {
            owners.insert(dir, id);
        }
    }

    let mut found = Vec::new();
    let mut stack = vec![(workspace.root.clone(), PackageId::ROOT, 0_usize)];
    while let Some((dir, owner, depth)) = stack.pop() {
        let Ok(entries) = fs.read_dir(&dir) else { continue };
        for entry in entries {
            let name = entry.path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
            if entry.is_dir {
                if depth >= MAX_DEPTH || SKIPPED_DIRS.contains(&name) || skipped.contains(&entry.path) {
                    continue;
                }
                let child_owner = owners.get(&entry.path).copied().unwrap_or(owner);
                stack.push((entry.path, child_owner, depth + 1));
            } else if entry.is_file
                && is_code_file(&entry.path)
                && let Some(rel) = relative(&entry.path, &workspace.root)
            {
                found.push(Candidate {
                    abs: entry.path,
                    rel,
                    owner,
                });
            }
        }
    }
    found.sort_by(|a, b| a.rel.cmp(&b.rel));
    found
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use skif_config::Config;
    use skif_core::MemoryFs;

    use super::*;

    #[test]
    fn owners_skips_and_sorting() {
        let fs = MemoryFs::new()
            .with_file("/ws/package.json", r#"{"name":"r","workspaces":["packages/*"]}"#)
            .with_file("/ws/packages/a/package.json", r#"{"name":"a"}"#)
            .with_file("/ws/packages/a/src/z.ts", "")
            .with_file("/ws/packages/a/dist/built.js", "")
            .with_file("/ws/packages/a/node_modules/x/index.js", "")
            .with_file("/ws/tools/run.js", "")
            .with_file("/ws/readme.md", "");
        let config = Config::from_toml_str("").unwrap();
        let workspace = Workspace::discover_with_fs(Path::new("/ws"), &config, &fs).unwrap();
        let files = discover(&fs, &workspace);
        let listed: Vec<(&str, usize)> = files.iter().map(|c| (c.rel.as_str(), c.owner.index())).collect();
        assert_eq!(listed, [("packages/a/src/z.ts", 1), ("tools/run.js", 0)]);
    }
}
