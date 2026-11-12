//! Packages the install hoists publicly into a `node_modules` above a package, where Node.js resolves them from the
//! package without a listing: pnpm's `public-hoist-pattern` and `shamefully-hoist`, and `public_hoist_packages` of
//! `rules_js`'s `npm_translate_lock`. Read for `[js] use-public-hoisting`.

use std::path::{Path, PathBuf};

use skif_core::FileSystem;
use skif_js_plugins::bazel::{public_hoist_packages, repository_root_within};
use skif_workspace::{Workspace, npmrc_entries};

/// The Bazel files at the root of a Bazel repository that call `npm_translate_lock`.
const BAZEL_FILES: [&str; 3] = ["MODULE.bazel", "WORKSPACE.bazel", "WORKSPACE"];

/// What the repository hoists publicly.
#[derive(Debug, Clone)]
pub(crate) struct PublicHoisting {
    /// pnpm's public hoist patterns, in order; they hoist into the root `node_modules`.
    patterns: Vec<String>,
    /// The packages `rules_js` hoists, each with the absolute directory whose `node_modules` it is hoisted into.
    bazel: Vec<(PathBuf, String)>,
}

impl PublicHoisting {
    /// Reads what the workspace hoists publicly. `pnpm` is `None` unless pnpm installs the workspace (not Rush), else
    /// whether that pnpm reads the root `.npmrc`. `git_root` bounds the search for the Bazel repository root.
    pub(crate) fn read(
        workspace: &Workspace,
        pnpm: Option<bool>,
        git_root: Option<&Path>,
        fs: &dyn FileSystem,
    ) -> Self {
        Self {
            patterns: pnpm
                .map(|reads_npmrc| pnpm_patterns(workspace, reads_npmrc, fs))
                .unwrap_or_default(),
            bazel: bazel_hoisted(workspace, git_root, fs),
        }
    }

    /// Whether package `name` is hoisted into a `node_modules` at or above `dir`, the absolute directory of a package.
    pub(crate) fn hoists(&self, dir: &Path, name: &str) -> bool {
        matches_patterns(&self.patterns, name)
            || self
                .bazel
                .iter()
                .any(|(at, hoisted)| hoisted == name && dir.starts_with(at))
    }
}

/// pnpm's public hoist patterns: `*` when `shamefully-hoist` is true, else `publicHoistPattern`. Each setting is
/// read from `pnpm-workspace.yaml`, else, when `reads_npmrc`, from the top level of the root `.npmrc`. Without either
/// there is none: pnpm's default is not assumed, since it changed in pnpm 10 and the version is often not declared.
fn pnpm_patterns(workspace: &Workspace, reads_npmrc: bool, fs: &dyn FileSystem) -> Vec<String> {
    let yaml = workspace.pnpm_workspace.as_ref();
    let npmrc = reads_npmrc
        .then(|| fs.read_to_string(&workspace.root.join(".npmrc")).ok())
        .flatten()
        .unwrap_or_default();
    let shamefully = yaml
        .and_then(|yaml| yaml.shamefully_hoist)
        .or_else(|| npmrc_flag(&npmrc, "shamefully-hoist"));
    if shamefully == Some(true) {
        return vec!["*".to_owned()];
    }
    yaml.and_then(|yaml| yaml.public_hoist_pattern.clone())
        .unwrap_or_else(|| npmrc_list(&npmrc, "public-hoist-pattern"))
}

/// The last top-level `key` of the `.npmrc` text `npmrc` as a boolean, when it is `true` or `false`.
fn npmrc_flag(npmrc: &str, key: &str) -> Option<bool> {
    let value = npmrc_entries(npmrc).filter(|(entry, _)| *entry == key).last()?.1;
    match value {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// The values of the list `key` of the `.npmrc` text `npmrc`, as npm's `ini` parser builds them: a `key[]` entry
/// appends and makes the value a list, a plain `key` entry appends to a list and replaces a single value.
fn npmrc_list(npmrc: &str, key: &str) -> Vec<String> {
    let array_key = format!("{key}[]");
    let mut values = Vec::new();
    let mut is_list = false;
    for (entry, value) in npmrc_entries(npmrc) {
        if entry == array_key {
            is_list = true;
        } else if entry != key {
            continue;
        } else if !is_list {
            values.clear();
        }
        values.push(value.to_owned());
    }
    values
}

/// Whether pnpm's matcher hoists `name` with `patterns`: `*` matches any run of characters and everything else
/// itself; a `!` pattern excludes what the patterns before it include, and patterns that all exclude include
/// everything they do not exclude. Without patterns nothing is hoisted.
fn matches_patterns(patterns: &[String], name: &str) -> bool {
    if !patterns.is_empty() && patterns.iter().all(|pattern| pattern.starts_with('!')) {
        return !patterns.iter().any(|pattern| {
            pattern
                .strip_prefix('!')
                .is_some_and(|excluded| wildcard(excluded, name))
        });
    }
    patterns
        .iter()
        .fold(false, |hoisted, pattern| match pattern.strip_prefix('!') {
            Some(excluded) => hoisted && !wildcard(excluded, name),
            None => hoisted || wildcard(pattern, name),
        })
}

/// Whether `name` matches `pattern` whole, each `*` of `pattern` matching any run of characters.
fn wildcard(pattern: &str, name: &str) -> bool {
    let mut parts = pattern.split('*');
    let Some(mut rest) = name.strip_prefix(parts.next().unwrap_or_default()) else {
        return false;
    };
    let parts: Vec<&str> = parts.collect();
    let Some((last, middle)) = parts.split_last() else {
        return rest.is_empty();
    };
    for part in middle {
        let Some(at) = rest.find(part) else {
            return false;
        };
        rest = &rest[at + part.len()..];
    }
    rest.ends_with(last)
}

/// The packages `public_hoist_packages` hoists in the Bazel repository that holds the workspace, read from the
/// repository root's [`BAZEL_FILES`], each with the absolute directory of the Bazel package it is hoisted into. A
/// version suffix of a key (`pkg@1.2.3`) is dropped.
fn bazel_hoisted(workspace: &Workspace, git_root: Option<&Path>, fs: &dyn FileSystem) -> Vec<(PathBuf, String)> {
    let Some(root) = repository_root_within(fs, &workspace.root, git_root) else {
        return Vec::new();
    };
    let mut hoisted = Vec::new();
    for file in BAZEL_FILES {
        let Ok(source) = fs.read_to_string(&root.join(file)) else {
            continue;
        };
        for (package, locations) in public_hoist_packages(&source) {
            let name = without_version(&package);
            for location in locations {
                let location = location.trim_start_matches("//");
                let dir = if location.is_empty() {
                    root.clone()
                } else {
                    root.join(location)
                };
                hoisted.push((dir, name.to_owned()));
            }
        }
    }
    hoisted
}

/// The package name of a `public_hoist_packages` key, without a version suffix: `@scope/name` of `@scope/name@1.0.0`.
fn without_version(key: &str) -> &str {
    let scoped = usize::from(key.starts_with('@'));
    key[scoped..].find('@').map_or(key, |at| &key[..scoped + at])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owned(patterns: &[&str]) -> Vec<String> {
        patterns.iter().map(|pattern| (*pattern).to_owned()).collect()
    }

    #[test]
    fn patterns_match_as_pnpm_s_matcher_does() {
        for (patterns, name, hoisted) in [
            (&[][..], "a", false),
            (&["*"][..], "@scope/a", true),
            (&["a"][..], "a", true),
            (&["a"][..], "ab", false),
            (&["*eslint*"][..], "eslint-plugin-x", true),
            (&["*eslint*"][..], "@typescript-eslint/parser", true),
            (&["*eslint*"][..], "prettier", false),
            (&["@scope/*"][..], "@scope/a", true),
            (&["@scope/*"][..], "@other/a", false),
            (&["a*b*c"][..], "aXbYc", true),
            (&["a*b*c"][..], "abc", true),
            (&["a*b*c"][..], "acb", false),
            (&["a.b"][..], "aXb", false),
            (&["!a"][..], "a", false),
            (&["!a"][..], "b", true),
            (&["!a", "!b"][..], "c", true),
            (&["*", "!a"][..], "a", false),
            (&["*", "!a"][..], "b", true),
            (&["!a", "a"][..], "a", true),
            (&["a", "!a", "a"][..], "a", true),
            (&["b", "!a"][..], "c", false),
        ] {
            assert_eq!(matches_patterns(&owned(patterns), name), hoisted, "{patterns:?} {name}");
        }
    }

    #[test]
    fn npmrc_lists_build_as_ini_builds_them() {
        for (npmrc, values) in [
            ("", &[][..]),
            ("p[]=a\np[]=b\n", &["a", "b"][..]),
            ("p=a\n", &["a"][..]),
            ("p=a\np=b\n", &["b"][..]),
            ("p=a\np[]=b\n", &["a", "b"][..]),
            ("p[]=a\np=b\n", &["a", "b"][..]),
            ("other[]=a\n", &[][..]),
            ("[section]\np[]=a\n", &[][..]),
        ] {
            assert_eq!(npmrc_list(npmrc, "p"), owned(values), "{npmrc:?}");
        }
    }

    #[test]
    fn npmrc_flags_are_the_last_boolean_entry() {
        assert_eq!(npmrc_flag("shamefully-hoist=true\n", "shamefully-hoist"), Some(true));
        assert_eq!(
            npmrc_flag("shamefully-hoist=true\nshamefully-hoist=false\n", "shamefully-hoist"),
            Some(false)
        );
        assert_eq!(npmrc_flag("shamefully-hoist=yes\n", "shamefully-hoist"), None);
        assert_eq!(npmrc_flag("", "shamefully-hoist"), None);
    }

    #[test]
    fn versions_are_dropped_from_hoisted_keys() {
        for (key, name) in [
            ("a", "a"),
            ("a@1.0.0", "a"),
            ("@scope/a", "@scope/a"),
            ("@scope/a@1.0.0", "@scope/a"),
            ("", ""),
        ] {
            assert_eq!(without_version(key), name, "{key}");
        }
    }
}
