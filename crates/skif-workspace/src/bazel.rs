//! What a Bazel repository's root files say about it, and the convenience links a build leaves at its root.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use skif_core::FileSystem;

/// The prefix of the convenience links a Bazel build creates at the root of the repository: `bazel-bin`,
/// `bazel-out`, `bazel-testlogs` and `bazel-<name>`, the last a link to the execroot, which mirrors the root.
const CONVENIENCE_LINK_PREFIX: &str = "bazel-";

/// Of the root-relative member directories `dirs`, those inside a Bazel convenience link: their first segment is a
/// symbolic link to a directory of `root` whose name starts with `bazel-`. `root` is listed only when a directory
/// starts so.
pub(crate) fn convenience_link_members<'a>(
    fs: &dyn FileSystem,
    root: &Path,
    dirs: impl IntoIterator<Item = &'a Path>,
) -> BTreeSet<PathBuf> {
    let first = |dir: &Path| match dir.components().next() {
        Some(Component::Normal(name)) => name.to_str().map(str::to_owned),
        _ => None,
    };
    let candidates: Vec<(&Path, String)> = dirs
        .into_iter()
        .filter_map(|dir| {
            first(dir)
                .filter(|name| name.starts_with(CONVENIENCE_LINK_PREFIX))
                .map(|name| (dir, name))
        })
        .collect();
    if candidates.is_empty() {
        return BTreeSet::new();
    }
    let links: BTreeSet<String> = fs
        .read_dir_shared(root)
        .unwrap_or_default()
        .iter()
        .filter(|entry| entry.is_symlink && entry.is_dir)
        .filter_map(|entry| entry.path.file_name()?.to_str().map(str::to_owned))
        .collect();
    candidates
        .into_iter()
        .filter(|(_, name)| links.contains(name))
        .map(|(dir, _)| dir.to_path_buf())
        .collect()
}

/// The `name` argument of the `module(...)` call of `MODULE.bazel` in `root`, or else of the `workspace(...)` call of
/// its `WORKSPACE` or `WORKSPACE.bazel`: the name the repository is known by.
#[must_use]
pub fn bazel_workspace_name(fs: &dyn FileSystem, root: &Path) -> Option<String> {
    ["MODULE.bazel", "WORKSPACE", "WORKSPACE.bazel"]
        .iter()
        .find_map(|file| {
            let text = fs.read_to_string(&root.join(file)).ok()?;
            let call = if *file == "MODULE.bazel" {
                "module("
            } else {
                "workspace("
            };
            let at = text.match_indices(call).find_map(|(at, _)| {
                text[..at]
                    .chars()
                    .next_back()
                    .is_none_or(|before| !(before.is_alphanumeric() || before == '_'))
                    .then_some(at)
            })?;
            let body = &text[at + call.len()..];
            let body = &body[..body.find(')')?];
            let at = body.match_indices("name").find_map(|(at, _)| {
                body[..at]
                    .chars()
                    .next_back()
                    .is_none_or(|before| !(before.is_alphanumeric() || before == '_'))
                    .then_some(at)
            })?;
            let value = body[at + "name".len()..].trim_start().strip_prefix('=')?.trim_start();
            let quote = value.chars().next().filter(|quote| matches!(quote, '"' | '\''))?;
            let value = &value[1..];
            let name = &value[..value.find(quote)?];
            (!name.is_empty()).then(|| name.to_owned())
        })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::io;
    use std::sync::Arc;

    use skif_config::Config;
    use skif_core::{DirEntry, MemoryFs};

    use super::*;
    use crate::Workspace;

    /// A tree whose root entries named `bazel-*` are symbolic links, as a Bazel build leaves them.
    struct Linked(MemoryFs);

    impl FileSystem for Linked {
        fn read_to_string(&self, path: &Path) -> io::Result<Arc<str>> {
            self.0.read_to_string(path)
        }

        fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
            let entries = self.0.read_dir(path)?;
            if path != Path::new("/ws") {
                return Ok(entries);
            }
            Ok(entries
                .into_iter()
                .map(|entry| {
                    let linked = entry
                        .path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.starts_with(CONVENIENCE_LINK_PREFIX));
                    if linked { entry.symlink() } else { entry }
                })
                .collect())
        }

        fn is_file(&self, path: &Path) -> bool {
            self.0.is_file(path)
        }

        fn is_dir(&self, path: &Path) -> bool {
            self.0.is_dir(path)
        }
    }

    const ROOT: &str = r#"{"name":"root","private":true}"#;

    fn tree() -> MemoryFs {
        MemoryFs::new()
            .with_file("/ws/package.json", ROOT)
            .with_file("/ws/pnpm-workspace.yaml", "packages: ['*', 'lib/*']\n")
            .with_file("/ws/app/package.json", r#"{"name":"app"}"#)
            .with_file("/ws/bazel-ws/package.json", ROOT)
            .with_file("/ws/bazel-ws/pnpm-workspace.yaml", "packages: ['*', 'lib/*']\n")
            .with_file("/ws/bazel-bin/package.json", ROOT)
            .with_file("/ws/lib/bazel-rules/package.json", r#"{"name":"bazel-rules"}"#)
    }

    #[test]
    fn members_inside_a_bazel_convenience_link_are_facts_and_ignored() {
        let workspace = Workspace::discover_with_fs(Path::new("/ws"), &Config::default(), &Linked(tree())).unwrap();
        let dirs: Vec<&Path> = workspace
            .packages()
            .iter()
            .map(|package| package.dir.as_path())
            .collect();
        assert_eq!(
            dirs,
            ["app", "bazel-bin", "bazel-ws", "lib/bazel-rules"].map(Path::new),
            "pnpm's semantics keep the links as members"
        );
        assert_eq!(
            workspace.facts.bazel_links,
            [PathBuf::from("bazel-bin"), PathBuf::from("bazel-ws")]
        );
        let ignored: Vec<&Path> = workspace
            .packages()
            .iter()
            .filter(|package| workspace.is_ignored(package.id))
            .map(|package| package.dir.as_path())
            .collect();
        assert_eq!(ignored, ["bazel-bin", "bazel-ws"].map(Path::new));
    }

    #[test]
    fn a_real_directory_named_like_a_link_is_an_ordinary_member() {
        let workspace = Workspace::discover_with_fs(Path::new("/ws"), &Config::default(), &tree()).unwrap();
        assert_eq!(workspace.facts.bazel_links, [] as [std::path::PathBuf; 0]);
        assert!(
            workspace
                .packages()
                .iter()
                .all(|package| !workspace.is_ignored(package.id))
        );
    }

    #[test]
    fn bazel_name_is_read_from_the_module_call_only() {
        let fs = MemoryFs::new().with_file(
            "/ws/MODULE.bazel",
            "bazel_dep(name = \"rules_x\", version = \"1\")\nmodule(repo_name = \"r\", name = \"mono\")\n",
        );
        assert_eq!(bazel_workspace_name(&fs, Path::new("/ws")).as_deref(), Some("mono"));
        let none = MemoryFs::new().with_file("/ws/MODULE.bazel", "bazel_dep(name = \"rules_x\")\n");
        assert_eq!(bazel_workspace_name(&none, Path::new("/ws")), None);
    }

    #[test]
    fn a_workspace_file_names_the_repository_too() {
        let fs = MemoryFs::new().with_file("/ws/WORKSPACE", "workspace(name = 'mono')\n");
        assert_eq!(bazel_workspace_name(&fs, Path::new("/ws")).as_deref(), Some("mono"));
    }
}
