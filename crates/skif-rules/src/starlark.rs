//! A lexer for the Starlark of Bazel files, enough to find names, string literals and the brackets around them
//! without evaluating anything. `#` comments and numbers yield no token; a string's prefix (`r"..."`) is dropped.

use std::ops::Range;

/// What a token is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// A name: an identifier or a keyword (`load`, `def`, `glob`).
    Name,
    /// A string literal; its span covers the text between the quotes, escapes kept as written.
    Str,
    /// Any other character that is not whitespace (`(`, `=`, `.`).
    Punct(u8),
}

/// A token of a Starlark file, with its byte range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Token {
    pub kind: Kind,
    pub span: Range<usize>,
}

impl Token {
    /// The text of the token in `source`: the name, the string's contents or the character.
    pub fn text<'s>(&self, source: &'s str) -> &'s str {
        source.get(self.span.clone()).unwrap_or_default()
    }

    /// Whether the token is the name `name`.
    pub fn is_name(&self, source: &str, name: &str) -> bool {
        self.kind == Kind::Name && self.text(source) == name
    }

    /// Whether the token is the character `punct`.
    pub fn is(&self, punct: u8) -> bool {
        self.kind == Kind::Punct(punct)
    }
}

/// The prefixes a Starlark string literal may carry.
const STRING_PREFIXES: [&str; 4] = ["r", "b", "rb", "br"];

/// The end of the identifier that starts at byte `at` of `source`.
fn identifier_end(source: &str, at: usize) -> usize {
    source[at..]
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .map_or(source.len(), |end| at + end)
}

/// The tokens of `source`, in order. An unterminated string ends at the end of its line, or of the file for a
/// triple-quoted one.
pub(crate) fn tokens(source: &str) -> Vec<Token> {
    let bytes = source.as_bytes();
    let mut found = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'#' => at = source[at..].find('\n').map_or(bytes.len(), |end| at + end),
            quote @ (b'"' | b'\'') => {
                let triple = if quote == b'"' { "\"\"\"" } else { "'''" };
                if source[at..].starts_with(triple) {
                    let start = at + 3;
                    let end = source[start..].find(triple).map_or(bytes.len(), |end| start + end);
                    found.push(Token {
                        kind: Kind::Str,
                        span: start..end,
                    });
                    at = (end + 3).min(bytes.len());
                    continue;
                }
                let start = at + 1;
                let mut end = start;
                while end < bytes.len() && bytes[end] != quote && bytes[end] != b'\n' {
                    end += if bytes[end] == b'\\' { 2 } else { 1 };
                }
                let end = end.min(bytes.len());
                found.push(Token {
                    kind: Kind::Str,
                    span: start..end,
                });
                at = end + 1;
            }
            byte if byte.is_ascii_alphabetic() || byte == b'_' => {
                let end = identifier_end(source, at);
                let prefix = bytes.get(end).is_some_and(|next| matches!(next, b'"' | b'\''))
                    && STRING_PREFIXES.contains(&&source[at..end]);
                if !prefix {
                    found.push(Token {
                        kind: Kind::Name,
                        span: at..end,
                    });
                }
                at = end;
            }
            byte if byte.is_ascii_digit() => at = identifier_end(source, at),
            byte if byte.is_ascii_whitespace() || !byte.is_ascii() => at += 1,
            byte => {
                found.push(Token {
                    kind: Kind::Punct(byte),
                    span: at..at + 1,
                });
                at += 1;
            }
        }
    }
    found
}

/// The index of the token that closes the bracket opened at `open`, or the last index when it is never closed.
pub(crate) fn closing(tokens: &[Token], open: usize) -> usize {
    let mut depth = 0_usize;
    for (index, token) in tokens.iter().enumerate().skip(open) {
        match token.kind {
            Kind::Punct(b'(' | b'[' | b'{') => depth += 1,
            Kind::Punct(b')' | b']' | b'}') => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return index;
                }
            }
            _ => {}
        }
    }
    tokens.len().saturating_sub(1)
}

/// The dotted name a call whose `(` is at `open` calls: `pnpm.pnpm` for `pnpm.pnpm(...)`; `None` when no name comes
/// before the bracket.
pub(crate) fn callee(source: &str, tokens: &[Token], open: usize) -> Option<String> {
    let mut parts = Vec::new();
    let mut index = open;
    while index > 0 && tokens[index - 1].kind == Kind::Name {
        parts.push(tokens[index - 1].text(source));
        index -= 1;
        if index > 0 && tokens[index - 1].is(b'.') {
            index -= 1;
        } else {
            break;
        }
    }
    if parts.is_empty() {
        return None;
    }
    parts.reverse();
    Some(parts.join("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(source: &str) -> Vec<(Kind, &str)> {
        tokens(source)
            .iter()
            .map(|token| (token.kind, token.text(source)))
            .collect()
    }

    #[test]
    fn names_strings_and_punctuation_are_tokens_and_comments_and_numbers_are_not() {
        let source = "# \"commented\"\nx = [r\"a\", 'b', \"\"\"c\nd\"\"\", 12, \"e\\\"f\"]  # 'tail'\n";
        assert_eq!(
            texts(source),
            [
                (Kind::Name, "x"),
                (Kind::Punct(b'='), "="),
                (Kind::Punct(b'['), "["),
                (Kind::Str, "a"),
                (Kind::Punct(b','), ","),
                (Kind::Str, "b"),
                (Kind::Punct(b','), ","),
                (Kind::Str, "c\nd"),
                (Kind::Punct(b','), ","),
                (Kind::Punct(b','), ","),
                (Kind::Str, "e\\\"f"),
                (Kind::Punct(b']'), "]"),
            ]
        );
    }

    #[test]
    fn unterminated_strings_end_at_their_line_or_the_file() {
        assert_eq!(
            texts("a = \"open\nb = '''never"),
            [
                (Kind::Name, "a"),
                (Kind::Punct(b'='), "="),
                (Kind::Str, "open"),
                (Kind::Name, "b"),
                (Kind::Punct(b'='), "="),
                (Kind::Str, "never"),
            ]
        );
    }

    #[test]
    fn calls_are_named_by_their_dotted_callee_and_closed_by_their_bracket() {
        let source = "pnpm.pnpm(name = f(x), v = [1])\nprint(y)";
        let found = tokens(source);
        let open = found.iter().position(|token| token.is(b'(')).unwrap();
        assert_eq!(callee(source, &found, open).as_deref(), Some("pnpm.pnpm"));
        let close = closing(&found, open);
        assert_eq!(found[close + 1].text(source), "print");
        let list = found.iter().position(|token| token.is(b'[')).unwrap();
        assert_eq!(callee(source, &found, list), None);
    }
}
