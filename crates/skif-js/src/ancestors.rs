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

/// The parsed `package.json` of every directory above `root`, up to and including the repository root, innermost
/// first. Directories without a readable manifest are skipped.
pub(crate) fn external_manifests(fs: &dyn FileSystem, root: &Path) -> Vec<(PathBuf, Value)> {
    let mut manifests = Vec::new();
    let mut dir = root;
    while !is_repository_root(fs, dir) {
        let Some(parent) = dir.parent() else { break };
        dir = parent;
        if let Some(manifest) = fs
            .read_to_string(&dir.join("package.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        {
            manifests.push((dir.to_path_buf(), manifest));
        }
    }
    manifests
}

/// The names `manifest` lists in any dependency section.
pub(crate) fn listed_names(manifest: &Value) -> impl Iterator<Item = &str> {
    SECTIONS
        .into_iter()
        .filter_map(|section| manifest.get(section).and_then(Value::as_object))
        .flat_map(|listed| listed.keys().map(String::as_str))
}

/// The sorted, distinct dependency names of every manifest above `root`.
pub(crate) fn external_dependencies(fs: &dyn FileSystem, root: &Path) -> Vec<String> {
    let mut names: Vec<String> = external_manifests(fs, root)
        .iter()
        .flat_map(|(_, manifest)| listed_names(manifest).map(str::to_owned))
        .collect();
    names.sort();
    names.dedup();
    names
}
