//! Static import extraction for `.tsrx` files.
//!
//! TSRX (`@tsrx/core`) is TypeScript with template syntax of its own (`component App() { <div/> }`), which no
//! JavaScript parser accepts and which skif does not compile (ADR 0001). As knip's built-in `.tsrx` compiler does,
//! only the `import` statements and `import("x")` calls are kept: every other byte becomes a space, line breaks stay,
//! and the result is parsed as TypeScript with JSX so spans point into the original file.

const fn is_ident(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$' | b'#' | b'.')
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
        // The bindings and `from` run up to the quote of the specifier; a statement or block ends before one.
        while !matches!(bytes.get(i)?, b'"' | b'\'') {
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
    while let Some(found) = source[i..].find("import") {
        let start = i + found;
        let preceded = start > 0 && is_ident(bytes[start - 1]);
        match (!preceded).then(|| import_end(bytes, start)).flatten() {
            Some(end) => {
                out[start..end].copy_from_slice(&bytes[start..end]);
                if out.get(end) == Some(&b' ') {
                    out[end] = b';';
                }
                i = end;
            }
            None => i = start + "import".len(),
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
    fn identifiers_containing_import_are_not_imports() {
        assert_eq!(
            kept("const reimport = 1; obj.import('x'); important('y');"),
            Vec::<String>::new()
        );
    }
}
