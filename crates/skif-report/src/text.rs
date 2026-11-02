//! Text helpers shared by the renderers: source lines, column conversion, control character escaping and display
//! width.

use std::borrow::Cow;
use std::fmt::Write as _;

use unicode_width::UnicodeWidthChar;

/// The 1-based `line` of `source` without its line terminator.
pub(crate) fn line_of(source: &str, line: u32) -> Option<&str> {
    let index = usize::try_from(line.checked_sub(1)?).ok()?;
    source
        .split('\n')
        .nth(index)
        .map(|text| text.strip_suffix('\r').unwrap_or(text))
}

/// Converts a 1-based column counted in Unicode scalar values to one counted in UTF-16 code units.
pub(crate) fn utf16_column(line: &str, column: u32) -> u32 {
    let skip = usize::try_from(column.saturating_sub(1)).unwrap_or(usize::MAX);
    let mut units = 0usize;
    let mut seen = 0usize;
    for c in line.chars().take(skip) {
        units += c.len_utf16();
        seen += 1;
    }
    units += skip - seen;
    u32::try_from(units).unwrap_or(u32::MAX).saturating_add(1)
}

/// Characters that must not reach a terminal as they are: controls (including ESC) and bidirectional overrides.
fn is_unsafe(c: char) -> bool {
    c.is_control() || matches!(c, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

fn escaped(c: char) -> String {
    let mut out = String::new();
    let _ = write!(out, "\\u{{{:x}}}", u32::from(c));
    out
}

/// Replaces control characters with a visible `\u{..}` escape, so report text cannot drive the terminal.
pub(crate) fn escape_controls(text: &str) -> Cow<'_, str> {
    if !text.chars().any(is_unsafe) {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len() + 8);
    for c in text.chars() {
        if is_unsafe(c) {
            out.push_str(&escaped(c));
        } else {
            out.push(c);
        }
    }
    Cow::Owned(out)
}

/// One source line made printable, with the display columns of its underline.
pub(crate) struct Frame {
    /// The line with tabs as spaces and control characters escaped.
    pub text: String,
    /// Display columns before the underline.
    pub pad: usize,
    /// Display columns of the underline, at least one.
    pub carets: usize,
}

/// Lays out `line` with the span from 1-based scalar column `start` up to (excluding) `end`, or to the end of the
/// line when `end` is `None`, measured in terminal cells.
pub(crate) fn frame(line: &str, start: u32, end: Option<u32>) -> Frame {
    let mut text = String::with_capacity(line.len());
    let mut pad = 0usize;
    let mut carets = 0usize;
    let mut count = 0usize;
    for (index, c) in line.chars().enumerate() {
        count += 1;
        let column = u32::try_from(index).unwrap_or(u32::MAX).saturating_add(1);
        let width = if c == '\t' {
            text.push(' ');
            1
        } else if is_unsafe(c) {
            let piece = escaped(c);
            text.push_str(&piece);
            piece.len()
        } else {
            text.push(c);
            c.width().unwrap_or(0)
        };
        if column < start {
            pad += width;
        } else if end.is_none_or(|end| column < end) {
            carets += width;
        }
    }
    let skipped = usize::try_from(start.saturating_sub(1)).unwrap_or(usize::MAX);
    pad += skipped.saturating_sub(count);
    Frame {
        text,
        pad,
        carets: carets.max(1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_columns_count_astral_characters_twice() {
        assert_eq!(utf16_column("a\u{1F600}b", 1), 1);
        assert_eq!(utf16_column("a\u{1F600}b", 2), 2);
        assert_eq!(utf16_column("a\u{1F600}b", 3), 4);
        assert_eq!(utf16_column("ab", 5), 5);
    }

    #[test]
    fn control_characters_are_escaped() {
        assert_eq!(escape_controls("plain"), "plain");
        assert_eq!(escape_controls("a\u{1b}[2Jb"), "a\\u{1b}[2Jb");
        assert_eq!(escape_controls("x\u{202e}y"), "x\\u{202e}y");
        assert_eq!(escape_controls("a\nb"), "a\\u{a}b");
    }

    #[test]
    fn frame_measures_wide_characters_in_cells() {
        let framed = frame("\u{4e2d}\u{6587}ab", 3, Some(5));
        assert_eq!((framed.pad, framed.carets), (4, 2));
        let framed = frame("a\tb", 2, None);
        assert_eq!(framed.text, "a b");
        assert_eq!((framed.pad, framed.carets), (1, 2));
        let framed = frame("ab", 5, Some(5));
        assert_eq!((framed.pad, framed.carets), (4, 1));
    }

    #[test]
    fn line_of_handles_crlf_and_bounds() {
        let text = "a\r\nb\n";
        assert_eq!(line_of(text, 1), Some("a"));
        assert_eq!(line_of(text, 2), Some("b"));
        assert_eq!(line_of(text, 4), None);
        assert_eq!(line_of(text, 0), None);
    }
}
