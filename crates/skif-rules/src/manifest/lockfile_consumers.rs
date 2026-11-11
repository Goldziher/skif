//! The lockfiles a build installs from, found by reading build files, never by running anything.
//!
//! Bazel is looked for only in a Bazel repository: the nearest directory at or above the workspace root, and not above
//! the git repository's root, that holds `MODULE.bazel`, `REPO.bazel`, `WORKSPACE.bazel` or `WORKSPACE`. A Bazel file
//! names a lockfile by a label of the main repository: `"//libs/a:pnpm-lock.yaml"`, also `@//`, `@@//` and
//! `@<module name>//`, and with the directory in the target (`"//:libs/a/pnpm-lock.yaml"`). The files read are the
//! `MODULE.bazel`, `REPO.bazel`, `WORKSPACE`, `WORKSPACE.bazel`, `*.bzl`, `BUILD` and `BUILD.bazel` files a walk below
//! the root finds, and those at the top of the Bazel repository when it starts above the root. The walk enters what
//! discovery enters (no `node_modules`, hidden, gitignored or linked directories), nor build output (`dist`, `target`,
//! `build`, `out`, `coverage`) or a directory `.bazelignore` lists. A `BUILD` file also names a lockfile of its own
//! directory or below by a relative path (`"pnpm-lock.yaml"`, `":a/pnpm-lock.yaml"`); those of the lockfile's
//! directory and the directories above it are read even when the walk skips them. A literal in an `exclude = [...]`
//! list names nothing.
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
use skif_workspace::{DiscoverOptions, LOCKFILES, PackageManager, bazel_workspace_name, repository_root, scan_files};

use crate::paths::{file_name, join, parent};
use crate::toolchain::{self, DockerfileCopies};

/// The files that mark the root of a Bazel repository.
const BAZEL_ROOT_FILES: [&str; 4] = ["MODULE.bazel", "REPO.bazel", "WORKSPACE.bazel", "WORKSPACE"];

/// The files that declare a Bazel package, whose relative paths refer to that package's directory.
const BUILD_FILES: [&str; 2] = ["BUILD", "BUILD.bazel"];

/// Directories of build output the walk for Bazel files does not enter; hidden ones (`.cache`, `.turbo`, `.nx`) it
/// never enters.
const OUTPUT_DIRECTORIES: [&str; 5] = ["dist", "target", "build", "out", "coverage"];

/// How deep the walk for Bazel files descends below the workspace root.
const MAX_DEPTH: usize = 16;

/// Bazel's list of directories it does not look into, at the top of the Bazel repository.
const BAZEL_IGNORE: &str = ".bazelignore";

/// Whether a file called `name` is read for Bazel labels.
fn is_bazel_file(name: &str) -> bool {
    BAZEL_ROOT_FILES.contains(&name)
        || BUILD_FILES.contains(&name)
        || name
            .rsplit_once('.')
            .is_some_and(|(stem, extension)| !stem.is_empty() && extension == "bzl")
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

    /// The lockfile a label of the main repository names, as a path relative to the Bazel root:
    /// `//libs/a:pnpm-lock.yaml`, `//libs:a/pnpm-lock.yaml` and `//:libs/a/pnpm-lock.yaml` are
    /// `libs/a/pnpm-lock.yaml`. A label of another repository (`@npm//...`) names none.
    fn labelled_lockfile(&self, literal: &str) -> Option<String> {
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
        is_lockfile_target(target).then(|| join(package, target))
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
        } else {
            path.strip_prefix(&format!("{}/", self.below)).map(str::to_owned)
        }
    }

    /// Gitignore-style patterns, anchored at the workspace root, of the directories `.bazelignore` lists that lie in
    /// the workspace.
    fn ignored_directories(&self, fs: &dyn FileSystem) -> Vec<String> {
        let Ok(text) = fs.read_to_string(&self.dir.join(BAZEL_IGNORE)) else {
            return Vec::new();
        };
        text.lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .filter_map(|line| self.in_workspace(line.trim_matches('/').to_owned()))
            .filter(|dir| !dir.is_empty())
            .map(|dir| format!("/{dir}"))
            .collect()
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
/// judge. Nothing is listed or read outside a Bazel repository.
fn bazel_consumers(
    fs: &dyn FileSystem,
    root: &Path,
    lockfile_dirs: &BTreeSet<&str>,
    sequential: bool,
) -> BTreeSet<String> {
    let Some(repository) = BazelRepository::find(fs, root) else {
        return BTreeSet::new();
    };
    let skipped = OUTPUT_DIRECTORIES
        .iter()
        .map(|name| (*name).to_owned())
        .chain(repository.ignored_directories(fs));
    let options = DiscoverOptions::default()
        .with_max_depth(MAX_DEPTH)
        .with_ignore(skipped);
    let mut files: BTreeSet<String> = scan_files(root, fs, &options, sequential, &is_bazel_file)
        .iter()
        .filter_map(|path| path.strip_prefix(root).ok().map(slash))
        .collect();
    for dir in lockfile_dirs.iter().flat_map(|dir| ancestors(dir)) {
        files.extend(
            BUILD_FILES
                .iter()
                .map(|name| join(dir, name))
                .filter(|path| fs.is_file(&root.join(path))),
        );
    }
    let mut consumed = BTreeSet::new();
    for path in files {
        let Ok(source) = fs.read_to_string(&root.join(&path)) else {
            continue;
        };
        let literals = string_literals(&source);
        consumed.extend(repository.labelled_in_workspace(&literals));
        if BUILD_FILES.contains(&file_name(&path)) {
            let dir = parent(&path);
            consumed.extend(literals.iter().filter_map(|literal| listed_lockfile(dir, literal)));
        }
    }
    if repository.dir != root {
        let names: Vec<String> = fs
            .read_dir_shared(&repository.dir)
            .unwrap_or_default()
            .iter()
            .filter(|entry| entry.is_file)
            .filter_map(|entry| entry.path.file_name().map(|name| name.to_string_lossy().into_owned()))
            .filter(|name| is_bazel_file(name))
            .collect();
        for name in names {
            if let Ok(source) = fs.read_to_string(&repository.dir.join(name)) {
                consumed.extend(repository.labelled_in_workspace(&string_literals(&source)));
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
    let bazel = bazel_consumers(cx.fs(), &cx.workspace.root, &dirs, cx.sequential);
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
        bazel_consumers(fs, Path::new(root), &dirs, true).into_iter().collect()
    }

    #[test]
    fn string_literals_skip_comments_and_keep_every_quote_style() {
        let source = "# \"//commented:pnpm-lock.yaml\"\nx = [\"a\", 'b', \"\"\"c\nd\"\"\", \"e\\\"f\"]  # 'tail'\n";
        assert_eq!(string_literals(source), ["a", "b", "c\nd", "e\\\"f"]);
    }

    #[test]
    fn string_literals_skip_exclude_lists() {
        let source = "srcs = glob([\"**/*\"], exclude = [\"pnpm-lock.yaml\", [\"x\"]], my_exclude = [\"kept\"]) + [\"after\"]\n";
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
        assert_eq!(repository("", None).labelled_lockfile("@mono//libs/a:pnpm-lock.yaml"), None);
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
    fn bazel_files_are_recognised_by_name() {
        for name in [
            "MODULE.bazel",
            "REPO.bazel",
            "WORKSPACE",
            "WORKSPACE.bazel",
            "BUILD",
            "BUILD.bazel",
            "deps.bzl",
        ] {
            assert!(is_bazel_file(name), "{name}");
        }
        for name in [".bzl", "BUILD.md", "module.bazel", "package.json"] {
            assert!(!is_bazel_file(name), "{name}");
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
            ["apps/web/yarn.lock", "libs/a/pnpm-lock.yaml"]
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
        assert!(with.listings.load(Ordering::SeqCst) > 0);
    }

    #[test]
    fn the_walk_skips_build_output_and_bazelignored_directories() {
        let fs = MemoryFs::new()
            .with_file("/ws/MODULE.bazel", "")
            .with_file("/ws/.bazelignore", "# generated\nvendor/\n")
            .with_file("/ws/dist/BUILD", "LOCK = \"//libs/a:pnpm-lock.yaml\"\n")
            .with_file("/ws/vendor/x.bzl", "LOCK = \"//libs/b:pnpm-lock.yaml\"\n")
            .with_file("/ws/tools/deps.bzl", "LOCK = \"//libs/c:pnpm-lock.yaml\"\n")
            .with_file("/ws/build/d/BUILD", "exports_files([\"pnpm-lock.yaml\"])\n")
            .with_file("/ws/build/d/pnpm-lock.yaml", "");
        assert_eq!(
            consumers(&fs, "/ws", &["libs/a/pnpm-lock.yaml", "build/d/pnpm-lock.yaml"]),
            ["build/d/pnpm-lock.yaml", "libs/c/pnpm-lock.yaml"]
        );
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
            (dockerfile("apps/web/Dockerfile", &["package.json", "pnpm-lock.yaml"], &[]), true),
            (dockerfile("apps/web/Dockerfile", &["./pnpm-lock.yaml"], &[]), true),
            (dockerfile("apps/web/Dockerfile", &["pnpm-lock.yaml*"], &[]), true),
            (dockerfile("apps/web/Dockerfile", &["apps/web/pnpm-lock.yaml"], &[]), true),
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
