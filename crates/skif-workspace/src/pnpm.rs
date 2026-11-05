//! `pnpm-workspace.yaml`: workspace patterns and catalogs.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;
use skif_core::{LineIndex, Span};

use crate::catalogs::{self, Catalogs};
use crate::error::WorkspaceError;
use crate::manifest::to_u32;

/// The raw text of `pnpm-workspace.yaml`, kept so rules and fixes can address it.
#[derive(Debug, Clone)]
pub struct YamlSource {
    /// Workspace-relative path.
    pub path: PathBuf,
    /// The file contents as read.
    pub source: Arc<str>,
    /// Offset-to-line/column conversion for `source`.
    pub line_index: Arc<LineIndex>,
    /// The `overrides:` entries as written: dependency selector and replacement.
    pub overrides: Vec<(String, String)>,
}

impl YamlSource {
    /// The span of the `selector: replacement` entry of the top-level `overrides:` mapping, from the key through the
    /// value. Block mappings are understood, with plain or quoted keys and values, comments and a byte order mark; a
    /// flow mapping (`overrides: {a: 1}`) is not, and gives `None`.
    #[must_use]
    pub fn override_span(&self, selector: &str) -> Option<Span> {
        let bom = if self.source.starts_with('\u{feff}') {
            '\u{feff}'.len_utf8()
        } else {
            0
        };
        let mut in_overrides = false;
        let mut offset = bom;
        for line in self.source[bom..].split_inclusive('\n') {
            let content = line.trim_end_matches(['\r', '\n']);
            let trimmed = content.trim_start();
            let indent = content.len() - trimmed.len();
            if !trimmed.is_empty() && !trimmed.starts_with('#') {
                let entry = mapping_entry(trimmed);
                if indent == 0 {
                    in_overrides = entry.is_some_and(|(key, ..)| key == "overrides");
                } else if in_overrides
                    && let Some((key, value_at, value_len)) = entry
                    && key == selector
                {
                    let start = offset + indent;
                    let end = offset + indent + value_at + value_len;
                    return Some(self.line_index.span(to_u32(start), to_u32(end)));
                }
            }
            offset += line.len();
        }
        None
    }
}

/// The key of a `key: value` line (without indentation) and the offset and length of the value's token in it; the
/// length is 0 when the value is on the following lines.
fn mapping_entry(line: &str) -> Option<(&str, usize, usize)> {
    let (key, after_key) = if let Some(quote) = line.chars().next().filter(|c| matches!(c, '\'' | '"')) {
        let close = line[1..].find(quote)? + 1;
        let tail = &line[close + 1..];
        (&line[1..close], close + 1 + tail.len() - tail.trim_start().len())
    } else {
        let colon = key_colon(line)?;
        (line[..colon].trim_end(), colon)
    };
    let rest = line[after_key..].strip_prefix(':')?;
    let value = rest.trim_start();
    let value_at = line.len() - value.len();
    if value.is_empty() || value.starts_with('#') {
        return Some((key, after_key, 0));
    }
    let (_, length) = scalar(value, false);
    Some((key, value_at, length))
}

/// The `:` that ends a plain key: the first one followed by a space, a tab or the end of the line. A `:` inside a
/// plain scalar, as in the selector `foo@npm:bar`, is part of the key.
fn key_colon(line: &str) -> Option<usize> {
    line.char_indices()
        .find(|&(index, c)| {
            c == ':'
                && line[index + 1..]
                    .chars()
                    .next()
                    .is_none_or(|next| matches!(next, ' ' | '\t'))
        })
        .map(|(index, _)| index)
}

#[derive(Debug, Default)]
pub(crate) struct PnpmWorkspace {
    pub packages: Option<Vec<String>>,
    pub catalogs: Catalogs,
    pub overrides: Vec<(String, String)>,
}

pub(crate) fn parse(path: &Path, source: &str) -> Result<PnpmWorkspace, WorkspaceError> {
    let invalid = |message: String| WorkspaceError::InvalidPnpmWorkspace {
        path: path.to_path_buf(),
        message,
    };
    let document: Value = serde_saphyr::from_str(source.strip_prefix('\u{feff}').unwrap_or(source))
        .map_err(|error| invalid(error.to_string()))?;
    let packages = match document.get("packages") {
        None | Some(Value::Null) => None,
        Some(Value::Array(items)) => Some(
            items
                .iter()
                .map(|item| {
                    item.as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| invalid("`packages` must be a list of strings".to_owned()))
                })
                .collect::<Result<Vec<_>, _>>()?,
        ),
        Some(_) => return Err(invalid("`packages` must be a list of strings".to_owned())),
    };
    let catalogs = catalogs::from_json(document.get("catalog"), document.get("catalogs"));
    let catalogs = if catalogs.is_empty() {
        Catalogs::default()
    } else {
        catalogs.with_source(path)
    };
    let overrides = document
        .get("overrides")
        .and_then(Value::as_object)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|(selector, replacement)| Some((selector.clone(), replacement.as_str()?.to_owned())))
                .collect()
        })
        .unwrap_or_default();
    Ok(PnpmWorkspace {
        packages,
        catalogs,
        overrides,
    })
}

/// Reads one scalar at the start of `text`: a quoted string, or a plain scalar up to a flow delimiter or ` #`.
///
/// Returns the value and the length of the written token (quotes included).
fn scalar(text: &str, flow: bool) -> (&str, usize) {
    if let Some(quote) = text.chars().next().filter(|c| matches!(c, '\'' | '"'))
        && let Some(close) = text[1..].find(quote)
    {
        return (&text[1..=close], close + 2);
    }
    let mut end = text.find(" #").unwrap_or(text.len());
    if flow && let Some(delimiter) = text[..end].find([',', ']']) {
        end = delimiter;
    }
    let plain = text[..end].trim_end();
    (plain, plain.len())
}

/// The patterns of the `packages:` list of `pnpm-workspace.yaml` as written, each with its span, in order.
///
/// Block lists, flow lists (`packages: [a, "b"]`, possibly over several lines), quoting, trailing comments and a
/// byte order mark are understood. Position `n` is the `n`-th list entry, so duplicates keep their own spans.
pub(crate) fn pattern_spans(source: &str, line_index: &LineIndex) -> Vec<(String, Span)> {
    let bom = if source.starts_with('\u{feff}') {
        '\u{feff}'.len_utf8()
    } else {
        0
    };
    let mut found = Vec::new();
    let mut in_packages = false;
    let mut in_flow = false;
    let mut offset = bom;
    for line in source[bom..].split_inclusive('\n') {
        let content = line.trim_end_matches(['\r', '\n']);
        let trimmed = content.trim_start();
        let indent = content.len() - trimmed.len();
        if in_flow {
            in_flow = flow_items(content, offset, line_index, &mut found);
        } else if indent == 0 && !trimmed.is_empty() && !trimmed.starts_with(['#', '-']) {
            let (key, rest) = trimmed.split_once(':').unwrap_or((trimmed, ""));
            in_packages = key.trim().trim_matches(['"', '\'']) == "packages";
            let value = rest.trim_start();
            if in_packages && value.starts_with('[') {
                let from = content.len() - value.len() + 1;
                in_flow = flow_items(&content[from..], offset + from, line_index, &mut found);
            }
        } else if in_packages && let Some(item) = trimmed.strip_prefix('-') {
            let token = item.trim_start();
            let start = indent + 1 + (item.len() - token.len());
            let (value, length) = scalar(token, false);
            found.push((
                value.to_owned(),
                line_index.span(to_u32(offset + start), to_u32(offset + start + length)),
            ));
        }
        offset += line.len();
    }
    found
}

/// Collects the entries of a flow sequence found in `text` (which begins at `base` in the source). Returns whether the
/// sequence continues on the next line.
fn flow_items(text: &str, base: usize, line_index: &LineIndex, found: &mut Vec<(String, Span)>) -> bool {
    let mut at = 0;
    while at < text.len() {
        let rest = &text[at..];
        let skipped = rest.len() - rest.trim_start_matches([' ', '\t', ',']).len();
        at += skipped;
        let rest = &text[at..];
        if rest.is_empty() || rest.starts_with('#') {
            break;
        }
        if rest.starts_with(']') {
            return false;
        }
        let (value, length) = scalar(rest, true);
        if length == 0 {
            break;
        }
        found.push((
            value.to_owned(),
            line_index.span(to_u32(base + at), to_u32(base + at + length)),
        ));
        at += length;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn yaml(text: &str) -> YamlSource {
        YamlSource {
            path: PathBuf::from("pnpm-workspace.yaml"),
            source: Arc::from(text),
            line_index: Arc::new(LineIndex::new(text)),
            overrides: Vec::new(),
        }
    }

    fn slice(text: &str, span: Option<Span>) -> Option<&str> {
        let span = span?;
        text.get(span.start as usize..span.end as usize)
    }

    #[test]
    fn override_entries_are_located_from_key_through_value() {
        let text = "packages:\n  - \"packages/*\"\noverrides:\n  lodash: 4.17.20 # pinned\n  \"@scope/pkg@<2\": '^2.1.0'\n  parent>child: ^1.0.0\ncatalog:\n  lodash: 4.17.21\n";
        let source = yaml(text);
        for (selector, expected) in [
            ("lodash", "lodash: 4.17.20"),
            ("@scope/pkg@<2", "\"@scope/pkg@<2\": '^2.1.0'"),
            ("parent>child", "parent>child: ^1.0.0"),
        ] {
            assert_eq!(
                slice(text, source.override_span(selector)),
                Some(expected),
                "{selector}"
            );
        }
        assert_eq!(source.override_span("missing"), None);
    }

    #[test]
    fn keys_with_a_colon_and_a_space_before_the_colon_are_located() {
        let text =
            "overrides:\n  foo@npm:bar: 1.0.0\n  \"a\" : ^2.0.0\n  'b'\t: 3.0.0\n  c:\n    - x\n  \"d:e\": 4.0.0\n";
        let source = yaml(text);
        for (selector, expected) in [
            ("foo@npm:bar", "foo@npm:bar: 1.0.0"),
            ("a", "\"a\" : ^2.0.0"),
            ("b", "'b'\t: 3.0.0"),
            ("d:e", "\"d:e\": 4.0.0"),
        ] {
            assert_eq!(
                slice(text, source.override_span(selector)),
                Some(expected),
                "{selector}"
            );
        }
    }

    #[test]
    fn only_the_top_level_overrides_mapping_is_read() {
        let text = "\u{feff}catalog:\r\n  lodash: 4.17.21\r\noverrides:\r\n  lodash: 4.17.20\r\n";
        let source = yaml(text);
        assert_eq!(slice(text, source.override_span("lodash")), Some("lodash: 4.17.20"));
        let catalog_only = yaml("catalog:\n  lodash: 4.17.21\n");
        assert_eq!(catalog_only.override_span("lodash"), None);
    }
}
