//! The lockfiles a build installs from, found by reading build files, never by running anything.
//!
//! Bazel is looked for only in a Bazel repository: the nearest directory at or above the workspace root, and not above
//! the git repository's root, that holds `MODULE.bazel`, `REPO.bazel`, `WORKSPACE.bazel` or `WORKSPACE`. A Bazel file
//! names a lockfile by a label of the main repository: `"//libs/a:pnpm-lock.yaml"`, also `@//`, `@@//` and
//! `@<module name>//`, and with the directory in the target (`"//:libs/a/pnpm-lock.yaml"`). The files read are those
//! Bazel reads: the `MODULE.bazel`, `REPO.bazel`, `WORKSPACE.bazel` and `WORKSPACE` at the top of the Bazel repository,
//! the `BUILD` and `BUILD.bazel` files of each lockfile's directory and the directories above it, and every `.bzl`
//! (or `*.MODULE.bazel`) file of the main repository those files load or name by a label, transitively. Nothing else
//! is listed or read, so a lockfile only another package's `BUILD` file names, through an implicitly exported file, is
//! not found. A `BUILD` file also names a lockfile of its own directory or below by a relative path
//! (`"pnpm-lock.yaml"`, `":a/pnpm-lock.yaml"`). A literal in an `exclude = [...]` list names nothing.
//!
//! A Dockerfile names a lockfile when a `COPY` or `ADD` from the build context copies it, by name or by a glob
//! (`pnpm-lock.yaml*`), with the build context taken as the Dockerfile's directory or the workspace root; and when a
//! Dockerfile of the lockfile's directory copies the whole context (`COPY . .`) and then installs with the lockfile's
//! package manager. The Dockerfiles are those the toolchain scan reads and those of the lockfiles' directories.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use skif_config::Selector;
use skif_core::{FileSystem, slash};
use skif_lint::LintContext;
use skif_workspace::{LOCKFILES, PackageManager, bazel_workspace_name, repository_root};

use crate::paths::{file_name, join, parent};
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

/// The end of the identifier that starts at byte `at` of `source`.
fn identifier_end(source: &str, at: usize) -> usize {
    source[at..]
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .map_or(source.len(), |end| at + end)
}

/// The contents of the string literals of Starlark `source`, without `#` comments and without those of an
/// `exclude = [...]` list; escapes are kept as written.
fn string_literals(source: &str) -> Vec<&str> {
    let bytes = source.as_bytes();
    let mut literals = Vec::new();
    let mut depth = 0_usize;
    // The bracket depth of the open `exclude = [` list, if any.
    let mut exclude: Option<usize> = None;
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'#' => at = source[at..].find('\n').map_or(bytes.len(), |end| at + end),
            quote @ (b'"' | b'\'') => {
                let keep = exclude.is_none();
                let triple = if quote == b'"' { "\"\"\"" } else { "'''" };
                if source[at..].starts_with(triple) {
                    let start = at + 3;
                    let end = source[start..].find(triple).map_or(bytes.len(), |end| start + end);
                    if keep {
                        literals.push(&source[start..end]);
                    }
                    at = end + 3;
                    continue;
                }
                let start = at + 1;
                let mut end = start;
                while end < bytes.len() && bytes[end] != quote && bytes[end] != b'\n' {
                    end += if bytes[end] == b'\\' { 2 } else { 1 };
                }
                let end = end.min(bytes.len());
                if keep {
                    literals.push(&source[start..end]);
                }
                at = end + 1;
            }
            b'[' => {
                depth += 1;
                at += 1;
            }
            b']' => {
                if exclude == Some(depth) {
                    exclude = None;
                }
                depth = depth.saturating_sub(1);
                at += 1;
            }
            byte if byte.is_ascii_alphabetic() || byte == b'_' => {
                let end = identifier_end(source, at);
                if exclude.is_none()
                    && &source[at..end] == "exclude"
                    && source[end..]
                        .trim_start()
                        .strip_prefix('=')
                        .is_some_and(|value| value.trim_start().starts_with('['))
                {
                    exclude = Some(depth + 1);
                }
                at = end;
            }
            _ => at += 1,
        }
    }
    literals
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

/// The workspace-relative lockfiles the Bazel files name; `lockfile_dirs` are the directories of the lockfiles to
/// judge. Nothing is read outside a Bazel repository, and no directory is listed.
fn bazel_consumers(fs: &dyn FileSystem, root: &Path, lockfile_dirs: &BTreeSet<&str>) -> BTreeSet<String> {
    let Some(repository) = BazelRepository::find(fs, root) else {
        return BTreeSet::new();
    };
    let repo = &repository;
    let build_files = lockfile_dirs
        .iter()
        .flat_map(|dir| ancestors(dir))
        .flat_map(move |dir| BUILD_FILES.iter().map(move |name| repo.bazel_path(&join(dir, name))));
    let mut queue: Vec<String> = BAZEL_ROOT_FILES
        .iter()
        .map(|name| (*name).to_owned())
        .chain(build_files)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|path| fs.is_file(&repository.dir.join(path)))
        .collect();
    let mut seen: BTreeSet<String> = queue.iter().cloned().collect();
    let mut consumed = BTreeSet::new();
    while let Some(path) = queue.pop() {
        let Ok(source) = fs.read_to_string(&repository.dir.join(&path)) else {
            continue;
        };
        let literals = string_literals(&source);
        consumed.extend(repository.labelled_in_workspace(&literals));
        let dir = parent(&path);
        if BUILD_FILES.contains(&file_name(&path))
            && let Some(dir) = repository.in_workspace(dir.to_owned())
        {
            consumed.extend(literals.iter().filter_map(|literal| listed_lockfile(&dir, literal)));
        }
        for loaded in literals
            .iter()
            .filter_map(|literal| repository.loaded_file(dir, literal))
        {
            if !seen.contains(&loaded) && fs.is_file(&repository.dir.join(&loaded)) {
                seen.insert(loaded.clone());
                queue.push(loaded);
            }
        }
    }
    consumed
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

/// Whether a Dockerfile copies `lockfile` (workspace-relative): a source names it from the Dockerfile's directory or
/// from the workspace root, or the Dockerfile sits in the lockfile's directory, copies the whole context and then
/// installs with the lockfile's package manager.
fn copies(dockerfile: &DockerfileCopies, lockfile: &str) -> bool {
    let dir = parent(&dockerfile.path);
    let named_from = |context: &str| dockerfile.sources.iter().any(|source| names(context, source, lockfile));
    named_from("")
        || (parent(lockfile) == dir
            && (named_from(dir)
                || manager_of(lockfile)
                    .is_some_and(|manager| dockerfile.installs_after_context_copy.contains(&manager))))
}

/// Which of `lockfiles` (workspace-relative, `/`-separated) a Bazel file or a Dockerfile of the workspace names.
pub(super) fn consumed(cx: &LintContext<'_>, lockfiles: &[String]) -> BTreeSet<String> {
    if lockfiles.is_empty() {
        return BTreeSet::new();
    }
    let dirs: BTreeSet<&str> = lockfiles.iter().map(|lockfile| parent(lockfile)).collect();
    let bazel = bazel_consumers(cx.fs(), &cx.workspace.root, &dirs);
    let dockerfiles = toolchain::dockerfile_copies(cx, &dirs);
    lockfiles
        .iter()
        .filter(|lockfile| {
            bazel.contains(lockfile.as_str()) || dockerfiles.iter().any(|dockerfile| copies(dockerfile, lockfile))
        })
        .cloned()
        .collect()
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
        let dirs: BTreeSet<&str> = lockfiles.iter().map(|lockfile| parent(lockfile)).collect();
        bazel_consumers(fs, Path::new(root), &dirs).into_iter().collect()
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

    #[test]
    fn only_the_files_bazel_loads_are_read() {
        let fs = MemoryFs::new()
            .with_file(
                "/ws/WORKSPACE",
                "load(\"//tools:deps.bzl\", \"deps\")\nload(\"@rules_js//npm:repositories.bzl\", \"x\")\n",
            )
            .with_file(
                "/ws/tools/deps.bzl",
                "load(\":more.bzl\", \"m\")\nLOCK = \"//libs/a:pnpm-lock.yaml\"\n",
            )
            .with_file("/ws/tools/more.bzl", "LOCK = \"//libs/b:pnpm-lock.yaml\"\n")
            .with_file("/ws/tools/unloaded.bzl", "LOCK = \"//libs/c:pnpm-lock.yaml\"\n")
            .with_file("/ws/apps/x/BUILD", "LOCK = \"//libs/d:pnpm-lock.yaml\"\n")
            .with_file("/ws/libs/e/BUILD.bazel", "load(\"//libs:macros.bzl\", \"m\")\n")
            .with_file("/ws/libs/macros.bzl", "LOCK = \"//libs/e:pnpm-lock.yaml\"\n");
        let counting = Counting::new(fs);
        let lockfiles = ["a", "b", "c", "d", "e"].map(|name| format!("libs/{name}/pnpm-lock.yaml"));
        let lockfiles: Vec<&str> = lockfiles.iter().map(String::as_str).collect();
        assert_eq!(
            consumers(&counting, "/ws", &lockfiles),
            [
                "libs/a/pnpm-lock.yaml",
                "libs/b/pnpm-lock.yaml",
                "libs/e/pnpm-lock.yaml"
            ]
        );
        assert_eq!(counting.listings.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_dockerfile_copies_a_lockfile_of_its_own_directory_or_of_the_root_context() {
        let dockerfile = |path: &str, sources: &[&str], installs: &[PackageManager]| DockerfileCopies {
            path: path.to_owned(),
            sources: sources.iter().map(|source| (*source).to_owned()).collect(),
            installs_after_context_copy: installs.to_vec(),
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
    }
}
