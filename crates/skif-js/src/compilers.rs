//! `[js.compilers]`: the files of extensions skif has no reader for, read with one of its readers or by the regular
//! expressions the configuration declares. No compiler runs, so this is the static counterpart of knip's `compilers`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use regex::Regex;
use skif_config::JsCompiler;
use skif_core::LineIndex;
use skif_js_syntax::{Import, ImportKind, ModuleInfo};

/// How the files of one declared extension are read.
#[derive(Debug)]
pub(crate) enum Compiler {
    /// With the reader of this native extension (`as`).
    Reader(String),
    /// By the captures of these patterns (`imports`), each with exactly one capture group.
    Patterns(Vec<Regex>),
}

/// The declared compilers, by lowercase extension.
#[derive(Debug, Default)]
pub(crate) struct Compilers(BTreeMap<String, Compiler>);

impl Compilers {
    /// The compilers of `[js.compilers]`. The configuration was validated when it was loaded, so an entry that is
    /// still invalid (set through the library without validation) is left out rather than guessed at.
    pub(crate) fn new(declared: &IndexMap<String, JsCompiler>) -> Self {
        let compilers = declared
            .iter()
            .filter_map(|(extension, compiler)| {
                let read = match (compiler.reader.as_deref(), compiler.imports.as_deref()) {
                    (Some(reader), None) if JsCompiler::READERS.contains(&reader) => {
                        Compiler::Reader(reader.to_owned())
                    }
                    (None, Some(patterns)) if !patterns.is_empty() => Compiler::Patterns(
                        patterns
                            .iter()
                            .map(|pattern| Regex::new(pattern).ok().filter(|regex| regex.captures_len() == 2))
                            .collect::<Option<Vec<_>>>()?,
                    ),
                    _ => return None,
                };
                let extension = extension.to_ascii_lowercase();
                (!JsCompiler::READERS.contains(&extension.as_str())).then_some((extension, read))
            })
            .collect();
        Self(compilers)
    }

    /// The compiler declared for the extension of `path`.
    pub(crate) fn of(&self, path: &Path) -> Option<&Compiler> {
        if self.0.is_empty() {
            return None;
        }
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        self.0.get(&extension)
    }

    /// Whether a compiler is declared for the extension of `path`.
    pub(crate) fn handles(&self, path: &Path) -> bool {
        self.of(path).is_some()
    }

    /// The declared extensions, sorted.
    pub(crate) fn extensions(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(String::as_str)
    }
}

/// The path `path` is parsed as when its compiler reads it with the reader of `reader`: the same file name with
/// that extension, so the parser and the parse cache select the reader.
pub(crate) fn reader_path(path: &Path, reader: &str) -> PathBuf {
    path.with_extension(reader)
}

/// The imports the captures of `patterns` find in `source`: side-effect imports of the captured specifiers, located
/// at the capture, in the order they appear. A capture that two patterns find is one import.
pub(crate) fn extract(patterns: &[Regex], source: &str) -> ModuleInfo {
    let lines = LineIndex::new(source);
    let mut found: Vec<(usize, usize)> = patterns
        .iter()
        .flat_map(|pattern| pattern.captures_iter(source))
        .filter_map(|captures| captures.get(1))
        .filter(|capture| !capture.as_str().trim().is_empty())
        .map(|capture| (capture.start(), capture.end()))
        .collect();
    found.sort_unstable();
    found.dedup();
    let offset = |at: usize| u32::try_from(at).unwrap_or(u32::MAX);
    ModuleInfo {
        imports: found
            .into_iter()
            .map(|(start, end)| Import {
                specifier: source[start..end].trim().to_owned(),
                kind: ImportKind::SideEffect,
                names: Vec::new(),
                call: None,
                span: lines.span(offset(start), offset(end)),
            })
            .collect(),
        ..ModuleInfo::default()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::resolve::is_read_file;

    fn declared(entries: &[(&str, JsCompiler)]) -> Compilers {
        let map: IndexMap<String, JsCompiler> =
            entries.iter().cloned().map(|(key, value)| (key.to_owned(), value)).collect();
        Compilers::new(&map)
    }

    #[test]
    fn the_readers_a_compiler_can_name_are_the_extensions_skif_reads() {
        for reader in JsCompiler::READERS {
            assert!(is_read_file(Path::new(&format!("x.{reader}"))), "{reader}");
        }
        for extension in crate::resolve::CODE_EXTENSIONS
            .iter()
            .chain(crate::resolve::EXTRACTED_EXTENSIONS)
        {
            assert!(JsCompiler::READERS.contains(extension), "{extension}");
        }
        for other in ["html", "json", "md", "hbs"] {
            assert!(!is_read_file(Path::new(&format!("x.{other}"))), "{other}");
        }
    }

    #[test]
    fn captures_of_every_pattern_are_side_effect_imports_in_file_order() {
        let source = "<link href=\"./a.css\">\n<script src=\"./main.ts\"></script>\n<script src=\" \"></script>\n\
                      <script src=\"./main.ts\"></script>\n";
        let patterns = [
            Regex::new(r#"<script[^>]+src="([^"]*)""#).unwrap(),
            Regex::new(r#"(?:src|href)="(\./[^"]+)""#).unwrap(),
        ];
        let info = extract(&patterns, source);
        let found: Vec<(&str, ImportKind, u32, u32)> = info
            .imports
            .iter()
            .map(|import| (import.specifier.as_str(), import.kind, import.span.start_line, import.span.start_col))
            .collect();
        assert_eq!(
            found,
            [
                ("./a.css", ImportKind::SideEffect, 1, 13),
                ("./main.ts", ImportKind::SideEffect, 2, 14),
                ("./main.ts", ImportKind::SideEffect, 4, 14),
            ]
        );
        assert!(info.exports.is_empty() && !info.fatal);
    }

    #[test]
    fn compilers_are_found_by_lowercase_extension_and_invalid_entries_are_left_out() {
        let compilers = declared(&[
            ("html", JsCompiler::matching(["src=\"([^\"]+)\""])),
            ("doc", JsCompiler::reading("mdx")),
            ("vue", JsCompiler::reading("mdx")),
            ("two", JsCompiler::matching(["(a)(b)"])),
            ("bad", JsCompiler::reading("html")),
        ]);
        assert!(matches!(compilers.of(Path::new("a/Index.HTML")), Some(Compiler::Patterns(p)) if p.len() == 1));
        assert!(matches!(compilers.of(Path::new("page.doc")), Some(Compiler::Reader(r)) if r == "mdx"));
        for path in ["a.vue", "a.two", "a.bad", "a.ts", "html"] {
            assert!(!compilers.handles(Path::new(path)), "{path}");
        }
        assert_eq!(compilers.extensions().collect::<Vec<_>>(), ["doc", "html"]);
        assert_eq!(reader_path(Path::new("/r/docs/page.doc"), "mdx"), Path::new("/r/docs/page.mdx"));
    }
}
