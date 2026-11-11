//! Finding every workspace root and standalone package below a directory.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ignore::Match;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use rayon::prelude::*;
use serde_json::Value;
use skif_config::{CONFIG_FILE_NAMES, Config, DEFAULT_DISCOVER_IGNORE, PackageManager};
use skif_core::{DirEntry, FileSystem};

use crate::detect;
use crate::manifest::parse_value;
use crate::parallel::map_ordered;
use crate::pattern::{self, Expander, Pattern};
use crate::pnpm;
#[cfg(test)]
use crate::workspace::Workspace;
use crate::{lerna, rush};

const MANIFEST: &str = "package.json";
const PNPM_WORKSPACE: &str = "pnpm-workspace.yaml";
const IGNORE_FILES: [&str; 2] = [".gitignore", ".ignore"];

/// A project root decided by [`claim_roots`].
///
/// Only a workspace declaration makes a directory a root of its own: a directory that declares a workspace is a
/// [`Workspace`](Self::Workspace) wherever it lies, also when an outer workspace lists it as a member or when it lies
/// below a member. A plain package is a [`Package`](Self::Package) root only when no workspace's members contain it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
pub enum RootKind {
    /// A directory declaring a workspace: a `package.json` with `workspaces`, a `pnpm-workspace.yaml` or a
    /// `rush.json`. It need not hold a `package.json`.
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

/// Bounds for [`scan_candidates`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct DiscoverOptions {
    /// Directories deeper than this below the start directory are not entered.
    pub max_depth: usize,
    /// Gitignore-style patterns, relative to the start directory, of directories the walk does not enter. A bare
    /// name matches at any depth, a pattern with a slash is anchored at the start directory and `!` re-includes. The
    /// start directory itself is never skipped. Defaults to [`DEFAULT_DISCOVER_IGNORE`].
    pub ignore: Vec<String>,
}

impl Default for DiscoverOptions {
    fn default() -> Self {
        Self {
            max_depth: 32,
            ignore: DEFAULT_DISCOVER_IGNORE.iter().map(|name| (*name).to_owned()).collect(),
        }
    }
}

impl DiscoverOptions {
    /// Sets [`max_depth`](Self::max_depth).
    #[must_use]
    pub fn with_max_depth(mut self, max_depth: usize) -> Self {
        self.max_depth = max_depth;
        self
    }

    /// Sets [`ignore`](Self::ignore), replacing the defaults.
    #[must_use]
    pub fn with_ignore(mut self, ignore: impl IntoIterator<Item = String>) -> Self {
        self.ignore = ignore.into_iter().collect();
        self
    }
}

fn manifest_declares_workspace(fs: &dyn FileSystem, dir: &Path) -> bool {
    fs.read_to_string(&dir.join(MANIFEST))
        .ok()
        .and_then(|text| parse_value(&text))
        .is_some_and(|manifest| declares_patterns(manifest.get("workspaces")))
}

/// Whether a `workspaces` value lists any pattern: a non-empty array, or an object with a non-empty `packages`.
/// `[]`, `null` and `false` declare nothing.
fn declares_patterns(workspaces: Option<&Value>) -> bool {
    match workspaces {
        Some(Value::Array(items)) => !items.is_empty(),
        Some(Value::Object(object)) => object
            .get("packages")
            .and_then(Value::as_array)
            .is_some_and(|items| !items.is_empty()),
        _ => false,
    }
}

/// The `packages` of the `lerna.json` in `dir`, when it has a `package.json` next to it and a `packages` list.
fn lerna_packages(fs: &dyn FileSystem, dir: &Path) -> Option<Vec<String>> {
    if !fs.is_file(&dir.join(MANIFEST)) {
        return None;
    }
    let path = dir.join(lerna::FILE);
    let text = fs.read_to_string(&path).ok()?;
    let packages = lerna::parse(path, text).ok()?.packages?;
    Some(packages.into_iter().map(|(_, glob, _)| glob).collect())
}

/// Whether `dir` declares a workspace: it holds a `pnpm-workspace.yaml` or a `rush.json`, its `package.json` has
/// a `workspaces` key, or a `lerna.json` next to it lists `packages`.
#[must_use]
pub fn declares_workspace(fs: &dyn FileSystem, dir: &Path) -> bool {
    fs.is_file(&dir.join(PNPM_WORKSPACE))
        || fs.is_file(&dir.join(rush::FILE))
        || manifest_declares_workspace(fs, dir)
        || lerna_packages(fs, dir).is_some()
}

fn has_git(fs: &dyn FileSystem, dir: &Path) -> bool {
    let git = dir.join(".git");
    fs.is_dir(&git) || fs.is_file(&git)
}

/// The nearest directory at or above `start` that holds a `.git` entry (a directory, or the file of a linked
/// worktree), which bounds every upward search.
#[must_use]
pub fn repository_root(fs: &dyn FileSystem, start: &Path) -> Option<PathBuf> {
    start.ancestors().find(|dir| has_git(fs, dir)).map(Path::to_path_buf)
}

/// Whether an ignore-file line holds no pattern: it is empty or a `#` comment.
fn is_blank_or_comment(line: &str) -> bool {
    line.trim().is_empty() || line.starts_with('#')
}

fn matcher_from(fs: &dyn FileSystem, base: &Path, files: impl IntoIterator<Item = PathBuf>) -> Option<Gitignore> {
    let mut builder = GitignoreBuilder::new(base);
    let mut any = false;
    for path in files {
        let Ok(text) = fs.read_to_string(&path) else { continue };
        for line in text.lines().filter(|line| !is_blank_or_comment(line)) {
            any |= builder.add_line(Some(path.clone()), line).is_ok();
        }
    }
    if any { builder.build().ok() } else { None }
}

/// The ignore files of the directories above a walk, innermost last: each parent is the next outer one.
struct Chain {
    matcher: Gitignore,
    parent: Ignores,
}

type Ignores = Option<Arc<Chain>>;

fn pushed(parent: Ignores, matcher: Option<Gitignore>) -> Ignores {
    match matcher {
        Some(matcher) => Some(Arc::new(Chain { matcher, parent })),
        None => parent,
    }
}

fn is_ignored(ignores: &Ignores, path: &Path) -> bool {
    let mut link = ignores.as_deref();
    while let Some(chain) = link {
        match chain.matcher.matched(path, true) {
            Match::Ignore(_) => return true,
            Match::Whitelist(_) => return false,
            Match::None => {}
        }
        link = chain.parent.as_deref();
    }
    false
}

/// The ignore rules that apply to `start` from above: `.git/info/exclude` of the repository and the `.gitignore` and
/// `.ignore` files of the directories between the repository root and `start`. Nothing outside a repository.
fn inherited_ignores(fs: &dyn FileSystem, start: &Path) -> Ignores {
    let git_root = repository_root(fs, start)?;
    let exclude = git_root.join(".git").join("info").join("exclude");
    let mut ignores = pushed(None, matcher_from(fs, &git_root, [exclude]));
    let mut above: Vec<&Path> = start
        .ancestors()
        .skip(1)
        .take_while(|dir| dir.starts_with(&git_root))
        .collect();
    above.reverse();
    for dir in above {
        let files = IGNORE_FILES.iter().map(|name| dir.join(name));
        ignores = pushed(ignores, matcher_from(fs, dir, files));
    }
    ignores
}

/// Whether the file at `path`, at or below `root`, is ignored: the `.gitignore` and `.ignore` files of the directories
/// from `root` down to the file's directory decide, under those [`inherited_ignores`] finds above `root`. A pattern
/// that excludes a parent directory excludes the file too.
pub(crate) fn is_ignored_file(fs: &dyn FileSystem, root: &Path, path: &Path) -> bool {
    let mut ignores = inherited_ignores(fs, root);
    let mut dirs: Vec<&Path> = path
        .ancestors()
        .skip(1)
        .take_while(|dir| dir.starts_with(root))
        .collect();
    dirs.reverse();
    for dir in dirs {
        let files = IGNORE_FILES.iter().map(|name| dir.join(name));
        ignores = pushed(ignores, matcher_from(fs, dir, files));
    }
    let mut link = ignores.as_deref();
    while let Some(chain) = link {
        match chain.matcher.matched_path_or_any_parents(path, false) {
            Match::Ignore(_) => return true,
            Match::Whitelist(_) => return false,
            Match::None => {}
        }
        link = chain.parent.as_deref();
    }
    false
}

/// The matcher for `options.ignore`, rooted at `start`; `None` when it holds no usable pattern. A pattern that is not a
/// valid glob is skipped.
fn skip_matcher(start: &Path, options: &DiscoverOptions) -> Option<Gitignore> {
    let mut builder = GitignoreBuilder::new(start);
    let mut any = false;
    for pattern in &options.ignore {
        any |= builder.add_line(None, pattern).is_ok();
    }
    if any { builder.build().ok() } else { None }
}

fn is_skipped(skip: Option<&Gitignore>, path: &Path) -> bool {
    skip.is_some_and(|matcher| matcher.matched(path, true).is_ignore())
}

fn descends(entry: &DirEntry) -> bool {
    entry.is_dir
        && !entry.is_symlink
        && entry
            .path
            .file_name()
            .is_some_and(|name| name != "node_modules" && !name.to_string_lossy().starts_with('.'))
}

/// What the walk found below a start directory, before deciding which of it is a root.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Candidates {
    /// Directories that declare a workspace.
    pub workspaces: BTreeSet<PathBuf>,
    /// Directories with a `package.json`, workspaces included.
    pub packages: BTreeSet<PathBuf>,
    /// The `skif.toml` and `.skif.toml` files found, whatever the directory they are in.
    pub config_files: BTreeSet<PathBuf>,
    /// Directories at the depth limit that have subdirectories the walk did not enter, sorted.
    pub truncated: Vec<PathBuf>,
    /// Directories whose listing failed (permissions, a directory removed during the walk), sorted. Nothing below
    /// them was searched.
    pub unreadable: Vec<PathBuf>,
}

/// Walks `start` and lists the workspace declarations and `package.json` files below it.
///
/// What a directory holds is decided from its listing, so a directory without a manifest costs one read. The walk
/// skips `node_modules`, hidden directories, symbolic links and directories excluded by a `.gitignore` or `.ignore`
/// at or below `start`, directories matching `options.ignore` and, when `start` lies in a git repository, by the
/// `.gitignore` and `.ignore` files between the repository root and `start` and by `.git/info/exclude`.
#[must_use]
pub fn scan_candidates(start: &Path, fs: &dyn FileSystem, options: &DiscoverOptions) -> Candidates {
    let skip = skip_matcher(start, options);
    let mut found = visit(
        fs,
        &Walk {
            max_depth: options.max_depth,
            skip: skip.as_ref(),
        },
        start.to_path_buf(),
        0,
        inherited_ignores(fs, start),
    );
    found.truncated.sort();
    found.unreadable.sort();
    found
}

/// What stays the same during one walk.
struct Walk<'a> {
    max_depth: usize,
    skip: Option<&'a Gitignore>,
}

/// Scans one directory and, in parallel, the directories below it. Results merge into sets and a list sorted at
/// the end, so the outcome does not depend on the order the threads finish in.
fn visit(fs: &dyn FileSystem, walk: &Walk<'_>, dir: PathBuf, depth: usize, ignores: Ignores) -> Candidates {
    let mut found = Candidates::default();
    let Ok(entries) = fs.read_dir_shared(&dir) else {
        found.unreadable.push(dir);
        return found;
    };
    let names: Vec<&str> = entries
        .iter()
        .filter(|entry| entry.is_file)
        .filter_map(|entry| entry.path.file_name().and_then(|name| name.to_str()))
        .collect();
    let manifest = names.contains(&MANIFEST);
    if manifest {
        found.packages.insert(dir.clone());
    }
    found.config_files.extend(
        CONFIG_FILE_NAMES
            .iter()
            .filter(|name| names.contains(name))
            .map(|name| dir.join(name)),
    );
    if names.contains(&PNPM_WORKSPACE)
        || names.contains(&rush::FILE)
        || (manifest && manifest_declares_workspace(fs, &dir))
        || (manifest && names.contains(&lerna::FILE) && lerna_packages(fs, &dir).is_some())
    {
        found.workspaces.insert(dir.clone());
    }
    let files = IGNORE_FILES
        .iter()
        .filter(|name| names.contains(name))
        .map(|name| dir.join(name));
    let ignores = pushed(ignores, matcher_from(fs, &dir, files));
    let children: Vec<PathBuf> = entries
        .iter()
        .filter(|entry| descends(entry) && !is_ignored(&ignores, &entry.path) && !is_skipped(walk.skip, &entry.path))
        .map(|entry| entry.path.clone())
        .collect();
    if depth >= walk.max_depth {
        if !children.is_empty() {
            found.truncated.push(dir);
        }
        return found;
    }
    let below: Vec<Candidates> = children
        .into_par_iter()
        .map(|child| visit(fs, walk, child, depth + 1, ignores.clone()))
        .collect();
    for child in below {
        found.packages.extend(child.packages);
        found.config_files.extend(child.config_files);
        found.workspaces.extend(child.workspaces);
        found.truncated.extend(child.truncated);
        found.unreadable.extend(child.unreadable);
    }
    found
}

/// Walks `start` and lists the files below it whose name `wanted` accepts, sorted.
///
/// The walk enters the directories [`scan_candidates`] enters: it skips `node_modules`, hidden directories, symbolic
/// links, directories a `.gitignore`, an `.ignore` or `.git/info/exclude` excludes and those `options.ignore` matches,
/// and stops `options.max_depth` directories down. It reads directory listings and those ignore files only. With
/// `sequential` it runs on the calling thread, otherwise on the current rayon pool.
#[must_use]
pub fn scan_files(
    start: &Path,
    fs: &dyn FileSystem,
    options: &DiscoverOptions,
    sequential: bool,
    wanted: &(dyn Fn(&str) -> bool + Sync),
) -> Vec<PathBuf> {
    let skip = skip_matcher(start, options);
    let search = FileSearch {
        fs,
        walk: Walk {
            max_depth: options.max_depth,
            skip: skip.as_ref(),
        },
        sequential,
        wanted,
    };
    let mut found = search.visit(start, 0, inherited_ignores(fs, start));
    found.sort_unstable();
    found
}

/// What stays the same during one [`scan_files`] walk.
struct FileSearch<'a> {
    fs: &'a dyn FileSystem,
    walk: Walk<'a>,
    sequential: bool,
    wanted: &'a (dyn Fn(&str) -> bool + Sync),
}

impl FileSearch<'_> {
    fn visit(&self, dir: &Path, depth: usize, ignores: Ignores) -> Vec<PathBuf> {
        let Ok(entries) = self.fs.read_dir_shared(dir) else {
            return Vec::new();
        };
        let file_name = |entry: &DirEntry| entry.path.file_name().and_then(|name| name.to_str()).map(str::to_owned);
        let names: Vec<String> = entries
            .iter()
            .filter(|entry| entry.is_file)
            .filter_map(file_name)
            .collect();
        let files = IGNORE_FILES
            .iter()
            .filter(|name| names.iter().any(|found| found == *name))
            .map(|name| dir.join(name));
        let ignores = pushed(ignores, matcher_from(self.fs, dir, files));
        let mut found: Vec<PathBuf> = entries
            .iter()
            .filter(|entry| entry.is_file && file_name(entry).is_some_and(|name| (self.wanted)(name.as_str())))
            .map(|entry| entry.path.clone())
            .collect();
        if depth >= self.walk.max_depth {
            return found;
        }
        let children: Vec<PathBuf> = entries
            .iter()
            .filter(|entry| {
                descends(entry) && !is_ignored(&ignores, &entry.path) && !is_skipped(self.walk.skip, &entry.path)
            })
            .map(|entry| entry.path.clone())
            .collect();
        let below = map_ordered(&children, self.sequential, |child| {
            self.visit(child, depth + 1, ignores.clone())
        });
        found.extend(below.into_iter().flatten());
        found
    }
}

/// The patterns `root` declares, from the source `Workspace::discover` would choose: `pnpm-workspace.yaml` for a pnpm
/// workspace, else the `workspaces` of `package.json`, then a leftover `pnpm-workspace.yaml`, then `rush.json`, then
/// the `packages` of `lerna.json`. A
/// stale `pnpm-workspace.yaml` in a Yarn or npm repository does not decide the members.
fn declared_patterns(fs: &dyn FileSystem, root: &Path) -> Vec<String> {
    let yaml_path = root.join(PNPM_WORKSPACE);
    let from_yaml = fs
        .read_to_string(&yaml_path)
        .ok()
        .and_then(|text| pnpm::parse(Path::new(PNPM_WORKSPACE), &text).ok())
        .and_then(|parsed| parsed.packages);
    let manifest = fs
        .read_to_string(&root.join(MANIFEST))
        .ok()
        .and_then(|text| parse_value(&text));
    let rush = fs
        .read_to_string(&root.join(rush::FILE))
        .ok()
        .and_then(|text| rush::parse(text).ok());
    let items = match manifest.as_ref().and_then(|manifest| manifest.get("workspaces")) {
        Some(Value::Array(items)) => Some(items),
        Some(Value::Object(object)) => object.get("packages").and_then(Value::as_array),
        _ => None,
    };
    // As in `Workspace::discover`, `lerna.json` is read only when nothing else declares members.
    let lerna_path = root.join(lerna::FILE);
    let lerna = (manifest.is_some() && items.is_none() && from_yaml.is_none() && rush.is_none())
        .then(|| fs.read_to_string(&lerna_path).ok())
        .flatten()
        .and_then(|text| lerna::parse(lerna_path, text).ok());
    let package_manager = detect::detect(
        fs,
        root,
        &Config::default(),
        manifest.as_ref().unwrap_or(&Value::Null),
        rush.as_ref()
            .and_then(|rush| rush.package_manager)
            .or_else(|| lerna.as_ref().and_then(|lerna| lerna.package_manager)),
        fs.is_file(&yaml_path),
    );
    if package_manager == Some(PackageManager::Pnpm)
        && let Some(patterns) = from_yaml.clone()
    {
        return patterns;
    }
    if let Some(items) = items {
        return items
            .iter()
            .filter_map(|item| item.as_str().map(str::to_owned))
            .collect();
    }
    if let Some(patterns) = from_yaml {
        return patterns;
    }
    if let Some(parsed) = rush {
        return parsed.projects.into_iter().map(|(_, folder, _)| folder).collect();
    }
    lerna
        .and_then(|lerna| lerna.packages)
        .map(|packages| packages.into_iter().map(|(_, glob, _)| glob).collect())
        .unwrap_or_default()
}

/// The absolute directories the patterns declared at `root` select (negations applied), read straight from
/// `pnpm-workspace.yaml`, the `workspaces` of `package.json` or the projects of `rush.json`.
///
/// This is how the members of a workspace that cannot be discovered (a broken root manifest or `rush.json`) are
/// still claimed. Only directories with a `package.json` count, and the result is empty when the
/// patterns cannot be read.
#[must_use]
pub fn declared_member_dirs(fs: &dyn FileSystem, root: &Path) -> BTreeSet<PathBuf> {
    let expander = Expander { fs, root };
    let mut selected = BTreeSet::new();
    let mut negations = Vec::new();
    for raw in declared_patterns(fs, root) {
        if raw.trim_start().starts_with('!') {
            negations.extend(pattern::negation(&raw).ok());
        } else if let Ok(pattern) = Pattern::new(&raw) {
            selected.extend(expander.expand(&pattern));
        }
    }
    selected
        .into_iter()
        .filter(|dir| expander.has_manifest(dir) && !negations.iter().any(|negation| negation.is_match(dir)))
        .map(|dir| root.join(dir))
        .collect()
}

/// The directories below `parent` that are symbolic links.
fn links_in(fs: &dyn FileSystem, parent: &Path) -> BTreeSet<PathBuf> {
    fs.read_dir_shared(parent)
        .map(|entries| {
            entries
                .iter()
                .filter(|entry| entry.is_symlink)
                .map(|entry| entry.path.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// `dir` as the OS resolves it when `dir` or a directory between `root` and `dir` is a symbolic link, else `dir`.
/// The walk never enters links, so a member reached through one has to be resolved to be recognised when the walk
/// finds its target.
fn resolved(fs: &dyn FileSystem, root: &Path, dir: &Path, links: &mut HashMap<PathBuf, BTreeSet<PathBuf>>) -> PathBuf {
    let linked = dir
        .ancestors()
        .take_while(|ancestor| *ancestor != root && ancestor.starts_with(root))
        .any(|ancestor| {
            ancestor.parent().is_some_and(|parent| {
                links
                    .entry(parent.to_path_buf())
                    .or_insert_with(|| links_in(fs, parent))
                    .contains(ancestor)
            })
        });
    if linked {
        fs.canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf())
    } else {
        dir.to_path_buf()
    }
}

/// Decides which of `candidates` are roots, given the member directories of each workspace (`members` is keyed by
/// workspace directory), sorted by path.
///
/// Every workspace is a root. A package is a root when it is not a workspace and neither it nor a directory above
/// it is a member of any workspace. Members reached through a symbolic link count as the directory they point to.
#[must_use]
pub fn claim_roots(
    fs: &dyn FileSystem,
    candidates: &Candidates,
    members: &BTreeMap<PathBuf, BTreeSet<PathBuf>>,
) -> Vec<RootKind> {
    let mut links = HashMap::new();
    let owned: BTreeSet<PathBuf> = members
        .iter()
        .flat_map(|(root, dirs)| dirs.iter().map(move |dir| (root, dir)))
        .map(|(root, dir)| resolved(fs, root, dir, &mut links))
        .collect();
    let mut roots: Vec<RootKind> = candidates.workspaces.iter().cloned().map(RootKind::Workspace).collect();
    roots.extend(
        candidates
            .packages
            .iter()
            .filter(|package| !candidates.workspaces.contains(*package))
            .filter(|package| !package.ancestors().any(|ancestor| owned.contains(ancestor)))
            .cloned()
            .map(RootKind::Package),
    );
    roots.sort_by(|a, b| a.path().cmp(b.path()));
    roots
}

/// Finds every root below `start` with the default configuration, as the tests of this module need it. The
/// configuration-aware pipeline lives in the `skif` facade (`discover_targets`), which is the one entry point.
#[cfg(test)]
fn discover_roots(start: &Path, fs: &dyn FileSystem, options: &DiscoverOptions) -> Vec<RootKind> {
    let candidates = scan_candidates(start, fs, options);
    let members = candidates
        .workspaces
        .iter()
        .map(|root| {
            let dirs = Workspace::discover_with_fs(root, &Config::default(), fs)
                .map_or_else(|_| declared_member_dirs(fs, root), |workspace| workspace.claimed_dirs());
            (root.clone(), dirs)
        })
        .collect();
    claim_roots(fs, &candidates, &members)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::io;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use skif_core::MemoryFs;

    use super::*;

    /// A file system that counts how it is asked about files.
    struct Counting {
        inner: MemoryFs,
        probes: AtomicUsize,
        reads: AtomicUsize,
    }

    impl FileSystem for Counting {
        fn read_to_string(&self, path: &Path) -> io::Result<Arc<str>> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            self.inner.read_to_string(path)
        }

        fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
            self.inner.read_dir(path)
        }

        fn is_file(&self, path: &Path) -> bool {
            self.probes.fetch_add(1, Ordering::SeqCst);
            self.inner.is_file(path)
        }

        fn is_dir(&self, path: &Path) -> bool {
            self.probes.fetch_add(1, Ordering::SeqCst);
            self.inner.is_dir(path)
        }
    }

    fn paths(roots: &[RootKind]) -> Vec<String> {
        roots
            .iter()
            .map(|root| {
                let kind = if matches!(root, RootKind::Workspace(_)) {
                    "ws"
                } else {
                    "pkg"
                };
                format!("{kind}:{}", root.path().display().to_string().replace('\\', "/"))
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
    fn workspace_listed_as_a_member_is_still_a_root_of_its_own() {
        let fs = MemoryFs::new()
            .with_file("/r/package.json", r#"{"workspaces": ["packages/*"]}"#)
            .with_file("/r/packages/inner/package.json", r#"{"workspaces": ["x/*"]}"#)
            .with_file("/r/packages/inner/x/y/package.json", r#"{"name": "y"}"#);
        assert_eq!(discover(&fs), ["ws:/r", "ws:/r/packages/inner"]);
    }

    #[test]
    fn workspace_below_a_member_is_a_root_and_a_package_below_a_member_is_not() {
        let fs = MemoryFs::new()
            .with_file("/r/package.json", r#"{"workspaces": ["packages/*"]}"#)
            .with_file("/r/packages/a/package.json", r#"{"name": "a"}"#)
            .with_file("/r/packages/a/fixture/package.json", r#"{"name": "fixture"}"#)
            .with_file("/r/packages/a/ex/package.json", r#"{"workspaces": ["p/*"]}"#)
            .with_file("/r/packages/a/ex/p/m/package.json", r#"{"name": "m"}"#);
        assert_eq!(discover(&fs), ["ws:/r", "ws:/r/packages/a/ex"]);
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
    fn unrelated_projects_below_a_workspace_that_does_not_list_them_are_roots() {
        let fs = MemoryFs::new()
            .with_file("/r/package.json", r#"{"workspaces": ["x/dist"]}"#)
            .with_file("/r/x/dist/package.json", r#"{"name": "x"}"#)
            .with_file("/r/projects/p/package.json", r#"{"name": "p"}"#)
            .with_file("/r/projects/q/package.json", r#"{"workspaces": ["pkgs/*"]}"#)
            .with_file("/r/projects/q/pkgs/m/package.json", r#"{"name": "m"}"#);
        assert_eq!(discover(&fs), ["ws:/r", "pkg:/r/projects/p", "ws:/r/projects/q"]);
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
    fn a_file_scan_skips_what_discovery_skips_and_the_ignored_directories() {
        let fs = MemoryFs::new()
            .with_file("/r/.gitignore", "generated/\n")
            .with_file("/r/BUILD", "")
            .with_file("/r/app/BUILD.bazel", "")
            .with_file("/r/app/README.md", "")
            .with_file("/r/app/node_modules/dep/BUILD", "")
            .with_file("/r/.hidden/BUILD", "")
            .with_file("/r/generated/BUILD", "")
            .with_file("/r/dist/BUILD", "")
            .with_file("/r/tools/dist/BUILD", "")
            .with_file("/r/vendor/BUILD", "")
            .with_file("/r/a/b/c/BUILD", "");
        let options = DiscoverOptions::default()
            .with_max_depth(2)
            .with_ignore(["dist".to_owned(), "/vendor".to_owned()]);
        let wanted = |name: &str| name.starts_with("BUILD");
        for sequential in [true, false] {
            let found = scan_files(Path::new("/r"), &fs, &options, sequential, &wanted);
            assert_eq!(
                found,
                [PathBuf::from("/r/BUILD"), PathBuf::from("/r/app/BUILD.bazel")],
                "sequential: {sequential}"
            );
        }
        let deep = scan_files(Path::new("/r"), &fs, &DiscoverOptions::default(), true, &wanted);
        assert!(deep.contains(&PathBuf::from("/r/a/b/c/BUILD")) && deep.contains(&PathBuf::from("/r/dist/BUILD")));
    }

    #[test]
    fn start_itself_can_be_the_root() {
        let fs = MemoryFs::new().with_file("/r/package.json", r#"{"name": "solo"}"#);
        assert_eq!(discover(&fs), ["pkg:/r"]);
    }

    #[test]
    fn nothing_found_is_empty() {
        let fs = MemoryFs::new().with_file("/r/readme.md", "x");
        assert_eq!(discover(&fs).len(), 0);
    }

    #[test]
    fn max_depth_bounds_the_walk() {
        let fs = MemoryFs::new().with_file("/r/a/b/c/package.json", r#"{"name": "deep"}"#);
        let shallow = discover_roots(Path::new("/r"), &fs, &DiscoverOptions::default().with_max_depth(2));
        assert_eq!(shallow.len(), 0);
        assert_eq!(discover(&fs), ["pkg:/r/a/b/c"]);
    }

    #[test]
    fn broken_workspace_is_still_a_root() {
        let fs = MemoryFs::new().with_file("/r/w/package.json", r#"{"workspaces": ["p/*"]"#);
        let roots = discover_roots(Path::new("/r"), &fs, &DiscoverOptions::default());
        assert_eq!(roots.len(), 1);
    }

    /// A tree where `/r/pkgs/a` is a link to `/r/real/a`.
    struct Linked(MemoryFs);

    impl FileSystem for Linked {
        fn read_to_string(&self, path: &Path) -> io::Result<Arc<str>> {
            self.0.read_to_string(path)
        }

        fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
            if path == Path::new("/r/pkgs") {
                return Ok(vec![DirEntry::new(PathBuf::from("/r/pkgs/a"), true, false).symlink()]);
            }
            self.0.read_dir(path)
        }

        fn is_file(&self, path: &Path) -> bool {
            self.0.is_file(path)
        }

        fn is_dir(&self, path: &Path) -> bool {
            self.0.is_dir(path)
        }

        fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
            Ok(path
                .strip_prefix("/r/pkgs/a")
                .map_or_else(|_| path.to_path_buf(), |rest| Path::new("/r/real/a").join(rest)))
        }
    }

    #[test]
    fn a_linked_member_is_resolved_through_the_file_system_not_the_disk() {
        let fs = Linked(MemoryFs::new().with_file("/r/real/a/package.json", r#"{"name": "a"}"#));
        let mut links = HashMap::new();
        assert_eq!(
            resolved(&fs, Path::new("/r"), Path::new("/r/pkgs/a"), &mut links),
            Path::new("/r/real/a"),
            "a path that does not exist on disk is resolved by the file system's own canonicalize"
        );
        assert_eq!(
            resolved(&fs, Path::new("/r"), Path::new("/r/pkgs/b"), &mut links),
            Path::new("/r/pkgs/b"),
            "a directory that is not a link stays as it is"
        );
    }

    #[test]
    fn an_empty_workspaces_value_does_not_make_a_workspace() {
        for value in ["[]", "null", "false", r#"{"packages": []}"#, r#"{"nohoist": ["x"]}"#] {
            let manifest = format!(r#"{{"name": "a", "workspaces": {value}}}"#);
            let fs = MemoryFs::new().with_file("/r/a/package.json", manifest);
            assert_eq!(discover(&fs), ["pkg:/r/a"], "workspaces: {value}");
        }
    }

    #[test]
    fn the_walk_lists_the_configuration_files_it_passes() {
        let fs = MemoryFs::new()
            .with_file("/r/skif.toml", "")
            .with_file("/r/a/package.json", r#"{"name": "a"}"#)
            .with_file("/r/a/.skif.toml", "")
            .with_file("/r/a/deep/skif.toml", "")
            .with_file("/r/node_modules/x/skif.toml", "");
        let found = scan_candidates(Path::new("/r"), &fs, &DiscoverOptions::default());
        let files: Vec<&Path> = found.config_files.iter().map(PathBuf::as_path).collect();
        assert_eq!(
            files,
            [
                Path::new("/r/a/.skif.toml"),
                Path::new("/r/a/deep/skif.toml"),
                Path::new("/r/skif.toml")
            ]
        );
    }

    #[test]
    fn the_walk_decides_from_listings_without_probing_each_directory() {
        let fs = Counting {
            inner: MemoryFs::new()
                .with_file("/r/a/x/readme.md", "x")
                .with_file("/r/b/y/readme.md", "y")
                .with_file("/r/c/package.json", r#"{"name": "c"}"#),
            probes: AtomicUsize::new(0),
            reads: AtomicUsize::new(0),
        };
        let found = scan_candidates(Path::new("/r"), &fs, &DiscoverOptions::default());
        assert_eq!(found.packages.len(), 1);
        // The start directory's own ancestors are probed for `.git`; nothing else is.
        let ancestors = Path::new("/r").ancestors().count();
        assert_eq!(fs.probes.load(Ordering::SeqCst), ancestors * 2);
        assert_eq!(fs.reads.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn gitignore_above_the_start_applies_inside_a_repository() {
        let fs = MemoryFs::new()
            .with_file("/r/.git/HEAD", "ref")
            .with_file("/r/.gitignore", "dist/\nvendor\n")
            .with_file("/r/sub/app/package.json", r#"{"name": "app"}"#)
            .with_file("/r/sub/dist/package.json", r#"{"name": "dist"}"#)
            .with_file("/r/sub/vendor/package.json", r#"{"name": "vendor"}"#);
        let roots = discover_roots(Path::new("/r/sub"), &fs, &DiscoverOptions::default());
        assert_eq!(paths(&roots), ["pkg:/r/sub/app"]);
    }

    #[test]
    fn git_exclude_applies_and_a_linked_worktree_git_file_marks_the_root() {
        let fs = MemoryFs::new()
            .with_file("/r/.git/info/exclude", "generated\n")
            .with_file("/r/app/package.json", r#"{"name": "app"}"#)
            .with_file("/r/generated/package.json", r#"{"name": "generated"}"#);
        assert_eq!(discover(&fs), ["pkg:/r/app"]);

        let linked = MemoryFs::new()
            .with_file("/w/.git", "gitdir: elsewhere")
            .with_file("/w/.gitignore", "out/\n")
            .with_file("/w/pkgs/a/package.json", r#"{"name": "a"}"#)
            .with_file("/w/pkgs/out/package.json", r#"{"name": "out"}"#);
        let roots = discover_roots(Path::new("/w/pkgs"), &linked, &DiscoverOptions::default());
        assert_eq!(paths(&roots), ["pkg:/w/pkgs/a"]);
    }

    #[test]
    fn ignore_files_above_the_start_are_not_consulted_outside_a_repository() {
        let fs = MemoryFs::new()
            .with_file("/r/.gitignore", "dist/\n")
            .with_file("/r/sub/dist/package.json", r#"{"name": "dist"}"#);
        let roots = discover_roots(Path::new("/r/sub"), &fs, &DiscoverOptions::default());
        assert_eq!(paths(&roots), ["pkg:/r/sub/dist"]);
    }

    #[test]
    fn the_walk_reports_where_the_depth_limit_cut_it() {
        let fs = MemoryFs::new()
            .with_file("/r/a/b/c/package.json", r#"{"name": "deep"}"#)
            .with_file("/r/a/b/leaf.txt", "x")
            .with_file("/r/z/package.json", r#"{"name": "z"}"#);
        let options = DiscoverOptions::default().with_max_depth(2);
        let found = scan_candidates(Path::new("/r"), &fs, &options);
        assert_eq!(found.truncated, [PathBuf::from("/r/a/b")]);
        assert_eq!(
            scan_candidates(Path::new("/r"), &fs, &DiscoverOptions::default()).truncated,
            [] as [std::path::PathBuf; 0]
        );
    }

    #[test]
    fn a_workspace_that_cannot_be_discovered_still_claims_its_declared_members() {
        let fs = MemoryFs::new()
            .with_file("/r/w/pnpm-workspace.yaml", "packages:\n  - 'p/*'\n  - '!p/skip'\n")
            .with_file("/r/w/package.json", r#"{"workspaces": ["p/*"]"#)
            .with_file("/r/w/p/m/package.json", r#"{"name": "m"}"#)
            .with_file("/r/w/p/skip/package.json", r#"{"name": "skip"}"#);
        assert_eq!(discover(&fs), ["ws:/r/w", "pkg:/r/w/p/skip"]);
    }

    #[test]
    fn a_workspace_without_a_root_manifest_claims_its_declared_members() {
        let fs = MemoryFs::new()
            .with_file("/r/w/pnpm-workspace.yaml", "packages:\n  - 'p/*'\n")
            .with_file("/r/w/p/m/package.json", r#"{"name": "m"}"#);
        assert_eq!(discover(&fs), ["ws:/r/w"]);
    }

    #[test]
    fn declared_member_dirs_reads_json_patterns_when_there_is_no_yaml() {
        let fs = MemoryFs::new()
            .with_file("/r/package.json", r#"{"workspaces": {"packages": ["p/*"]}}"#)
            .with_file("/r/p/m/package.json", r#"{"name": "m"}"#)
            .with_file("/r/p/empty/readme.md", "x");
        assert_eq!(
            declared_member_dirs(&fs, Path::new("/r")),
            BTreeSet::from([PathBuf::from("/r/p/m")])
        );
    }

    #[test]
    fn an_ignore_file_of_comments_builds_no_matcher() {
        let fs = MemoryFs::new()
            .with_file("/r/.gitignore", "# nothing here\n\n   \n")
            .with_file("/r/.ignore", "# comment\nbuild/\n");
        let only_comments = matcher_from(&fs, Path::new("/r"), [PathBuf::from("/r/.gitignore")]);
        assert!(only_comments.is_none());
        let with_pattern = matcher_from(&fs, Path::new("/r"), [PathBuf::from("/r/.ignore")]).unwrap();
        assert!(with_pattern.matched(Path::new("/r/build"), true).is_ignore());
    }

    #[test]
    fn a_stale_pnpm_workspace_file_does_not_decide_the_members_of_a_yarn_repository() {
        let fs = MemoryFs::new()
            .with_file(
                "/r/package.json",
                r#"{"packageManager": "yarn@4.1.0", "workspaces": ["apps/*"]}"#,
            )
            .with_file("/r/pnpm-workspace.yaml", "packages:\n  - 'old/*'\n")
            .with_file("/r/apps/a/package.json", r#"{"name": "a"}"#)
            .with_file("/r/old/o/package.json", r#"{"name": "o"}"#);
        assert_eq!(
            declared_member_dirs(&fs, Path::new("/r")),
            BTreeSet::from([PathBuf::from("/r/apps/a")])
        );
        let pnpm = MemoryFs::new()
            .with_file(
                "/r/package.json",
                r#"{"packageManager": "pnpm@9.0.0", "workspaces": ["apps/*"]}"#,
            )
            .with_file("/r/pnpm-workspace.yaml", "packages:\n  - 'old/*'\n")
            .with_file("/r/apps/a/package.json", r#"{"name": "a"}"#)
            .with_file("/r/old/o/package.json", r#"{"name": "o"}"#);
        assert_eq!(
            declared_member_dirs(&pnpm, Path::new("/r")),
            BTreeSet::from([PathBuf::from("/r/old/o")])
        );
    }

    #[test]
    fn ignored_dirs_are_excluded_from_a_workspace_without_leaving_it() {
        let fs = MemoryFs::new()
            .with_file("/r/package.json", r#"{"workspaces": ["p/*"]}"#)
            .with_file("/r/p/a/package.json", r#"{"name": "a"}"#)
            .with_file("/r/p/b/package.json", r#"{"name": "b"}"#);
        let mut workspace = Workspace::discover_with_fs(Path::new("/r"), &Config::default(), &fs).unwrap();
        workspace.ignore_dirs(&BTreeSet::from([PathBuf::from("/r/p/b")]));
        let ignored: Vec<_> = workspace
            .iter()
            .filter(|(id, _)| workspace.is_ignored(*id))
            .filter_map(|(_, package)| package.name.clone())
            .collect();
        assert_eq!(ignored, ["b"]);
        assert_eq!(workspace.packages().len(), 2);
    }

    fn rush_repo() -> MemoryFs {
        MemoryFs::new()
            .with_file(
                "/r/rush.json",
                "// rush\n{\"pnpmVersion\": \"9.0.0\", \"projects\": [{\"packageName\": \"a\", \"projectFolder\": \"apps/a\"}, {\"packageName\": \"b\", \"projectFolder\": \"libs/b\"}]}",
            )
            .with_file("/r/apps/a/package.json", r#"{"name": "a"}"#)
            .with_file("/r/libs/b/package.json", r#"{"name": "b"}"#)
            .with_file("/r/libs/b/sub/package.json", r#"{"name": "sub"}"#)
    }

    #[test]
    fn a_lerna_json_with_packages_declares_a_workspace() {
        let fs = MemoryFs::new()
            .with_file("/r/package.json", r#"{"name": "root"}"#)
            .with_file("/r/lerna.json", r#"{"packages": ["modules/*"]}"#)
            .with_file("/r/modules/a/package.json", r#"{"name": "a"}"#);
        assert_eq!(discover(&fs), ["ws:/r"]);
        assert!(declares_workspace(&fs, Path::new("/r")));
        assert_eq!(
            declared_member_dirs(&fs, Path::new("/r")),
            BTreeSet::from([PathBuf::from("/r/modules/a")])
        );
        let without_packages = MemoryFs::new()
            .with_file("/r/package.json", r#"{"name": "root"}"#)
            .with_file("/r/lerna.json", r#"{"useWorkspaces": true}"#);
        assert!(!declares_workspace(&without_packages, Path::new("/r")));
    }

    #[test]
    fn a_rush_json_declares_one_workspace_whose_projects_are_members() {
        assert_eq!(discover(&rush_repo()), ["ws:/r"]);
        assert!(declares_workspace(&rush_repo(), Path::new("/r")));
    }

    #[test]
    fn a_broken_rush_json_still_claims_nothing_but_is_a_root() {
        let fs = MemoryFs::new()
            .with_file("/r/rush.json", "{\"projects\": [")
            .with_file("/r/apps/a/package.json", r#"{"name": "a"}"#);
        assert_eq!(discover(&fs), ["ws:/r", "pkg:/r/apps/a"]);
    }

    #[test]
    fn declared_member_dirs_reads_the_rush_projects() {
        assert_eq!(
            declared_member_dirs(&rush_repo(), Path::new("/r")),
            BTreeSet::from([PathBuf::from("/r/apps/a"), PathBuf::from("/r/libs/b")])
        );
    }

    fn fixture_tree() -> MemoryFs {
        MemoryFs::new()
            .with_file("/r/package.json", r#"{"name": "root"}"#)
            .with_file("/r/real/package.json", r#"{"name": "real"}"#)
            .with_file("/r/real/fixtures/x/package.json", r#"{"name": "x"}"#)
            .with_file("/r/__fixtures__/y/package.json", r#"{"name": "y"}"#)
            .with_file("/r/real/__mocks__/m/package.json", r#"{"name": "m"}"#)
            .with_file("/r/real/__snapshots__/s/package.json", r#"{"name": "s"}"#)
            .with_file("/r/test/e2e/z/package.json", r#"{"name": "z"}"#)
    }

    #[test]
    fn fixture_directories_are_not_searched_for_roots_by_default() {
        assert_eq!(
            discover(&fixture_tree()),
            ["pkg:/r", "pkg:/r/real", "pkg:/r/test/e2e/z"]
        );
    }

    #[test]
    fn the_start_directory_is_never_skipped_even_when_it_is_named_like_a_fixture() {
        let fs = MemoryFs::new().with_file("/r/fixtures/a/package.json", r#"{"name": "a"}"#);
        let roots = discover_roots(Path::new("/r/fixtures"), &fs, &DiscoverOptions::default());
        assert_eq!(paths(&roots), ["pkg:/r/fixtures/a"]);
    }

    #[test]
    fn the_skipped_directories_are_configurable() {
        let options = |patterns: &[&str]| {
            DiscoverOptions::default().with_ignore(patterns.iter().map(|pattern| (*pattern).to_owned()))
        };
        let found = |options: &DiscoverOptions| paths(&discover_roots(Path::new("/r"), &fixture_tree(), options));
        assert_eq!(found(&options(&[])).len(), 7);
        assert_eq!(
            found(&options(&["test/e2e"])),
            [
                "pkg:/r",
                "pkg:/r/__fixtures__/y",
                "pkg:/r/real",
                "pkg:/r/real/__mocks__/m",
                "pkg:/r/real/__snapshots__/s",
                "pkg:/r/real/fixtures/x"
            ]
        );
        assert_eq!(found(&options(&["fixtures"])).len(), 6);
        assert!(found(&options(&["fixtures", "!real/fixtures"])).contains(&"pkg:/r/real/fixtures/x".to_owned()));
    }

    #[test]
    fn a_root_manifest_with_a_bom_comments_or_trailing_commas_still_declares_its_workspace() {
        let manifests = [
            "\u{feff}{\"workspaces\": [\"p/*\"]}",
            "{\n  // the members\n  \"workspaces\": [\"p/*\",],\n}",
            "/* header */ {\"workspaces\": {\"packages\": [\"p/*\"]}}",
        ];
        for manifest in manifests {
            let fs = MemoryFs::new()
                .with_file("/r/package.json", manifest)
                .with_file("/r/p/a/package.json", r#"{"name": "a"}"#);
            assert!(declares_workspace(&fs, Path::new("/r")), "{manifest}");
            assert_eq!(discover(&fs), ["ws:/r"], "{manifest}");
        }
    }

    #[test]
    fn declared_member_dirs_reads_a_manifest_with_comments() {
        let fs = MemoryFs::new()
            .with_file("/r/package.json", "\u{feff}{\n // c\n \"workspaces\": [\"p/*\"]}")
            .with_file("/r/p/m/package.json", r#"{"name": "m"}"#);
        assert_eq!(
            declared_member_dirs(&fs, Path::new("/r")),
            BTreeSet::from([PathBuf::from("/r/p/m")])
        );
    }

    #[test]
    fn a_member_whose_manifest_cannot_be_parsed_is_claimed_not_a_second_root() {
        let fs = MemoryFs::new()
            .with_file("/r/package.json", r#"{"workspaces": ["p/*"]}"#)
            .with_file("/r/p/good/package.json", r#"{"name": "good"}"#)
            .with_file("/r/p/bad/package.json", "{ not json");
        assert_eq!(discover(&fs), ["ws:/r"]);
        let workspace = Workspace::discover_with_fs(Path::new("/r"), &Config::default(), &fs).unwrap();
        assert_eq!(workspace.manifest_errors.len(), 1);
        assert_eq!(
            workspace.claimed_dirs(),
            BTreeSet::from([PathBuf::from("/r/p/bad"), PathBuf::from("/r/p/good")])
        );
        assert_eq!(workspace.member_dirs(), BTreeSet::from([PathBuf::from("/r/p/good")]));
    }

    /// A tree in which listing one directory fails.
    struct Unreadable {
        inner: MemoryFs,
        broken: PathBuf,
    }

    impl FileSystem for Unreadable {
        fn read_to_string(&self, path: &Path) -> io::Result<Arc<str>> {
            self.inner.read_to_string(path)
        }

        fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
            if path == self.broken {
                Err(io::Error::from(io::ErrorKind::PermissionDenied))
            } else {
                self.inner.read_dir(path)
            }
        }

        fn is_file(&self, path: &Path) -> bool {
            self.inner.is_file(path)
        }

        fn is_dir(&self, path: &Path) -> bool {
            self.inner.is_dir(path)
        }
    }

    #[test]
    fn a_directory_that_cannot_be_listed_is_reported_and_its_siblings_are_still_found() {
        let fs = Unreadable {
            inner: MemoryFs::new()
                .with_file("/r/ok/package.json", r#"{"name": "ok"}"#)
                .with_file("/r/locked/package.json", r#"{"name": "locked"}"#),
            broken: PathBuf::from("/r/locked"),
        };
        let found = scan_candidates(Path::new("/r"), &fs, &DiscoverOptions::default());
        assert_eq!(found.packages, BTreeSet::from([PathBuf::from("/r/ok")]));
        assert_eq!(found.unreadable, [PathBuf::from("/r/locked")]);
    }
}
