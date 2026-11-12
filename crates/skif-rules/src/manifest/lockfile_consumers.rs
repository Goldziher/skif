//! The lockfiles a build installs from, found by reading build files, never by running anything.
//!
//! Bazel is looked for only in a Bazel repository: the nearest directory at or above the workspace root, and not above
//! the git repository's root, that holds `MODULE.bazel`, `REPO.bazel`, `WORKSPACE.bazel` or `WORKSPACE`. A Bazel file
//! names a lockfile by a label of the main repository: `"//libs/a:pnpm-lock.yaml"`, also `@//`, `@@//` and
//! `@<module name>//`, and with the directory in the target (`"//:libs/a/pnpm-lock.yaml"`). The files read are those
//! Bazel reads: the `MODULE.bazel`, `REPO.bazel`, `WORKSPACE.bazel` and `WORKSPACE` at the top of the Bazel repository,
//! the `BUILD` and `BUILD.bazel` files of each lockfile's directory and the directories above it, and every `.bzl`
//! (or `*.MODULE.bazel`) file of the main repository those files load or name by a label, transitively. Only when
//! those leave a lockfile unnamed is the Bazel repository listed for its `BUILD` files, which are read with the files
//! they load, so that a lockfile only another package's `BUILD` file names (through an implicitly exported file) is
//! found too; the listing does not enter hidden directories, `node_modules`, symbolic links or the directories
//! `.bazelignore` names. A `BUILD` file also names a lockfile of its own directory or below by a relative path
//! (`"pnpm-lock.yaml"`, `":a/pnpm-lock.yaml"`), and the `BUILD` file of the lockfile's package by a `glob(...)` whose
//! include patterns match it and whose exclude patterns do not. A label also counts inside a longer string where
//! `$(location ...)`, `$(execpath ...)` or another location function expands it (a `genrule`'s `cmd`). A literal in
//! an `exclude = [...]` list names nothing, and neither does a docstring.
//!
//! A Dockerfile names a lockfile when a `COPY` or `ADD` from the build context copies it, by name or by a glob
//! (`pnpm-lock.yaml*`), with the build context taken as the Dockerfile's directory or the workspace root; and when a
//! Dockerfile of the lockfile's directory copies the whole context (`COPY . .`) and then installs with the lockfile's
//! package manager. A build command in a script or workflow (`docker build -f docker/web.Dockerfile apps/web`) or a
//! Compose file at the root (`build.context`) also gives a Dockerfile elsewhere the member's directory as its context.
//! The Dockerfiles are those the toolchain scan reads, those of the lockfiles' directories and those such builds name.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use skif_config::Selector;
use skif_core::{FileSystem, slash};
use skif_lint::LintContext;
use skif_workspace::{LOCKFILES, PackageManager, bazel_workspace_name, repository_root};

use crate::paths::{file_name, join, parent};
use crate::starlark::{self, Kind, Token};
use crate::toolchain::{self, DockerfileCopies};

/// The files that mark the root of a Bazel repository.
const BAZEL_ROOT_FILES: [&str; 4] = ["MODULE.bazel", "REPO.bazel", "WORKSPACE.bazel", "WORKSPACE"];

/// The files that declare a Bazel package, whose relative paths refer to that package's directory.
const BUILD_FILES: [&str; 2] = ["BUILD", "BUILD.bazel"];

/// Whether a label's target is a file Bazel loads Starlark from.
fn is_starlark_target(target: &str) -> bool {
    let name = file_name(target);
    name.rsplit_once('.')
        .is_some_and(|(stem, extension)| !stem.is_empty() && extension == "bzl")
        || name
            .strip_suffix(".bazel")
            .is_some_and(|stem| stem.rsplit_once('.').is_some_and(|(_, last)| last == "MODULE"))
}

/// The package manager that installs from the lockfile at `path`.
fn manager_of(path: &str) -> Option<PackageManager> {
    match file_name(path) {
        "package-lock.json" | "npm-shrinkwrap.json" => Some(PackageManager::Npm),
        "pnpm-lock.yaml" => Some(PackageManager::Pnpm),
        "yarn.lock" => Some(PackageManager::Yarn),
        "bun.lock" | "bun.lockb" => Some(PackageManager::Bun),
        _ => None,
    }
}

/// The functions of a `genrule` command or `args` that expand a label to a path: `$(location //libs/a:x)`.
const LOCATION_FUNCTIONS: [&str; 8] = [
    "location",
    "locations",
    "execpath",
    "execpaths",
    "rootpath",
    "rootpaths",
    "rlocationpath",
    "rlocationpaths",
];

/// How deep the listing of a Bazel repository for its `BUILD` files descends.
const MAX_DEPTH: usize = 32;

/// The labels that `$(location <label>)` and the other [`LOCATION_FUNCTIONS`] expand inside `literal`, as a
/// `genrule`'s `cmd` writes them.
fn expanded_labels(literal: &str) -> impl Iterator<Item = &str> {
    literal.match_indices("$(").filter_map(move |(at, _)| {
        let (function, rest) = literal[at + 2..].split_once(char::is_whitespace)?;
        if !LOCATION_FUNCTIONS.contains(&function) {
            return None;
        }
        let rest = rest.trim_start();
        let end = rest.find(|c: char| c == ')' || c.is_whitespace())?;
        Some(&rest[..end]).filter(|label| !label.is_empty())
    })
}

/// A `glob(...)` call: the patterns it includes and those it excludes, relative to its package.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Glob {
    include: Vec<String>,
    exclude: Vec<String>,
}

impl Glob {
    /// Whether the glob selects `path`, relative to its package.
    fn selects(&self, path: &str) -> bool {
        self.include.iter().any(|pattern| glob_matches(pattern, path))
            && !self.exclude.iter().any(|pattern| glob_matches(pattern, path))
    }
}

/// Whether the Bazel glob `pattern` matches `path`, both relative to a package: `**` is any number of segments and
/// `*` any run of characters within one.
fn glob_matches(pattern: &str, path: &str) -> bool {
    let pattern: Vec<&str> = pattern.split('/').collect();
    let path: Vec<&str> = path.split('/').collect();
    segments_match(&pattern, &path)
}

fn segments_match(pattern: &[&str], path: &[&str]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some((&"**", rest)) => (0..=path.len()).any(|skip| segments_match(rest, &path[skip..])),
        Some((first, rest)) => path
            .split_first()
            .is_some_and(|(segment, others)| segment_matches(first, segment) && segments_match(rest, others)),
    }
}

fn segment_matches(pattern: &str, name: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == name,
        Some((prefix, rest)) => name.strip_prefix(prefix).is_some_and(|tail| {
            (0..=tail.len())
                .filter(|&skip| tail.is_char_boundary(skip))
                .any(|skip| segment_matches(rest, &tail[skip..]))
        }),
    }
}

/// What a Starlark file says that can name a lockfile.
#[derive(Debug, Default, PartialEq, Eq)]
struct Starlark<'s> {
    /// Its string literals, escapes kept as written, then the labels a `$(location ...)` inside one expands; without
    /// those of an `exclude = [...]` list and without docstrings (a string that is the first statement of the file or
    /// of a `def`).
    literals: Vec<&'s str>,
    /// Its `glob(...)` calls.
    globs: Vec<Glob>,
}

impl<'s> Starlark<'s> {
    fn read(source: &'s str) -> Self {
        let tokens = starlark::tokens(source);
        let mut literals = Vec::new();
        let mut expanded = Vec::new();
        let mut depth = 0_usize;
        // The bracket depth inside the open `exclude = [` list, if any.
        let mut exclude: Option<usize> = None;
        // The bracket depth of a `def` whose `:` has not come yet.
        let mut header: Option<usize> = None;
        let mut docstring = true;
        for (index, token) in tokens.iter().enumerate() {
            let at_docstring = std::mem::replace(&mut docstring, false);
            match token.kind {
                Kind::Str if exclude.is_none() && !at_docstring => {
                    let literal = token.text(source);
                    literals.push(literal);
                    expanded.extend(expanded_labels(literal));
                }
                Kind::Punct(b'(' | b'[' | b'{') => depth += 1,
                Kind::Punct(b')' | b']' | b'}') => {
                    if exclude == Some(depth) {
                        exclude = None;
                    }
                    depth = depth.saturating_sub(1);
                }
                Kind::Punct(b':') if header == Some(depth) => {
                    header = None;
                    docstring = true;
                }
                Kind::Name if token.is_name(source, "def") => header = Some(depth),
                Kind::Name
                    if exclude.is_none()
                        && token.is_name(source, "exclude")
                        && tokens.get(index + 1).is_some_and(|next| next.is(b'='))
                        && tokens.get(index + 2).is_some_and(|next| next.is(b'[')) =>
                {
                    exclude = Some(depth + 1);
                }
                _ => {}
            }
        }
        literals.extend(expanded);
        Self {
            literals,
            globs: globs(source, &tokens),
        }
    }
}

/// The index of the `,` that ends the argument starting at token `start` of a call closed at `close`, or `close`.
fn argument_end(tokens: &[Token], start: usize, close: usize) -> usize {
    let mut depth = 0_usize;
    for (index, token) in tokens.iter().enumerate().take(close).skip(start) {
        match token.kind {
            Kind::Punct(b'(' | b'[' | b'{') => depth += 1,
            Kind::Punct(b')' | b']' | b'}') => depth = depth.saturating_sub(1),
            Kind::Punct(b',') if depth == 0 => return index,
            _ => {}
        }
    }
    close
}

/// The `glob(...)` calls among `tokens`, also as `native.glob(...)`: the string literals of the list given first or
/// as `include`, and of the one given second or as `exclude`.
fn globs(source: &str, tokens: &[Token]) -> Vec<Glob> {
    let mut found = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        if !token.is_name(source, "glob") || !tokens.get(index + 1).is_some_and(|next| next.is(b'(')) {
            continue;
        }
        let close = starlark::closing(tokens, index + 1);
        let mut glob = Glob::default();
        let mut positional = 0;
        let mut at = index + 2;
        while at < close {
            let named = tokens[at].kind == Kind::Name && tokens.get(at + 1).is_some_and(|next| next.is(b'='));
            let (include, list) = if named {
                match tokens[at].text(source) {
                    "include" => (Some(true), at + 2),
                    "exclude" => (Some(false), at + 2),
                    _ => (None, at + 2),
                }
            } else {
                positional += 1;
                let include = match positional {
                    1 => Some(true),
                    2 => Some(false),
                    _ => None,
                };
                (include, at)
            };
            let end = argument_end(tokens, list, close);
            if let Some(include) = include
                && tokens.get(list).is_some_and(|open| open.is(b'['))
            {
                let patterns = tokens[list..end]
                    .iter()
                    .filter(|token| token.kind == Kind::Str)
                    .map(|token| token.text(source).to_owned());
                if include {
                    glob.include.extend(patterns);
                } else {
                    glob.exclude.extend(patterns);
                }
            }
            at = end + 1;
        }
        found.push(glob);
    }
    found
}

/// Whether `source` can name a lockfile: it holds a lockfile's name, a `glob(` or the name of a Starlark file it may
/// load. A file that cannot is not lexed.
fn may_name_lockfile(source: &str) -> bool {
    LOCKFILES.iter().any(|name| source.contains(name))
        || source.contains("glob(")
        || source.contains(".bzl")
        || source.contains(".MODULE.bazel")
}

/// The Bazel repository the workspace root belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
struct BazelRepository {
    /// The directory that holds one of [`BAZEL_ROOT_FILES`].
    dir: PathBuf,
    /// The workspace root's path below [`dir`](Self::dir) with `/` separators, `""` when they are the same.
    below: String,
    /// The name its `module(...)` or `workspace(...)` call gives it.
    name: Option<String>,
}

impl BazelRepository {
    /// The nearest directory at or above `root` that holds one of [`BAZEL_ROOT_FILES`]; above `root` only within the
    /// git repository `root` is in, so that a stray file higher up is not taken for it.
    fn find(fs: &dyn FileSystem, root: &Path) -> Option<Self> {
        let ceiling = repository_root(fs, root);
        let dir = root
            .ancestors()
            .take_while(|dir| *dir == root || ceiling.as_deref().is_some_and(|top| dir.starts_with(top)))
            .find(|dir| BAZEL_ROOT_FILES.iter().any(|name| fs.is_file(&dir.join(name))))?;
        Some(Self {
            below: slash(root.strip_prefix(dir).ok()?),
            name: bazel_workspace_name(fs, dir),
            dir: dir.to_path_buf(),
        })
    }

    /// The package and target of a label of the main repository: `//libs/a:x` is `("libs/a", "x")`, also with `@//`,
    /// `@@//` and `@<module name>//`. A label of another repository (`@npm//...`) is none, and so is a target that
    /// climbs out of its package.
    fn main_label<'l>(&self, literal: &'l str) -> Option<(&'l str, &'l str)> {
        let label = match literal.strip_prefix('@') {
            Some(canonical) if canonical.starts_with('@') => &canonical[1..],
            Some(apparent) => self
                .name
                .as_deref()
                .and_then(|name| apparent.strip_prefix(name))
                .filter(|rest| rest.starts_with("//"))
                .unwrap_or(apparent),
            None => literal,
        };
        let (package, target) = label.strip_prefix("//")?.split_once(':')?;
        (!target.split('/').any(|segment| segment == "..")).then_some((package, target))
    }

    /// The lockfile a label of the main repository names, as a path relative to the Bazel root:
    /// `//libs/a:pnpm-lock.yaml`, `//libs:a/pnpm-lock.yaml` and `//:libs/a/pnpm-lock.yaml` are
    /// `libs/a/pnpm-lock.yaml`.
    fn labelled_lockfile(&self, literal: &str) -> Option<String> {
        let (package, target) = self.main_label(literal)?;
        is_lockfile_target(target).then(|| join(package, target))
    }

    /// The Starlark file, relative to the Bazel root, that a file in `dir` loads or names by `literal`: a label of the
    /// main repository (`//tools:deps.bzl`) or one relative to `dir` (`:deps.bzl`).
    fn loaded_file(&self, dir: &str, literal: &str) -> Option<String> {
        let (package, target) = match literal.strip_prefix(':') {
            Some(target) => (dir, target),
            None => self.main_label(literal)?,
        };
        (is_starlark_target(target) && !target.split('/').any(|segment| segment == "..")).then(|| join(package, target))
    }

    /// The workspace-relative lockfiles the labels among `literals` name.
    fn labelled_in_workspace(&self, literals: &[&str]) -> Vec<String> {
        literals
            .iter()
            .filter_map(|literal| self.labelled_lockfile(literal))
            .filter_map(|relative| self.in_workspace(relative))
            .collect()
    }

    /// The workspace-relative path of `path`, relative to the Bazel root, when it lies in the workspace.
    fn in_workspace(&self, path: String) -> Option<String> {
        if self.below.is_empty() {
            Some(path)
        } else if path == self.below {
            Some(String::new())
        } else {
            path.strip_prefix(&format!("{}/", self.below)).map(str::to_owned)
        }
    }

    /// The path relative to the Bazel root of the workspace-relative `path`.
    fn bazel_path(&self, path: &str) -> String {
        join(&self.below, path)
    }
}

/// Whether a label's target names a lockfile: its last segment is one, and it does not climb out of its package.
fn is_lockfile_target(target: &str) -> bool {
    LOCKFILES.contains(&file_name(target)) && !target.split('/').any(|segment| segment == "..")
}

/// The lockfile a `BUILD` file in `dir` names by a path relative to it, plain or as a relative label:
/// `"pnpm-lock.yaml"`, `":pnpm-lock.yaml"`, `"a/pnpm-lock.yaml"`.
fn listed_lockfile(dir: &str, literal: &str) -> Option<String> {
    if literal.starts_with(['/', '@']) {
        return None;
    }
    let target = literal.strip_prefix(':').unwrap_or(literal);
    (!target.contains(':') && is_lockfile_target(target)).then(|| join(dir, target))
}

/// The reading of Bazel files for the lockfiles they name, each file read once.
struct Search<'a> {
    fs: &'a dyn FileSystem,
    repository: &'a BazelRepository,
    /// The files queued so far, relative to the Bazel root.
    seen: BTreeSet<String>,
    /// The workspace-relative lockfiles named so far.
    consumed: BTreeSet<String>,
    /// The `glob(...)` calls of each `BUILD` file read, by its directory relative to the Bazel root.
    globs: BTreeMap<String, Vec<Glob>>,
}

impl<'a> Search<'a> {
    fn new(fs: &'a dyn FileSystem, repository: &'a BazelRepository) -> Self {
        Self {
            fs,
            repository,
            seen: BTreeSet::new(),
            consumed: BTreeSet::new(),
            globs: BTreeMap::new(),
        }
    }

    /// Reads the files at `paths` (relative to the Bazel root) that exist and were not queued before, and the
    /// Starlark files they load, transitively.
    fn read(&mut self, paths: impl IntoIterator<Item = String>) {
        let repository = self.repository;
        let mut queue: Vec<String> = paths
            .into_iter()
            .filter(|path| !self.seen.contains(path) && self.fs.is_file(&repository.dir.join(path)))
            .collect();
        self.seen.extend(queue.iter().cloned());
        while let Some(path) = queue.pop() {
            let Ok(source) = self.fs.read_to_string(&repository.dir.join(&path)) else {
                continue;
            };
            let dir = parent(&path);
            let build = BUILD_FILES.contains(&file_name(&path));
            if build {
                self.globs.entry(dir.to_owned()).or_default();
            }
            if !may_name_lockfile(&source) {
                continue;
            }
            let file = Starlark::read(&source);
            self.consumed.extend(repository.labelled_in_workspace(&file.literals));
            if build {
                if let Some(dir) = repository.in_workspace(dir.to_owned()) {
                    let listed = file
                        .literals
                        .iter()
                        .filter_map(|literal| listed_lockfile(&dir, literal));
                    self.consumed.extend(listed);
                }
                self.globs.entry(dir.to_owned()).or_default().extend(file.globs);
            }
            for loaded in file
                .literals
                .iter()
                .filter_map(|literal| repository.loaded_file(dir, literal))
            {
                if !self.seen.contains(&loaded) && self.fs.is_file(&repository.dir.join(&loaded)) {
                    self.seen.insert(loaded.clone());
                    queue.push(loaded);
                }
            }
        }
    }

    /// Whether a `glob(...)` of the `BUILD` file of the package `lockfile` (workspace-relative) belongs to selects
    /// it. The package is the nearest directory at or above the lockfile's that holds a `BUILD` file read; a glob
    /// does not reach into the directories of other packages.
    fn globbed(&self, lockfile: &str) -> bool {
        let path = self.repository.bazel_path(lockfile);
        ancestors(parent(&path))
            .into_iter()
            .find_map(|dir| self.globs.get(dir).map(|globs| (dir, globs)))
            .is_some_and(|(dir, globs)| {
                let relative = if dir.is_empty() {
                    path.as_str()
                } else {
                    &path[dir.len() + 1..]
                };
                globs.iter().any(|glob| glob.selects(relative))
            })
    }
}

/// The `BUILD` and `BUILD.bazel` files below the Bazel root `dir`, relative to it, found by listing its directories.
/// Hidden directories, `node_modules`, symbolic links (the `bazel-*` convenience links among them) and the
/// directories `.bazelignore` names are not entered, as Bazel does not look for packages there.
fn listed_build_files(fs: &dyn FileSystem, dir: &Path) -> Vec<String> {
    let ignored: BTreeSet<String> = fs
        .read_to_string(&dir.join(".bazelignore"))
        .map(|text| {
            text.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !line.starts_with('#'))
                .map(|line| line.trim_end_matches('/').to_owned())
                .collect()
        })
        .unwrap_or_default();
    let mut found = Vec::new();
    let mut pending = vec![(String::new(), 0_usize)];
    while let Some((relative, depth)) = pending.pop() {
        let Ok(entries) = fs.read_dir_shared(&dir.join(&relative)) else {
            continue;
        };
        for entry in entries.iter() {
            let Some(name) = entry.path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let path = join(&relative, name);
            if entry.is_file && BUILD_FILES.contains(&name) {
                found.push(path);
            } else if entry.is_dir
                && !entry.is_symlink
                && !name.starts_with('.')
                && name != "node_modules"
                && depth < MAX_DEPTH
                && !ignored.contains(&path)
            {
                pending.push((path, depth + 1));
            }
        }
    }
    found.sort_unstable();
    found
}

/// Which of `lockfiles` (workspace-relative) the Bazel files name. Nothing is read outside a Bazel repository. The
/// files Bazel reads to find each lockfile come first, and no directory is listed when they name every lockfile;
/// otherwise the repository is listed for its `BUILD` files, which are read with the files they load, for a lockfile
/// that only another package names.
fn bazel_consumers(fs: &dyn FileSystem, root: &Path, lockfiles: &[String]) -> BTreeSet<String> {
    let Some(repository) = BazelRepository::find(fs, root) else {
        return BTreeSet::new();
    };
    let repo = &repository;
    let dirs: BTreeSet<&str> = lockfiles.iter().map(|lockfile| parent(lockfile)).collect();
    let build_files = dirs
        .iter()
        .flat_map(|dir| ancestors(dir))
        .flat_map(move |dir| BUILD_FILES.iter().map(move |name| repo.bazel_path(&join(dir, name))));
    let first: BTreeSet<String> = BAZEL_ROOT_FILES
        .iter()
        .map(|name| (*name).to_owned())
        .chain(build_files)
        .collect();
    let mut search = Search::new(fs, repo);
    search.read(first);
    let named = |search: &Search<'_>, lockfile: &String| search.consumed.contains(lockfile) || search.globbed(lockfile);
    if !lockfiles.iter().all(|lockfile| named(&search, lockfile)) {
        search.read(listed_build_files(fs, &repository.dir));
    }
    lockfiles
        .iter()
        .filter(|lockfile| named(&search, lockfile))
        .cloned()
        .collect()
}

/// `dir` and the directories above it, up to the root (`""`).
fn ancestors(dir: &str) -> Vec<&str> {
    let mut found = vec![dir];
    let mut current = dir;
    while !current.is_empty() {
        current = parent(current);
        found.push(current);
    }
    found
}

/// Whether `source`, resolved from the build context `context`, is `lockfile` or a glob that matches it.
fn names(context: &str, source: &str, lockfile: &str) -> bool {
    let path = join(context, source.trim_start_matches("./").trim_start_matches('/'));
    path == lockfile
        || (source.contains(['*', '?', '[']) && Selector::new([path]).is_ok_and(|glob| glob.matches(lockfile, false)))
}

/// Whether a Dockerfile copies `lockfile` (workspace-relative): a source names it from the Dockerfile's directory,
/// from the workspace root or from a context a build gives the Dockerfile, or the Dockerfile copies the whole context
/// of the lockfile's directory (its own, or one a build gives it) and then installs with the lockfile's package
/// manager.
fn copies(dockerfile: &DockerfileCopies, lockfile: &str) -> bool {
    let dir = parent(&dockerfile.path);
    let named_from = |context: &str| dockerfile.sources.iter().any(|source| names(context, source, lockfile));
    let installs =
        || manager_of(lockfile).is_some_and(|manager| dockerfile.installs_after_context_copy.contains(&manager));
    let from_context = |context: &str| named_from(context) || (parent(lockfile) == context && installs());
    named_from("")
        || (parent(lockfile) == dir && from_context(dir))
        || dockerfile.contexts.iter().any(|context| from_context(context))
}

/// Which of `lockfiles` (workspace-relative, `/`-separated) a Dockerfile or a Bazel file of the workspace names; the
/// Bazel files are searched only for those no Dockerfile copies.
pub(super) fn consumed(cx: &LintContext<'_>, lockfiles: &[String]) -> BTreeSet<String> {
    if lockfiles.is_empty() {
        return BTreeSet::new();
    }
    let dirs: BTreeSet<&str> = lockfiles.iter().map(|lockfile| parent(lockfile)).collect();
    let dockerfiles = toolchain::dockerfile_copies(cx, &dirs);
    let (copied, rest): (Vec<String>, Vec<String>) = lockfiles
        .iter()
        .cloned()
        .partition(|lockfile| dockerfiles.iter().any(|dockerfile| copies(dockerfile, lockfile)));
    let mut consumed: BTreeSet<String> = copied.into_iter().collect();
    if !rest.is_empty() {
        consumed.extend(bazel_consumers(cx.fs(), &cx.workspace.root, &rest));
    }
    consumed
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use skif_core::{DirEntry, MemoryFs};

    use super::*;

    /// A [`MemoryFs`] that counts its directory listings and file reads.
    struct Counting {
        inner: MemoryFs,
        listings: AtomicUsize,
        reads: AtomicUsize,
    }

    impl Counting {
        fn new(inner: MemoryFs) -> Self {
            Self {
                inner,
                listings: AtomicUsize::new(0),
                reads: AtomicUsize::new(0),
            }
        }
    }

    impl FileSystem for Counting {
        fn read_to_string(&self, path: &Path) -> io::Result<Arc<str>> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            self.inner.read_to_string(path)
        }

        fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
            self.listings.fetch_add(1, Ordering::SeqCst);
            self.inner.read_dir(path)
        }

        fn is_file(&self, path: &Path) -> bool {
            self.inner.is_file(path)
        }

        fn is_dir(&self, path: &Path) -> bool {
            self.inner.is_dir(path)
        }
    }

    fn repository(below: &str, name: Option<&str>) -> BazelRepository {
        BazelRepository {
            dir: PathBuf::from("/repo"),
            below: below.to_owned(),
            name: name.map(str::to_owned),
        }
    }

    fn consumers(fs: &dyn FileSystem, root: &str, lockfiles: &[&str]) -> Vec<String> {
        let lockfiles: Vec<String> = lockfiles.iter().map(|lockfile| (*lockfile).to_owned()).collect();
        bazel_consumers(fs, Path::new(root), &lockfiles).into_iter().collect()
    }

    fn string_literals(source: &str) -> Vec<&str> {
        Starlark::read(source).literals
    }

    #[test]
    fn string_literals_skip_comments_and_keep_every_quote_style() {
        let source = "# \"//commented:pnpm-lock.yaml\"\nx = [\"a\", 'b', \"\"\"c\nd\"\"\", \"e\\\"f\"]  # 'tail'\n";
        assert_eq!(string_literals(source), ["a", "b", "c\nd", "e\\\"f"]);
    }

    #[test]
    fn string_literals_skip_exclude_lists() {
        let source =
            "srcs = glob([\"**/*\"], exclude = [\"pnpm-lock.yaml\", [\"x\"]], my_exclude = [\"kept\"]) + [\"after\"]\n";
        assert_eq!(string_literals(source), ["**/*", "kept", "after"]);
    }

    #[test]
    fn string_literals_skip_docstrings_of_the_file_and_of_functions() {
        let source = "\
\"\"\"//libs/a:pnpm-lock.yaml\"\"\"
load(\"//tools:deps.bzl\", \"deps\")
def lock(name, default = {\"k\": \"v\"}):
    \"//libs/b:pnpm-lock.yaml\"
    return \"//libs/c:pnpm-lock.yaml\"
x = {\"key\": \"value\"}
";
        assert_eq!(
            string_literals(source),
            [
                "//tools:deps.bzl",
                "deps",
                "k",
                "v",
                "//libs/c:pnpm-lock.yaml",
                "key",
                "value"
            ]
        );
    }

    #[test]
    fn labels_a_location_function_expands_count_inside_longer_strings() {
        let source = "genrule(cmd = \"cp $(location //libs/a:pnpm-lock.yaml) $@ && cat $(execpath  :yarn.lock)\", \
                      args = [\"$(rlocationpath @npm//:x)\", \"$(PWD) $(locations\"])\n";
        let literals = string_literals(source);
        assert_eq!(
            &literals[literals.len() - 3..],
            ["//libs/a:pnpm-lock.yaml", ":yarn.lock", "@npm//:x"]
        );
    }

    #[test]
    fn globs_read_their_include_and_exclude_lists_by_position_or_name() {
        let source = "\
a = glob([\"**/pnpm-lock.yaml\", \"*.json\"], [\"x/**\"])
b = native.glob(include = [\"sub/*\"], exclude = [\"sub/yarn.lock\"], allow_empty = True)
c = glob(exclude = [\"z\"], include = [\"**\"])
d = glob(SRCS)
";
        let glob = |include: &[&str], exclude: &[&str]| Glob {
            include: include.iter().map(|pattern| (*pattern).to_owned()).collect(),
            exclude: exclude.iter().map(|pattern| (*pattern).to_owned()).collect(),
        };
        assert_eq!(
            Starlark::read(source).globs,
            [
                glob(&["**/pnpm-lock.yaml", "*.json"], &["x/**"]),
                glob(&["sub/*"], &["sub/yarn.lock"]),
                glob(&["**"], &["z"]),
                glob(&[], &[]),
            ]
        );
    }

    #[test]
    fn globs_match_as_bazel_matches_them() {
        let cases = [
            ("**/pnpm-lock.yaml", "pnpm-lock.yaml", true),
            ("**/pnpm-lock.yaml", "a/b/pnpm-lock.yaml", true),
            ("**", "a/pnpm-lock.yaml", true),
            ("*", "pnpm-lock.yaml", true),
            ("*", "a/pnpm-lock.yaml", false),
            ("pnpm-*.yaml", "pnpm-lock.yaml", true),
            ("*.json", "pnpm-lock.yaml", false),
            ("a/**/*.yaml", "a/pnpm-lock.yaml", true),
            ("b/**", "a/pnpm-lock.yaml", false),
        ];
        for (pattern, path, expected) in cases {
            assert_eq!(glob_matches(pattern, path), expected, "{pattern} {path}");
        }
        let excluding = Glob {
            include: vec!["**".to_owned()],
            exclude: vec!["pnpm-lock.yaml".to_owned()],
        };
        assert!(!excluding.selects("pnpm-lock.yaml"));
        assert!(excluding.selects("package.json"));
    }

    #[test]
    fn labels_of_the_main_repository_name_lockfiles() {
        let cases = [
            ("//libs/a:pnpm-lock.yaml", Some("libs/a/pnpm-lock.yaml")),
            ("@//libs/a:package-lock.json", Some("libs/a/package-lock.json")),
            ("@@//apps/web:yarn.lock", Some("apps/web/yarn.lock")),
            ("//:bun.lock", Some("bun.lock")),
            ("//:libs/a/pnpm-lock.yaml", Some("libs/a/pnpm-lock.yaml")),
            ("//libs:a/pnpm-lock.yaml", Some("libs/a/pnpm-lock.yaml")),
            ("@mono//libs/a:pnpm-lock.yaml", Some("libs/a/pnpm-lock.yaml")),
            ("@npm//libs/a:pnpm-lock.yaml", None),
            ("@@mono//libs/a:pnpm-lock.yaml", None),
            ("//libs/a:package.json", None),
            ("//libs/a:../b/pnpm-lock.yaml", None),
            ("//libs/a", None),
            ("libs/a/pnpm-lock.yaml", None),
        ];
        let named = repository("", Some("mono"));
        for (literal, expected) in cases {
            assert_eq!(named.labelled_lockfile(literal).as_deref(), expected, "{literal}");
        }
        assert_eq!(
            repository("", None).labelled_lockfile("@mono//libs/a:pnpm-lock.yaml"),
            None
        );
    }

    #[test]
    fn a_build_file_lists_lockfiles_of_its_own_directory_and_below() {
        let cases = [
            ("pnpm-lock.yaml", Some("libs/a/pnpm-lock.yaml")),
            (":yarn.lock", Some("libs/a/yarn.lock")),
            ("sub/pnpm-lock.yaml", Some("libs/a/sub/pnpm-lock.yaml")),
            (":sub/pnpm-lock.yaml", Some("libs/a/sub/pnpm-lock.yaml")),
            ("../b/pnpm-lock.yaml", None),
            ("//:libs/b/pnpm-lock.yaml", None),
            ("@npm//:pnpm-lock.yaml", None),
            ("src/index.ts", None),
        ];
        for (literal, expected) in cases {
            assert_eq!(listed_lockfile("libs/a", literal).as_deref(), expected, "{literal}");
        }
    }

    #[test]
    fn starlark_files_are_followed_through_labels() {
        let named = repository("", Some("mono"));
        let cases = [
            ("//tools:deps.bzl", Some("tools/deps.bzl")),
            ("@mono//tools:sub/deps.bzl", Some("tools/sub/deps.bzl")),
            (":local.bzl", Some("libs/a/local.bzl")),
            ("//:extra.MODULE.bazel", Some("extra.MODULE.bazel")),
            ("@rules_js//npm:repositories.bzl", None),
            ("//tools:BUILD", None),
            ("//tools:.bzl", None),
            ("//tools:../x.bzl", None),
            ("deps.bzl", None),
        ];
        for (literal, expected) in cases {
            assert_eq!(named.loaded_file("libs/a", literal).as_deref(), expected, "{literal}");
        }
    }

    #[test]
    fn labels_are_relative_to_a_bazel_root_above_the_workspace() {
        let fs = MemoryFs::new()
            .with_file("/repo/.git/HEAD", "")
            .with_file(
                "/repo/MODULE.bazel",
                "npm.npm_translate_lock(name = \"a\", pnpm_lock = \"//js/libs/a:pnpm-lock.yaml\")\n",
            )
            .with_file("/repo/deps.bzl", "LOCK = \"//other/libs/b:pnpm-lock.yaml\"\n")
            .with_file("/repo/js/package.json", "{}")
            .with_file("/repo/js/apps/web/BUILD.bazel", "LOCK = \"//js/apps/web:yarn.lock\"\n")
            .with_file("/repo/js/libs/a/pnpm-lock.yaml", "");
        assert_eq!(
            consumers(&fs, "/repo/js", &["libs/a/pnpm-lock.yaml"]),
            ["libs/a/pnpm-lock.yaml"]
        );
    }

    #[test]
    fn the_bazel_root_is_not_looked_for_above_the_git_repository() {
        let fs = MemoryFs::new()
            .with_file("/MODULE.bazel", "LOCK = \"//repo/libs/a:pnpm-lock.yaml\"\n")
            .with_file("/repo/.git/HEAD", "")
            .with_file("/repo/package.json", "{}")
            .with_file("/repo/libs/a/pnpm-lock.yaml", "");
        assert_eq!(BazelRepository::find(&fs, Path::new("/repo")), None);
        let inside = fs.with_file("/repo/MODULE.bazel", "module(name = \"mono\")\n");
        assert_eq!(
            BazelRepository::find(&inside, Path::new("/repo")),
            Some(BazelRepository {
                dir: PathBuf::from("/repo"),
                below: String::new(),
                name: Some("mono".to_owned()),
            })
        );
    }

    #[test]
    fn without_a_bazel_root_nothing_is_listed_or_read() {
        let tree = MemoryFs::new()
            .with_file("/ws/package.json", "{}")
            .with_file("/ws/libs/a/BUILD.bazel", "exports_files([\"pnpm-lock.yaml\"])\n")
            .with_file("/ws/libs/a/pnpm-lock.yaml", "")
            .with_file("/ws/tools/deps.bzl", "LOCK = \"//libs/a:pnpm-lock.yaml\"\n");
        let without = Counting::new(tree.clone());
        assert_eq!(consumers(&without, "/ws", &["libs/a/pnpm-lock.yaml"]), [] as [&str; 0]);
        assert_eq!(without.listings.load(Ordering::SeqCst), 0);
        assert_eq!(without.reads.load(Ordering::SeqCst), 0);
        let with = Counting::new(tree.with_file("/ws/MODULE.bazel", ""));
        assert_eq!(
            consumers(&with, "/ws", &["libs/a/pnpm-lock.yaml"]),
            ["libs/a/pnpm-lock.yaml"]
        );
        assert_eq!(with.listings.load(Ordering::SeqCst), 0);
    }

    /// A Bazel repository whose lockfiles are named by the files Bazel reads for them (`a`, `b`, `e`), only by the
    /// `BUILD` file of another package or a macro it loads (`d`, `f`), by a `.bzl` file nothing loads (`c`), by a
    /// `BUILD` file below a `.bazelignore`d directory (`g`) and by a glob of their own package (`h`).
    fn bazel_tree() -> MemoryFs {
        MemoryFs::new()
            .with_file(
                "/ws/WORKSPACE",
                "load(\"//tools:deps.bzl\", \"deps\")\nload(\"@rules_js//npm:repositories.bzl\", \"x\")\n",
            )
            .with_file("/ws/.bazelignore", "# nested modules\nexamples/\n")
            .with_file(
                "/ws/tools/deps.bzl",
                "load(\":more.bzl\", \"m\")\nLOCK = \"//libs/a:pnpm-lock.yaml\"\n",
            )
            .with_file("/ws/tools/more.bzl", "LOCK = \"//libs/b:pnpm-lock.yaml\"\n")
            .with_file("/ws/tools/unloaded.bzl", "LOCK = \"//libs/c:pnpm-lock.yaml\"\n")
            .with_file("/ws/apps/x/BUILD", "LOCK = \"//libs/d:pnpm-lock.yaml\"\n")
            .with_file(
                "/ws/apps/y/BUILD.bazel",
                "load(\"//apps/y:lock.bzl\", \"lock\")\nlock()\n",
            )
            .with_file(
                "/ws/apps/y/lock.bzl",
                "def lock():\n    native.filegroup(srcs = [\"//libs/f:pnpm-lock.yaml\"])\n",
            )
            .with_file("/ws/examples/z/BUILD", "LOCK = \"//libs/g:pnpm-lock.yaml\"\n")
            .with_file("/ws/libs/e/BUILD.bazel", "load(\"//libs:macros.bzl\", \"m\")\n")
            .with_file("/ws/libs/macros.bzl", "LOCK = \"//libs/e:pnpm-lock.yaml\"\n")
            .with_file(
                "/ws/libs/h/BUILD.bazel",
                "filegroup(name = \"all\", srcs = glob([\"**/*.yaml\"]))\n",
            )
            .with_file("/ws/libs/h/sub/BUILD", "filegroup(name = \"sub\")\n")
    }

    fn lockfiles(names: &str) -> Vec<String> {
        names
            .chars()
            .map(|name| format!("libs/{name}/pnpm-lock.yaml"))
            .collect()
    }

    #[test]
    fn when_the_files_bazel_reads_name_every_lockfile_no_directory_is_listed() {
        let counting = Counting::new(bazel_tree());
        let judged = lockfiles("abeh");
        let judged: Vec<&str> = judged.iter().map(String::as_str).collect();
        assert_eq!(consumers(&counting, "/ws", &judged), judged);
        assert_eq!(counting.listings.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_lockfile_only_another_package_names_is_found_by_listing_the_build_files() {
        let counting = Counting::new(bazel_tree());
        let judged = lockfiles("abcdefgh");
        let judged: Vec<&str> = judged.iter().map(String::as_str).collect();
        assert_eq!(consumers(&counting, "/ws", &judged), lockfiles("abdefh"));
        assert!(counting.listings.load(Ordering::SeqCst) > 0);
    }

    #[test]
    fn a_glob_reaches_only_into_its_own_package() {
        let judged = ["libs/h/sub/pnpm-lock.yaml", "libs/h/pnpm-lock.yaml"];
        assert_eq!(consumers(&bazel_tree(), "/ws", &judged), ["libs/h/pnpm-lock.yaml"]);
    }

    #[test]
    fn the_listing_skips_hidden_and_ignored_directories_and_node_modules() {
        let fs = MemoryFs::new()
            .with_file("/ws/.bazelignore", "examples\n")
            .with_file("/ws/apps/BUILD", "")
            .with_file("/ws/apps/web/BUILD.bazel", "")
            .with_file("/ws/examples/BUILD", "")
            .with_file("/ws/.cache/BUILD", "")
            .with_file("/ws/node_modules/x/BUILD", "")
            .with_file("/ws/BUILD.bazel", "");
        assert_eq!(
            listed_build_files(&fs, Path::new("/ws")),
            ["BUILD.bazel", "apps/BUILD", "apps/web/BUILD.bazel"]
        );
    }

    #[test]
    fn a_dockerfile_copies_a_lockfile_of_its_own_directory_or_of_the_root_context() {
        let dockerfile = |path: &str, sources: &[&str], installs: &[PackageManager]| DockerfileCopies {
            path: path.to_owned(),
            sources: sources.iter().map(|source| (*source).to_owned()).collect(),
            installs_after_context_copy: installs.to_vec(),
            contexts: Vec::new(),
        };
        let lockfile = "apps/web/pnpm-lock.yaml";
        let cases = [
            (
                dockerfile("apps/web/Dockerfile", &["package.json", "pnpm-lock.yaml"], &[]),
                true,
            ),
            (dockerfile("apps/web/Dockerfile", &["./pnpm-lock.yaml"], &[]), true),
            (dockerfile("apps/web/Dockerfile", &["pnpm-lock.yaml*"], &[]), true),
            (
                dockerfile("apps/web/Dockerfile", &["apps/web/pnpm-lock.yaml"], &[]),
                true,
            ),
            (dockerfile("Dockerfile", &["apps/web/pnpm-lock.yaml"], &[]), true),
            (dockerfile("apps/web/Dockerfile", &["."], &[PackageManager::Pnpm]), true),
            (dockerfile("apps/web/Dockerfile", &["."], &[]), false),
            (dockerfile("apps/web/Dockerfile", &["."], &[PackageManager::Npm]), false),
            (dockerfile("Dockerfile", &["."], &[PackageManager::Pnpm]), false),
            (dockerfile("apps/web/Dockerfile", &["package.json"], &[]), false),
            (dockerfile("apps/Dockerfile", &["web/pnpm-lock.yaml"], &[]), false),
        ];
        for (copying, expected) in cases {
            assert_eq!(copies(&copying, lockfile), expected, "{copying:?}");
        }
        let built = |sources: &[&str], installs: &[PackageManager], context: &str| DockerfileCopies {
            contexts: vec![context.to_owned()],
            ..dockerfile("docker/web.Dockerfile", sources, installs)
        };
        let cases = [
            (built(&["package.json", "pnpm-lock.yaml"], &[], "apps/web"), true),
            (built(&["."], &[PackageManager::Pnpm], "apps/web"), true),
            (built(&["."], &[PackageManager::Npm], "apps/web"), false),
            (built(&["pnpm-lock.yaml"], &[], "apps/api"), false),
            (built(&["."], &[PackageManager::Pnpm], "apps"), false),
        ];
        for (copying, expected) in cases {
            assert_eq!(copies(&copying, lockfile), expected, "{copying:?}");
        }
    }
}
