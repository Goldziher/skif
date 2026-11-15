//! A cheap text scan for module specifiers the parser does not see: `new URL("./x", import.meta.url)` and
//! `new Worker(path.join(__dirname, "x.js"))`. Such a target is loaded as a program of its own, so every export of it
//! counts as used. Test-runner mocks and identifiers used only in
//! component templates are read by `skif-js-syntax`.

use skif_core::Span;
use skif_js_syntax::{Import, ImportKind, ModuleInfo};

use crate::resolve::CODE_EXTENSIONS;

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

/// The relative specifier a `new URL` literal names: a path starting with `./` or `../`, or the bare name of a module
/// file beside the importer (`check.mjs`), which URL resolution reads as `./check.mjs`. A bare name with a directory
/// (`worker-package/worker.js`) is left out, as bundlers may read it as a package; so are a name without a code
/// extension and anything with a scheme, query or fragment.
fn url_specifier(literal: &str) -> Option<String> {
    if literal.starts_with("./") || literal.starts_with("../") {
        return Some(literal.to_owned());
    }
    let sibling = !literal.starts_with('.')
        && !literal.contains(['/', '\\', ':', '?', '#'])
        && literal
            .rsplit_once('.')
            .is_some_and(|(_, extension)| CODE_EXTENSIONS.contains(&extension));
    sibling.then(|| format!("./{literal}"))
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
        if !source[base..].starts_with("import.meta.url") || in_node_modules(literal) {
            continue;
        }
        if let Some(specifier) = url_specifier(literal) {
            found.push((specifier, ImportKind::Url, start, base + "import.meta.url".len()));
        }
    }
    found.sort_by_key(|(_, _, start, _)| *start);
    found
}

/// The calls of Node's `path` that build a path from the file's directory.
const DIRNAME_JOINS: [&str; 4] = ["path.join(", "path.resolve(", "join(", "resolve("];

/// The file a path built from the file's directory names (`path.join(__dirname, "..", "worker.js")` or
/// `path.resolve` with string literals after `__dirname`), as a relative specifier, with the offset past the call.
/// `None` for any other argument, a computed segment, and a segment that starts at the file system root.
fn dirname_path(source: &str, position: usize) -> Option<(String, usize)> {
    let start = skip_whitespace(source, position);
    let call = DIRNAME_JOINS.iter().find(|call| source[start..].starts_with(**call))?;
    let mut at = skip_whitespace(source, start + call.len());
    at += source[at..].strip_prefix("__dirname").map(|_| "__dirname".len())?;
    let mut segments = Vec::new();
    loop {
        at = skip_whitespace(source, at);
        if source[at..].starts_with(')') {
            break;
        }
        if !source[at..].starts_with(',') {
            return None;
        }
        let (segment, after) = string_literal(source, at + 1)?;
        segments.push(segment);
        at = after;
    }
    if segments.is_empty() || segments.iter().any(|segment| segment.starts_with('/')) {
        return None;
    }
    let path = segments.join("/");
    let specifier = if path.starts_with("./") || path.starts_with("../") {
        path
    } else {
        format!("./{path}")
    };
    Some((specifier, at + 1))
}

/// `(specifier, kind, start, end)` of the worker threads started from a path built from the file's directory:
/// `new Worker(path.join(__dirname, "worker.js"))`. A plain string is left out, since Node resolves it against the
/// working directory.
fn worker_specifiers(source: &str) -> Vec<(String, ImportKind, usize, usize)> {
    source
        .match_indices("new Worker(")
        .filter_map(|(start, call)| {
            let (specifier, end) = dirname_path(source, start + call.len())?;
            (!in_node_modules(&specifier)).then_some((specifier, ImportKind::Url, start, end))
        })
        .collect()
}

/// Adds the module specifiers the parser does not report, each an [`ImportKind::Url`] of the whole module: those of
/// `new URL(…, import.meta.url)` and of a worker started from a path built from `__dirname`.
pub(crate) fn add_textual_imports(info: &mut ModuleInfo, source: &str) {
    let mut found = Vec::new();
    if source.contains("import.meta.url") {
        found.extend(extra_specifiers(source));
    }
    if source.contains("__dirname") {
        found.extend(worker_specifiers(source));
    }
    found.sort_by_key(|(_, _, start, _)| *start);
    for (specifier, kind, start, end) in found {
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
    fn a_bare_module_name_is_a_sibling_file() {
        let source = "new URL('check.mjs', import.meta.url);\nnew URL('generated', import.meta.url);\n\
                      new URL('worker-package/worker.js', import.meta.url);\nnew URL('file.png', import.meta.url);\n\
                      new URL('data:text/javascript,1.js', import.meta.url);\nnew URL('a.js?worker', import.meta.url);";
        let found: Vec<String> = extra_specifiers(source).into_iter().map(|(s, _, _, _)| s).collect();
        assert_eq!(found, ["./check.mjs"]);
    }

    #[test]
    fn a_worker_path_built_from_dirname_is_a_whole_module_import() {
        let source = "const a = new Worker(path.join(__dirname, \"../workers/slots.worker.js\"));\n\
                      const b = new Worker( resolve(__dirname, 'jobs', 'run.js') );\n\
                      new Worker(path.join(process.cwd(), 'x.js'));\nnew Worker(path.join(__dirname, name));\n\
                      new Worker(path.join(__dirname, '/abs.js'));\nnew Worker('./relative-to-cwd.js');";
        let mut info = ModuleInfo::default();
        add_textual_imports(&mut info, source);
        let found: Vec<(&str, ImportKind)> = info
            .imports
            .iter()
            .map(|import| (import.specifier.as_str(), import.kind))
            .collect();
        assert_eq!(
            found,
            [
                ("../workers/slots.worker.js", ImportKind::Url),
                ("./jobs/run.js", ImportKind::Url)
            ]
        );
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
