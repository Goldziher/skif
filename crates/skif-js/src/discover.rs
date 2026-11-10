//! Finds the source files of the workspace and assigns each to the package that owns it.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ignore::Match;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use skif_core::{DirEntry, FileSystem};
use skif_workspace::{PackageId, Workspace, map_ordered};

use crate::git_index::{common_dir, locate};
use crate::resolve::is_code_file;

/// Directories that never hold analysed sources: dependencies, VCS data and common build output.
///
/// `build` is not listed: it is often a source directory, so it is only skipped when a `.gitignore` says so.
const SKIPPED_DIRS: &[&str] = &[
    "node_modules",
    ".git",
    "dist",
    "coverage",
    ".next",
    "out",
    ".turbo",
    ".svelte-kit",
    "storybook-static",
    ".yarn",
    ".vercel",
    ".nuxt",
    "target",
];

/// Directory names that are build or tool output at the top of a package but a legitimate source folder name
/// further down (`src/out/`, `lib/target/`): they are skipped only directly inside a package directory.
const PACKAGE_ROOT_ONLY_DIRS: &[&str] = &["out", "target", "coverage"];

/// Hidden directories that hold tool configuration and sources; every other hidden directory is skipped.
const HIDDEN_DIRS: &[&str] = &[
    ".storybook",
    ".rnstorybook",
    ".config",
    ".vitepress",
    ".vuepress",
    ".husky",
    ".github",
    ".changeset",
];

/// Ignore files honoured like the `ignore` crate does, in increasing priority.
const IGNORE_FILES: &[&str] = &[".gitignore", ".ignore"];

/// Files package managers generate next to the manifest; they are never project source.
const GENERATED_FILES: &[&str] = &[".pnp.cjs", ".pnp.js", ".pnp.loader.mjs"];

const MAX_DEPTH: usize = 48;

/// A source file on disk.
#[derive(Debug, Clone)]
pub(crate) struct Candidate {
    pub abs: PathBuf,
    /// Relative to the workspace root, `/`-separated.
    pub rel: String,
    /// The package whose directory is the closest ancestor.
    pub owner: PackageId,
    /// The path is a symbolic link.
    pub is_link: bool,
}

/// The path relative to `base`, `/`-separated; `None` when `path` is not below `base`.
pub(crate) fn relative(path: &Path, base: &Path) -> Option<String> {
    let rest = path.strip_prefix(base).ok()?;
    Some(rest.to_string_lossy().replace('\\', "/"))
}

/// Whether the listing holds a regular file called `name`.
fn lists_file(entries: &[DirEntry], name: &str) -> bool {
    entries
        .iter()
        .any(|entry| entry.is_file && entry.path.file_name().is_some_and(|candidate| candidate == name))
}

/// Fields a `package.json` may hold and still be only a marker for the files below it (module type, side effects,
/// subpath imports), not a package of its own.
const MARKER_FIELDS: &[&str] = &["type", "sideEffects", "imports"];

/// Whether `dir/package.json` is a real package: a manifest holding only marker fields such as `{"type":"module"}`
/// is not. An unreadable or invalid manifest counts as a package.
pub(crate) fn is_package_manifest(fs: &dyn FileSystem, dir: &Path) -> bool {
    let Ok(text) = fs.read_to_string(&dir.join("package.json")) else {
        return true;
    };
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(serde_json::Value::Object(fields)) => fields.keys().any(|key| !MARKER_FIELDS.contains(&key.as_str())),
        _ => true,
    }
}

/// The matchers of the ignore files listed in `entries`, the listing of `dir`, if any.
///
/// Only files the listing names are opened, so a directory without ignore files costs no `open`.
fn ignore_file_matcher(fs: &dyn FileSystem, dir: &Path, entries: &[DirEntry], gitignore: bool) -> Option<Gitignore> {
    let mut builder = GitignoreBuilder::new(dir);
    let mut any = false;
    for name in IGNORE_FILES
        .iter()
        .filter(|name| gitignore || **name != ".gitignore")
        .filter(|name| lists_file(entries, name))
    {
        let path = dir.join(name);
        let Ok(text) = fs.read_to_string(&path) else { continue };
        for line in text.lines() {
            any |= builder.add_line(Some(path.clone()), line).is_ok();
        }
    }
    if any { builder.build().ok() } else { None }
}

/// Whether the closest matcher with an opinion ignores `path`.
fn is_ignored(matchers: &[Arc<Gitignore>], path: &Path, is_dir: bool) -> bool {
    for matcher in matchers.iter().rev() {
        match matcher.matched(path, is_dir) {
            Match::Ignore(_) => return true,
            Match::Whitelist(_) => return false,
            Match::None => {}
        }
    }
    false
}

/// Whether the ignore files (`.gitignore`, `.ignore`) from `root` down to the directory of `path` ignore `path` or
/// one of its directories below `root`, as the walk of [`discover`] would. `path` is absolute and below `root`.
pub(crate) fn is_ignored_path(fs: &dyn FileSystem, root: &Path, path: &Path) -> bool {
    ignored_below(fs, root, path, Vec::new())
}

/// Whether git ignores `path`, which is absolute and below `root`: the ignore files from the work tree `root` belongs
/// to down to the directory of `path`, including those between the work tree and `root` that the walk of
/// [`discover`] does not read, and the repository's `info/exclude`. Outside a work tree only the ignore files from
/// `root` down count, as for [`is_ignored_path`]. The global excludes file (`core.excludesFile`) is not read.
pub(crate) fn is_ignored_in_repository(fs: &dyn FileSystem, root: &Path, path: &Path) -> bool {
    let Some((work_tree, git_dir)) = locate(fs, root) else {
        return is_ignored_path(fs, root, path);
    };
    let exclude = common_dir(fs, &git_dir).join("info").join("exclude");
    let mut builder = GitignoreBuilder::new(&work_tree);
    let mut any = false;
    if let Ok(text) = fs.read_to_string(&exclude) {
        for line in text.lines() {
            any |= builder.add_line(Some(exclude.clone()), line).is_ok();
        }
    }
    let matchers = if any {
        builder.build().ok().map(Arc::new).into_iter().collect()
    } else {
        Vec::new()
    };
    ignored_below(fs, &work_tree, path, matchers)
}

/// Whether `matchers` or the ignore files from `top` down to the directory of `path` ignore `path` or one of its
/// directories below `top`.
fn ignored_below(fs: &dyn FileSystem, top: &Path, path: &Path, mut matchers: Vec<Arc<Gitignore>>) -> bool {
    let Ok(rest) = path.strip_prefix(top) else {
        return false;
    };
    let mut dir = top.to_path_buf();
    let parts: Vec<_> = rest.components().collect();
    for (index, part) in parts.iter().enumerate() {
        if let Ok(entries) = fs.read_dir_shared(&dir)
            && let Some(matcher) = ignore_file_matcher(fs, &dir, &entries, true)
        {
            matchers.push(Arc::new(matcher));
        }
        let next = dir.join(part);
        if is_ignored(&matchers, &next, index + 1 < parts.len()) {
            return true;
        }
        dir = next;
    }
    false
}

/// Whether a directory is skipped by name, unless it is a workspace package.
fn skipped_by_name(name: &str) -> bool {
    SKIPPED_DIRS.contains(&name) || (name.starts_with('.') && !HIDDEN_DIRS.contains(&name))
}

struct Frame {
    dir: PathBuf,
    owner: PackageId,
    depth: usize,
    matchers: Vec<Arc<Gitignore>>,
    /// The directory is skipped, with everything below it, when it holds its own `package.json`.
    skip_if_project: bool,
    /// The directory is walked only for the workspace packages below it (a hidden directory or build output that
    /// holds members, such as `.vscode/extensions/*`): its own files and the other directories in it are skipped.
    members_only: bool,
}

/// What the walk shares between directories.
struct Walk<'a> {
    fs: &'a dyn FileSystem,
    root: &'a Path,
    owners: HashMap<PathBuf, PackageId>,
    /// The directories below the root that hold a workspace package further down.
    member_ancestors: HashSet<PathBuf>,
    skipped: HashSet<PathBuf>,
    sequential: bool,
    /// Honour `.gitignore` files; `.ignore` files are honoured either way.
    gitignore: bool,
}

impl Walk<'_> {
    /// Whether a directory called `name` inside `parent` is skipped by name: the names that can also be source
    /// folders are skipped only directly inside a package directory.
    fn skips_here(&self, parent: &Path, name: &str) -> bool {
        !PACKAGE_ROOT_ONLY_DIRS.contains(&name) || parent == self.root || self.owners.contains_key(parent)
    }

    /// The source files below `frame`, subdirectories walked in parallel unless `sequential`.
    fn directory(&self, frame: &Frame) -> Vec<Candidate> {
        let Ok(entries) = self.fs.read_dir_shared(&frame.dir) else {
            return Vec::new();
        };
        if frame.skip_if_project && lists_file(&entries, "package.json") && is_package_manifest(self.fs, &frame.dir) {
            return Vec::new();
        }
        let mut matchers = frame.matchers.clone();
        if let Some(matcher) = ignore_file_matcher(self.fs, &frame.dir, &entries, self.gitignore) {
            matchers.push(Arc::new(matcher));
        }
        let mut found = Vec::new();
        let mut children = Vec::new();
        for entry in &*entries {
            let name = entry
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            if entry.is_dir {
                let package = self.owners.get(&entry.path).copied();
                let holds_members = package.is_none() && self.member_ancestors.contains(&entry.path);
                let by_name = package.is_none() && skipped_by_name(name) && self.skips_here(&frame.dir, name);
                if frame.depth >= MAX_DEPTH
                    || entry.is_symlink
                    || self.skipped.contains(&entry.path)
                    || ((by_name || frame.members_only) && !holds_members && package.is_none())
                    || is_ignored(&matchers, &entry.path, true)
                {
                    continue;
                }
                children.push(Frame {
                    dir: entry.path.clone(),
                    owner: package.unwrap_or(frame.owner),
                    depth: frame.depth + 1,
                    matchers: matchers.clone(),
                    skip_if_project: package.is_none(),
                    members_only: package.is_none() && (by_name || frame.members_only),
                });
            } else if entry.is_file
                && !frame.members_only
                && !GENERATED_FILES.contains(&name)
                && is_code_file(&entry.path)
                && !is_ignored(&matchers, &entry.path, false)
                && let Some(rel) = relative(&entry.path, self.root)
            {
                found.push(Candidate {
                    abs: entry.path.clone(),
                    rel,
                    owner: frame.owner,
                    is_link: entry.is_symlink,
                });
            }
        }
        found.extend(
            map_ordered(&children, self.sequential, |child| self.directory(child))
                .into_iter()
                .flatten(),
        );
        found
    }
}

/// Lists every `.js`/`.ts`-family file below the workspace root, sorted by relative path.
///
/// Files of a nested package belong to that package; packages excluded by `ignore-packages` contribute nothing.
/// A directory with its own `package.json` that is not a member of the workspace is a separate project: it and
/// everything below it are skipped. A manifest without `name`, `workspaces` or dependency fields only marks the
/// directory (`{"type":"module"}`) and does not count.
/// `.gitignore` and `.ignore` files are honoured with the semantics of the `ignore` crate, hidden directories
/// (except tool configuration ones) and common build output are skipped, and symbolic links to directories are
/// not followed. A skipped directory that holds workspace packages (`.vscode/extensions/*` listed as members) is
/// walked for those packages only.
///
/// Directories are listed once each, and the ignore files and manifests are found in the listing instead of probed.
/// Sibling directories are walked in parallel (on the calling thread with `sequential`); the sort makes the result
/// independent of scheduling.
#[cfg(test)]
pub(crate) fn discover(fs: &dyn FileSystem, workspace: &Workspace, sequential: bool) -> Vec<Candidate> {
    discover_with(fs, workspace, sequential, true)
}

/// Like [`discover`]; without `gitignore` the `.gitignore` files are not read, so the files they hide are found too
/// (`skif --no-gitignore`).
pub(crate) fn discover_with(
    fs: &dyn FileSystem,
    workspace: &Workspace,
    sequential: bool,
    gitignore: bool,
) -> Vec<Candidate> {
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
    let member_ancestors: HashSet<PathBuf> = owners
        .keys()
        .flat_map(|dir| {
            dir.ancestors()
                .skip(1)
                .take_while(|ancestor| *ancestor != workspace.root.as_path() && ancestor.starts_with(&workspace.root))
                .map(Path::to_path_buf)
        })
        .collect();
    let walk = Walk {
        fs,
        root: &workspace.root,
        owners,
        member_ancestors,
        skipped,
        sequential,
        gitignore,
    };
    let mut found = walk.directory(&Frame {
        dir: workspace.root.clone(),
        owner: PackageId::ROOT,
        depth: 0,
        matchers: Vec::new(),
        skip_if_project: false,
        members_only: false,
    });
    found.sort_by(|a, b| a.rel.cmp(&b.rel));
    found
}

/// Takes the symbolic links to other candidates out of `candidates`, returning each link with its target.
///
/// A linked file is one module, as it is for Node and TypeScript: otherwise its exports are reported once per link
/// and the file itself looks unused. A link that leaves the workspace or points at a file outside the analysis stays
/// a candidate of its own. Only links are canonicalized, so a tree without them pays nothing.
pub(crate) fn take_file_links(
    fs: &dyn FileSystem,
    root: &Path,
    candidates: &mut Vec<Candidate>,
) -> HashMap<PathBuf, PathBuf> {
    if !candidates.iter().any(|candidate| candidate.is_link) {
        return HashMap::new();
    }
    let real_root = fs.canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let files: HashSet<&Path> = candidates
        .iter()
        .filter(|candidate| !candidate.is_link)
        .map(|candidate| candidate.abs.as_path())
        .collect();
    let links: HashMap<PathBuf, PathBuf> = candidates
        .iter()
        .filter(|candidate| candidate.is_link)
        .filter_map(|candidate| {
            let real = fs.canonicalize(&candidate.abs).ok()?;
            let target = root.join(real.strip_prefix(&real_root).ok()?);
            files
                .contains(target.as_path())
                .then(|| (candidate.abs.clone(), target))
        })
        .collect();
    candidates.retain(|candidate| !links.contains_key(&candidate.abs));
    links
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
            .with_file("/ws/.pnp.cjs", "")
            .with_file("/ws/.pnp.loader.mjs", "")
            .with_file("/ws/readme.md", "");
        let config = Config::from_toml_str("").unwrap();
        let workspace = Workspace::discover_with_fs(Path::new("/ws"), &config, &fs).unwrap();
        let files = discover(&fs, &workspace, false);
        let listed: Vec<(&str, usize)> = files.iter().map(|c| (c.rel.as_str(), c.owner.index())).collect();
        assert_eq!(listed, [("packages/a/src/z.ts", 1), ("tools/run.js", 0)]);
    }

    #[test]
    fn marker_manifests_do_not_hide_a_directory() {
        let fs = MemoryFs::new()
            .with_file("/ws/package.json", r#"{"name":"r"}"#)
            .with_file("/ws/scripts/package.json", r#"{"type":"module"}"#)
            .with_file("/ws/scripts/run.js", "")
            .with_file("/ws/vendored/package.json", r#"{"name":"other"}"#)
            .with_file("/ws/vendored/index.js", "")
            .with_file("/ws/apps/package.json", r#"{"dependencies":{"a":"1"}}"#)
            .with_file("/ws/apps/main.js", "");
        assert_eq!(files(&fs), ["scripts/run.js"]);
    }

    fn files(fs: &MemoryFs) -> Vec<String> {
        let config = Config::from_toml_str("").unwrap();
        let workspace = Workspace::discover_with_fs(Path::new("/ws"), &config, fs).unwrap();
        discover(fs, &workspace, false).into_iter().map(|c| c.rel).collect()
    }

    #[test]
    fn directories_with_their_own_manifest_outside_the_members_are_separate_projects() {
        let fs = MemoryFs::new()
            .with_file("/ws/package.json", r#"{"name":"r","workspaces":["packages/*"]}"#)
            .with_file("/ws/packages/a/package.json", r#"{"name":"a"}"#)
            .with_file("/ws/packages/a/src/a.ts", "")
            .with_file("/ws/other/package.json", r#"{"name":"other"}"#)
            .with_file("/ws/other/src/o.ts", "")
            .with_file("/ws/other/deep/more/d.ts", "")
            .with_file("/ws/scripts/run.ts", "");
        assert_eq!(files(&fs), ["packages/a/src/a.ts", "scripts/run.ts"]);
    }

    #[test]
    fn build_directory_is_source_unless_gitignored() {
        let fs = MemoryFs::new()
            .with_file("/ws/package.json", r#"{"name":"r"}"#)
            .with_file("/ws/build/tool.ts", "")
            .with_file("/ws/src/a.ts", "");
        assert_eq!(files(&fs), ["build/tool.ts", "src/a.ts"]);

        let fs = fs.with_file("/ws/.gitignore", "build/\n");
        assert_eq!(files(&fs), ["src/a.ts"]);
    }

    #[test]
    fn gitignore_patterns_nesting_and_negation() {
        let fs = MemoryFs::new()
            .with_file("/ws/package.json", r#"{"name":"r"}"#)
            .with_file("/ws/.gitignore", "*.generated.ts\ngen/\n!keep.generated.ts\n")
            .with_file("/ws/src/a.ts", "")
            .with_file("/ws/src/a.generated.ts", "")
            .with_file("/ws/src/keep.generated.ts", "")
            .with_file("/ws/gen/x.ts", "")
            .with_file("/ws/src/nested/.ignore", "private.ts\n")
            .with_file("/ws/src/nested/private.ts", "")
            .with_file("/ws/src/nested/public.ts", "");
        assert_eq!(
            files(&fs),
            ["src/a.ts", "src/keep.generated.ts", "src/nested/public.ts"]
        );
    }

    #[test]
    fn hidden_and_build_output_directories_are_skipped() {
        let fs = MemoryFs::new()
            .with_file("/ws/package.json", r#"{"name":"r"}"#)
            .with_file("/ws/.next/server.js", "")
            .with_file("/ws/out/x.js", "")
            .with_file("/ws/.turbo/x.js", "")
            .with_file("/ws/.svelte-kit/x.js", "")
            .with_file("/ws/storybook-static/x.js", "")
            .with_file("/ws/.yarn/x.js", "")
            .with_file("/ws/.vercel/x.js", "")
            .with_file("/ws/.nuxt/x.js", "")
            .with_file("/ws/target/x.js", "")
            .with_file("/ws/.cache/x.js", "")
            .with_file("/ws/.storybook/main.ts", "")
            .with_file("/ws/.eslintrc.js", "")
            .with_file("/ws/src/a.ts", "");
        assert_eq!(files(&fs), [".eslintrc.js", ".storybook/main.ts", "src/a.ts"]);
    }

    #[test]
    fn a_workspace_member_below_a_hidden_directory_is_walked() {
        // `.vscode/extensions/*` are members; the rest of `.vscode` and other hidden directories stay skipped.
        let fs = MemoryFs::new()
            .with_file(
                "/ws/package.json",
                r#"{"name":"r","workspaces":["packages/*",".vscode/extensions/*"]}"#,
            )
            .with_file("/ws/packages/a/package.json", r#"{"name":"a"}"#)
            .with_file("/ws/packages/a/src/a.ts", "")
            .with_file("/ws/packages/a/.cache/x.js", "")
            .with_file("/ws/.vscode/extensions/helper/package.json", r#"{"name":"helper"}"#)
            .with_file("/ws/.vscode/extensions/helper/src/extension.ts", "")
            .with_file("/ws/.vscode/extensions/helper/.hidden/x.ts", "")
            .with_file("/ws/.vscode/settings.js", "")
            .with_file("/ws/.vscode/extensions/stray.ts", "")
            .with_file("/ws/.vscode/other/tool.ts", "")
            .with_file("/ws/.cache/x.js", "");
        let config = Config::from_toml_str("").unwrap();
        let workspace = Workspace::discover_with_fs(Path::new("/ws"), &config, &fs).unwrap();
        let index_of = |name: &str| {
            workspace
                .iter()
                .find(|(_, package)| package.display_name() == name)
                .map(|(id, _)| id.index())
                .unwrap()
        };
        let listed: Vec<(String, usize)> = discover(&fs, &workspace, false)
            .into_iter()
            .map(|candidate| (candidate.rel, candidate.owner.index()))
            .collect();
        assert_eq!(
            listed,
            [
                (".vscode/extensions/helper/src/extension.ts".to_owned(), index_of("helper")),
                ("packages/a/src/a.ts".to_owned(), index_of("a")),
            ]
        );
    }

    #[test]
    fn sequential_and_parallel_walks_agree() {
        let mut fs = MemoryFs::new().with_file("/ws/package.json", r#"{"name":"r"}"#);
        for dir in 0..12 {
            for file in 0..5 {
                fs.insert(format!("/ws/d{dir}/s{file}/f.ts"), "");
            }
        }
        let config = Config::from_toml_str("").unwrap();
        let workspace = Workspace::discover_with_fs(Path::new("/ws"), &config, &fs).unwrap();
        let rels = |sequential| -> Vec<String> {
            discover(&fs, &workspace, sequential)
                .into_iter()
                .map(|candidate| candidate.rel)
                .collect()
        };
        assert_eq!(rels(true), rels(false));
        assert_eq!(rels(true).len(), 60);
    }
}
