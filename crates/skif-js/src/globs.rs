//! Glob sets with exclusions, and name matchers for the ignore lists.

use globset::{Glob, GlobBuilder, GlobSet, GlobSetBuilder};

/// Include and exclude globs matched against `/`-separated relative paths.
#[derive(Debug, Clone, Default)]
pub(crate) struct Globs {
    include: GlobSet,
    exclude: GlobSet,
    has_include: bool,
}

fn compile(pattern: &str) -> Option<Glob> {
    let pattern = pattern.trim().trim_start_matches("./").trim_end_matches('!');
    if pattern.is_empty() {
        return None;
    }
    GlobBuilder::new(pattern).literal_separator(true).build().ok()
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
        }
    }

    /// Whether `path` matches an include glob and no exclude glob.
    pub(crate) fn is_match(&self, path: &str) -> bool {
        self.has_include && self.include.is_match(path) && !self.exclude.is_match(path)
    }

    /// Whether `path` matches any include glob, ignoring exclusions (for ignore lists, where `!` has no meaning).
    pub(crate) fn matches_any(&self, path: &str) -> bool {
        self.has_include && self.include.is_match(path)
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
            .filter_map(|pattern| Glob::new(pattern.as_ref()).ok())
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
