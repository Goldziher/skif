//! Glob sets with exclusions, and name matchers for the ignore lists.

use std::collections::BTreeMap;

use globset::{Glob, GlobBuilder, GlobSet, GlobSetBuilder};
use regex::RegexSet;

use crate::input::ScopedEntryRegex;

/// Include and exclude globs matched against `/`-separated relative paths.
#[derive(Debug, Clone, Default)]
pub(crate) struct Globs {
    include: GlobSet,
    exclude: GlobSet,
    has_include: bool,
    /// Regular expressions matched besides `include` (Jest's `testRegex`).
    include_regex: Option<EntryRegexes>,
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
            also: None,
        }
    }

    /// Also includes the paths that one of `regexes` matches; the exclude globs apply to them as to the include globs.
    #[must_use]
    pub(crate) fn with_regexes(mut self, regexes: Option<EntryRegexes>) -> Self {
        self.has_include |= regexes.is_some();
        self.include_regex = regexes;
        self
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
        (self.includes(path) && !self.exclude.is_match(path))
            || self.also.as_ref().is_some_and(|also| also.is_match(path))
    }

    /// Whether `path` matches any include glob, ignoring exclusions (for ignore lists, where `!` has no meaning).
    pub(crate) fn matches_any(&self, path: &str) -> bool {
        self.includes(path) || self.also.as_ref().is_some_and(|also| also.matches_any(path))
    }

    /// Whether `path` matches an include glob or regular expression.
    fn includes(&self, path: &str) -> bool {
        self.has_include
            && (self.include.is_match(path)
                || self
                    .include_regex
                    .as_ref()
                    .is_some_and(|regexes| regexes.is_match(path)))
    }
}

/// The narrowed scope of an incomplete package ([`skif_core::IncompleteScope::Within`]): its globs, relative to the
/// package directory, compiled once. A file the scope matches may be an entry a configuration hides, so skif does not
/// judge it. Public for the benchmark harness, which matches the scopes of skif's JSON report the way the analysis does.
#[doc(hidden)]
#[derive(Debug, Clone)]
pub struct IncompleteScopeGlobs {
    /// The globs as the input sources gave them, for `explain-usage`.
    pub(crate) patterns: Vec<String>,
    pub(crate) globs: Globs,
}

impl IncompleteScopeGlobs {
    /// Compiles the scope globs `patterns`; a pattern that is not a valid glob is skipped.
    #[must_use]
    pub fn new(patterns: Vec<String>) -> Self {
        Self {
            globs: Globs::new(&patterns),
            patterns,
        }
    }

    /// The globs as given.
    #[must_use]
    pub fn patterns(&self) -> &[String] {
        &self.patterns
    }

    /// Whether `rel`, a `/`-separated path relative to the package directory, lies in the scope.
    #[must_use]
    pub fn is_match(&self, rel: &str) -> bool {
        self.globs.is_match(rel)
    }
}

/// Entry regular expressions compiled once, shared by the [`Globs`] including them: those of the package, matched
/// against `/` followed by the path relative to the package directory, and those of each scope (an Nx project, ADR
/// 0109), matched against `/` followed by the path relative to the scope, for the files below it only.
#[derive(Debug, Clone)]
pub(crate) struct EntryRegexes {
    package: Option<RegexSet>,
    /// Each scope with a trailing `/`, and its expressions, in scope order.
    scoped: Vec<(String, RegexSet)>,
}

impl EntryRegexes {
    /// The expressions `package` of the package and `scoped` of its scopes; `None` when there is none. A set that does
    /// not compile is left out (the plugin that names an expression checks that it compiles).
    pub(crate) fn new(package: &[String], scoped: &[ScopedEntryRegex]) -> Option<Self> {
        let mut by_scope: BTreeMap<String, Vec<&str>> = BTreeMap::new();
        for regex in scoped {
            let scope = format!("{}/", regex.scope.trim_end_matches('/'));
            by_scope.entry(scope).or_default().push(&regex.regex);
        }
        let regexes = Self {
            package: regex_set(package),
            scoped: by_scope
                .into_iter()
                .filter_map(|(scope, regexes)| regex_set(regexes.as_slice()).map(|set| (scope, set)))
                .collect(),
        };
        (regexes.package.is_some() || !regexes.scoped.is_empty()).then_some(regexes)
    }

    /// Whether an expression selects `path`, relative to the package directory.
    fn is_match(&self, path: &str) -> bool {
        self.package.as_ref().is_some_and(|set| set.is_match(&format!("/{path}")))
            || self.scoped.iter().any(|(scope, set)| {
                path.strip_prefix(scope.as_str())
                    .is_some_and(|rest| set.is_match(&format!("/{rest}")))
            })
    }
}

/// `regexes` compiled as one set; `None` when there is none or the set does not compile.
fn regex_set<S: AsRef<str>>(regexes: &[S]) -> Option<RegexSet> {
    if regexes.is_empty() {
        None
    } else {
        RegexSet::new(regexes).ok()
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
    fn a_narrowed_incomplete_scope_matches_paths_relative_to_the_package() {
        let patterns = vec!["src/**/*.stories.tsx".to_owned(), "./.storybook/*.ts".to_owned()];
        let scope = IncompleteScopeGlobs::new(patterns);
        assert!(scope.is_match("src/ui/Button.stories.tsx"));
        assert!(scope.is_match(".storybook/main.ts"));
        assert!(!scope.is_match("src/ui/Button.tsx"));
        assert!(!scope.is_match(".storybook/deep/main.ts"), "`*` stops at `/`");
        assert_eq!(scope.patterns(), ["src/**/*.stories.tsx", "./.storybook/*.ts"]);
        assert!(!IncompleteScopeGlobs::new(Vec::new()).is_match("src/a.ts"));
    }

    #[test]
    fn regular_expressions_match_the_path_after_a_slash_and_the_exclusions_apply() {
        let regexes = [r"\.check\.js$".to_owned(), r"\/__tests__\/".to_owned()];
        let jest = Globs::new(["**/__mocks__/**/*.js", "!vendor/**"]).with_regexes(EntryRegexes::new(&regexes, &[]));
        assert!(jest.is_match("src/a.check.js"));
        assert!(jest.is_match("src/__tests__/a.js"));
        assert!(jest.is_match("__tests__/a.js"));
        assert!(jest.is_match("__mocks__/fs.js"));
        assert!(!jest.is_match("src/a.test.js"));
        assert!(!jest.is_match("src/x__tests__/a.js"));
        assert!(!jest.is_match("vendor/a.check.js"));
        assert!(jest.matches_any("vendor/a.check.js"));

        // Regular expressions alone include paths.
        let only = Globs::new(Vec::<String>::new()).with_regexes(EntryRegexes::new(&[r"^/e2e/".to_owned()], &[]));
        assert!(!only.is_empty());
        assert!(only.is_match("e2e/flow.js"));
        assert!(!only.is_match("lib/e2e/flow.js"));
        assert!(EntryRegexes::new(&[], &[]).is_none());
        assert!(Globs::new(Vec::<String>::new()).with_regexes(None).is_empty());
    }

    /// A scope's expressions see the path below the scope only: an unanchored one does not reach into a sibling
    /// directory whose name starts with the scope's (`apps/web2` for `apps/web`), and an anchored one is anchored at the
    /// scope.
    #[test]
    fn scoped_regular_expressions_match_below_their_scope_only() {
        let scoped = |regex: &str| ScopedEntryRegex {
            scope: "apps/web".to_owned(),
            regex: regex.to_owned(),
        };
        let globs = Globs::new(Vec::<String>::new()).with_regexes(EntryRegexes::new(
            &[],
            &[scoped(r".*\.spec\.ts$"), scoped(r"^.*\.e2e\.ts$")],
        ));
        assert!(globs.is_match("apps/web/src/a.spec.ts"));
        assert!(globs.is_match("apps/web/a.spec.ts"));
        assert!(globs.is_match("apps/web/e2e/flow.e2e.ts"));
        assert!(!globs.is_match("apps/web2/a.spec.ts"));
        assert!(!globs.is_match("apps/web2/flow.e2e.ts"));
        assert!(!globs.is_match("apps/web.spec.ts"));
        assert!(!globs.is_match("apps/api/a.spec.ts"));
        assert!(!globs.is_match("a.spec.ts"));

        let anchored = Globs::new(Vec::<String>::new()).with_regexes(EntryRegexes::new(&[], &[scoped("^/e2e/")]));
        assert!(anchored.is_match("apps/web/e2e/a.ts"));
        assert!(!anchored.is_match("apps/web/src/e2e/a.ts"));
        assert!(!anchored.is_match("e2e/a.ts"));
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
