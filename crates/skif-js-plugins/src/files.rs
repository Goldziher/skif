//! Finds files matching config globs without walking the whole workspace: only directories that can
//! still lead to a match are listed.

use std::path::Path;

use globset::{Glob, GlobMatcher};
use skif_core::FileSystem;

const MAX_DEPTH: usize = 8;
const SKIPPED_DIRS: &[&str] = &["node_modules", ".git"];

enum Segment {
    AnyDepth,
    Glob(GlobMatcher),
}

struct Pattern {
    segments: Vec<Segment>,
}

/// Splits on `/` outside braces so `{a/b,c}` stays one segment.
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

fn matches_all(segments: &[Segment], parts: &[String]) -> bool {
    match (segments.first(), parts.first()) {
        (None, None) => true,
        (Some(Segment::AnyDepth), _) => {
            matches_all(&segments[1..], parts) || (!parts.is_empty() && matches_all(segments, &parts[1..]))
        }
        (Some(Segment::Glob(glob)), Some(part)) => {
            glob.is_match(part.as_str()) && matches_all(&segments[1..], &parts[1..])
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

fn walk(fs: &dyn FileSystem, dir: &Path, parts: &mut Vec<String>, patterns: &[Pattern], out: &mut Vec<String>) {
    let Ok(entries) = fs.read_dir_shared(dir) else { return };
    for entry in &*entries {
        let Some(name) = entry.path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        parts.push(name.to_string());
        if entry.is_file {
            if patterns.iter().any(|p| matches_all(&p.segments, parts)) {
                out.push(parts.join("/"));
            }
        } else if entry.is_dir
            && parts.len() < MAX_DEPTH
            && !SKIPPED_DIRS.contains(&name)
            && patterns.iter().any(|p| could_contain(p, parts))
        {
            walk(fs, &entry.path, parts, patterns, out);
        }
        parts.pop();
    }
}

/// Files below `dir` matching any of `globs`, as sorted forward-slash paths relative to `dir`.
pub(crate) fn find_files(fs: &dyn FileSystem, dir: &Path, globs: &[String]) -> Vec<String> {
    let patterns: Vec<Pattern> = globs
        .iter()
        .flat_map(|g| crate::glob::expand_empty_alternatives(g))
        .filter_map(|g| compile(&g))
        .collect();
    let mut out = Vec::new();
    if !patterns.is_empty() {
        walk(fs, dir, &mut Vec::new(), &patterns, &mut out);
    }
    out.sort();
    out.dedup();
    out
}
