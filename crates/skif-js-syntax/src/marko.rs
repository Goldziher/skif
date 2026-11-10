//! The style blocks of a `.marko` file (ADR 0082).
//!
//! Marko templates are no JavaScript a parser accepts, and skif runs no Marko compiler (ADR 0001). As knip's Marko
//! compiler does, a `.marko` file is read for its imports (see [`crate::tsrx`]) and its styles: the `<style>` blocks of
//! the HTML syntax and the `style { … }` blocks of the concise syntax, whose language is the last part of the name
//! (`style.less { … }`). A concise block without one is Sass, as in knip.

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

/// The `style { … }` blocks: a line that starts with `style`, an optional `.language` and a `{`, up to the next line
/// that starts with `}`.
fn concise_styles(source: &str) -> Vec<StyleBlock> {
    let mut found = Vec::new();
    let mut open: Option<(usize, &'static str)> = None;
    let mut offset = 0;
    for line in source.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        match open {
            None => {
                if let Some((body, language)) = concise_style_start(trimmed) {
                    open = Some((offset + indent + body, language));
                }
            }
            Some((start, language)) => {
                if trimmed.starts_with('}') {
                    found.push(StyleBlock {
                        start,
                        end: offset + indent,
                        extension: language,
                    });
                    open = None;
                }
            }
        }
        offset += line.len();
    }
    found
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
}
