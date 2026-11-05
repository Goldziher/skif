//! Entries of the ignore lists: exact names, globs and regular expressions, with the production marker and a record
//! of which entries matched something.

use std::sync::atomic::{AtomicBool, Ordering};

use globset::{Glob, GlobMatcher};
use regex::Regex;

/// Characters that make a plain string a regular expression instead of a glob. They are the ones knip treats as
/// regular expression syntax (`+ \ ( | ^ $`) that no glob uses, plus `)`.
const REGEX_ONLY: [char; 6] = ['+', '\\', '(', ')', '|', '^'];

/// The regular expression an entry stands for, or `None` when it is a glob or an exact name.
///
/// An entry is a regular expression when it is delimited by slashes with optional flags (`/^@org\/.+$/`, `/x/i`; the
/// flags `i`, `m` and `s` apply, `g`, `u`, `y` and `d` are accepted and ignored) or, as in knip, when it contains one
/// of `+ \ ( ) | ^`, or `.*` / `.+`, or ends in `$`. The expression is not anchored: it matches
/// when it matches anywhere in the name, as a JavaScript `RegExp.test` does. Globs (`@types/*`, `eslint-*`) are
/// everything else.
///
/// # Errors
/// The message of the regex compiler.
pub(crate) fn regex_of(entry: &str) -> Result<Option<Regex>, String> {
    let Some(source) = regex_source(entry) else {
        return Ok(None);
    };
    Regex::new(&source).map(Some).map_err(|err| {
        err.to_string()
            .lines()
            .last()
            .unwrap_or("invalid regular expression")
            .trim()
            .to_owned()
    })
}

fn regex_source(entry: &str) -> Option<String> {
    if let Some(rest) = entry.strip_prefix('/')
        && let Some(close) = rest.rfind('/')
    {
        let (body, flags) = (&rest[..close], &rest[close + 1..]);
        if !body.is_empty() && flags.chars().all(|flag| "gimsuyd".contains(flag)) {
            let inline: String = ['i', 'm', 's'].into_iter().filter(|flag| flags.contains(*flag)).collect();
            return Some(if inline.is_empty() {
                body.to_owned()
            } else {
                format!("(?{inline}){body}")
            });
        }
    }
    let regex_like = entry.contains(REGEX_ONLY) || entry.contains(".*") || entry.contains(".+") || entry.ends_with('$');
    regex_like.then(|| entry.to_owned())
}

/// The glob variants of an entry (`{,a}` expands to two), or an error when one does not compile.
fn glob_variants(entry: &str) -> Result<Vec<GlobMatcher>, String> {
    skif_js_plugins::glob::expand_empty_alternatives(entry)
        .iter()
        .map(|variant| {
            Glob::new(variant)
                .map(|glob| glob.compile_matcher())
                .map_err(|err| err.kind().to_string())
        })
        .collect()
}

/// Whether `entry` is a usable ignore-list entry: a regular expression that compiles or a glob that compiles.
///
/// # Errors
/// What is wrong with the entry.
pub(crate) fn check(entry: &str) -> Result<(), String> {
    match regex_of(entry)? {
        Some(_) => Ok(()),
        None => glob_variants(entry).map(drop),
    }
}

/// `entry` for the run mode: an entry ending in `!` counts in production mode only (the marker is removed), the others
/// always count. `None` when the entry does not apply.
pub(crate) fn for_mode(entry: &str, production: bool) -> Option<&str> {
    match entry.strip_suffix('!') {
        Some(body) => production.then_some(body),
        None => Some(entry),
    }
}

/// Patterns of an `entry` or `project` list as the production run sees them: an entry ending in `!` is kept (without
/// the marker), any other becomes an exclusion.
pub(crate) fn production_view(patterns: &[String]) -> Vec<String> {
    patterns
        .iter()
        .filter(|pattern| !pattern.is_empty())
        .map(|pattern| match pattern.strip_suffix('!') {
            Some(body) => body.to_owned(),
            None if pattern.starts_with('!') => pattern.clone(),
            None => format!("!{pattern}"),
        })
        .collect()
}

/// Patterns of an `entry` or `project` list as a normal run sees them: the production marker is removed.
pub(crate) fn plain_view(patterns: &[String]) -> Vec<String> {
    patterns
        .iter()
        .map(|pattern| pattern.strip_suffix('!').unwrap_or(pattern).to_owned())
        .collect()
}

/// The globs to use for a user-configured `entry` or `project` list: `default` when nothing is configured; in a
/// production run the entries ending in `!` (with the others excluded), falling back to `default` when none ends in `!`.
pub(crate) fn user_globs(patterns: Vec<String>, production: bool, default: &[String]) -> Vec<String> {
    if patterns.is_empty() {
        return default.to_vec();
    }
    if !production {
        return plain_view(&patterns);
    }
    let view = production_view(&patterns);
    if view.iter().any(|pattern| !pattern.starts_with('!')) {
        view
    } else {
        default.iter().cloned().chain(view).collect()
    }
}

enum Matcher {
    Globs(Vec<GlobMatcher>),
    Regex(Regex),
}

/// One configured entry and whether it ever matched.
struct Entry {
    text: String,
    matcher: Matcher,
    hit: AtomicBool,
}

/// A user-configured ignore list that remembers which entries matched, so entries that matched nothing can be
/// reported.
#[derive(Default)]
pub(crate) struct Listed {
    entries: Vec<Entry>,
}

impl std::fmt::Debug for Listed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list()
            .entries(self.entries.iter().map(|entry| entry.text.as_str()))
            .finish()
    }
}

impl Listed {
    /// Compiles `patterns` for the run mode; an entry that does not compile is skipped (configuration validation
    /// reports it).
    pub(crate) fn new<I, S>(patterns: I, production: bool) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let entries = patterns
            .into_iter()
            .filter_map(|pattern| {
                let text = for_mode(pattern.as_ref(), production)?.to_owned();
                let matcher = match regex_of(&text) {
                    Ok(Some(regex)) => Matcher::Regex(regex),
                    Ok(None) => Matcher::Globs(glob_variants(&text).ok()?),
                    Err(_) => return None,
                };
                Some(Entry {
                    text,
                    matcher,
                    hit: AtomicBool::new(false),
                })
            })
            .collect();
        Self { entries }
    }

    /// Whether `name` matches an entry. Every entry that matches is recorded, not only the first.
    pub(crate) fn is_match(&self, name: &str) -> bool {
        let mut found = false;
        for entry in &self.entries {
            let matched = match &entry.matcher {
                Matcher::Globs(globs) => globs.iter().any(|glob| glob.is_match(name)),
                Matcher::Regex(regex) => regex.is_match(name),
            };
            if matched {
                entry.hit.store(true, Ordering::Relaxed);
                found = true;
            }
        }
        found
    }

    /// The entries that matched nothing so far, as written (without the production marker).
    pub(crate) fn unmatched(&self) -> Vec<&str> {
        self.entries
            .iter()
            .filter(|entry| !entry.hit.load(Ordering::Relaxed))
            .map(|entry| entry.text.as_str())
            .collect()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn slash_delimited_and_regex_like_entries_are_regular_expressions() {
        for entry in ["/^@org\\/.+$/", "^@org/.+$", "@org/.*", "a|b", "/x/i", "(a)"] {
            assert!(regex_of(entry).unwrap().is_some(), "{entry}");
        }
        for entry in ["@types/*", "eslint", "eslint-*", "{a,b}", "/abs/*", "/abs/path"] {
            assert!(regex_of(entry).unwrap().is_none(), "{entry}");
        }
    }

    #[test]
    fn regular_expressions_are_unanchored_and_honour_flags() {
        let listed = Listed::new(["/^@org\\/.+$/", "/^VITE/i", "tool-.*"], false);
        assert!(listed.is_match("@org/a"));
        assert!(!listed.is_match("x@org/a"));
        assert!(listed.is_match("vite-env"));
        assert!(listed.is_match("my-tool-x"));
        assert!(!listed.is_match("other"));
    }

    #[test]
    fn globs_and_exact_names_match() {
        let listed = Listed::new(["@types/*", "eslint"], false);
        assert!(listed.is_match("@types/node"));
        assert!(listed.is_match("eslint"));
        assert!(!listed.is_match("eslint-plugin-x"));
    }

    #[test]
    fn invalid_entries_are_reported_by_check() {
        assert!(check("/(/").is_err());
        assert!(check("/x/q").is_ok());
        assert!(check("[bad").is_err());
        assert!(check("@org/*").is_ok());
        assert!(check("{,src/}a").is_ok());
    }

    #[test]
    fn the_production_marker_limits_an_entry_to_production() {
        let normal = Listed::new(["a!", "b"], false);
        assert!(!normal.is_match("a"));
        assert!(normal.is_match("b"));
        let production = Listed::new(["a!", "b"], true);
        assert!(production.is_match("a"));
        assert!(production.is_match("b"));
    }

    #[test]
    fn unmatched_lists_entries_that_never_matched() {
        let listed = Listed::new(["a", "b", "c*"], false);
        assert!(listed.is_match("a"));
        assert!(listed.is_match("cx"));
        assert_eq!(listed.unmatched(), ["b"]);
    }

    #[test]
    fn entry_and_project_lists_follow_the_mode() {
        let patterns = vec!["src/index.ts!".to_owned(), "src/cli.ts".to_owned(), "!skip/**".to_owned()];
        let default = vec!["index.ts".to_owned()];
        assert_eq!(
            user_globs(patterns.clone(), false, &default),
            ["src/index.ts", "src/cli.ts", "!skip/**"]
        );
        assert_eq!(
            user_globs(patterns, true, &default),
            ["src/index.ts", "!src/cli.ts", "!skip/**"]
        );
        let unmarked = vec!["src/cli.ts".to_owned()];
        assert_eq!(user_globs(unmarked, true, &default), ["index.ts", "!src/cli.ts"]);
        assert_eq!(user_globs(Vec::new(), true, &default), ["index.ts"]);
    }
}
