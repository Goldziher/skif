//! Static import extraction for `.tsrx` files.
//!
//! TSRX (`@tsrx/core`) is TypeScript with template syntax of its own (`component App() { <div/> }`), which no
//! JavaScript parser accepts and which skif does not compile (ADR 0001). As knip's built-in `.tsrx` compiler does,
//! only the `import` statements and `import("x")` calls are kept: every other byte becomes a space, line breaks stay,
//! and the result is parsed as TypeScript with JSX so spans point into the original file.
//!
//! A light lexer finds them: `import` inside a comment, a string or a template literal is not an import. A quote that
//! does not close on its line is not a string but template text (`<p>don't</p>`). Imports inside the `${}` of a
//! template literal are not kept.

const fn is_ident(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$' | b'#' | b'.')
}

/// The end of the comment that starts at `i` (past the line of a `//` comment, before its line break, or past the
/// `*/` of a block comment), or `None` when no comment starts there. A `/*` with no `*/` after it is not a comment,
/// which TypeScript would reject, but a regular expression (`/\/*/`) or template text the lexer does not tell apart:
/// it runs to the end of its line, not of the file, so the imports after it are kept.
fn comment_end(bytes: &[u8], i: usize) -> Option<usize> {
    if bytes.get(i) != Some(&b'/') {
        return None;
    }
    match bytes.get(i + 1)? {
        b'/' => Some(line_end(bytes, i)),
        b'*' => Some(
            bytes[i + 2..]
                .windows(2)
                .position(|pair| pair == b"*/")
                .map_or_else(|| line_end(bytes, i), |at| i + 2 + at + 2),
        ),
        _ => None,
    }
}

/// The offset of the line break that ends the line `i` is on, or the end of `bytes`.
fn line_end(bytes: &[u8], i: usize) -> usize {
    bytes[i..]
        .iter()
        .position(|&b| b == b'\n')
        .map_or(bytes.len(), |at| i + at)
}

/// The end of the `'` or `"` string that starts at `i`, past its closing quote, or `None` when the quote does not
/// close on its line: then it is template text such as `<p>don't</p>`, not a string.
fn string_end(bytes: &[u8], i: usize) -> Option<usize> {
    let quote = bytes[i];
    let mut j = i + 1;
    while let Some(&b) = bytes.get(j) {
        match b {
            b'\\' => j += 2,
            b'\n' => return None,
            _ if b == quote => return Some(j + 1),
            _ => j += 1,
        }
    }
    None
}

/// The end of the template literal that starts at `i` (its opening backtick), past the closing backtick. `${}`
/// expressions are skipped with their own strings, comments and nested templates. An unclosed template runs to the
/// end.
fn template_end(bytes: &[u8], i: usize) -> usize {
    let mut j = i + 1;
    while let Some(&b) = bytes.get(j) {
        match b {
            b'\\' => j += 2,
            b'`' => return j + 1,
            b'$' if bytes.get(j + 1) == Some(&b'{') => {
                j += 2;
                let mut depth = 1_usize;
                while depth > 0 && j < bytes.len() {
                    if let Some(end) = skipped_end(bytes, j) {
                        j = end;
                        continue;
                    }
                    match bytes[j] {
                        b'{' => depth += 1,
                        b'}' => depth -= 1,
                        _ => {}
                    }
                    j += 1;
                }
            }
            _ => j += 1,
        }
    }
    bytes.len()
}

/// The end of the comment, string or template literal that starts at `i`, or `None` when none starts there.
fn skipped_end(bytes: &[u8], i: usize) -> Option<usize> {
    match bytes[i] {
        b'/' => comment_end(bytes, i),
        b'\'' | b'"' => string_end(bytes, i),
        b'`' => Some(template_end(bytes, i)),
        _ => None,
    }
}

/// The end of the import that starts at `start` (the `i` of `import`): just past the closing quote of its specifier,
/// or past the `)` of `import("x")`. `None` when no import starts there.
fn import_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut i = start + "import".len();
    if bytes.get(i).is_some_and(|&b| is_ident(b) && b != b'.') {
        return None;
    }
    while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
        i += 1;
    }
    let dynamic = match bytes.get(i)? {
        b'.' => return None,
        b'(' => true,
        _ => false,
    };
    if dynamic {
        i += 1;
        while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        if !matches!(bytes.get(i)?, b'"' | b'\'') {
            return None;
        }
    } else {
        // The bindings and `from` run up to the quote of the specifier; a statement or block ends before one. Comments
        // between the bindings are skipped, so a quote inside one is not taken for the specifier.
        while !matches!(bytes.get(i)?, b'"' | b'\'') {
            if let Some(end) = comment_end(bytes, i) {
                i = end;
                continue;
            }
            if matches!(bytes[i], b';' | b'(' | b')' | b'=' | b'<') {
                return None;
            }
            i += 1;
        }
    }
    let quote = bytes[i];
    i += 1;
    while *bytes.get(i)? != quote {
        if matches!(bytes[i], b'\n' | b'\\') {
            return None;
        }
        i += 1;
    }
    i += 1;
    if dynamic {
        while bytes.get(i).is_some_and(|&b| b != b')' && b != b'\n') {
            i += 1;
        }
        if bytes.get(i) != Some(&b')') {
            return None;
        }
        i += 1;
    }
    Some(i)
}

/// The source with everything but its imports blanked. Each kept import is followed by a `;` in the blanked space
/// after it, so two imports on one line stay separate statements.
pub(crate) fn extract(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut out: Vec<u8> = bytes
        .iter()
        .map(|&b| if b == b'\n' || b == b'\r' { b } else { b' ' })
        .collect();
    let mut i = 0;
    while i < bytes.len() {
        if let Some(end) = skipped_end(bytes, i) {
            i = end;
            continue;
        }
        let at_import = bytes[i..].starts_with(b"import") && (i == 0 || !is_ident(bytes[i - 1]));
        match at_import.then(|| import_end(bytes, i)).flatten() {
            Some(end) => {
                out[i..end].copy_from_slice(&bytes[i..end]);
                if out.get(end) == Some(&b' ') {
                    out[end] = b';';
                }
                i = end;
            }
            None => i += 1,
        }
    }
    // Only whole ASCII-delimited ranges are copied, the rest is ASCII, so the buffer is valid UTF-8.
    String::from_utf8(out).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kept(source: &str) -> Vec<String> {
        extract(source)
            .split(['\n', ';'])
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn imports_are_kept_and_templates_dropped() {
        let source = "import { track } from '@tsrx/core';\nimport type { Props } from \"./types\";\nimport './global.css'\n\ncomponent App(props: Props) {\n  const lazy = import('./lazy');\n  <div>{import.meta.env.MODE}</div>\n}\nexport default App;\n";
        assert_eq!(
            kept(source),
            [
                "import { track } from '@tsrx/core'",
                "import type { Props } from \"./types\"",
                "import './global.css'",
                "import('./lazy')"
            ]
        );
        assert_eq!(extract(source).len(), source.len());
    }

    #[test]
    fn imports_in_comments_strings_and_templates_are_not_imports() {
        let source = "// import a from 'commented'\n/* import b from 'block' */\nconst s = \"import c from 'string'\";\nconst t = `\nimport d from 'template'\n${'x'}`;\nimport e from './e';\n";
        assert_eq!(kept(source), ["import e from './e'"]);
        assert_eq!(extract(source).len(), source.len());
    }

    #[test]
    fn a_comment_with_a_quote_inside_an_import_list_keeps_the_import() {
        let source = "import {\n  a, // don't\n  /* b's */ b,\n} from './ab';\n";
        assert_eq!(
            kept(source),
            ["import {", "a, // don't", "/* b's */ b,", "} from './ab'"]
        );
    }

    #[test]
    fn an_apostrophe_in_template_text_does_not_hide_the_next_import() {
        let source = "component App() {\n  <p>don't</p>\n  const lazy = import('./lazy');\n}\n";
        assert_eq!(kept(source), ["import('./lazy')"]);
    }

    #[test]
    fn an_unclosed_comment_opener_ends_at_its_line() {
        let source = "const re = /\\/*/;\nimport a from './a';\n";
        assert_eq!(kept(source), ["import a from './a'"]);
    }

    #[test]
    fn crlf_line_breaks_are_kept() {
        let source = "import a from './a';\r\n// x\r\nconst re = /\\/*/;\r\nimport b from './b';\r\n";
        let extracted = extract(source);
        assert_eq!(extracted.len(), source.len());
        assert_eq!(extracted.matches("\r\n").count(), 4);
        assert_eq!(kept(source), ["import a from './a'", "import b from './b'"]);
    }

    #[test]
    fn identifiers_containing_import_are_not_imports() {
        assert_eq!(
            kept("const reimport = 1; obj.import('x'); important('y');"),
            Vec::<String>::new()
        );
    }
}
