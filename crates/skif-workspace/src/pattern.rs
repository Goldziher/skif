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

/// The root-relative directory a positive pattern names without any glob character (`packages/tool`); `None` for a
/// glob or the root itself.
pub(crate) fn literal_dir(raw: &str) -> Option<String> {
    let normalized = normalize(raw);
    (!normalized.is_empty() && !normalized.contains(GLOB_CHARS)).then_some(normalized)
}

fn reject_parent_segments(pattern: &str) -> Result<(), String> {
    if pattern.split('/').any(|segment| segment == "..") {
        return Err("`..` would select directories outside the workspace root".to_owned());
    }
    Ok(())
}

/// The leading `..` segments of `pattern`, and the rest of it.
fn split_parent_segments(pattern: &str) -> (usize, &str) {
    let mut rest = pattern;
    let mut ups = 0;
    loop {
        if rest == ".." {
            return (ups + 1, "");
        }
        match rest.strip_prefix("../") {
            Some(after) => {
                ups += 1;
                rest = after.trim_start_matches('/');
            }
            None => return (ups, rest),
        }
    }
}

/// Whether the root-relative directory `dir` lies outside the root: it starts with a `..` segment.
pub(crate) fn is_outside_root(dir: &str) -> bool {
    dir == ".." || dir.starts_with("../")
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
    /// A segment with glob characters (`*`, `?`, `[` or `{`), e.g. `foo-*`. Extglobs such as `@(a|b)` are
    /// not supported: those characters are literal.
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
        Self::parse(raw, false)
    }

    /// As [`new`](Self::new); with `outside_root` (Bun, which installs members outside the root), leading `..`
    /// segments are accepted and select directories above the root (`../shared`, `../libs/*`), which the expansion
    /// returns with those segments (`../shared`). A `..` after the first other segment is still refused.
    ///
    /// # Errors
    /// As [`new`](Self::new), apart from the leading `..` segments `outside_root` accepts.
    pub(crate) fn parse(raw: &str, outside_root: bool) -> Result<Self, String> {
        let normalized = normalize(raw);
        let (ups, rest) = if outside_root {
            split_parent_segments(&normalized)
        } else {
            (0, normalized.as_str())
        };
        reject_parent_segments(rest)?;
        let mut expanded = Vec::new();
        expand_braces(rest, &mut expanded)?;
        let alternatives = expanded
            .iter()
            .map(|alternative| {
                let parents = std::iter::repeat_with(|| Segment::Literal("..".to_owned())).take(ups);
                parents
                    .chain(
                        alternative
                            .split('/')
                            .filter(|segment| !segment.is_empty() && *segment != ".")
                            .map(|segment| match segment {
                                "**" => Segment::DoubleStar,
                                "*" => Segment::Star,
                                _ if segment.contains(GLOB_CHARS) => {
                                    compile(segment).map_or(Segment::Never, Segment::Glob)
                                }
                                _ => Segment::Literal(segment.to_owned()),
                            }),
                    )
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

    /// Non-hidden subdirectories of `dir`; symbolic links to directories only when `follow_symlinks`. A name that is
    /// not UTF-8 is left out: the root-relative strings could not spell it, so nothing below it could match.
    fn child_dirs(&self, dir: &str, follow_symlinks: bool) -> Vec<String> {
        self.fs
            .read_dir_shared(&self.absolute(dir))
            .unwrap_or_default()
            .iter()
            .filter(|entry| entry.is_dir && (follow_symlinks || !entry.is_symlink))
            .filter_map(|entry| entry.path.file_name()?.to_str())
            .filter(|name| !is_ignored_name(name))
            .map(str::to_owned)
            .collect()
    }

    /// All existing directories selected by `pattern`, root-relative.
    pub(crate) fn expand(&self, pattern: &Pattern) -> BTreeSet<String> {
        let mut all = BTreeSet::new();
        for segments in &pattern.alternatives {
            let mut found = BTreeSet::new();
            self.walk("", segments, 0, false, &mut found);
            if is_root_star(segments) {
                found.retain(|dir| self.has_manifest(dir));
            }
            all.extend(found);
        }
        all
    }

    /// Walks `segments` from `dir`. `listed` says that `dir` came from the listing of its parent as a non-hidden
    /// directory ([`child_dirs`](Self::child_dirs)), so it need not be probed again when the segments end there.
    fn walk(&self, dir: &str, segments: &[Segment], depth: usize, listed: bool, found: &mut BTreeSet<String>) {
        if depth > MAX_DEPTH {
            return;
        }
        let Some((first, rest)) = segments.split_first() else {
            let name = dir.rsplit('/').next().unwrap_or_default();
            if !dir.is_empty() && (listed || (!is_ignored_name(name) && self.fs.is_dir(&self.absolute(dir)))) {
                found.insert(dir.to_owned());
            }
            return;
        };
        match first {
            Segment::Literal(name) => self.walk(&join(dir, name), rest, depth + 1, false, found),
            Segment::Star => {
                for name in self.child_dirs(dir, true) {
                    self.walk(&join(dir, &name), rest, depth + 1, true, found);
                }
            }
            Segment::Glob(matcher) => {
                for name in self
                    .child_dirs(dir, true)
                    .into_iter()
                    .filter(|name| matcher.is_match(name))
                {
                    self.walk(&join(dir, &name), rest, depth + 1, true, found);
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
                self.walk(dir, rest, depth + 1, false, found);
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
        // Listed before the manifest is probed, so a caching file system answers the probe from the listing.
        let children = self.child_dirs(dir, false);
        if include_self && !dir.is_empty() && self.has_manifest(dir) {
            found.insert(dir.to_owned());
        }
        for name in children {
            self.nested_packages(&join(dir, &name), true, depth + 1, found);
        }
    }
}

/// The root-relative directories with a `package.json` that `patterns` select, negations applied. With
/// `literals_kept` (Bun), a negation does not remove a directory a positive pattern names by its literal path. A pattern
/// that cannot be used selects nothing.
pub(crate) fn selected_with_manifest<'p>(
    expander: &Expander<'_>,
    patterns: impl IntoIterator<Item = &'p str>,
    literals_kept: bool,
) -> BTreeSet<String> {
    let mut selected = BTreeSet::new();
    let mut negations = Vec::new();
    let mut literals = BTreeSet::new();
    for raw in patterns {
        if raw.trim_start().starts_with('!') {
            negations.extend(negation(raw).ok());
        } else if let Ok(pattern) = Pattern::new(raw) {
            if literals_kept {
                literals.extend(literal_dir(raw));
            }
            selected.extend(expander.expand(&pattern));
        }
    }
    selected.retain(|dir| {
        expander.has_manifest(dir)
            && (literals.contains(dir) || !negations.iter().any(|negation| negation.is_match(dir)))
    });
    selected
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::io;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use skif_core::{DirEntry, MemoryFs};

    use super::*;

    /// Counts the directory probes made outside listings.
    struct Probes {
        inner: MemoryFs,
        is_dir: AtomicUsize,
    }

    impl FileSystem for Probes {
        fn read_to_string(&self, path: &Path) -> io::Result<Arc<str>> {
            self.inner.read_to_string(path)
        }

        fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
            self.inner.read_dir(path)
        }

        fn is_file(&self, path: &Path) -> bool {
            self.inner.is_file(path)
        }

        fn is_dir(&self, path: &Path) -> bool {
            self.is_dir.fetch_add(1, Ordering::SeqCst);
            self.inner.is_dir(path)
        }
    }

    fn probes(files: &[&str]) -> Probes {
        let mut inner = MemoryFs::new();
        for file in files {
            inner.insert(Path::new("/ws").join(file), "{}");
        }
        Probes {
            inner,
            is_dir: AtomicUsize::new(0),
        }
    }

    fn expand(fs: &Probes, raw: &str) -> Vec<String> {
        let expander = Expander {
            fs,
            root: Path::new("/ws"),
        };
        expander.expand(&Pattern::new(raw).unwrap()).into_iter().collect()
    }

    #[test]
    fn directories_taken_from_a_listing_are_not_probed_again() {
        let fs = probes(&[
            "packages/a/package.json",
            "packages/b/package.json",
            "packages/b-c/x.txt",
        ]);
        assert_eq!(expand(&fs, "packages/*"), ["packages/a", "packages/b", "packages/b-c"]);
        assert_eq!(expand(&fs, "packages/b*"), ["packages/b", "packages/b-c"]);
        assert_eq!(fs.is_dir.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn leading_parent_segments_select_directories_above_the_root_only_when_allowed() {
        let mut inner = MemoryFs::new();
        for file in [
            "/ws/../shared/package.json",
            "/ws/../libs/a/package.json",
            "/ws/../libs/b/package.json",
        ] {
            inner.insert(file, "{}");
        }
        let fs = Probes {
            inner,
            is_dir: AtomicUsize::new(0),
        };
        let expander = Expander {
            fs: &fs,
            root: Path::new("/ws"),
        };
        let outside = |raw: &str| -> Vec<String> {
            expander
                .expand(&Pattern::parse(raw, true).unwrap())
                .into_iter()
                .collect()
        };
        assert_eq!(outside("../shared"), ["../shared"]);
        assert_eq!(outside("./../libs/*/"), ["../libs/a", "../libs/b"]);
        assert_eq!(outside("../missing"), Vec::<String>::new());
        assert!(Pattern::new("../shared").is_err());
        assert!(Pattern::parse("libs/../shared", true).is_err());
        assert!(is_outside_root("../shared") && is_outside_root(".."));
        assert!(!is_outside_root("..x/y") && !is_outside_root("packages/a"));
    }

    #[test]
    fn a_literal_directory_is_still_probed() {
        let fs = probes(&["tools/x/package.json"]);
        assert_eq!(expand(&fs, "tools/x"), ["tools/x"]);
        assert_eq!(expand(&fs, "tools/missing"), Vec::<String>::new());
        assert_eq!(fs.is_dir.load(Ordering::SeqCst), 2);
    }
}
