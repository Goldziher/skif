//! The package manifests above the workspace root, which list tools the packages below may use.

use std::path::{Path, PathBuf};

use serde_json::Value;
use skif_core::FileSystem;

const SECTIONS: [&str; 4] = [
    "dependencies",
    "devDependencies",
    "optionalDependencies",
    "peerDependencies",
];

/// Whether `dir` is where the repository starts: it holds the `.git` entry.
pub(crate) fn is_repository_root(fs: &dyn FileSystem, dir: &Path) -> bool {
    let kind = fs.kind(&dir.join(".git"));
    kind.is_dir || kind.is_file
}

/// The repository root at or above `dir`: the nearest directory holding `.git`, `None` when there is none.
pub(crate) fn repository_root(fs: &dyn FileSystem, dir: &Path) -> Option<PathBuf> {
    dir.ancestors()
        .find(|candidate| is_repository_root(fs, candidate))
        .map(Path::to_path_buf)
}

/// The parsed `package.json` of every directory above `root`, up to and including the repository root, innermost
/// first. Directories without a readable manifest are skipped.
pub(crate) fn external_manifests(fs: &dyn FileSystem, root: &Path) -> Vec<(PathBuf, Value)> {
    external_manifests_up_to(fs, root, repository_root(fs, root).as_deref())
}

/// [`external_manifests`] for a caller that knows the repository root `git_root` (`None` outside a repository, where
/// every directory above `root` counts), probing no `.git`.
pub(crate) fn external_manifests_up_to(
    fs: &dyn FileSystem,
    root: &Path,
    git_root: Option<&Path>,
) -> Vec<(PathBuf, Value)> {
    if git_root == Some(root) {
        return Vec::new();
    }
    let mut manifests = Vec::new();
    for dir in root.ancestors().skip(1) {
        if let Some(manifest) = read_manifest(fs, dir) {
            manifests.push((dir.to_path_buf(), manifest));
        }
        if git_root == Some(dir) {
            break;
        }
    }
    manifests
}

/// The parsed `package.json` of `dir`, when it is readable.
pub(crate) fn read_manifest(fs: &dyn FileSystem, dir: &Path) -> Option<Value> {
    fs.read_to_string(&dir.join("package.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
}

/// The names `manifest` lists in any dependency section.
pub(crate) fn listed_names(manifest: &Value) -> impl Iterator<Item = &str> {
    SECTIONS
        .into_iter()
        .filter_map(|section| manifest.get(section).and_then(Value::as_object))
        .flat_map(|listed| listed.keys().map(String::as_str))
}

/// The sorted, distinct dependency names of every manifest above `root`, up to the repository root `git_root`.
pub(crate) fn external_dependencies(fs: &dyn FileSystem, root: &Path, git_root: Option<&Path>) -> Vec<String> {
    let mut names: Vec<String> = external_manifests_up_to(fs, root, git_root)
        .iter()
        .flat_map(|(_, manifest)| listed_names(manifest).map(str::to_owned))
        .collect();
    names.sort();
    names.dedup();
    names
}
