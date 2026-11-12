//! A cheap text scan for module specifiers the parser does not see: `new URL("./x", import.meta.url)`. A `new URL`
//! target is loaded as a program of its own, so every export of it counts as used. Test-runner mocks and identifiers used only in
//! component templates are read by `skif-js-syntax`.

use skif_core::Span;
use skif_js_syntax::{Import, ImportKind, ModuleInfo};

fn skip_whitespace(text: &str, position: usize) -> usize {
    position + (text.len() - position - text[position..].trim_start().len())
}

/// The string literal starting at `position` (after whitespace), with its end offset.
fn string_literal(text: &str, position: usize) -> Option<(&str, usize)> {
    let start = skip_whitespace(text, position);
    let quote = text[start..].chars().next().filter(|c| matches!(c, '"' | '\'' | '`'))?;
    let body = start + 1;
    let length = text[body..].find(quote)?;
    let literal = &text[body..body + length];
    if literal.contains(['\n', '\\']) || (quote == '`' && literal.contains("${")) {
        return None;
    }
    Some((literal, body + length + 1))
}

fn span(source: &str, start: usize, end: usize) -> Span {
    let position = |offset: usize| {
        let before = &source[..offset];
        let line = before.matches('\n').count() + 1;
        let column = before.rsplit('\n').next().map_or(0, |last| last.chars().count()) + 1;
        (
            u32::try_from(line).unwrap_or(u32::MAX),
            u32::try_from(column).unwrap_or(u32::MAX),
        )
    };
    let (start_line, start_col) = position(start);
    let (end_line, end_col) = position(end);
    Span::new(
        u32::try_from(start).unwrap_or(u32::MAX),
        u32::try_from(end).unwrap_or(u32::MAX),
        start_line,
        start_col,
        end_line,
        end_col,
    )
}

/// Whether a relative path reaches into a `node_modules` directory (`../node_modules/.bin/tsx`). Such a `new URL`
/// names an installed file or a directory the code creates, not a module it loads, so it is no import.
fn in_node_modules(path: &str) -> bool {
    path.split('/').any(|segment| segment == "node_modules")
}

/// `(specifier, kind, start, end)` of the specifiers written in `new URL("./x", import.meta.url)`.
fn extra_specifiers(source: &str) -> Vec<(String, ImportKind, usize, usize)> {
    let mut found = Vec::new();
    for (start, _) in source.match_indices("new URL(") {
        let Some((literal, after)) = string_literal(source, start + "new URL(".len()) else {
            continue;
        };
        let comma = skip_whitespace(source, after);
        if !source[comma..].starts_with(',') {
            continue;
        }
        let base = skip_whitespace(source, comma + 1);
        if source[base..].starts_with("import.meta.url")
            && (literal.starts_with("./") || literal.starts_with("../"))
            && !in_node_modules(literal)
        {
            found.push((
                literal.to_owned(),
                ImportKind::Url,
                start,
                base + "import.meta.url".len(),
            ));
        }
    }
    found.sort_by_key(|(_, _, start, _)| *start);
    found
}

/// Adds the specifiers of `new URL(…, import.meta.url)` that the parser does not report: an [`ImportKind::Url`] of
/// the whole module.
pub(crate) fn add_textual_imports(info: &mut ModuleInfo, source: &str) {
    if !source.contains("import.meta.url") {
        return;
    }
    for (specifier, kind, start, end) in extra_specifiers(source) {
        if info
            .imports
            .iter()
            .any(|import| import.specifier == specifier && import.kind == kind)
        {
            continue;
        }
        info.imports.push(Import {
            specifier,
            kind,
            names: Vec::new(),
            call: None,
            span: span(source, start, end),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_into_node_modules_are_not_imports() {
        let source = "new URL('../../node_modules/.bin/tsx', import.meta.url);\n\
                      new URL('./node_modules/later', import.meta.url);\nnew URL('./modules/a.ts', import.meta.url);";
        let found: Vec<String> = extra_specifiers(source).into_iter().map(|(s, _, _, _)| s).collect();
        assert_eq!(found, ["./modules/a.ts"]);
    }

    #[test]
    fn url_specifiers_are_whole_module_imports() {
        let source = "const w = new URL('./worker.ts', import.meta.url);\nvi.mock(\"./dep\", () => ({}));\nnew URL('https://x.dev', import.meta.url);\nnew URL(`./${name}.ts`, import.meta.url);";
        let found: Vec<(String, ImportKind)> = extra_specifiers(source)
            .into_iter()
            .map(|(s, kind, _, _)| (s, kind))
            .collect();
        assert_eq!(found, [("./worker.ts".to_owned(), ImportKind::Url)]);
    }

    #[test]
    fn a_url_target_is_added_next_to_a_named_import_of_the_same_file() {
        let source =
            "import { a } from './a.ts';\nnew URL('./a.ts', import.meta.url);\nnew URL('./a.ts', import.meta.url);";
        let mut info = ModuleInfo::default();
        info.imports.push(Import {
            specifier: "./a.ts".to_owned(),
            kind: ImportKind::Static,
            names: Vec::new(),
            call: None,
            span: span(source, 0, 26),
        });
        add_textual_imports(&mut info, source);
        let kinds: Vec<ImportKind> = info.imports.iter().map(|import| import.kind).collect();
        assert_eq!(kinds, [ImportKind::Static, ImportKind::Url]);
    }

    #[test]
    fn spans_count_lines_and_columns() {
        let span = span("a\nbc\ndef", 5, 8);
        assert_eq!(
            (span.start_line, span.start_col, span.end_line, span.end_col),
            (3, 1, 3, 4)
        );
    }
}
