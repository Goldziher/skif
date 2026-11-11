//! The lockfiles a build installs from, found by reading build files, never by running anything.
//!
//! A Bazel file names a lockfile by its label: `"//libs/a:pnpm-lock.yaml"` (also `@//` and `@@//`) in a
//! `MODULE.bazel`, `REPO.bazel`, `WORKSPACE`, `WORKSPACE.bazel`, `*.bzl`, `BUILD` or `BUILD.bazel` file below the
//! workspace root, or in those at the top of the Bazel repository when it starts above the root; a `BUILD` file also
//! by its plain name (`"pnpm-lock.yaml"`, `":pnpm-lock.yaml"`) in the lockfile's own directory. A Dockerfile
//! (`Dockerfile`, `Dockerfile.*`, `*.Dockerfile`, `Containerfile`) in the lockfile's directory names it when a `COPY` or
//! `ADD` from the build context copies it, by name or by a glob (`pnpm-lock.yaml*`). Only the names of the
//! directories below the root are listed; the files read are the ones these names match.

use std::collections::BTreeSet;
use std::path::Path;

use skif_config::Selector;
use skif_core::FileSystem;
use skif_lint::LintContext;
use skif_workspace::LOCKFILES;

use crate::toolchain;

/// The files that mark the root of a Bazel repository.
const BAZEL_ROOT_FILES: [&str; 4] = ["MODULE.bazel", "REPO.bazel", "WORKSPACE.bazel", "WORKSPACE"];

/// The files that declare a Bazel package, whose plain names refer to that package's directory.
const BUILD_FILES: [&str; 2] = ["BUILD", "BUILD.bazel"];

/// Whether a file called `name` is read for Bazel labels.
fn is_bazel_file(name: &str) -> bool {
    BAZEL_ROOT_FILES.contains(&name)
        || BUILD_FILES.contains(&name)
        || name.rsplit_once('.').is_some_and(|(stem, extension)| !stem.is_empty() && extension == "bzl")
}

/// The directory of a root-relative `/`-separated path, `""` at the root.
fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    }
}

/// The contents of the string literals of Starlark `source`, without `#` comments; escapes are kept as written.
fn string_literals(source: &str) -> Vec<&str> {
    let bytes = source.as_bytes();
    let mut literals = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'#' => at = source[at..].find('\n').map_or(bytes.len(), |end| at + end),
            quote @ (b'"' | b'\'') => {
                let triple = if quote == b'"' { "\"\"\"" } else { "'''" };
                if source[at..].starts_with(triple) {
                    let start = at + 3;
                    let end = source[start..].find(triple).map_or(bytes.len(), |end| start + end);
                    literals.push(&source[start..end]);
                    at = end + 3;
                    continue;
                }
                let start = at + 1;
                let mut end = start;
                while end < bytes.len() && bytes[end] != quote && bytes[end] != b'\n' {
                    end += if bytes[end] == b'\\' { 2 } else { 1 };
                }
                let end = end.min(bytes.len());
                literals.push(&source[start..end]);
                at = end + 1;
            }
            _ => at += 1,
        }
    }
    literals
}

/// The lockfile a label of the main repository names, as a path relative to the Bazel root: `//libs/a:pnpm-lock.yaml`
/// is `libs/a/pnpm-lock.yaml`, `//:yarn.lock` is `yarn.lock`. A label of another repository (`@npm//...`) names none.
fn labelled_lockfile(literal: &str) -> Option<String> {
    let label = literal
        .strip_prefix("@@")
        .or_else(|| literal.strip_prefix('@'))
        .unwrap_or(literal);
    let (package, name) = label.strip_prefix("//")?.split_once(':')?;
    LOCKFILES.contains(&name).then(|| join(package, name))
}

/// The lockfile a `BUILD` file in `dir` names by its plain name or a relative label (`":pnpm-lock.yaml"`).
fn listed_lockfile(dir: &str, literal: &str) -> Option<String> {
    let name = literal.strip_prefix(':').unwrap_or(literal);
    LOCKFILES.contains(&name).then(|| join(dir, name))
}

/// The Bazel repository the workspace root belongs to: the nearest directory at or above it that holds one of
/// [`BAZEL_ROOT_FILES`], and the root's path below it with `/` separators (`""` when they are the same).
fn bazel_root<'r>(fs: &dyn FileSystem, root: &'r Path) -> Option<(&'r Path, String)> {
    let found = root
        .ancestors()
        .find(|dir| BAZEL_ROOT_FILES.iter().any(|name| fs.is_file(&dir.join(name))))?;
    let below = root.strip_prefix(found).ok()?.to_string_lossy().replace('\\', "/");
    Some((found, below))
}

/// The workspace-relative lockfiles the Bazel files name.
fn bazel_consumers(fs: &dyn FileSystem, root: &Path) -> BTreeSet<String> {
    let (repository, below) = bazel_root(fs, root).unwrap_or((root, String::new()));
    // A label is relative to the Bazel root; only those inside the workspace root name one of its lockfiles.
    let in_workspace = |path: String| {
        if below.is_empty() {
            Some(path)
        } else {
            path.strip_prefix(&format!("{below}/")).map(str::to_owned)
        }
    };
    let mut consumed = BTreeSet::new();
    for path in toolchain::find_files(fs, root, &is_bazel_file) {
        let Ok(source) = fs.read_to_string(&root.join(&path)) else {
            continue;
        };
        let dir = parent(&path);
        let is_build = BUILD_FILES.contains(&path.rsplit('/').next().unwrap_or(&path));
        for literal in string_literals(&source) {
            consumed.extend(labelled_lockfile(literal).and_then(in_workspace));
            if is_build {
                consumed.extend(listed_lockfile(dir, literal));
            }
        }
    }
    if repository != root {
        let names: Vec<String> = fs
            .read_dir_shared(repository)
            .unwrap_or_default()
            .iter()
            .filter(|entry| entry.is_file)
            .filter_map(|entry| entry.path.file_name().map(|name| name.to_string_lossy().into_owned()))
            .filter(|name| is_bazel_file(name))
            .collect();
        for name in names {
            let Ok(source) = fs.read_to_string(&repository.join(&name)) else {
                continue;
            };
            for literal in string_literals(&source) {
                consumed.extend(labelled_lockfile(literal).and_then(in_workspace));
            }
        }
    }
    consumed
}

/// Whether a Dockerfile at `dockerfile` copies `lockfile`, both workspace-relative: it sits in the lockfile's
/// directory, its build context, and a source names the lockfile or is a glob that matches it.
fn copies(dockerfile: &str, sources: &[String], lockfile: &str) -> bool {
    let dir = parent(dockerfile);
    parent(lockfile) == dir
        && sources.iter().any(|source| {
            let path = join(dir, source.trim_start_matches("./").trim_start_matches('/'));
            path == lockfile
                || (source.contains(['*', '?', '['])
                    && Selector::new([path]).is_ok_and(|glob| glob.matches(lockfile, false)))
        })
}

/// Which of `lockfiles` (workspace-relative, `/`-separated) a Bazel file or a Dockerfile of the workspace names.
pub(super) fn consumed(cx: &LintContext<'_>, lockfiles: &[String]) -> BTreeSet<String> {
    if lockfiles.is_empty() {
        return BTreeSet::new();
    }
    let bazel = bazel_consumers(cx.fs(), &cx.workspace.root);
    let dockerfiles = toolchain::dockerfile_copies(cx);
    lockfiles
        .iter()
        .filter(|lockfile| {
            bazel.contains(lockfile.as_str())
                || dockerfiles
                    .iter()
                    .any(|(dockerfile, sources)| copies(dockerfile, sources, lockfile))
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_literals_skip_comments_and_keep_every_quote_style() {
        let source = "# \"//commented:pnpm-lock.yaml\"\nx = [\"a\", 'b', \"\"\"c\nd\"\"\", \"e\\\"f\"]  # 'tail'\n";
        assert_eq!(string_literals(source), ["a", "b", "c\nd", "e\\\"f"]);
    }

    #[test]
    fn labels_of_the_main_repository_name_lockfiles() {
        let cases = [
            ("//libs/a:pnpm-lock.yaml", Some("libs/a/pnpm-lock.yaml")),
            ("@//libs/a:package-lock.json", Some("libs/a/package-lock.json")),
            ("@@//apps/web:yarn.lock", Some("apps/web/yarn.lock")),
            ("//:bun.lock", Some("bun.lock")),
            ("@npm//libs/a:pnpm-lock.yaml", None),
            ("//libs/a:package.json", None),
            ("//libs/a", None),
            ("libs/a/pnpm-lock.yaml", None),
        ];
        for (literal, expected) in cases {
            assert_eq!(labelled_lockfile(literal).as_deref(), expected, "{literal}");
        }
    }

    #[test]
    fn a_build_file_lists_lockfiles_of_its_own_directory() {
        assert_eq!(listed_lockfile("libs/a", "pnpm-lock.yaml").as_deref(), Some("libs/a/pnpm-lock.yaml"));
        assert_eq!(listed_lockfile("libs/a", ":yarn.lock").as_deref(), Some("libs/a/yarn.lock"));
        assert_eq!(listed_lockfile("libs/a", "src/index.ts"), None);
    }

    #[test]
    fn bazel_files_are_recognised_by_name() {
        for name in ["MODULE.bazel", "REPO.bazel", "WORKSPACE", "WORKSPACE.bazel", "BUILD", "BUILD.bazel", "deps.bzl"] {
            assert!(is_bazel_file(name), "{name}");
        }
        for name in [".bzl", "BUILD.md", "module.bazel", "package.json"] {
            assert!(!is_bazel_file(name), "{name}");
        }
    }

    #[test]
    fn labels_are_relative_to_a_bazel_root_above_the_workspace() {
        let fs = skif_core::MemoryFs::new()
            .with_file(
                "/repo/MODULE.bazel",
                "npm.npm_translate_lock(name = \"a\", pnpm_lock = \"//js/libs/a:pnpm-lock.yaml\")\n",
            )
            .with_file("/repo/deps.bzl", "LOCK = \"//other/libs/b:pnpm-lock.yaml\"\n")
            .with_file("/repo/js/package.json", "{}")
            .with_file("/repo/js/apps/web/BUILD.bazel", "LOCK = \"//js/apps/web:yarn.lock\"\n")
            .with_file("/repo/js/libs/a/pnpm-lock.yaml", "");
        let consumed: Vec<String> = bazel_consumers(&fs, Path::new("/repo/js")).into_iter().collect();
        assert_eq!(consumed, ["apps/web/yarn.lock", "libs/a/pnpm-lock.yaml"]);
    }

    #[test]
    fn a_dockerfile_copies_a_lockfile_of_its_own_directory() {
        let sources = |list: &[&str]| list.iter().map(|source| (*source).to_owned()).collect::<Vec<_>>();
        let lockfile = "apps/web/pnpm-lock.yaml";
        assert!(copies("apps/web/Dockerfile", &sources(&["package.json", "pnpm-lock.yaml"]), lockfile));
        assert!(copies("apps/web/Dockerfile", &sources(&["./pnpm-lock.yaml"]), lockfile));
        assert!(copies("apps/web/Dockerfile", &sources(&["pnpm-lock.yaml*"]), lockfile));
        assert!(!copies("apps/web/Dockerfile", &sources(&["."]), lockfile));
        assert!(!copies("apps/web/Dockerfile", &sources(&["package.json"]), lockfile));
        assert!(!copies("apps/Dockerfile", &sources(&["web/pnpm-lock.yaml"]), lockfile));
    }
}
