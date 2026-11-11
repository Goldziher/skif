//! Finds files matching config globs without walking the whole workspace: only directories that can
//! still lead to a match are listed.

use skif_core::HashMap;
use std::path::Path;
use std::sync::{Arc, LazyLock, PoisonError, RwLock};

use globset::{Glob, GlobMatcher};
use skif_core::FileSystem;

const MAX_DEPTH: usize = 8;
const SKIPPED_DIRS: &[&str] = &["node_modules", ".git", "cdk.out"];

enum Segment {
    AnyDepth,
    Glob(GlobMatcher),
}

struct Pattern {
    segments: Vec<Segment>,
}

/// Splits on `/` outside braces; groups containing `/` were expanded before.
fn split_segments(pattern: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0_u32;
    let mut start = 0;
    for (i, c) in pattern.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            '/' if depth == 0 => {
                parts.push(&pattern[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&pattern[start..]);
    parts
}

fn compile(pattern: &str) -> Option<Pattern> {
    let pattern = pattern.trim_start_matches("./");
    let mut segments = Vec::new();
    for part in split_segments(pattern).into_iter().filter(|p| !p.is_empty()) {
        if part == "**" {
            segments.push(Segment::AnyDepth);
        } else {
            segments.push(Segment::Glob(Glob::new(part).ok()?.compile_matcher()));
        }
    }
    (!segments.is_empty()).then_some(Pattern { segments })
}

/// The compiled alternatives of a glob, by its text: the same config globs are looked for in every package, and
/// expanding and compiling a glob costs more than matching a few hundred names with it.
fn compiled(glob: &str) -> Arc<[Arc<Pattern>]> {
    static CACHE: LazyLock<RwLock<HashMap<String, Arc<[Arc<Pattern>]>>>> = LazyLock::new(Default::default);
    if let Some(hit) = CACHE.read().unwrap_or_else(PoisonError::into_inner).get(glob) {
        return Arc::clone(hit);
    }
    let built: Arc<[Arc<Pattern>]> = crate::glob::expand_path_alternatives(glob)
        .iter()
        .filter_map(|alternative| compile(alternative).map(Arc::new))
        .collect();
    CACHE
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(glob.to_owned(), Arc::clone(&built));
    built
}

fn matches_all<S: AsRef<str>>(segments: &[Segment], parts: &[S]) -> bool {
    match (segments.first(), parts.first()) {
        (None, None) => true,
        (Some(Segment::AnyDepth), _) => {
            matches_all(&segments[1..], parts) || (!parts.is_empty() && matches_all(segments, &parts[1..]))
        }
        (Some(Segment::Glob(glob)), Some(part)) => {
            glob.is_match(AsRef::<str>::as_ref(part)) && matches_all(&segments[1..], &parts[1..])
        }
        _ => false,
    }
}

/// Whether a directory with these path parts may contain a match.
fn could_contain(pattern: &Pattern, parts: &[String]) -> bool {
    for (i, part) in parts.iter().enumerate() {
        match pattern.segments.get(i) {
            None => return false,
            Some(Segment::AnyDepth) => return true,
            Some(Segment::Glob(glob)) if !glob.is_match(part.as_str()) => return false,
            Some(Segment::Glob(_)) => {}
        }
    }
    parts.len() < pattern.segments.len()
}

/// What a walk looks for: the files matching `patterns` (every file without), at most `max_depth` directories down.
struct Search<'a> {
    patterns: Option<&'a [Arc<Pattern>]>,
    max_depth: usize,
}

/// The names of the directories a walk is in, then the entry it looks at: each level's buffer is reused for the next
/// entry at that level rather than allocated per entry.
#[derive(Default)]
struct Parts {
    names: Vec<String>,
    len: usize,
}

impl Parts {
    fn push(&mut self, name: &str) {
        if let Some(slot) = self.names.get_mut(self.len) {
            slot.clear();
            slot.push_str(name);
        } else {
            self.names.push(name.to_owned());
        }
        self.len += 1;
    }

    fn pop(&mut self) {
        self.len -= 1;
    }

    fn as_slice(&self) -> &[String] {
        &self.names[..self.len]
    }
}

fn walk(fs: &dyn FileSystem, dir: &Path, parts: &mut Parts, search: &Search<'_>, out: &mut Vec<String>) {
    let Ok(entries) = fs.read_dir_shared(dir) else { return };
    for entry in &*entries {
        let Some(name) = entry.path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        parts.push(name);
        if entry.is_file {
            if search
                .patterns
                .is_none_or(|patterns| patterns.iter().any(|p| matches_all(&p.segments, parts.as_slice())))
            {
                out.push(parts.as_slice().join("/"));
            }
        } else if entry.is_dir
            && parts.len < search.max_depth
            && !SKIPPED_DIRS.contains(&name)
            && search
                .patterns
                .is_none_or(|patterns| patterns.iter().any(|p| could_contain(p, parts.as_slice())))
        {
            walk(fs, &entry.path, parts, search, out);
        }
        parts.pop();
    }
}

fn patterns<S: AsRef<str>>(globs: &[S]) -> Vec<Arc<Pattern>> {
    let mut out = Vec::with_capacity(globs.len());
    for glob in globs {
        out.extend(compiled(glob.as_ref()).iter().cloned());
    }
    out
}

/// Files below `dir` matching any of `globs`, as sorted forward-slash paths relative to `dir`.
pub(crate) fn find_files(fs: &dyn FileSystem, dir: &Path, globs: &[String]) -> Vec<String> {
    find_files_within(fs, dir, globs, MAX_DEPTH)
}

/// Like [`find_files`], looking at most `max_depth` directories down instead of the config depth of 8.
pub(crate) fn find_files_within(fs: &dyn FileSystem, dir: &Path, globs: &[String], max_depth: usize) -> Vec<String> {
    let patterns = patterns(globs);
    let mut out = Vec::new();
    if !patterns.is_empty() {
        let search = Search {
            patterns: Some(patterns.as_slice()),
            max_depth,
        };
        walk(fs, dir, &mut Parts::default(), &search, &mut out);
    }
    out.sort();
    out.dedup();
    out
}

/// Every file below `dir` that [`find_files`] can find, as sorted forward-slash paths relative to `dir`: one walk,
/// which [`matching`] then searches for the globs of each plugin.
pub(crate) fn list_files(fs: &dyn FileSystem, dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let search = Search {
        patterns: None,
        max_depth: MAX_DEPTH,
    };
    walk(fs, dir, &mut Parts::default(), &search, &mut out);
    out.sort();
    out
}

/// The files of `files`, a [`list_files`] listing, that match any of `globs`: what [`find_files`] finds in the
/// directory listed.
#[cfg(test)]
pub(crate) fn matching(files: &[String], globs: &[String]) -> Vec<String> {
    Listing::new(files).matching(globs)
}

/// A [`list_files`] listing with each file split into its path parts once, as it is searched for the globs of every
/// plugin.
pub(crate) struct Listing<'a> {
    files: &'a [String],
    parts: Vec<Vec<&'a str>>,
}

impl<'a> Listing<'a> {
    pub(crate) fn new(files: &'a [String]) -> Self {
        Self {
            files,
            parts: files.iter().map(|file| file.split('/').collect()).collect(),
        }
    }

    /// The files that match any of `globs`: what [`find_files`] finds in the directory listed.
    pub(crate) fn matching<S: AsRef<str>>(&self, globs: &[S]) -> Vec<String> {
        let patterns = patterns(globs);
        if patterns.is_empty() {
            return Vec::new();
        }
        self.files
            .iter()
            .zip(&self.parts)
            .filter(|(_, parts)| patterns.iter().any(|p| matches_all(&p.segments, parts.as_slice())))
            .map(|(file, _)| file.clone())
            .collect()
    }

    /// Whether any file matches any of `globs`.
    pub(crate) fn any_matching<S: AsRef<str>>(&self, globs: &[S]) -> bool {
        let patterns = patterns(globs);
        !patterns.is_empty()
            && self
                .parts
                .iter()
                .any(|parts| patterns.iter().any(|p| matches_all(&p.segments, parts.as_slice())))
    }
}

#[cfg(test)]
mod tests {
    use skif_core::MemoryFs;

    use super::*;

    fn fs_with(paths: &[&str]) -> MemoryFs {
        let mut fs = MemoryFs::new();
        for path in paths {
            fs.insert(format!("/ws/{path}"), "");
        }
        fs
    }

    #[test]
    fn brace_groups_containing_a_slash_match_across_segments() {
        let fs = fs_with(&["src/app/a.ts", "lib/b.ts", "other/c.ts", "src/app/a.js"]);
        let found = find_files(&fs, Path::new("/ws"), &["{src/app,lib}/*.ts".to_owned()]);
        assert_eq!(found, ["lib/b.ts", "src/app/a.ts"]);
    }

    /// A listing searched for globs finds what a walk for them finds, and a deeper bound reaches deeper files.
    #[test]
    fn a_listing_matches_what_a_walk_finds() {
        let fs = fs_with(&[
            "jest.config.ts",
            "src/a.ts",
            "src/deep/b.spec.ts",
            "node_modules/x/jest.config.ts",
            "a/b/c/d/e/f/g/h/project.json",
        ]);
        let dir = Path::new("/ws");
        let listing = list_files(&fs, dir);
        assert_eq!(listing, ["jest.config.ts", "src/a.ts", "src/deep/b.spec.ts"]);
        for globs in [
            vec!["jest.config.{js,ts}".to_owned()],
            vec!["**/*.spec.ts".to_owned(), "src/*.ts".to_owned()],
            vec!["**/project.json".to_owned()],
        ] {
            assert_eq!(matching(&listing, &globs), find_files(&fs, dir, &globs), "{globs:?}");
        }
        let projects = ["**/project.json".to_owned()];
        assert_eq!(find_files(&fs, dir, &projects), Vec::<String>::new());
        assert_eq!(
            find_files_within(&fs, dir, &projects, 9),
            ["a/b/c/d/e/f/g/h/project.json"]
        );
    }

    /// A listing answers whether a glob matches as its search for the glob does, for a match at the top, a match
    /// below a directory, no match and no glob.
    #[test]
    fn a_listing_finds_any_match_as_it_finds_the_matches() {
        let files = ["a.ts".to_owned(), "src/deep/b.spec.ts".to_owned(), "vite.config.ts".to_owned()];
        let listing = Listing::new(&files);
        for (globs, expected) in [
            (vec!["vite.config.{js,ts}"], vec!["vite.config.ts"]),
            (vec!["x.ts", "**/*.spec.ts"], vec!["src/deep/b.spec.ts"]),
            (vec!["src/*.spec.ts"], vec![]),
            (vec![], vec![]),
        ] {
            assert_eq!(listing.matching(&globs), expected, "{globs:?}");
            assert_eq!(listing.any_matching(&globs), !expected.is_empty(), "{globs:?}");
        }
    }

    /// A walk that leaves a deep directory for a shallow one reuses the buffers of the levels it left without
    /// carrying their names along.
    #[test]
    fn a_walk_reports_each_path_once_with_its_own_parts() {
        let fs = fs_with(&["a/bbbbbbbb/c.ts", "a/d.ts", "e.ts", "ff/g/h.ts"]);
        assert_eq!(
            list_files(&fs, Path::new("/ws")),
            ["a/bbbbbbbb/c.ts", "a/d.ts", "e.ts", "ff/g/h.ts"]
        );
    }

    #[test]
    fn nested_and_empty_groups_match() {
        let fs = fs_with(&["a.ts", "src/a.ts", "src/lib/a.ts", "x/a.ts"]);
        let found = find_files(&fs, Path::new("/ws"), &["{,src/{,lib/}}a.ts".to_owned()]);
        assert_eq!(found, ["a.ts", "src/a.ts", "src/lib/a.ts"]);
    }
}
