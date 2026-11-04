//! Compiled name selectors: glob lists with `!` negation and the `$LOCAL` token.

use std::path::Path;

use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::ConfigError;

/// The token that selects workspace packages (for dependencies: those that are themselves workspace packages).
pub const LOCAL: &str = "$LOCAL";

/// The prefix that anchors a pattern at the root being linted instead of the file that declares it.
pub const ROOT: &str = "$ROOT";

/// A list of name patterns compiled once.
///
/// Each pattern is a glob (`*` also crosses `/`, so `@repo/*` matches `@repo/a/b`), the token `$LOCAL`, or either
/// prefixed with `!` to exclude. A name is selected when
///
/// - no exclusion matches it, and
/// - some inclusion matches it, or the selector has exclusions only (then every other name is selected).
///
/// Exclusions win regardless of their position in the list. An empty selector matches nothing, so callers whose
/// empty list means "everything" check [`is_empty`](Self::is_empty) first.
///
/// Serialized as the list of patterns; deserializing compiles them, so an invalid pattern is a parse error.
#[derive(Debug, Clone)]
pub struct Selector {
    patterns: Vec<String>,
    include: GlobSet,
    exclude: GlobSet,
    include_local: bool,
    exclude_local: bool,
    has_include: bool,
}

impl Default for Selector {
    fn default() -> Self {
        Self {
            patterns: Vec::new(),
            include: GlobSet::empty(),
            exclude: GlobSet::empty(),
            include_local: false,
            exclude_local: false,
            has_include: false,
        }
    }
}

impl Selector {
    /// Compiles `patterns`.
    ///
    /// # Errors
    /// [`ConfigError::Invalid`] naming the first pattern that is empty or not a valid glob.
    pub fn new<I, S>(patterns: I) -> Result<Self, ConfigError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self::compile(patterns.into_iter().map(Into::into).collect())
            .map_err(|message| ConfigError::invalid(None, message))
    }

    fn compile(patterns: Vec<String>) -> Result<Self, String> {
        let mut include = GlobSetBuilder::new();
        let mut exclude = GlobSetBuilder::new();
        let mut selector = Self::default();
        for pattern in &patterns {
            let (negated, body) = pattern
                .strip_prefix('!')
                .map_or((false, pattern.as_str()), |body| (true, body));
            if body.is_empty() {
                return Err(format!("invalid selector pattern \"{pattern}\": the pattern is empty"));
            }
            if !negated {
                selector.has_include = true;
            }
            if body == LOCAL {
                if negated {
                    selector.exclude_local = true;
                } else {
                    selector.include_local = true;
                }
                continue;
            }
            let glob =
                Glob::new(body).map_err(|error| format!("invalid selector pattern \"{pattern}\": {}", error.kind()))?;
            if negated {
                exclude.add(glob);
            } else {
                include.add(glob);
            }
        }
        let build = |builder: GlobSetBuilder, pattern: &str| {
            builder
                .build()
                .map_err(|error| format!("invalid selector pattern in \"{pattern}\": {error}"))
        };
        let joined = patterns.join(", ");
        selector.include = build(include, &joined)?;
        selector.exclude = build(exclude, &joined)?;
        selector.patterns = patterns;
        Ok(selector)
    }

    /// The patterns as written.
    #[must_use]
    pub fn patterns(&self) -> &[String] {
        &self.patterns
    }

    /// Whether the selector has no patterns.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    /// Whether `name` is selected; `is_local` says whether it is a workspace package, for `$LOCAL`.
    #[must_use]
    pub fn matches(&self, name: &str, is_local: bool) -> bool {
        self.matches_any([name], is_local)
    }

    /// Whether an entity known by several `names` (a package's name and directory) is selected: none of the names
    /// may be excluded, and one of them must be included.
    #[must_use]
    pub fn matches_any<'a>(&self, names: impl IntoIterator<Item = &'a str>, is_local: bool) -> bool {
        if self.patterns.is_empty() || (self.exclude_local && is_local) {
            return false;
        }
        let mut included = self.include_local && is_local;
        for name in names {
            if self.exclude.is_match(name) {
                return false;
            }
            included = included || self.include.is_match(name);
        }
        included || !self.has_include
    }
}

impl PartialEq for Selector {
    fn eq(&self, other: &Self) -> bool {
        self.patterns == other.patterns
    }
}

impl Eq for Selector {}

impl<const N: usize> PartialEq<[&str; N]> for Selector {
    fn eq(&self, other: &[&str; N]) -> bool {
        self.patterns.iter().map(String::as_str).eq(other.iter().copied())
    }
}

impl Serialize for Selector {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.patterns.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Selector {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let patterns = Vec::<String>::deserialize(deserializer)?;
        Self::compile(patterns).map_err(D::Error::custom)
    }
}

/// Compiles `patterns` of a file in `declared_in` for the root `root`, rewriting the path patterns to absolute ones.
///
/// A pattern starting with `./` or `../` is anchored at `declared_in`, one starting with `$ROOT/` at `root`; with
/// `paths_only` every other pattern is anchored at `declared_in` too, otherwise it is a name pattern and stays as
/// written. A path pattern that would leave its anchor never matches and is dropped.
pub(crate) fn anchor_patterns(
    patterns: &[String],
    declared_in: &Path,
    root: &Path,
    paths_only: bool,
) -> Result<Selector, ConfigError> {
    let mut anchored = Vec::with_capacity(patterns.len());
    for pattern in patterns {
        let (negation, body) = pattern
            .strip_prefix('!')
            .map_or(("", pattern.as_str()), |body| ("!", body));
        let target = if body == ROOT || body.starts_with("$ROOT/") {
            Some((root, body.strip_prefix("$ROOT/").unwrap_or("")))
        } else if matches!(body, "." | "..") || body.starts_with("./") || body.starts_with("../") || paths_only {
            Some((declared_in, body))
        } else {
            None
        };
        match target {
            None => anchored.push(pattern.clone()),
            Some((base, rest)) => {
                for path in anchored_path(base, rest).into_iter().flat_map(with_directory) {
                    anchored.push(format!("{negation}{path}"));
                }
            }
        }
    }
    Selector::new(anchored)
}

/// `dir/**` also selects `dir` itself: a glob ending in `/**` matches what is below the directory, not the directory,
/// and `./apps/old/**` is meant to select the package that lives at `apps/old` too.
fn with_directory(path: String) -> Vec<String> {
    match path.strip_suffix("/**") {
        Some(directory) if !directory.is_empty() => vec![directory.to_owned(), path],
        _ => vec![path],
    }
}

/// `rest` below `base` as an absolute glob, or `None` when `rest` climbs out of `base`.
fn anchored_path(base: &Path, rest: &str) -> Option<String> {
    let mut below: Vec<&str> = Vec::new();
    for segment in rest.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                below.pop()?;
            }
            other => below.push(other),
        }
    }
    let base = skif_core::slash(base);
    let mut path = escape_glob(base.trim_end_matches('/'));
    for segment in below {
        path.push('/');
        path.push_str(segment);
    }
    if path.is_empty() {
        path.push('/');
    }
    Some(path)
}

fn escape_glob(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        if "*?[]{}".contains(character) {
            escaped.push('[');
            escaped.push(character);
            escaped.push(']');
        } else {
            escaped.push(character);
        }
    }
    escaped
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn selector(patterns: &[&str]) -> Selector {
        Selector::new(patterns.iter().copied()).unwrap()
    }

    #[test]
    fn match_table() {
        #[rustfmt::skip]
        let cases: &[(&[&str], &str, bool, bool)] = &[
            (&[],                       "a",           false, false),
            (&["a"],                    "a",           false, true),
            (&["a"],                    "ab",          false, false),
            (&["a*"],                   "ab",          false, true),
            (&["*b"],                   "ab",          false, true),
            (&["*x*"],                  "axb",         false, true),
            (&["@repo/*"],              "@repo/a/b",   false, true),
            (&["@repo/*"],              "@other/a",    false, false),
            (&["pkgs/**"],              "pkgs/a/b",    false, true),
            (&["{a,b}"],                "b",           false, true),
            (&["a", "b"],               "b",           false, true),
            (&["!a"],                   "b",           false, true),
            (&["!a"],                   "a",           false, false),
            (&["@repo/*", "!@repo/x"],  "@repo/y",     false, true),
            (&["@repo/*", "!@repo/x"],  "@repo/x",     false, false),
            (&["!@repo/x", "@repo/*"],  "@repo/x",     false, false),
            (&["$LOCAL"],               "anything",    true,  true),
            (&["$LOCAL"],               "anything",    false, false),
            (&["!$LOCAL"],              "anything",    true,  false),
            (&["!$LOCAL"],              "anything",    false, true),
            (&["$LOCAL", "!legacy"],    "legacy",      true,  false),
            (&["x", "$LOCAL"],          "y",           true,  true),
        ];
        for &(patterns, name, local, expected) in cases {
            assert_eq!(
                selector(patterns).matches(name, local),
                expected,
                "{patterns:?} vs {name} local={local}"
            );
        }
    }

    #[test]
    fn an_anchored_recursive_pattern_also_selects_the_directory_itself() {
        let anchored =
            |pattern: &str| anchor_patterns(&[pattern.to_owned()], Path::new("/r"), Path::new("/r"), false).unwrap();
        let selector = anchored("./apps/old/**");
        assert!(selector.matches("/r/apps/old", false));
        assert!(selector.matches("/r/apps/old/x/y", false));
        assert!(!selector.matches("/r/apps/older", false));
        assert!(!selector.matches("/r/apps", false));
        let negated = anchor_patterns(
            &["./apps/**".to_owned(), "!./apps/old/**".to_owned()],
            Path::new("/r"),
            Path::new("/r"),
            false,
        )
        .unwrap();
        assert!(negated.matches("/r/apps/new", false));
        assert!(!negated.matches("/r/apps/old", false));
        assert!(!negated.matches("/r/apps/old/x", false));
    }

    #[test]
    fn exclusions_win_regardless_of_order() {
        let first = selector(&["!@repo/x", "@repo/*"]);
        let last = selector(&["@repo/*", "!@repo/x"]);
        for name in ["@repo/x", "@repo/y", "other"] {
            assert_eq!(first.matches(name, false), last.matches(name, false), "{name}");
        }
        assert!(!first.matches("@repo/x", false) && first.matches("@repo/y", false));
    }

    #[test]
    fn matches_any_excludes_when_any_name_is_excluded() {
        let selector = selector(&["packages/*", "!legacy"]);
        assert!(selector.matches_any(["app", "packages/app"], true));
        assert!(!selector.matches_any(["legacy", "packages/legacy"], true));
        assert!(!selector.matches_any(["legacy", "packages/app"], true));
        assert!(!selector.matches_any(["other", "tools/other"], true));
        assert!(!selector.matches_any(std::iter::empty::<&str>(), true));
    }

    #[test]
    fn invalid_patterns_name_the_pattern() {
        for pattern in ["a[", "!{a", "", "!"] {
            let error = Selector::new([pattern]).unwrap_err();
            assert!(matches!(error, ConfigError::Invalid { .. }), "{pattern}");
            assert!(
                error.to_string().contains(&format!("\"{pattern}\"")),
                "{pattern}: {error}"
            );
        }
    }

    #[test]
    fn default_and_empty_selectors_match_nothing() {
        for selector in [Selector::default(), selector(&[])] {
            assert!(selector.is_empty());
            assert!(!selector.matches("a", true) && !selector.matches("a", false));
        }
    }

    #[test]
    fn serde_round_trips_and_rejects_invalid_patterns() {
        let original = selector(&["a*", "!b", "$LOCAL"]);
        let json = serde_json::to_string(&original).unwrap();
        assert_eq!(json, r#"["a*","!b","$LOCAL"]"#);
        let parsed: Selector = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, original);
        assert!(parsed.matches("ax", false) && !parsed.matches("b", true));
        let error = serde_json::from_str::<Selector>(r#"["ok","a["]"#).unwrap_err();
        assert!(error.to_string().contains("\"a[\""), "{error}");
    }

    #[test]
    fn compares_with_string_arrays() {
        assert_eq!(selector(&["a", "!b"]), ["a", "!b"]);
        assert_ne!(selector(&["a"]), ["a", "b"]);
    }
}
