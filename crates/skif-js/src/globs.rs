//! Glob sets with exclusions, and name matchers for the ignore lists.

use globset::{Glob, GlobBuilder, GlobSet, GlobSetBuilder};
use regex::{Regex, RegexSet};

/// Include and exclude globs matched against `/`-separated relative paths.
#[derive(Debug, Clone, Default)]
pub(crate) struct Globs {
    include: GlobSet,
    exclude: GlobSet,
    has_include: bool,
    /// Regular expressions matched besides `include`, against `/` followed by the path (Jest's `testRegex`).
    include_regex: Option<RegexSet>,
    /// Regular expressions matched besides `exclude`, against `/` followed by the path.
    exclude_regex: Option<RegexSet>,
    /// A second set matched besides this one, with exclusions of its own: the `files` of a `tsconfig`, which its
    /// `exclude` does not filter.
    also: Option<Box<Globs>>,
}

fn compile(pattern: &str) -> Vec<Glob> {
    let pattern = pattern.trim().trim_start_matches("./").trim_end_matches('!');
    if pattern.is_empty() {
        return Vec::new();
    }
    skif_js_plugins::glob::expand_empty_alternatives(pattern)
        .iter()
        .filter_map(|variant| GlobBuilder::new(variant).literal_separator(true).build().ok())
        .collect()
}

fn build(globs: Vec<Glob>) -> GlobSet {
    let mut builder = GlobSetBuilder::new();
    for glob in globs {
        builder.add(glob);
    }
    builder.build().unwrap_or_default()
}

impl Globs {
    /// Compiles `patterns`; a pattern that is not a valid glob is skipped.
    pub(crate) fn new<I, S>(patterns: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut include = Vec::new();
        let mut exclude = Vec::new();
        for pattern in patterns {
            let pattern = pattern.as_ref();
            if let Some(negated) = pattern.strip_prefix('!') {
                exclude.extend(compile(negated));
            } else {
                include.extend(compile(pattern));
            }
        }
        Self {
            has_include: !include.is_empty(),
            include: build(include),
            exclude: build(exclude),
            include_regex: None,
            exclude_regex: None,
            also: None,
        }
    }

    /// The globs `patterns` together with the regular expressions `regexes`, less what `ignore_regexes` match. A
    /// regular expression the `regex` crate does not accept (a lookaround) is skipped among `ignore_regexes`; among
    /// `regexes` it makes every path match, as its files cannot be told apart.
    pub(crate) fn with_regexes(patterns: &[String], regexes: &[String], ignore_regexes: &[String]) -> Self {
        let any_invalid = regexes.iter().any(|regex| Regex::new(regex).is_err());
        let mut all: Vec<&str> = patterns.iter().map(String::as_str).collect();
        if any_invalid {
            all.push("**");
        }
        let mut globs = Self::new(all);
        globs.include_regex = regex_set(regexes);
        globs.exclude_regex = regex_set(ignore_regexes);
        globs.has_include |= globs.include_regex.is_some();
        globs
    }

    /// Also matches what any of `others` matches, each with its own exclusions; see [`Self::or`].
    #[must_use]
    pub(crate) fn or_each(self, others: Vec<Self>) -> Self {
        match others.into_iter().rev().reduce(|later, earlier| earlier.or(later)) {
            Some(chained) => self.or(chained),
            None => self,
        }
    }

    /// Also matches what `other` matches, whatever the exclusions of this set say.
    #[must_use]
    pub(crate) fn or(mut self, other: Self) -> Self {
        if !other.is_empty() {
            self.also = Some(Box::new(other));
        }
        self
    }

    /// Whether no include glob exists, so nothing can match.
    pub(crate) fn is_empty(&self) -> bool {
        !self.has_include && self.also.as_ref().is_none_or(|also| also.is_empty())
    }

    /// Whether `path` matches an include glob and no exclude glob.
    pub(crate) fn is_match(&self, path: &str) -> bool {
        (self.includes(path) && !self.excludes(path)) || self.also.as_ref().is_some_and(|also| also.is_match(path))
    }

    /// Whether `path` matches any include glob, ignoring exclusions (for ignore lists, where `!` has no meaning).
    pub(crate) fn matches_any(&self, path: &str) -> bool {
        self.includes(path) || self.also.as_ref().is_some_and(|also| also.matches_any(path))
    }

    fn includes(&self, path: &str) -> bool {
        self.has_include
            && (self.include.is_match(path)
                || self
                    .include_regex
                    .as_ref()
                    .is_some_and(|set| set.is_match(&format!("/{path}"))))
    }

    fn excludes(&self, path: &str) -> bool {
        self.exclude.is_match(path)
            || self
                .exclude_regex
                .as_ref()
                .is_some_and(|set| set.is_match(&format!("/{path}")))
    }
}

/// The regular expressions of `patterns` the `regex` crate accepts, as one set; `None` when there is none.
fn regex_set(patterns: &[String]) -> Option<RegexSet> {
    let valid: Vec<&str> = patterns
        .iter()
        .map(String::as_str)
        .filter(|pattern| Regex::new(pattern).is_ok())
        .collect();
    if valid.is_empty() {
        None
    } else {
        RegexSet::new(valid).ok()
    }
}

/// Matches package and binary names against exact names and globs such as `@types/*`.
#[derive(Debug, Clone, Default)]
pub(crate) struct Names {
    set: GlobSet,
}

impl Names {
    pub(crate) fn new<I, S>(patterns: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let globs = patterns
            .into_iter()
            .flat_map(|pattern| skif_js_plugins::glob::expand_empty_alternatives(pattern.as_ref()))
            .filter_map(|pattern| Glob::new(&pattern).ok())
            .collect();
        Self { set: build(globs) }
    }

    pub(crate) fn is_match(&self, name: &str) -> bool {
        self.set.is_match(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn include_and_exclude() {
        let globs = Globs::new(["**/*.ts", "!**/*.test.ts", "./src/**/*.{js,jsx}"]);
        assert!(globs.is_match("src/a.ts"));
        assert!(globs.is_match("a.ts"));
        assert!(!globs.is_match("src/a.test.ts"));
        assert!(globs.is_match("src/deep/b.jsx"));
        assert!(!globs.is_match("lib/b.jsx"));
        assert!(!Globs::new(["!a"]).is_match("a"));
    }

    #[test]
    fn regular_expressions_match_the_path_after_a_slash_and_exclude_only_their_own_set() {
        let owned = |items: &[&str]| items.iter().map(|item| (*item).to_owned()).collect::<Vec<_>>();
        let jest = Globs::with_regexes(
            &owned(&["**/__mocks__/**/*.js"]),
            &owned(&[r"\.check\.js$"]),
            &owned(&["/__benchmarks__/", r"^/e2e/"]),
        );
        assert!(jest.is_match("src/a.check.js"));
        assert!(jest.is_match("__mocks__/fs.js"));
        assert!(jest.is_match("lib/e2e/b.check.js"));
        assert!(!jest.is_match("src/a.test.js"));
        assert!(!jest.is_match("src/__benchmarks__/b.check.js"));
        assert!(!jest.is_match("e2e/flow.check.js"));
        assert!(!jest.is_match("__mocks__/__benchmarks__/x.js"));

        // The exclusions of one set do not reach the other sets it is chained with.
        let entries = Globs::new(["**/*.check.js"]).or_each(vec![jest]);
        assert!(entries.is_match("e2e/flow.check.js"));
        assert!(entries.is_match("src/a.check.js"));
        assert!(!entries.is_match("src/a.test.js"));

        // A lookahead the `regex` crate rejects matches every path rather than none.
        let lookahead = Globs::with_regexes(&[], &owned(&[r"^(?!.*fixtures).*\.js$"]), &[]);
        assert!(lookahead.is_match("src/anything.ts"));
        // An exclusion it rejects excludes nothing.
        assert!(Globs::with_regexes(&owned(&["**/*.js"]), &[], &owned(&[r"(?!x)"])).is_match("a.js"));
    }

    #[test]
    fn names() {
        let names = Names::new(["@types/*", "eslint"]);
        assert!(names.is_match("@types/node"));
        assert!(names.is_match("eslint"));
        assert!(!names.is_match("eslint-plugin-x"));
    }

    #[test]
    fn single_star_stays_in_one_segment() {
        let globs = Globs::new(["src/*.ts"]);
        assert!(globs.is_match("src/a.ts"));
        assert!(!globs.is_match("src/deep/a.ts"));
    }
}
