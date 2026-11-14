//! A lightweight import scanner for stylesheets (`.css .scss .sass .less .pcss .styl .stylus`).
//!
//! It understands comments and strings, and recognises `@import`, `@use`, `@forward`, `@plugin`, `@config`,
//! `@reference`, Stylus `@require` and CSS modules `composes: … from "x"`. `@source` is a glob and `url()` in other
//! properties is an asset, so neither is reported, except a `url()` that names a file of a package (`~pkg/file`,
//! `@scope/pkg/file`), which uses the package.

use std::path::Path;

use skif_core::LineIndex;

use crate::model::{Import, ImportKind, ModuleInfo};

const EXTENSIONS: &[&str] = &["css", "scss", "sass", "less", "pcss", "styl", "stylus"];

/// Whether `path` has a stylesheet extension.
#[must_use]
pub fn is_stylesheet(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str()))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Flavor {
    Css,
    Scss,
    Sass,
    Less,
    Stylus,
}

struct Found {
    specifier: String,
    start: usize,
    end: usize,
}

/// Scans the stylesheet `source` (the content of `path`) for the files and packages it imports. Every import has
/// [`ImportKind::SideEffect`]. Nothing is reported for remote URLs, `data:` URLs, Sass built-in modules,
/// specifiers with interpolation and Stylus globs (`@import "mixins/*"`).
#[must_use]
pub fn parse_stylesheet(path: &Path, source: &str) -> ModuleInfo {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let flavor = match extension.as_str() {
        "scss" | "pcss" => Flavor::Scss,
        "sass" => Flavor::Sass,
        "less" => Flavor::Less,
        "styl" | "stylus" => Flavor::Stylus,
        _ => Flavor::Css,
    };
    let mut scanner = Scanner {
        src: source.as_bytes(),
        text: source,
        pos: 0,
        flavor,
        found: Vec::new(),
    };
    scanner.run();
    let lines = LineIndex::new(source);
    let imports = scanner
        .found
        .into_iter()
        .filter(|found| flavor != Flavor::Stylus || !found.specifier.contains('*'))
        .filter_map(|found| {
            let specifier = normalize(&found.specifier)?;
            let start = u32::try_from(found.start.min(source.len())).ok()?;
            let end = u32::try_from(found.end.min(source.len())).ok()?;
            Some(Import {
                specifier,
                kind: ImportKind::SideEffect,
                names: Vec::new(),
                call: None,
                span: lines.span(start, end),
            })
        })
        .collect();
    ModuleInfo {
        imports,
        ..ModuleInfo::default()
    }
}

/// Whether `specifier` contains a Sass variable. A leading `$name` segment is an alias root (`$lib/x.css`), not one.
fn has_variable(specifier: &str) -> bool {
    let rest = specifier
        .strip_prefix('$')
        .map_or(specifier, |after| after.find('/').map_or("", |at| &after[at..]));
    rest.contains('$')
}

/// The specifier to resolve, or `None` for one that is not a file or package.
fn normalize(raw: &str) -> Option<String> {
    let specifier = raw.trim();
    let skipped = specifier.is_empty()
        || ["http:", "https:", "//", "data:", "sass:", "#"]
            .iter()
            .any(|prefix| specifier.starts_with(prefix))
        || specifier.contains("#{")
        || specifier.contains("@{")
        || has_variable(specifier);
    if skipped {
        return None;
    }
    match specifier.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => None,
        Some(rest) => Some(rest.to_owned()),
        None => Some(specifier.to_owned()),
    }
}

/// Whether the `url()` value `value` names a file inside a package rather than an asset beside the stylesheet: a
/// webpack `~pkg/file` path or a scoped `@scope/pkg/file` path, which a relative file name does not look like. An
/// unscoped bare path (`images/bg.png`) is relative to the stylesheet, and `@/…` is an alias.
fn names_package_file(value: &str) -> bool {
    let value = value.trim();
    let (path, scoped) = match value.strip_prefix('~') {
        Some(rest) => (rest, rest.starts_with('@')),
        None if value.starts_with('@') => (value, true),
        None => return false,
    };
    let mut parts = path
        .strip_prefix('@')
        .unwrap_or(path)
        .splitn(if scoped { 3 } else { 2 }, '/');
    let names = if scoped {
        [parts.next(), parts.next()]
            .into_iter()
            .all(|part| part.is_some_and(is_package_segment))
    } else {
        parts.next().is_some_and(is_package_segment)
    };
    names && parts.next().is_some_and(|file| !file.is_empty())
}

/// The file a `url()` value names, without the query or fragment after it (`a.eot?#iefix`, `sprite.svg#icon`).
fn without_query(value: &str) -> &str {
    value.find(['?', '#']).map_or(value, |at| &value[..at])
}

/// Whether `segment` can be an npm package scope or name: lowercase letters, digits, `-`, `_` and `.`, not starting
/// with `.` or `_`. A path step, interpolation, URL syntax and a capitalised alias directory (`~Assets`) are not.
fn is_package_segment(segment: &str) -> bool {
    !segment.is_empty()
        && !segment.starts_with(['.', '_'])
        && segment
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_' | b'.'))
}

const fn is_ident(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')
}

struct Scanner<'a> {
    src: &'a [u8],
    text: &'a str,
    pos: usize,
    flavor: Flavor,
    found: Vec<Found>,
}

impl Scanner<'_> {
    fn cur(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn peek(&self, offset: usize) -> Option<u8> {
        self.src.get(self.pos + offset).copied()
    }

    fn before_is_ident(&self) -> bool {
        self.pos > 0 && self.src.get(self.pos - 1).copied().is_some_and(is_ident)
    }

    /// The word `word` (ASCII case-insensitive) starts here and is not part of a longer identifier.
    fn at_word(&self, word: &str) -> bool {
        let end = self.pos + word.len();
        self.src
            .get(self.pos..end)
            .is_some_and(|slice| slice.eq_ignore_ascii_case(word.as_bytes()))
            && !self.before_is_ident()
            && !self.src.get(end).copied().is_some_and(is_ident)
    }

    fn at_url(&self) -> bool {
        !self.before_is_ident()
            && self
                .src
                .get(self.pos..self.pos + 4)
                .is_some_and(|slice| slice.eq_ignore_ascii_case(b"url("))
    }

    fn skip_block_comment(&mut self) {
        self.pos += 2;
        while self.pos < self.src.len() {
            if self.cur() == Some(b'*') && self.peek(1) == Some(b'/') {
                self.pos += 2;
                return;
            }
            self.pos += 1;
        }
    }

    fn skip_line_comment(&mut self) {
        while self.cur().is_some_and(|byte| byte != b'\n') {
            self.pos += 1;
        }
    }

    fn at_line_comment(&self) -> bool {
        self.flavor != Flavor::Css && self.cur() == Some(b'/') && self.peek(1) == Some(b'/')
    }

    fn skip_trivia(&mut self) {
        loop {
            match self.cur() {
                Some(byte) if byte.is_ascii_whitespace() => self.pos += 1,
                Some(b'/') if self.peek(1) == Some(b'*') => self.skip_block_comment(),
                Some(b'/') if self.at_line_comment() => self.skip_line_comment(),
                _ => return,
            }
        }
    }

    /// Reads the string literal starting at the quote under the cursor: its content and the end offset.
    fn read_string(&mut self) -> Option<(String, usize)> {
        let quote = self.cur()?;
        let start = self.pos + 1;
        self.pos = start;
        while let Some(byte) = self.cur() {
            match byte {
                b'\\' => self.pos += 2,
                b'\n' => break,
                _ if byte == quote => {
                    let content = self.text.get(start..self.pos)?.to_owned();
                    self.pos += 1;
                    return Some((content, self.pos));
                }
                _ => self.pos += 1,
            }
        }
        self.pos = self.pos.min(self.src.len());
        None
    }

    /// Reads a `url(…)` outside an import and records it when it names a file of a package ([`names_package_file`]).
    /// Only a quote right after the parenthesis starts a string: in an unquoted URL an apostrophe is part of the file
    /// name.
    fn url(&mut self) {
        let start = self.pos;
        self.pos += 4;
        while self.cur().is_some_and(|byte| byte.is_ascii_whitespace()) {
            self.pos += 1;
        }
        let quoted = matches!(self.cur(), Some(b'"' | b'\''));
        let string = if quoted {
            self.read_string().map(|(content, _)| content)
        } else {
            None
        };
        let from = self.pos;
        let mut close = self.src.len();
        while let Some(byte) = self.cur() {
            self.pos += 1;
            if byte == b')' {
                close = self.pos - 1;
                break;
            }
        }
        let value = if quoted {
            string
        } else {
            self.text.get(from..close).map(|value| value.trim().to_owned())
        };
        if let Some(specifier) = value
            .map(|value| without_query(&value).to_owned())
            .filter(|value| names_package_file(value))
        {
            self.found.push(Found {
                specifier,
                start,
                end: self.pos,
            });
        }
    }

    fn run(&mut self) {
        while let Some(byte) = self.cur() {
            match byte {
                b'/' if self.peek(1) == Some(b'*') => self.skip_block_comment(),
                b'/' if self.at_line_comment() => self.skip_line_comment(),
                b'"' | b'\'' => {
                    self.read_string();
                }
                b'@' => self.at_rule(),
                b'u' | b'U' if self.at_url() => self.url(),
                b'c' if self.at_word("composes") => self.composes(),
                _ => self.pos += 1,
            }
        }
    }

    fn at_rule(&mut self) {
        let start = self.pos;
        self.pos += 1;
        let name_start = self.pos;
        while self.cur().is_some_and(is_ident) {
            self.pos += 1;
        }
        let name = self
            .text
            .get(name_start..self.pos)
            .unwrap_or_default()
            .to_ascii_lowercase();
        match name.as_str() {
            "import" => self.import_list(start),
            "require" if self.flavor == Flavor::Stylus => self.import_list(start),
            "use" | "forward" | "plugin" | "config" | "reference" => {
                self.skip_trivia();
                self.target(start);
            }
            _ => {}
        }
    }

    /// `@import` takes a comma-separated list and, in Less, leading options such as `(reference)`.
    fn import_list(&mut self, start: usize) {
        self.skip_trivia();
        if self.cur() == Some(b'(') {
            while self.cur().is_some_and(|byte| byte != b')') {
                self.pos += 1;
            }
            self.pos += 1;
            self.skip_trivia();
        }
        while self.target(start) {
            self.skip_trivia();
            if self.cur() != Some(b',') {
                break;
            }
            self.pos += 1;
            self.skip_trivia();
        }
    }

    /// Reads a string, `url(…)` or (in indented Sass and Stylus) bare word and records it. False when none is here.
    fn target(&mut self, start: usize) -> bool {
        let Some(byte) = self.cur() else {
            return false;
        };
        if byte == b'"' || byte == b'\'' {
            return self.record_string(start);
        }
        if self.at_url() {
            self.pos += 4;
            self.skip_trivia();
            if matches!(self.cur(), Some(b'"' | b'\'')) {
                let recorded = self.record_string(start);
                while self.cur().is_some_and(|byte| byte != b')') {
                    self.pos += 1;
                }
                self.pos += 1;
                return recorded;
            }
            let from = self.pos;
            while self.cur().is_some_and(|byte| byte != b')') {
                self.pos += 1;
            }
            let specifier = self.text.get(from..self.pos).unwrap_or_default().to_owned();
            self.pos += 1;
            self.found.push(Found {
                specifier,
                start,
                end: self.pos,
            });
            return true;
        }
        if matches!(self.flavor, Flavor::Sass | Flavor::Stylus)
            && !byte.is_ascii_whitespace()
            && !matches!(byte, b';' | b',')
        {
            let from = self.pos;
            while self
                .cur()
                .is_some_and(|byte| !byte.is_ascii_whitespace() && !matches!(byte, b';' | b','))
            {
                self.pos += 1;
            }
            let specifier = self.text.get(from..self.pos).unwrap_or_default().to_owned();
            self.found.push(Found {
                specifier,
                start,
                end: self.pos,
            });
            return true;
        }
        false
    }

    fn record_string(&mut self, start: usize) -> bool {
        match self.read_string() {
            Some((specifier, end)) => {
                self.found.push(Found { specifier, start, end });
                true
            }
            None => false,
        }
    }

    /// `composes: a b from "./other.module.css"`; `from global` has no string and is not an import.
    fn composes(&mut self) {
        let start = self.pos;
        self.pos += "composes".len();
        self.skip_trivia();
        if self.cur() != Some(b':') {
            return;
        }
        self.pos += 1;
        let mut last = String::new();
        while let Some(byte) = self.cur() {
            match byte {
                b';' | b'}' => return,
                b'"' | b'\'' => {
                    if let Some((specifier, end)) = self.read_string()
                        && last == "from"
                    {
                        self.found.push(Found { specifier, start, end });
                    }
                    last.clear();
                }
                _ if is_ident(byte) => {
                    let from = self.pos;
                    while self.cur().is_some_and(is_ident) {
                        self.pos += 1;
                    }
                    self.text.get(from..self.pos).unwrap_or_default().clone_into(&mut last);
                }
                _ => self.pos += 1,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn specifiers(path: &str, source: &str) -> Vec<String> {
        parse_stylesheet(Path::new(path), source)
            .imports
            .into_iter()
            .map(|import| import.specifier)
            .collect()
    }

    #[test]
    fn tailwind_entry_stylesheet() {
        let source = "@import \"tailwindcss\";\n@import \"tw-animate-css\";\n@import \"kit/tailwind.css\";\n\
                      @import \"@fonts/geist\";\n@source \"../node_modules/lib\";\n@plugin \"@tw/typography\";\n\
                      @config \"../tailwind.config.js\";\n@reference \"./app.css\";";
        assert_eq!(
            specifiers("index.css", source),
            [
                "tailwindcss",
                "tw-animate-css",
                "kit/tailwind.css",
                "@fonts/geist",
                "@tw/typography",
                "../tailwind.config.js",
                "./app.css",
            ]
        );
    }

    #[test]
    fn import_forms() {
        let source = "@import url(\"a.css\");\n@import url(b.css) screen;\n@import 'c.css' layer(base) supports(display: grid) print;\n\
                      @import \"d.css\", \"e.css\";\n@import url(https://cdn.test/x.css);\n@import \"https://cdn.test/y.css\";";
        assert_eq!(
            specifiers("a.css", source),
            ["a.css", "b.css", "c.css", "d.css", "e.css"]
        );
    }

    #[test]
    fn comments_and_assets_are_ignored() {
        let source = "/* @import \"gone\"; */\n.a { background: url(\"./img.png\"); content: \"@import 'nope'\"; }\n\
                      @import \"kept\";";
        assert_eq!(specifiers("a.css", source), ["kept"]);
        let line_comment = "// @use \"gone\";\n@use \"kept\";\n.a { background: url(http://x.test/a.png); }";
        assert_eq!(specifiers("a.scss", line_comment), ["kept"]);
    }

    #[test]
    fn urls_of_package_files_are_imports() {
        let source = "@font-face { src: url(\"@ibm/plex/Mono/fonts/a.woff2\") format(\"woff2\"); }\n\
                      .a { background: url( '@scope/pkg/img.png' ); }\n\
                      .b { background: url(~legacy-icons/sprite.svg); }\n\
                      .c { background: url(\"./img.png\"), url(images/bg.png), url(\"@/assets/a.png\"); }\n\
                      .d { background: url(~/x.png), url(\"@scope\"), url(\"@scope/pkg\"), url(@{var}/x.png); }\n\
                      .e { background: url(https://x.test/a.png); }\n\
                      @font-face { src: url(\"@fontsource/inter/files/a.eot?#iefix\"), url(~icons/sprite.svg#home); }\n\
                      .f { background: url(~Assets/a.png), url(\"@Scope/pkg/a.png\"), url(~_private/a.png); }\n\
                      .g { background: url(\"@scope/.hidden/a.png\"), url(~my_icons.v2/a.svg); }";
        assert_eq!(
            specifiers("a.css", source),
            [
                "@ibm/plex/Mono/fonts/a.woff2",
                "@scope/pkg/img.png",
                "legacy-icons/sprite.svg",
                "@fontsource/inter/files/a.eot",
                "icons/sprite.svg",
                "my_icons.v2/a.svg"
            ]
        );
        let info = parse_stylesheet(Path::new("a.css"), ".a { b: url(\"@s/p/x.png\"); }");
        assert_eq!(info.imports[0].kind, ImportKind::SideEffect);
        assert_eq!((info.imports[0].span.start, info.imports[0].span.end), (8, 25));
    }

    #[test]
    fn scss_modules() {
        let source =
            "@use \"sass:math\";\n@use \"pkg\" as p with ($a: 1);\n@forward \"./theme\";\n@use \"~legacy/mixins\";";
        assert_eq!(specifiers("a.scss", source), ["pkg", "./theme", "legacy/mixins"]);
    }

    #[test]
    fn stylus_import_and_require() {
        let source = "// @import 'gone'\n@import 'vars'\n@require \"./mixins\", '~nib'\n@import mixins/*\n@import theme\n\
                      @require 'grid.css'\n.a\n  color red";
        assert_eq!(
            specifiers("a.styl", source),
            ["vars", "./mixins", "nib", "theme", "grid.css"]
        );
        assert_eq!(specifiers("a.stylus", "@require 'b'"), ["b"]);
        assert_eq!(specifiers("a.scss", "@require 'not-scss';"), [] as [String; 0]);
    }

    #[test]
    fn indented_sass_bare_words() {
        assert_eq!(
            specifiers("a.sass", "@use sass:math\n@use pkg\n@import a, b\n"),
            ["pkg", "a", "b"]
        );
    }

    #[test]
    fn less_import_options() {
        assert_eq!(
            specifiers("a.less", "@import (reference) \"./base\";\n@plugin \"p\";"),
            ["./base", "p"]
        );
    }

    #[test]
    fn css_modules_composes() {
        let source =
            ".a { composes: b c from \"./other.module.css\"; }\n.d { composes: e from global; }\n.f { composes: g; }";
        assert_eq!(specifiers("a.module.css", source), ["./other.module.css"]);
    }

    #[test]
    fn interpolation_is_skipped_and_spans_are_located() {
        let info = parse_stylesheet(Path::new("a.scss"), "@import \"a#{$b}\";\n@import \"b\";");
        assert_eq!(info.imports.len(), 1);
        assert_eq!(info.imports[0].span.start_line, 2);
    }

    #[test]
    fn pcss_line_comments_are_skipped() {
        assert_eq!(
            specifiers("a.pcss", "// @import \"gone\";\n@import \"kept\";"),
            ["kept"]
        );
    }

    #[test]
    fn unquoted_url_with_an_apostrophe_does_not_swallow_the_next_import() {
        let source = ".a { background: url(it's.png); } @import \"kept\";";
        assert_eq!(specifiers("a.css", source), ["kept"]);
    }

    #[test]
    fn unterminated_url_keeps_spans_inside_the_source() {
        let source = "@import url(a.css";
        let info = parse_stylesheet(Path::new("a.css"), source);
        assert_eq!(info.imports.len(), 1);
        assert!(info.imports[0].span.end as usize <= source.len());
    }

    #[test]
    fn stylesheet_extensions() {
        assert!(is_stylesheet(Path::new("a/B.SCSS")));
        assert!(!is_stylesheet(Path::new("a.ts")));
    }
    #[test]
    fn alias_roots_starting_with_a_dollar_are_imports_but_variables_are_not() {
        let source = "@import \"$lib/theme.css\";\n@import \"pkg/$name\";\n@import \"$lib/$part.css\";";
        assert_eq!(specifiers("a.css", source), ["$lib/theme.css"]);
    }
}
