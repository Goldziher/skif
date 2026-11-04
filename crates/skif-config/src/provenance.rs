//! Where each resolved setting came from: the configuration files that were applied and the keys they set.

use std::path::{Path, PathBuf};

use indexmap::IndexMap;

/// How a configuration file entered the chain of a root.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SourceKind {
    /// The file in the root's own directory.
    Root,
    /// A file in a directory above the root.
    Ancestor,
    /// A file reached through the `extends` of `by`.
    Extends {
        /// The file whose `extends` names this one.
        by: PathBuf,
    },
    /// The file given with `--config`.
    Explicit,
    /// A file in the directory of a workspace member, applied to that package only.
    Package,
}

/// One configuration file applied to a root.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ConfigSource {
    /// Absolute, lexically normalized path.
    pub path: PathBuf,
    /// How the file was reached.
    pub kind: SourceKind,
}

impl ConfigSource {
    /// Whether the file was found by discovery (or given explicitly) rather than reached through `extends`.
    #[must_use]
    pub const fn is_discovered(&self) -> bool {
        matches!(
            self.kind,
            SourceKind::Root | SourceKind::Ancestor | SourceKind::Explicit
        )
    }
}

/// Appends `key` to the dotted origin path `prefix`.
///
/// A key that contains `.`, `[`, `]` or `"` is quoted (`js.plugins."a.b"`), so it cannot be mistaken for nested
/// tables or a list index.
#[must_use]
pub fn join_key(prefix: &str, key: &str) -> String {
    let segment = if key.is_empty() || key.contains(['.', '[', ']', '"']) {
        format!("\"{}\"", key.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        key.to_owned()
    };
    if prefix.is_empty() {
        segment
    } else {
        format!("{prefix}.{segment}")
    }
}

/// What a file did to a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Op {
    /// Replaced (or merged into) the inherited value.
    Set,
    /// Added after the inherited entries (`extend-*` keys, `[[ignore]]`, `[[overrides]]`).
    Append,
    /// Added before the inherited entries (groups: the nearer file wins).
    Prepend,
    /// Discarded the inherited value (`reset`).
    Reset,
}

/// One place that set a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Assignment {
    /// Index into the `sources` of the resolved configuration.
    pub source: usize,
    /// 1-based line of the key in that file; 0 when unknown.
    pub line: usize,
    /// 1-based column (in characters) of the key in that file; 0 when unknown.
    pub column: usize,
    /// What the assignment did to the key.
    pub op: Op,
}

/// The assignments behind every resolved key, oldest first.
///
/// Keys are dotted paths: `lint.fail-on-warnings`, `js.ignore-dependencies`, `rules.unused-files.level`,
/// `rules.unused-files.options`, `js.plugins.vitest`. Entries of the lists that accumulate are `version-groups[0]`,
/// `semver-groups[1]`, `ignore[2]`, `dependency-families[0]` and `overrides[3]`, numbered as in the resolved
/// configuration. The last assignment of a key is the one that decides its value, except for `Append` and `Prepend`,
/// which add to what the earlier ones set.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct Origins {
    entries: IndexMap<String, Vec<Assignment>>,
}

impl Origins {
    /// Every assignment of `key`, oldest first; empty when no file set it.
    #[must_use]
    pub fn get(&self, key: &str) -> &[Assignment] {
        match self.entries.get(key) {
            Some(list) => list.as_slice(),
            None => &[],
        }
    }

    /// The assignment that decided `key`.
    #[must_use]
    pub fn last(&self, key: &str) -> Option<&Assignment> {
        self.get(key).last()
    }

    /// Every key with its assignments, in the order the keys were first set.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &[Assignment])> {
        self.entries.iter().map(|(key, list)| (key.as_str(), list.as_slice()))
    }

    /// Number of keys with an assignment.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no key has an assignment.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub(crate) fn push(&mut self, key: String, assignment: Assignment) {
        self.entries.entry(key).or_default().push(assignment);
    }
}

/// The configuration of one root with where it came from.
#[derive(Debug, Clone, PartialEq, Default)]
#[non_exhaustive]
pub struct ResolvedConfig {
    /// The merged configuration, with the CLI overrides applied when it came from [`ConfigLoader::load`](crate::ConfigLoader::load).
    pub config: crate::Config,
    /// The files that were applied, in the order they were applied: farthest first, each after what it extends.
    pub sources: Vec<ConfigSource>,
    /// Where each key of `config` was set.
    pub origins: Origins,
}

impl ResolvedConfig {
    /// The nearest file found by discovery (or the explicit one): the file diagnostics about the configuration point
    /// at.
    #[must_use]
    pub fn nearest_file(&self) -> Option<&Path> {
        self.sources
            .iter()
            .rev()
            .find(|source| source.is_discovered())
            .map(|source| source.path.as_path())
    }
}

#[cfg(test)]
mod tests {
    use super::join_key;

    #[test]
    fn keys_with_separators_are_quoted() {
        assert_eq!(join_key("", "lint"), "lint");
        assert_eq!(join_key("js.plugins", "vitest"), "js.plugins.vitest");
        assert_eq!(join_key("js.plugins", "a.b"), "js.plugins.\"a.b\"");
        assert_eq!(join_key("js.plugins", "a[0]"), "js.plugins.\"a[0]\"");
        assert_eq!(join_key("x", "q\"r"), "x.\"q\\\"r\"");
    }
}
