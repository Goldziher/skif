//! Workspace pattern expansion with pnpm semantics.
//!
//! Directories are handled as root-relative, `/`-separated strings so results do not depend on how the root was
//! spelled or on the platform's separator.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use globset::{GlobBuilder, GlobMatcher};

use crate::fs::FileSystem;

const MANIFEST: &str = "package.json";
const GLOB_CHARS: [char; 4] = ['*', '?', '[', '{'];
const MAX_DEPTH: usize = 64;

/// Hidden directories and `node_modules` are never workspace packages and never traversed.
pub(crate) fn is_ignored_name(name: &str) -> bool {
    name.starts_with('.') || name == "node_modules"
}

fn normalize(raw: &str) -> String {
    let slashed = raw.trim().replace('\\', "/");
    let mut trimmed = slashed.as_str();
    while let Some(rest) = trimmed.strip_prefix("./") {
        trimmed = rest;
    }
    trimmed.trim_end_matches('/').to_owned()
}

fn compile(glob: &str) -> Option<GlobMatcher> {
    GlobBuilder::new(glob)
        .literal_separator(true)
        .build()
        .ok()
        .map(|glob| glob.compile_matcher())
}

/// A negated pattern (`!packages/x`): a glob over root-relative directories.
pub(crate) fn negation(raw: &str) -> Option<GlobMatcher> {
    compile(&normalize(raw.trim().trim_start_matches('!')))
}

enum Segment {
    Literal(String),
    /// A bare `*`.
    Star,
    /// A segment with glob characters, e.g. `foo-*` or `@(a|b)`.
    Glob(GlobMatcher),
    /// `**`
    DoubleStar,
    /// A segment that is not a valid glob and so matches nothing.
    Never,
}

/// A positive workspace pattern split into segments.
pub(crate) struct Pattern {
    segments: Vec<Segment>,
}

impl Pattern {
    pub(crate) fn new(raw: &str) -> Self {
        let segments = normalize(raw)
            .split('/')
            .filter(|segment| !segment.is_empty() && *segment != ".")
            .map(|segment| match segment {
                "**" => Segment::DoubleStar,
                "*" => Segment::Star,
                _ if segment.contains(GLOB_CHARS) => compile(segment).map_or(Segment::Never, Segment::Glob),
                _ => Segment::Literal(segment.to_owned()),
            })
            .collect();
        Self { segments }
    }

    /// A bare `*` selects only direct children of the root that contain a `package.json`.
    fn is_root_star(&self) -> bool {
        matches!(self.segments.as_slice(), [Segment::Star])
    }
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    }
}

pub(crate) struct Expander<'a> {
    pub fs: &'a dyn FileSystem,
    pub root: &'a Path,
}

impl Expander<'_> {
    fn absolute(&self, dir: &str) -> PathBuf {
        if dir.is_empty() {
            self.root.to_path_buf()
        } else {
            self.root.join(dir)
        }
    }

    pub(crate) fn has_manifest(&self, dir: &str) -> bool {
        self.fs.is_file(&self.absolute(dir).join(MANIFEST))
    }

    fn child_dirs(&self, dir: &str) -> Vec<String> {
        self.fs
            .read_dir(&self.absolute(dir))
            .unwrap_or_default()
            .into_iter()
            .filter(|entry| entry.is_dir)
            .filter_map(|entry| entry.path.file_name().map(|name| name.to_string_lossy().into_owned()))
            .filter(|name| !is_ignored_name(name))
            .collect()
    }

    /// All existing directories selected by `pattern`, root-relative.
    pub(crate) fn expand(&self, pattern: &Pattern) -> BTreeSet<String> {
        let mut found = BTreeSet::new();
        self.walk("", &pattern.segments, 0, &mut found);
        if pattern.is_root_star() {
            found.retain(|dir| self.has_manifest(dir));
        }
        found
    }

    fn walk(&self, dir: &str, segments: &[Segment], depth: usize, found: &mut BTreeSet<String>) {
        if depth > MAX_DEPTH {
            return;
        }
        let Some((first, rest)) = segments.split_first() else {
            let name = dir.rsplit('/').next().unwrap_or_default();
            if !dir.is_empty() && !is_ignored_name(name) && self.fs.is_dir(&self.absolute(dir)) {
                found.insert(dir.to_owned());
            }
            return;
        };
        match first {
            Segment::Literal(name) => self.walk(&join(dir, name), rest, depth + 1, found),
            Segment::Star => {
                for name in self.child_dirs(dir) {
                    self.walk(&join(dir, &name), rest, depth + 1, found);
                }
            }
            Segment::Glob(matcher) => {
                for name in self.child_dirs(dir).into_iter().filter(|name| matcher.is_match(name)) {
                    self.walk(&join(dir, &name), rest, depth + 1, found);
                }
            }
            Segment::DoubleStar => self.double_star(dir, rest, depth, found),
            Segment::Never => {}
        }
    }

    fn double_star(&self, dir: &str, rest: &[Segment], depth: usize, found: &mut BTreeSet<String>) {
        match rest {
            [] => self.nested_packages(dir, true, 0, found),
            [Segment::Star] => self.nested_packages(dir, false, 0, found),
            _ => {
                if depth > MAX_DEPTH {
                    return;
                }
                self.walk(dir, rest, depth + 1, found);
                for name in self.child_dirs(dir) {
                    self.double_star(&join(dir, &name), rest, depth + 1, found);
                }
            }
        }
    }

    /// Directories with a `package.json` at any depth below `dir`, descending through packages like pnpm's
    /// `**/package.json` glob does.
    fn nested_packages(&self, dir: &str, include_self: bool, depth: usize, found: &mut BTreeSet<String>) {
        if depth > MAX_DEPTH {
            return;
        }
        if include_self && !dir.is_empty() && self.has_manifest(dir) {
            found.insert(dir.to_owned());
        }
        for name in self.child_dirs(dir) {
            self.nested_packages(&join(dir, &name), true, depth + 1, found);
        }
    }
}
