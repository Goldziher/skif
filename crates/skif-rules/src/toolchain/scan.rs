//! Finding the toolchain files of a workspace through the [`FileSystem`] and reading them, once per run for every
//! toolchain rule.
//!
//! The usual places are the root and every package directory that is not ignored (version files, Dockerfiles and
//! `*.sh`), their `docker` and `scripts` subdirectories (Dockerfiles and `*.sh`) and the root's `.github/workflows`.
//! The `files` globs of the rules add more. A file is owned by the innermost package whose directory holds it; one
//! below a directory with a `package.json` that is not a workspace package, or below an ignored package, belongs to
//! another project and is not read.

use std::collections::BTreeMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use skif_config::Selector;
use skif_core::{FileSystem, LineIndex, Location, PackageId};
use skif_lint::LintContext;

use super::options::{Resolved, ToolchainKind, ToolchainOptions};
use super::parse::{self, Parsed};

const VERSION_FILES: [&str; 5] = [".nvmrc", ".node-version", ".tool-versions", "mise.toml", ".mise.toml"];
const SUBDIRECTORIES: [&str; 2] = ["docker", "scripts"];
const WORKFLOWS: &str = ".github/workflows";
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
    pub fn of(cx: &LintContext<'_>, fs: &Arc<dyn FileSystem>, rules: &[&str]) -> Arc<Self> {
        cx.shared(|| {
            let globs: Vec<String> = rules
                .iter()
                .flat_map(|id| ToolchainOptions::of(cx, id).files)
                .collect();
            Self::build(cx, fs.as_ref(), &globs)
        })
    }

    /// The files a rule reads under its `options`.
    pub fn files_for<'a>(&'a self, options: &'a Resolved) -> impl Iterator<Item = &'a ToolchainFile> {
        self.files
            .iter()
            .filter(|file| options.reads(&file.path, file.kind, file.at_default_place))
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
        let mut found: BTreeMap<String, (ToolchainKind, PackageId, bool)> = BTreeMap::new();
        for (dir, &(id, ignored)) in &owners.dirs {
            if ignored {
                continue;
            }
            for name in owners.file_names(dir) {
                let kind = default_kind(&name, true);
                if let Some(kind) = kind {
                    found.insert(join(dir, &name), (kind, id, true));
                }
            }
            for sub in SUBDIRECTORIES {
                let sub = join(dir, sub);
                if owners.dirs.contains_key(&sub) || owners.is_project(&sub) {
                    continue;
                }
                for name in owners.file_names(&sub) {
                    if let Some(kind) = default_kind(&name, false) {
                        found.insert(join(&sub, &name), (kind, id, true));
                    }
                }
            }
        }
        if let Some(&(root, false)) = owners.dirs.get("") {
            for name in owners.file_names(WORKFLOWS) {
                if name.ends_with(".yml") || name.ends_with(".yaml") {
                    found.insert(join(WORKFLOWS, &name), (ToolchainKind::Workflows, root, true));
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
        let files = found
            .into_iter()
            .filter_map(|(path, (kind, package, at_default_place))| {
                let source = fs.read_to_string(&workspace.root.join(&path)).ok()?;
                let name = path.rsplit('/').next().unwrap_or(&path);
                Some(ToolchainFile {
                    kind,
                    package,
                    at_default_place,
                    lines: LineIndex::new(&source),
                    parsed: parse::parse(kind, name, &source),
                    path,
                })
            })
            .collect();
        Self { files }
    }
}

/// A path with `/` separators.
fn slashed(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    }
}

fn is_dockerfile(name: &str) -> bool {
    matches!(name, "Dockerfile" | "Containerfile")
        || name.starts_with("Dockerfile.")
        || name.ends_with(".Dockerfile")
        || name.ends_with(".dockerfile")
}

/// The kind of a file found at a usual place by its name; version files count only in a package directory itself.
fn default_kind(name: &str, package_dir: bool) -> Option<ToolchainKind> {
    if package_dir && VERSION_FILES.contains(&name) {
        Some(ToolchainKind::VersionFiles)
    } else if is_dockerfile(name) {
        Some(ToolchainKind::Dockerfiles)
    } else if name.ends_with(".sh") {
        Some(ToolchainKind::ShellScripts)
    } else {
        None
    }
}

/// The kind of a file a glob found: by its name, YAML read as a workflow, anything else as a shell script.
fn glob_kind(path: &str) -> ToolchainKind {
    let name = path.rsplit('/').next().unwrap_or(path);
    if VERSION_FILES.contains(&name) {
        ToolchainKind::VersionFiles
    } else if is_dockerfile(name) {
        ToolchainKind::Dockerfiles
    } else if name.ends_with(".yml") || name.ends_with(".yaml") {
        ToolchainKind::Workflows
    } else {
        ToolchainKind::ShellScripts
    }
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
        self.fs
            .read_dir_shared(&self.root.join(dir))
            .unwrap_or_default()
            .iter()
            .filter(|entry| entry.is_file)
            .filter_map(|entry| entry.path.file_name().map(|name| name.to_string_lossy().into_owned()))
            .collect()
    }

    /// Whether `dir` holds a `package.json` of its own.
    fn is_project(&self, dir: &str) -> bool {
        self.fs.is_file(&self.root.join(dir).join(MANIFEST))
    }

    /// The package owning the file at `path`: the innermost package directory above it, unless that is ignored or a
    /// directory with a `package.json` that is no workspace package comes first.
    fn owner(&self, path: &str) -> Option<PackageId> {
        let mut dir = path.rsplit_once('/').map_or("", |(dir, _)| dir);
        loop {
            if let Some(&(id, ignored)) = self.dirs.get(dir) {
                return (!ignored).then_some(id);
            }
            if dir.is_empty() || self.is_project(dir) {
                return None;
            }
            dir = dir.rsplit_once('/').map_or("", |(parent, _)| parent);
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
        walk(fs, root, &segments[..literal].join("/"), 0, selector, &mut found);
    }
    found.sort_unstable();
    found.dedup();
    found
}

fn walk(fs: &dyn FileSystem, root: &Path, dir: &str, depth: usize, selector: &Selector, found: &mut Vec<String>) {
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
                walk(fs, root, &path, depth + 1, selector, found);
            }
        } else if entry.is_file && selector.matches(&path, false) {
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
        for name in ["Dockerfile", "Dockerfile.prod", "api.Dockerfile", "web.dockerfile", "Containerfile"] {
            assert_eq!(default_kind(name, false), Some(ToolchainKind::Dockerfiles), "{name}");
        }
        assert_eq!(default_kind("deploy.sh", true), Some(ToolchainKind::ShellScripts));
        assert_eq!(default_kind("docker-compose.yml", true), None);
        assert_eq!(glob_kind("ci/pipeline.yaml"), ToolchainKind::Workflows);
        assert_eq!(glob_kind("ci/release"), ToolchainKind::ShellScripts);
        assert_eq!(glob_kind("deploy/Dockerfile.base"), ToolchainKind::Dockerfiles);
    }
}
