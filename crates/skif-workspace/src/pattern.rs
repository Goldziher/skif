//! Workspace pattern expansion with pnpm semantics.
//!
//! Directories are handled as root-relative, `/`-separated strings so results do not depend on how the root was
//! spelled or on the platform's separator.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use globset::{GlobBuilder, GlobMatcher};

use skif_core::FileSystem;

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

/// A negated pattern (`!packages/x`): globs over root-relative directories.
///
/// A negation ending in `/**` also removes the directory it is rooted at, as pnpm and fast-glob do: `!**/test/**`
/// drops `test` itself, not just what is inside it.
pub(crate) struct Negation {
    matchers: Vec<GlobMatcher>,
}

impl Negation {
    pub(crate) fn is_match(&self, dir: &str) -> bool {
        self.matchers.iter().any(|matcher| matcher.is_match(dir))
    }
}

/// Compiles a negated pattern. A glob that does not compile matches nothing.
///
/// # Errors
/// A message when the pattern contains a `..` segment.
pub(crate) fn negation(raw: &str) -> Result<Negation, String> {
    let glob = normalize(raw.trim().trim_start_matches('!'));
    reject_parent_segments(&glob)?;
    let rooted = glob.strip_suffix("/**").filter(|rooted| !rooted.is_empty());
    let matchers = std::iter::once(glob.as_str())
        .chain(rooted)
        .filter_map(compile)
        .collect();
    Ok(Negation { matchers })
}

fn reject_parent_segments(pattern: &str) -> Result<(), String> {
    if pattern.split('/').any(|segment| segment == "..") {
        return Err("`..` would select directories outside the workspace root".to_owned());
    }
    Ok(())
}

/// Upper bound on the patterns one brace expression may expand to.
const MAX_ALTERNATIVES: usize = 256;

/// The first `{a,b}` group of `pattern` that has at least two alternatives: its brace offsets and the alternatives.
fn first_group(pattern: &str) -> Option<(usize, usize, Vec<&str>)> {
    let mut search = 0;
    while let Some(relative) = pattern[search..].find('{') {
        let open = search + relative;
        let mut depth = 0_usize;
        let mut parts = Vec::new();
        let mut part_start = open + 1;
        for (at, byte) in pattern.bytes().enumerate().skip(open) {
            match byte {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        parts.push(&pattern[part_start..at]);
                        if parts.len() > 1 {
                            return Some((open, at, parts));
                        }
                        break;
                    }
                }
                b',' if depth == 1 => {
                    parts.push(&pattern[part_start..at]);
                    part_start = at + 1;
                }
                _ => {}
            }
        }
        search = open + 1;
    }
    None
}

/// Expands brace alternation, including alternatives that contain `/`, into plain patterns.
fn expand_braces(pattern: &str, out: &mut Vec<String>) -> Result<(), String> {
    let Some((open, close, parts)) = first_group(pattern) else {
        out.push(pattern.to_owned());
        return Ok(());
    };
    for part in parts {
        expand_braces(&format!("{}{part}{}", &pattern[..open], &pattern[close + 1..]), out)?;
        if out.len() > MAX_ALTERNATIVES {
            return Err(format!(
                "brace expansion produces more than {MAX_ALTERNATIVES} patterns"
            ));
        }
    }
    Ok(())
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

/// A positive workspace pattern: one segment list per brace alternative.
pub(crate) struct Pattern {
    alternatives: Vec<Vec<Segment>>,
}

impl Pattern {
    /// # Errors
    /// A message when the pattern contains a `..` segment or expands to too many alternatives.
    pub(crate) fn new(raw: &str) -> Result<Self, String> {
        let normalized = normalize(raw);
        reject_parent_segments(&normalized)?;
        let mut expanded = Vec::new();
        expand_braces(&normalized, &mut expanded)?;
        let alternatives = expanded
            .iter()
            .map(|alternative| {
                alternative
                    .split('/')
                    .filter(|segment| !segment.is_empty() && *segment != ".")
                    .map(|segment| match segment {
                        "**" => Segment::DoubleStar,
                        "*" => Segment::Star,
                        _ if segment.contains(GLOB_CHARS) => compile(segment).map_or(Segment::Never, Segment::Glob),
                        _ => Segment::Literal(segment.to_owned()),
                    })
                    .collect()
            })
            .collect();
        Ok(Self { alternatives })
    }
}

/// A bare `*` selects only direct children of the root that contain a `package.json`.
fn is_root_star(segments: &[Segment]) -> bool {
    matches!(segments, [Segment::Star])
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

    /// Non-hidden subdirectories of `dir`; symbolic links to directories only when `follow_symlinks`.
    fn child_dirs(&self, dir: &str, follow_symlinks: bool) -> Vec<String> {
        self.fs
            .read_dir_shared(&self.absolute(dir))
            .unwrap_or_default()
            .iter()
            .filter(|entry| entry.is_dir && (follow_symlinks || !entry.is_symlink))
            .filter_map(|entry| entry.path.file_name().map(|name| name.to_string_lossy().into_owned()))
            .filter(|name| !is_ignored_name(name))
            .collect()
    }

    /// All existing directories selected by `pattern`, root-relative.
    pub(crate) fn expand(&self, pattern: &Pattern) -> BTreeSet<String> {
        let mut all = BTreeSet::new();
        for segments in &pattern.alternatives {
            let mut found = BTreeSet::new();
            self.walk("", segments, 0, &mut found);
            if is_root_star(segments) {
                found.retain(|dir| self.has_manifest(dir));
            }
            all.extend(found);
        }
        all
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
                for name in self.child_dirs(dir, true) {
                    self.walk(&join(dir, &name), rest, depth + 1, found);
                }
            }
            Segment::Glob(matcher) => {
                for name in self
                    .child_dirs(dir, true)
                    .into_iter()
                    .filter(|name| matcher.is_match(name))
                {
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
                for name in self.child_dirs(dir, false) {
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
        for name in self.child_dirs(dir, false) {
            self.nested_packages(&join(dir, &name), true, depth + 1, found);
        }
    }
}
