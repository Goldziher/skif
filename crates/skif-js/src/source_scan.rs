//! Cheap text scans for what the parser does not see: identifiers used only in component templates, and module
//! specifiers written as `new URL("./x", import.meta.url)` or `vi.mock("./x")`.

use std::collections::HashSet;
use std::path::Path;

use skif_core::Span;
use skif_js_syntax::{Import, ImportKind, ModuleInfo};

/// Whether the file is a component with a template next to its script (`.vue`, `.svelte`, `.astro`).
pub(crate) fn is_template_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            ["vue", "svelte", "astro"]
                .iter()
                .any(|template| extension.eq_ignore_ascii_case(template))
        })
}

/// `source` without the `<tag …>…</tag>` blocks.
fn strip_blocks(source: &str, tag: &str) -> String {
    let lower = source.to_ascii_lowercase();
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut out = String::with_capacity(source.len());
    let mut position = 0;
    while let Some(found) = lower[position..].find(&open) {
        let start = position + found;
        let after = start + open.len();
        let is_tag = lower
            .as_bytes()
            .get(after)
            .is_some_and(|byte| matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | b'>' | b'/'));
        if !is_tag {
            out.push_str(&source[position..after]);
            position = after;
            continue;
        }
        out.push_str(&source[position..start]);
        match lower[start..].find(&close) {
            Some(end) => position = start + end + close.len(),
            None => {
                position = source.len();
                break;
            }
        }
    }
    out.push_str(&source[position..]);
    out
}

/// The template part of a component: no `<script>` and `<style>` blocks and no Astro frontmatter.
fn markup(source: &str) -> String {
    let mut text = strip_blocks(&strip_blocks(source, "script"), "style");
    let trimmed = text.trim_start();
    if trimmed.starts_with("---")
        && let Some(end) = trimmed[3..].find("\n---")
    {
        text = trimmed[3 + end + 4..].to_owned();
    }
    text
}

/// Every identifier-like word of the template of a component.
///
/// This is deliberately coarse: a word in the text of the page counts like an expression. Over-approximating keeps
/// bindings that only a template reads (enums, namespaces, components) from being reported as unused.
pub(crate) fn template_identifiers(source: &str) -> HashSet<String> {
    let text = markup(source);
    let mut identifiers = HashSet::new();
    let mut start: Option<usize> = None;
    for (index, character) in text.char_indices() {
        let part = character.is_alphanumeric() || character == '_' || character == '$';
        match (part, start) {
            (true, None) => start = Some(index),
            (false, Some(from)) => {
                identifiers.insert(text[from..index].to_owned());
                start = None;
            }
            _ => {}
        }
    }
    if let Some(from) = start {
        identifiers.insert(text[from..].to_owned());
    }
    identifiers
}

/// Marks every import whose local name appears in the template as used in full.
pub(crate) fn mark_template_usage(info: &mut ModuleInfo, source: &str) {
    let identifiers = template_identifiers(source);
    for import in &mut info.imports {
        for name in &mut import.names {
            if name.local.as_deref().is_some_and(|local| identifiers.contains(local)) {
                name.escapes = true;
            }
        }
    }
}

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
        (u32::try_from(line).unwrap_or(u32::MAX), u32::try_from(column).unwrap_or(u32::MAX))
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
    fn template_words_exclude_script_style_and_frontmatter() {
        let vue = "<script setup>import { Color } from './c'; const hidden = 1</script>\n<template><p :class=\"Color.Red\">{{ shown }}</p></template>\n<style>.x { color: red }</style>";
        let words = template_identifiers(vue);
        assert!(words.contains("Color") && words.contains("Red") && words.contains("shown"));
        assert!(!words.contains("hidden") && !words.contains("setup"));

        let astro = "---\nimport Layout from './l.astro';\nconst title = 1;\n---\n<Layout title={title}><Widget /></Layout>";
        let words = template_identifiers(astro);
        assert!(words.contains("Layout") && words.contains("Widget") && words.contains("title"));
        assert!(!words.contains("import"));
    }

    #[test]
    fn url_and_mock_specifiers_are_found() {
        let source = "const w = new URL('./worker.ts', import.meta.url);\nvi.mock(\"./dep\", () => ({}));\njest.mock('axios');\nnew URL('https://x.dev', import.meta.url);\nnew URL(`./${name}.ts`, import.meta.url);";
        let found: Vec<String> = extra_specifiers(source).into_iter().map(|(s, _, _)| s).collect();
        assert_eq!(found, ["./worker.ts", "./dep", "axios"]);
    }

    #[test]
    fn spans_count_lines_and_columns() {
        let span = span("a\nbc\ndef", 5, 8);
        assert_eq!((span.start_line, span.start_col, span.end_line, span.end_col), (3, 1, 3, 4));
    }
}
