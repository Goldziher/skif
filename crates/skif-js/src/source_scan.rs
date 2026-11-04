//! A cheap text scan for module specifiers the parser does not see: `new URL("./x", import.meta.url)` and
//! `vi.mock("./x")`. Identifiers used only in component templates are read by `skif-js-syntax`.

use skif_core::Span;
use skif_js_syntax::{Import, ImportKind, ModuleInfo};

const MOCK_CALLS: &[&str] = &[
    "vi.mock(",
    "vi.doMock(",
    "vi.unmock(",
    "vi.importActual(",
    "vi.importMock(",
    "jest.mock(",
    "jest.doMock(",
    "jest.unmock(",
    "jest.requireActual(",
    "jest.requireMock(",
    "jest.createMockFromModule(",
];

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
    Span {
        start: u32::try_from(start).unwrap_or(u32::MAX),
        end: u32::try_from(end).unwrap_or(u32::MAX),
        start_line,
        start_col,
        end_line,
        end_col,
    }
}

/// `(specifier, start, end)` of the specifiers written in `new URL("./x", import.meta.url)` and in mock calls.
fn extra_specifiers(source: &str) -> Vec<(String, usize, usize)> {
    let mut found = Vec::new();
    for call in MOCK_CALLS {
        for (start, _) in source.match_indices(call) {
            if let Some((literal, end)) = string_literal(source, start + call.len())
                && !literal.is_empty()
            {
                found.push((literal.to_owned(), start, end));
            }
        }
    }
    for (start, _) in source.match_indices("new URL(") {
        let Some((literal, after)) = string_literal(source, start + "new URL(".len()) else {
            continue;
        };
        let comma = skip_whitespace(source, after);
        if !source[comma..].starts_with(',') {
            continue;
        }
        let base = skip_whitespace(source, comma + 1);
        if source[base..].starts_with("import.meta.url") && (literal.starts_with("./") || literal.starts_with("../")) {
            found.push((literal.to_owned(), start, base + "import.meta.url".len()));
        }
    }
    found.sort_by_key(|(_, start, _)| *start);
    found
}

/// Adds the specifiers of `new URL(…, import.meta.url)` and mock calls that the parser does not report, as imports
/// that only load the module.
pub(crate) fn add_textual_imports(info: &mut ModuleInfo, source: &str) {
    if !["import.meta.url", "mock", "Mock", "Actual"]
        .iter()
        .any(|needle| source.contains(needle))
    {
        return;
    }
    for (specifier, start, end) in extra_specifiers(source) {
        if info.imports.iter().any(|import| import.specifier == specifier) {
            continue;
        }
        info.imports.push(Import {
            specifier,
            kind: ImportKind::SideEffect,
            names: Vec::new(),
            span: span(source, start, end),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_and_mock_specifiers_are_found() {
        let source = "const w = new URL('./worker.ts', import.meta.url);\nvi.mock(\"./dep\", () => ({}));\njest.mock('axios');\nnew URL('https://x.dev', import.meta.url);\nnew URL(`./${name}.ts`, import.meta.url);";
        let found: Vec<String> = extra_specifiers(source).into_iter().map(|(s, _, _)| s).collect();
        assert_eq!(found, ["./worker.ts", "./dep", "axios"]);
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
