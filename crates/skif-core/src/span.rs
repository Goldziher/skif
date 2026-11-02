//! Source positions: byte spans, locations and offset-to-line/column conversion.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Half-open UTF-8 byte range `[start, end)` with 1-based line and column numbers.
///
/// Columns count Unicode scalar values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Span {
    /// Byte offset of the first byte.
    pub start: u32,
    /// Byte offset one past the last byte.
    pub end: u32,
    /// 1-based line of `start`.
    pub start_line: u32,
    /// 1-based column of `start`.
    pub start_col: u32,
    /// 1-based line of `end`.
    pub end_line: u32,
    /// 1-based column of `end`.
    pub end_col: u32,
}

/// A file, relative to the workspace root, and an optional position within it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Location {
    /// Workspace-relative path.
    #[serde(serialize_with = "crate::display::serialize_path")]
    pub path: PathBuf,
    /// Position within the file, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<Span>,
}

impl Location {
    /// The file at `path`, optionally at `span`.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>, span: Option<Span>) -> Self {
        Self {
            path: path.into(),
            span,
        }
    }

    /// The file at `path` as a whole.
    #[must_use]
    pub fn file(path: impl Into<PathBuf>) -> Self {
        Self::new(path, None)
    }
}

/// A secondary location that explains a diagnostic.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Related {
    /// Why this location is relevant.
    pub message: String,
    /// Where it is.
    pub location: Location,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WideChar {
    offset: u32,
    utf8_len: u32,
    utf16_len: u32,
}

impl WideChar {
    const fn end(self) -> u32 {
        self.offset + self.utf8_len
    }
}

/// Converts byte offsets of one source text into line and column numbers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineIndex {
    line_starts: Vec<u32>,
    wide_chars: Vec<WideChar>,
}

fn to_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

impl LineIndex {
    /// Indexes `source`; lines end at `\n` (so `\r\n` is handled, with the `\r` belonging to the line it ends).
    #[must_use]
    pub fn new(source: &str) -> Self {
        let line_starts = std::iter::once(0)
            .chain(
                source
                    .bytes()
                    .enumerate()
                    .filter(|&(_, b)| b == b'\n')
                    .map(|(i, _)| to_u32(i + 1)),
            )
            .collect();
        let wide_chars = source
            .char_indices()
            .filter(|&(_, c)| c.len_utf8() > 1)
            .map(|(i, c)| WideChar {
                offset: to_u32(i),
                utf8_len: to_u32(c.len_utf8()),
                utf16_len: to_u32(c.len_utf16()),
            })
            .collect();
        Self {
            line_starts,
            wide_chars,
        }
    }

    /// 1-based `(line, column)` of a byte offset; the column counts Unicode scalar values.
    #[must_use]
    pub fn line_col(&self, offset: u32) -> (u32, u32) {
        let (line, start) = self.line_of(offset);
        let skipped = self.skipped_bytes(start, offset, |w| w.utf8_len - 1);
        (line, offset - start - skipped + 1)
    }

    /// Builds a [`Span`] from two byte offsets.
    #[must_use]
    pub fn span(&self, start: u32, end: u32) -> Span {
        let (start_line, start_col) = self.line_col(start);
        let (end_line, end_col) = self.line_col(end);
        Span {
            start,
            end,
            start_line,
            start_col,
            end_line,
            end_col,
        }
    }

    /// 1-based column of a byte offset counted in UTF-16 code units, as SARIF requires.
    #[must_use]
    pub fn utf16_col(&self, offset: u32) -> u32 {
        let (_, start) = self.line_of(offset);
        let skipped = self.skipped_bytes(start, offset, |w| w.utf8_len - w.utf16_len);
        offset - start - skipped + 1
    }

    fn line_of(&self, offset: u32) -> (u32, u32) {
        let line = self.line_starts.partition_point(|&start| start <= offset);
        let start = line
            .checked_sub(1)
            .and_then(|i| self.line_starts.get(i))
            .copied()
            .unwrap_or(0);
        (to_u32(line.max(1)), start)
    }

    fn skipped_bytes(&self, line_start: u32, offset: u32, weight: impl Fn(WideChar) -> u32) -> u32 {
        let lo = self.wide_chars.partition_point(|w| w.offset < line_start);
        let hi = self.wide_chars.partition_point(|w| w.end() <= offset).max(lo);
        self.wide_chars
            .get(lo..hi)
            .unwrap_or_default()
            .iter()
            .copied()
            .map(weight)
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_single_line() {
        let index = LineIndex::new("hello");
        assert_eq!(index.line_col(0), (1, 1));
        assert_eq!(index.line_col(4), (1, 5));
        assert_eq!(index.line_col(5), (1, 6));
    }

    #[test]
    fn multi_line_and_line_ends() {
        let index = LineIndex::new("ab\ncd\n\nef");
        let cases = [
            (0, (1, 1)),
            (2, (1, 3)),
            (3, (2, 1)),
            (5, (2, 3)),
            (6, (3, 1)),
            (7, (4, 1)),
            (9, (4, 3)),
        ];
        for (offset, expected) in cases {
            assert_eq!(index.line_col(offset), expected, "offset {offset}");
        }
    }

    #[test]
    fn crlf_keeps_carriage_return_on_previous_line() {
        let index = LineIndex::new("ab\r\ncd");
        assert_eq!(index.line_col(2), (1, 3));
        assert_eq!(index.line_col(3), (1, 4));
        assert_eq!(index.line_col(4), (2, 1));
        assert_eq!(index.line_col(6), (2, 3));
    }

    #[test]
    fn multibyte_columns_count_scalar_values() {
        let index = LineIndex::new("h\u{e9}llo\nw\u{e9}\u{e9}");
        assert_eq!(index.line_col(1), (1, 2));
        assert_eq!(index.line_col(3), (1, 3));
        assert_eq!(index.line_col(6), (1, 6));
        assert_eq!(index.line_col(7), (2, 1));
        assert_eq!(index.line_col(10), (2, 3));
    }

    #[test]
    fn astral_chars_are_one_column_but_two_utf16_units() {
        let index = LineIndex::new("a\u{1F600}b\n\u{1F600}c");
        assert_eq!(index.line_col(5), (1, 3));
        assert_eq!(index.utf16_col(5), 4);
        assert_eq!(index.line_col(6), (1, 4));
        assert_eq!(index.utf16_col(6), 5);
        assert_eq!(index.line_col(11), (2, 2));
        assert_eq!(index.utf16_col(11), 3);
    }

    #[test]
    fn utf16_col_matches_scalar_col_for_bmp_text() {
        let index = LineIndex::new("h\u{e9}llo");
        for offset in [0, 1, 3, 4, 6] {
            assert_eq!(index.utf16_col(offset), index.line_col(offset).1, "offset {offset}");
        }
    }

    #[test]
    fn span_combines_both_ends() {
        let index = LineIndex::new("ab\ncd");
        assert_eq!(
            index.span(1, 4),
            Span {
                start: 1,
                end: 4,
                start_line: 1,
                start_col: 2,
                end_line: 2,
                end_col: 2
            }
        );
    }

    #[test]
    fn empty_source_and_location_serde() {
        let index = LineIndex::new("");
        assert_eq!(index.line_col(0), (1, 1));
        let location = Location {
            path: "a/package.json".into(),
            span: None,
        };
        assert_eq!(
            serde_json::to_value(&location).unwrap_or_default(),
            serde_json::json!({ "path": "a/package.json" })
        );
    }

    #[test]
    fn location_constructors_set_path_and_span() {
        let whole = Location::file("package.json");
        assert_eq!(
            whole,
            Location {
                path: "package.json".into(),
                span: None
            }
        );
        assert_eq!(Location::new("a", whole.span).path, PathBuf::from("a"));
    }
}
