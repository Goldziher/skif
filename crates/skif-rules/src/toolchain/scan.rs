//! Finding the toolchain files of a workspace through the run's [`FileSystem`] ([`LintContext::fs`]) and reading
//! them, once per run for every toolchain rule.
//!
//! The usual places are the root and every package directory that is not ignored (version files, Dockerfiles and
//! `*.sh`), their `docker` and `scripts` subdirectories (Dockerfiles and `*.sh`), the root's `.github/workflows` and
//! the root's Bazel files (`MODULE.bazel`, `WORKSPACE.bazel`, `WORKSPACE`).
//! The `files` globs of the rules add more. A file is owned by the innermost package whose directory holds it; one
//! below a directory with a `package.json` that is not a workspace package, or below an ignored package, belongs to
//! another project and is not read.

use std::collections::BTreeMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use skif_config::Selector;
use skif_core::{DirEntry, FileSystem, LineIndex, Location, PackageId};
use skif_lint::LintContext;
use skif_workspace::map_ordered;

use super::options::{Resolved, ToolchainKind, ToolchainOptions};
use super::parse::{self, Parsed};
use crate::paths::{file_name, join, parent};

const VERSION_FILES: [&str; 6] = [
    ".nvmrc",
    ".node-version",
    ".bun-version",
    ".tool-versions",
    "mise.toml",
    ".mise.toml",
];
const SUBDIRECTORIES: [&str; 2] = ["docker", "scripts"];
const WORKFLOWS: &str = ".github/workflows";
/// The Bazel files read at the workspace root, where `rules_js` and `rules_nodejs` take their versions.
const BAZEL_FILES: [&str; 3] = ["MODULE.bazel", "WORKSPACE.bazel", "WORKSPACE"];
const MANIFEST: &str = "package.json";
/// How deep a glob's walk descends below its literal prefix.
const MAX_DEPTH: usize = 16;

/// A toolchain file of the workspace, read.
#[derive(Debug)]
pub(crate) struct ToolchainFile {
    /// Workspace-relative path with `/` separators.
    pub path: String,
    pub kind: ToolchainKind,
    /// The package whose directory holds the file.
    pub package: PackageId,
    /// Found at a usual place for its kind, as opposed to only through a `files` glob.
    pub at_default_place: bool,
    pub lines: LineIndex,
    pub parsed: Parsed,
}

impl ToolchainFile {
    /// The location of a byte range of the file.
    pub fn location(&self, span: &Range<usize>) -> Location {
        let offset = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
        Location::new(
            PathBuf::from(&self.path),
            Some(self.lines.span(offset(span.start), offset(span.end))),
        )
    }

    /// The 1-based line of a byte offset of the file.
    pub fn line(&self, offset: usize) -> u32 {
        self.lines.line_col(u32::try_from(offset).unwrap_or(u32::MAX)).0
    }
}

/// Every toolchain file of a workspace, sorted by path.
#[derive(Debug, Default)]
pub(crate) struct Scan {
    pub files: Vec<ToolchainFile>,
}

impl Scan {
    /// The scan of the run, built by the first toolchain rule that asks for it; `rules` are the ids of every
    /// toolchain rule, whose `files` globs it reads too.
    pub fn of(cx: &LintContext<'_>, rules: &[&str]) -> Arc<Self> {
        cx.shared(|| {
            let globs: Vec<String> = rules.iter().flat_map(|id| ToolchainOptions::of(cx, id).files).collect();
            Self::build(cx, cx.fs(), &globs)
        })
    }

    /// The files a rule reads under its `options`.
    pub fn files_for<'a>(&'a self, options: &'a Resolved) -> impl Iterator<Item = &'a ToolchainFile> {
        self.files
            .iter()
            .filter(|file| options.reads(&file.path, file.kind, file.at_default_place))
    }

    /// The Dockerfiles directly in the workspace-relative `dir` that the scan did not read, read and parsed, by path.
    pub fn unscanned_dockerfiles(&self, fs: &dyn FileSystem, root: &Path, dir: &str) -> Vec<(String, Parsed)> {
        file_names(fs, root, dir)
            .into_iter()
            .filter(|name| is_dockerfile(name))
            .map(|name| join(dir, &name))
            .filter(|path| self.files.iter().all(|file| file.path != *path))
            .filter_map(|path| {
                let source = fs.read_to_string(&root.join(&path)).ok()?;
                let parsed = parse::parse(ToolchainKind::Dockerfiles, file_name(&path), &source);
                Some((path, parsed))
            })
            .collect()
    }

    fn build(cx: &LintContext<'_>, fs: &dyn FileSystem, globs: &[String]) -> Self {
        let workspace = cx.workspace;
        let owners = Owners {
            fs,
            root: &workspace.root,
            dirs: workspace
                .iter()
                .map(|(id, package)| (slashed(&package.dir), (id, workspace.is_ignored(id))))
                .collect(),
        };
        let scanned: Vec<(&String, PackageId)> = owners
            .dirs
            .iter()
            .filter(|(_, (_, ignored))| !ignored)
            .map(|(dir, &(id, _))| (dir, id))
            .collect();
        let mut found: BTreeMap<String, (ToolchainKind, PackageId, bool)> =
            map_ordered(&scanned, cx.sequential, |&(dir, id)| owners.default_files(dir, id))
                .into_iter()
                .flatten()
                .collect();
        if let Some(&(root, false)) = owners.dirs.get("") {
            for name in owners.file_names(WORKFLOWS) {
                if has_extension(&name, &["yml", "yaml"]) {
                    found.insert(join(WORKFLOWS, &name), (ToolchainKind::Workflows, root, true));
                }
            }
            for name in owners.file_names("") {
                if BAZEL_FILES.contains(&name.as_str()) {
                    found.insert(name, (ToolchainKind::Bazel, root, true));
                }
            }
        }
        if let Ok(selector) = Selector::new(globs.iter().cloned())
            && !selector.is_empty()
        {
            for path in globbed(fs, &workspace.root, globs, &selector) {
                if found.contains_key(&path) {
                    continue;
                }
                if let Some(id) = owners.owner(&path) {
                    found.insert(path.clone(), (glob_kind(&path), id, false));
                }
            }
        }
        let found: Vec<(String, (ToolchainKind, PackageId, bool))> = found.into_iter().collect();
        let files = map_ordered(&found, cx.sequential, |(path, (kind, package, at_default_place))| {
            let (kind, package, at_default_place) = (*kind, *package, *at_default_place);
            let source = fs.read_to_string(&workspace.root.join(path)).ok()?;
            let name = file_name(path);
            Some(ToolchainFile {
                kind,
                package,
                at_default_place,
                lines: LineIndex::new(&source),
                parsed: parse::parse(kind, name, &source),
                path: path.clone(),
            })
        })
        .into_iter()
        .flatten()
        .collect();
        Self { files }
    }
}

/// A path with `/` separators.
fn slashed(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Whether `name` ends in `.<extension>` for one of `extensions`, in any case.
fn has_extension(name: &str, extensions: &[&str]) -> bool {
    name.rsplit_once('.').is_some_and(|(stem, extension)| {
        !stem.is_empty() && extensions.iter().any(|known| extension.eq_ignore_ascii_case(known))
    })
}

fn is_dockerfile(name: &str) -> bool {
    matches!(name, "Dockerfile" | "Containerfile")
        || name.starts_with("Dockerfile.")
        || has_extension(name, &["dockerfile"])
}

/// Whether `name` is a Bazel file that may pin a toolchain: one of [`BAZEL_FILES`], a `.bzl` file or an included
/// `*.MODULE.bazel` file.
fn is_bazel_file(name: &str) -> bool {
    BAZEL_FILES.contains(&name) || has_extension(name, &["bzl"]) || name.ends_with(".MODULE.bazel")
}

/// The kind of a file found at a usual place by its name; version files count only in a package directory itself.
fn default_kind(name: &str, package_dir: bool) -> Option<ToolchainKind> {
    if package_dir && VERSION_FILES.contains(&name) {
        Some(ToolchainKind::VersionFiles)
    } else if is_dockerfile(name) {
        Some(ToolchainKind::Dockerfiles)
    } else if has_extension(name, &["sh"]) {
        Some(ToolchainKind::ShellScripts)
    } else {
        None
    }
}

/// The kind of a file a glob found: by its name, YAML read as a workflow, Bazel files as Starlark, anything else as a
/// shell script.
fn glob_kind(path: &str) -> ToolchainKind {
    let name = file_name(path);
    if is_bazel_file(name) {
        ToolchainKind::Bazel
    } else if VERSION_FILES.contains(&name) {
        ToolchainKind::VersionFiles
    } else if is_dockerfile(name) {
        ToolchainKind::Dockerfiles
    } else if has_extension(name, &["yml", "yaml"]) {
        ToolchainKind::Workflows
    } else {
        ToolchainKind::ShellScripts
    }
}

/// The names of the files directly in the workspace-relative `dir`.
fn file_names(fs: &dyn FileSystem, root: &Path, dir: &str) -> Vec<String> {
    fs.read_dir_shared(&root.join(dir))
        .unwrap_or_default()
        .iter()
        .filter(|entry| entry.is_file)
        .filter_map(|entry| entry.path.file_name().map(|name| name.to_string_lossy().into_owned()))
        .collect()
}

/// The workspace packages by directory (`/` separators, `""` for the root), with whether each is ignored.
struct Owners<'a> {
    fs: &'a dyn FileSystem,
    root: &'a Path,
    dirs: BTreeMap<String, (PackageId, bool)>,
}

impl Owners<'_> {
    /// The names of the files directly in `dir`.
    fn file_names(&self, dir: &str) -> Vec<String> {
        file_names(self.fs, self.root, dir)
    }

    /// The toolchain files at the usual places of the package `id` in `dir`: its own directory, and those of
    /// [`SUBDIRECTORIES`] it holds that are neither a workspace package nor a project of their own.
    fn default_files(&self, dir: &str, id: PackageId) -> Vec<(String, (ToolchainKind, PackageId, bool))> {
        let entries = self.fs.read_dir_shared(&self.root.join(dir)).unwrap_or_default();
        let named = |entry: &DirEntry| entry.path.file_name().map(|name| name.to_string_lossy().into_owned());
        let own = entries
            .iter()
            .filter(|entry| entry.is_file)
            .filter_map(named)
            .filter_map(|name| default_kind(&name, true).map(|kind| (join(dir, &name), (kind, id, true))));
        let below = entries
            .iter()
            .filter(|entry| entry.is_dir)
            .filter_map(named)
            .filter(|name| SUBDIRECTORIES.contains(&name.as_str()))
            .map(|name| join(dir, &name))
            .filter(|sub| !self.dirs.contains_key(sub) && !self.is_project(sub))
            .flat_map(|sub| {
                self.file_names(&sub).into_iter().filter_map(move |name| {
                    default_kind(&name, false).map(|kind| (join(&sub, &name), (kind, id, true)))
                })
            });
        own.chain(below).collect()
    }

    /// Whether `dir` holds a `package.json` of its own.
    fn is_project(&self, dir: &str) -> bool {
        self.fs.is_file(&self.root.join(dir).join(MANIFEST))
    }

    /// The package owning the file at `path`: the innermost package directory above it, unless that is ignored or a
    /// directory with a `package.json` that is no workspace package comes first.
    fn owner(&self, path: &str) -> Option<PackageId> {
        let mut dir = parent(path);
        loop {
            if let Some(&(id, ignored)) = self.dirs.get(dir) {
                return (!ignored).then_some(id);
            }
            if dir.is_empty() || self.is_project(dir) {
                return None;
            }
            dir = parent(dir);
        }
    }
}

/// The files below the root that `selector` matches, found by walking the literal directory each glob starts with.
fn globbed(fs: &dyn FileSystem, root: &Path, globs: &[String], selector: &Selector) -> Vec<String> {
    let mut found = Vec::new();
    for glob in globs.iter().filter(|glob| !glob.starts_with('!')) {
        let segments: Vec<&str> = glob.trim_start_matches("./").split('/').collect();
        let literal = segments
            .iter()
            .take_while(|segment| !segment.contains(['*', '?', '[', '{']))
            .count();
        if literal == segments.len() {
            let path = segments.join("/");
            if fs.is_file(&root.join(&path)) && selector.matches(&path, false) {
                found.push(path);
            }
            continue;
        }
        let matches = |path: &str| selector.matches(path, false);
        walk(fs, root, &segments[..literal].join("/"), 0, &matches, &mut found);
    }
    found.sort_unstable();
    found.dedup();
    found
}

fn walk(
    fs: &dyn FileSystem,
    root: &Path,
    dir: &str,
    depth: usize,
    matches: &dyn Fn(&str) -> bool,
    found: &mut Vec<String>,
) {
    if depth > MAX_DEPTH {
        return;
    }
    let entries = fs.read_dir_shared(&root.join(dir)).unwrap_or_default();
    for entry in entries.iter() {
        let Some(name) = entry.path.file_name().map(|name| name.to_string_lossy().into_owned()) else {
            continue;
        };
        let path = join(dir, &name);
        if entry.is_dir {
            if !entry.is_symlink && name != "node_modules" && name != ".git" {
                walk(fs, root, &path, depth + 1, matches, found);
            }
        } else if entry.is_file && matches(&path) {
            found.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_follow_the_file_name() {
        assert_eq!(default_kind(".nvmrc", true), Some(ToolchainKind::VersionFiles));
        assert_eq!(default_kind(".nvmrc", false), None);
        for name in [
            "Dockerfile",
            "Dockerfile.prod",
            "api.Dockerfile",
            "web.dockerfile",
            "Containerfile",
        ] {
            assert_eq!(default_kind(name, false), Some(ToolchainKind::Dockerfiles), "{name}");
        }
        assert_eq!(default_kind("deploy.sh", true), Some(ToolchainKind::ShellScripts));
        assert_eq!(default_kind("docker-compose.yml", true), None);
        assert_eq!(glob_kind("ci/pipeline.yaml"), ToolchainKind::Workflows);
        assert_eq!(glob_kind("ci/release"), ToolchainKind::ShellScripts);
        assert_eq!(glob_kind("deploy/Dockerfile.base"), ToolchainKind::Dockerfiles);
        for path in ["js/MODULE.bazel", "tools/node.bzl", "deps/npm.MODULE.bazel", "WORKSPACE.bazel"] {
            assert_eq!(glob_kind(path), ToolchainKind::Bazel, "{path}");
        }
        assert_eq!(default_kind("MODULE.bazel", true), None, "read at the root only");
    }
}
