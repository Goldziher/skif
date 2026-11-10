//! The style blocks of a `.marko` file (ADR 0082).
//!
//! Marko templates are no JavaScript a parser accepts, and skif runs no Marko compiler (ADR 0001). As knip's Marko
//! compiler does, a `.marko` file is read for its imports (see [`crate::tsrx`]) and its styles: the `<style>` blocks of
//! the HTML syntax and the `style { … }` blocks of the concise syntax, which end at the `}` matching their `{` and whose
//! language is the last part of the name (`style.less { … }`). A concise block without one is Sass, as in knip.

use crate::sfc::{self, StyleBlock};

/// Every style block of `source`, in source order.
pub(crate) fn styles(source: &str) -> Vec<StyleBlock> {
    let mut found = sfc::blocks(source, false).styles;
    found.extend(concise_styles(source));
    found.sort_by_key(|block| block.start);
    found
}

/// The extension of the stylesheet language a concise block names.
fn extension(language: &str) -> &'static str {
    match language {
        "css" => "css",
        "less" => "less",
        "sass" => "sass",
        "styl" | "stylus" => "styl",
        _ => "scss",
    }
}

/// The `style { … }` blocks: a line that starts with `style`, an optional `.language` and a `{`, up to the `}` that
/// closes that `{`, counting the braces of the rules inside (`style { .a { color: red } }` on one line).
fn concise_styles(source: &str) -> Vec<StyleBlock> {
    let mut found = Vec::new();
    let mut offset = 0;
    // The end of the last block found: lines inside it open nothing.
    let mut after = 0;
    for line in source.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        if start < after {
            continue;
        }
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        if let Some((body, language)) = concise_style_start(trimmed) {
            let body = start + indent + body;
            let end = closing_brace(source.as_bytes(), body);
            found.push(StyleBlock {
                start: body,
                end,
                extension: language,
            });
            after = end;
        }
    }
    found
}

/// Index of the `}` that closes the block whose body starts at `body`, just after its `{`, or the end of `bytes`.
/// Braces in comments and strings do not count.
fn closing_brace(bytes: &[u8], body: usize) -> usize {
    let mut depth = 1_u32;
    let mut i = body;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i = bytes[i + 2..]
                    .windows(2)
                    .position(|pair| pair == b"*/")
                    .map_or(bytes.len(), |at| i + 2 + at + 2);
                continue;
            }
            quote @ (b'"' | b'\'') => {
                i = bytes[i + 1..]
                    .iter()
                    .position(|byte| *byte == quote || *byte == b'\n')
                    .map_or(bytes.len(), |at| i + 1 + at + 1);
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    bytes.len()
}

/// For a line (without its indentation) that opens a concise style block, the offset of its body in the line and the
/// stylesheet language.
fn concise_style_start(line: &str) -> Option<(usize, &'static str)> {
    let rest = line.strip_prefix("style")?;
    let (language, after) = match rest.strip_prefix('.') {
        Some(named) => {
            let end = named
                .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')))
                .unwrap_or(named.len());
            let last = named[..end].rsplit('.').next().unwrap_or_default();
            (extension(last), &named[end..])
        }
        None if rest.starts_with(|c: char| c.is_ascii_whitespace() || c == '{') => ("scss", rest),
        None => return None,
    };
    let brace = after.find(['{', '\n'])?;
    (after.as_bytes()[brace] == b'{').then_some((line.len() - after.len() + brace + 1, language))
}

#[cfg(test)]
mod tests {
    use super::styles;

    #[test]
    fn html_and_concise_style_blocks() {
        let source = "<div/>\nstyle.less {\n  @import './theme.less';\n}\nstyle {\n  @use 'tokens';\n}\nstylesheet-note { not a style }\n<style>\n@import './plain.css';\n</style>\n";
        let found: Vec<(&str, &str)> = styles(source)
            .iter()
            .map(|block| (&source[block.start..block.end], block.extension))
            .collect();
        assert_eq!(
            found,
            [
                ("\n  @import './theme.less';\n", "less"),
                ("\n  @use 'tokens';\n", "scss"),
                ("\n@import './plain.css';\n", "css"),
            ]
        );
    }

    #[test]
    fn a_concise_block_ends_at_its_own_closing_brace() {
        let source = "style { .a { color: red } }\n<div/>\nstyle.less {\n  .b { /* } */ content: '}'; }\n  @import './theme.less';\n}\n<p>{ text }</p>\n";
        let found: Vec<(&str, &str)> = styles(source)
            .iter()
            .map(|block| (&source[block.start..block.end], block.extension))
            .collect();
        assert_eq!(
            found,
            [
                (" .a { color: red } ", "scss"),
                ("\n  .b { /* } */ content: '}'; }\n  @import './theme.less';\n", "less"),
            ]
        );
    }
}
