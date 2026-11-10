//! Entries of the ignore lists: exact names, globs and regular expressions, with the production marker and a record
//! of which entries matched something.

use std::sync::atomic::{AtomicBool, Ordering};

use globset::{GlobBuilder, GlobMatcher};
use regex::Regex;

/// Characters that make a plain string a regular expression instead of a glob. They are the ones knip treats as
/// regular expression syntax (`+ \ ( | ^ $`) that no glob uses, plus `)`. This is narrower than knip's test
/// (`/[*+\\(|{^$]/`): `*` and `{` stay glob syntax here, so `eslint-*` is a prefix glob and not a regular expression.
const REGEX_ONLY: [char; 6] = ['+', '\\', '(', ')', '|', '^'];

/// The regular expression an entry stands for, or `None` when it is a glob or an exact name.
///
/// An entry is a regular expression when it is delimited by slashes with optional flags (`/^@org\/.+$/`, `/x/i`; the
/// flags `i`, `m` and `s` apply, `g`, `u`, `y` and `d` are accepted and ignored) or, as in knip, when it contains one
/// of `+ \ ( ) | ^`, or `.*` / `.+`, or ends in `$`. The expression is not anchored: it matches
/// when it matches anywhere in the name, as a JavaScript `RegExp.test` does. Globs (`@types/*`, `eslint-*`) are
/// everything else.
///
/// The expression is compiled by the Rust `regex` crate, so JavaScript-only syntax (look-ahead, look-behind,
/// back-references) is an error and `\d` and `\w` are Unicode aware.
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
            let inline: String = ['i', 'm', 's']
                .into_iter()
                .filter(|flag| flags.contains(*flag))
                .collect();
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

/// How the globs of a list treat `/`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Separator {
    /// `*` matches `/` too: names such as `@types/*`.
    Any,
    /// `*` stops at `/`, as in every other path glob: directories such as `packages/*`.
    Literal,
}

/// The glob variants of an entry (`{,a}` expands to two), or an error when one does not compile.
fn glob_variants(entry: &str, separator: Separator) -> Result<Vec<GlobMatcher>, String> {
    skif_js_plugins::glob::expand_empty_alternatives(entry)
        .iter()
        .map(|variant| {
            GlobBuilder::new(variant)
                .literal_separator(separator == Separator::Literal)
                .build()
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
        None => glob_variants(entry, Separator::Any).map(drop),
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

/// The exclusions a production run adds to the project for the entries of `patterns` that do not end in `!`: those
/// files run in development only, so they are not project files (`entry = ["src/index.ts!", "scripts/*.ts"]` keeps
/// the scripts out of `unused-files`).
pub(crate) fn non_production_entries(patterns: &[String]) -> Vec<String> {
    patterns
        .iter()
        .filter(|pattern| !pattern.is_empty() && !pattern.ends_with('!') && !pattern.starts_with('!'))
        .map(|pattern| format!("!{pattern}"))
        .collect()
}

/// `patterns` as exclusions: the files a tool contributes to the development project are not project files in a
/// production run.
pub(crate) fn production_exclusions(patterns: &[String]) -> Vec<String> {
    patterns
        .iter()
        .filter(|pattern| !pattern.is_empty())
        .map(|pattern| {
            if pattern.starts_with('!') {
                pattern.clone()
            } else {
                format!("!{pattern}")
            }
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
pub(crate) fn user_globs(patterns: &[String], production: bool, default: &[String]) -> Vec<String> {
    if patterns.is_empty() {
        return default.to_vec();
    }
    if !production {
        return plain_view(patterns);
    }
    let view = production_view(patterns);
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

impl Entry {
    fn matches(&self, name: &str) -> bool {
        match &self.matcher {
            Matcher::Globs(globs) => globs.iter().any(|glob| glob.is_match(name)),
            Matcher::Regex(regex) => regex.is_match(name),
        }
    }
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
        Self::compile(patterns, production, Separator::Any)
    }

    /// Like [`Listed::new`] for a list of directories, whose globs do not let `*` cross a `/`.
    pub(crate) fn directories<I, S>(patterns: I, production: bool) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        Self::compile(patterns, production, Separator::Literal)
    }

    fn compile<I, S>(patterns: I, production: bool, separator: Separator) -> Self
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
                    Ok(None) => Matcher::Globs(glob_variants(&text, separator).ok()?),
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
            if entry.matches(name) {
                entry.hit.store(true, Ordering::Relaxed);
                found = true;
            }
        }
        found
    }

    /// Every entry, as written (without the production marker), and whether it is stale: it matched nothing so far.
    pub(crate) fn verdicts(&self) -> Vec<(&str, bool)> {
        self.verdicts_except(&[])
    }

    /// Like [`verdicts`](Self::verdicts), an entry that matches one of `names` not being stale either. Testing
    /// `names` records nothing.
    pub(crate) fn verdicts_except(&self, names: &[&str]) -> Vec<(&str, bool)> {
        self.entries
            .iter()
            .map(|entry| {
                let stale = !entry.hit.load(Ordering::Relaxed) && !names.iter().any(|name| entry.matches(name));
                (entry.text.as_str(), stale)
            })
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
    fn directory_globs_do_not_cross_a_slash() {
        let names = Listed::new(["@types/*"], false);
        assert!(names.is_match("@types/node") && names.is_match("@types/a/b"));
        let directories = Listed::directories(["packages/*", "tools/**"], false);
        assert!(directories.is_match("packages/a"));
        assert!(!directories.is_match("packages/a/nested"));
        assert!(directories.is_match("tools/a/deep"));
    }

    #[test]
    fn a_star_is_a_glob_here_and_a_regular_expression_in_knip() {
        let listed = Listed::new(["eslint-*"], false);
        assert!(listed.is_match("eslint-plugin"));
        assert!(!listed.is_match("my-eslint-plugin"));
        assert!(check("/^(?!@types)/").is_err());
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
    fn verdicts_call_the_entries_that_never_matched_stale() {
        let listed = Listed::new(["a", "b", "c*", "d*"], false);
        assert!(listed.is_match("a"));
        assert!(listed.is_match("cx"));
        assert_eq!(
            listed.verdicts(),
            [("a", false), ("b", true), ("c*", false), ("d*", true)]
        );
        assert_eq!(
            listed.verdicts_except(&["dx"]),
            [("a", false), ("b", true), ("c*", false), ("d*", false)]
        );
        assert_eq!(listed.verdicts()[3], ("d*", true), "testing names records nothing");
    }

    #[test]
    fn production_exclusions_cover_entries_without_the_marker_and_tool_project_files() {
        let entries = vec![
            "src/index.ts!".to_owned(),
            "scripts/*.ts".to_owned(),
            "!skip".to_owned(),
        ];
        assert_eq!(non_production_entries(&entries), ["!scripts/*.ts"]);
        let tool = vec!["stories/**".to_owned(), "!keep/**".to_owned()];
        assert_eq!(production_exclusions(&tool), ["!stories/**", "!keep/**"]);
    }

    #[test]
    fn entry_and_project_lists_follow_the_mode() {
        let patterns = vec![
            "src/index.ts!".to_owned(),
            "src/cli.ts".to_owned(),
            "!skip/**".to_owned(),
        ];
        let default = vec!["index.ts".to_owned()];
        assert_eq!(
            user_globs(&patterns, false, &default),
            ["src/index.ts", "src/cli.ts", "!skip/**"]
        );
        assert_eq!(
            user_globs(&patterns, true, &default),
            ["src/index.ts", "!src/cli.ts", "!skip/**"]
        );
        let unmarked = vec!["src/cli.ts".to_owned()];
        assert_eq!(user_globs(&unmarked, true, &default), ["index.ts", "!src/cli.ts"]);
        assert_eq!(user_globs(&[], true, &default), ["index.ts"]);
    }
}
