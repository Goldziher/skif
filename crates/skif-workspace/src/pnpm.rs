//! `pnpm-workspace.yaml`: workspace patterns and catalogs.

use std::ops::{ControlFlow, Range};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;
use skif_core::{HashMap, LineIndex, Span};

use crate::catalogs::{self, Catalogs};
use crate::error::WorkspaceError;
use crate::manifest::to_u32;

/// The raw text of `pnpm-workspace.yaml`, kept so rules and fixes can address it.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct YamlSource {
    /// Workspace-relative path.
    pub path: PathBuf,
    /// The file contents as read.
    pub source: Arc<str>,
    /// Offset-to-line/column conversion for `source`.
    pub line_index: Arc<LineIndex>,
    /// The `overrides:` entries as written: dependency selector and replacement.
    pub overrides: Vec<(String, String)>,
    /// pnpm's `minimumReleaseAge`, in minutes, when it is a non-negative integer.
    pub minimum_release_age: Option<u64>,
    /// pnpm's `nodeLinker` (`isolated`, `hoisted` or `pnp`) as written, when it is a string.
    pub node_linker: Option<String>,
    /// pnpm's `publicHoistPattern`, in order, when it is a list of strings or one string.
    pub public_hoist_pattern: Option<Vec<String>>,
    /// pnpm's `shamefullyHoist`, when it is a boolean.
    pub shamefully_hoist: Option<bool>,
}

impl YamlSource {
    /// The span of the `selector: replacement` entry of the top-level `overrides:` mapping, from the key through the
    /// value. Block mappings are understood, with plain or quoted keys and values, comments and a byte order mark; a
    /// flow mapping (`overrides: {a: 1}`) is not, and gives `None`.
    #[must_use]
    pub fn override_span(&self, selector: &str) -> Option<Span> {
        self.entry_span("overrides", selector)
    }

    /// The span of the `key: value` entry directly under the top-level `section:` mapping, from the key through the
    /// value (the key alone when the value is on the following lines), as [`override_span`](Self::override_span)
    /// reads `overrides:`.
    #[must_use]
    pub fn entry_span(&self, section: &str, key: &str) -> Option<Span> {
        entry_span(&self.source, &self.line_index, section, key)
    }
}

/// The span of the `key: value` entry directly under the top-level `section:` block mapping of the YAML `source`: the
/// first entry of the section sets the indentation its entries share, so a deeper line naming `key` is not taken.
fn entry_span(source: &str, line_index: &LineIndex, section: &str, key: &str) -> Option<Span> {
    let mut span = None;
    section_entries(source, section, |name, start, end| {
        if name == key {
            span = Some(line_index.span(to_u32(start), to_u32(end)));
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    });
    span
}

/// The span of every entry [`entry_span`] finds under `section` within the `document` byte range of `source`, by
/// key; a key written twice keeps its first entry, the one [`entry_span`] returns. One pass over the text answers any
/// number of lookups.
pub(crate) fn section_index(
    source: &str,
    document: Range<usize>,
    line_index: &LineIndex,
    section: &str,
) -> HashMap<Box<str>, Span> {
    let base = document.start;
    let mut index = HashMap::default();
    section_entries(&source[document], section, |name, start, end| {
        index
            .entry(Box::from(name))
            .or_insert_with(|| line_index.span(to_u32(base + start), to_u32(base + end)));
        ControlFlow::Continue(())
    });
    index
}

/// Hands the key and the byte range of each entry directly under a top-level `section:` block mapping to `visit`, in
/// file order, until it breaks.
fn section_entries<'s>(
    source: &'s str,
    section: &str,
    mut visit: impl FnMut(&'s str, usize, usize) -> ControlFlow<()>,
) {
    let bom = if source.starts_with('\u{feff}') {
        '\u{feff}'.len_utf8()
    } else {
        0
    };
    let mut in_section = false;
    let mut entry_indent = None;
    let mut offset = bom;
    for line in source[bom..].split_inclusive('\n') {
        let content = line.trim_end_matches(['\r', '\n']);
        let trimmed = content.trim_start();
        let indent = content.len() - trimmed.len();
        // Lines inside other sections are skipped without being read as entries.
        if !trimmed.is_empty() && !trimmed.starts_with('#') && (indent == 0 || in_section) {
            let entry = mapping_entry(trimmed);
            if indent == 0 {
                in_section = entry.is_some_and(|(name, ..)| name == section);
                entry_indent = None;
            } else if *entry_indent.get_or_insert(indent) == indent
                && let Some((name, value_at, value_len)) = entry
            {
                let start = offset + indent;
                let end = offset + indent + value_at + value_len;
                if visit(name, start, end).is_break() {
                    return;
                }
            }
        }
        offset += line.len();
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
    pub minimum_release_age: Option<u64>,
    pub node_linker: Option<String>,
    pub public_hoist_pattern: Option<Vec<String>>,
    pub shamefully_hoist: Option<bool>,
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
        .map_or_default(|entries| {
            entries
                .iter()
                .filter_map(|(selector, replacement)| Some((selector.clone(), replacement.as_str()?.to_owned())))
                .collect()
        });
    let minimum_release_age = document.get("minimumReleaseAge").and_then(Value::as_u64);
    let node_linker = document.get("nodeLinker").and_then(Value::as_str).map(str::to_owned);
    let public_hoist_pattern = match document.get("publicHoistPattern") {
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| item.as_str().map(str::to_owned))
            .collect::<Option<Vec<_>>>(),
        Some(Value::String(pattern)) => Some(vec![pattern.clone()]),
        _ => None,
    };
    let shamefully_hoist = document.get("shamefullyHoist").and_then(Value::as_bool);
    Ok(PnpmWorkspace {
        packages,
        catalogs,
        overrides,
        minimum_release_age,
        node_linker,
        public_hoist_pattern,
        shamefully_hoist,
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
            minimum_release_age: None,
            node_linker: None,
            public_hoist_pattern: None,
            shamefully_hoist: None,
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

    #[test]
    fn the_section_index_finds_what_each_lookup_finds() {
        let text = "\u{feff}# lock\nsnapshots:\n  a@1.0.0: {}\n  'b@2.0.0':\n    dependencies:\n      a@1.0.0: x\n  # c@3.0.0: {}\n  a@1.0.0: again\n  \"d:e\": 1\npackages:\n  a@1.0.0:\n    resolution: {}\n  nested:\n      deeper: 1\nsnapshots:\n    f@1.0.0: {}\n  g@1.0.0: {}\r\n";
        let line_index = LineIndex::new(text);
        let keys = [
            "a@1.0.0",
            "b@2.0.0",
            "c@3.0.0",
            "d:e",
            "f@1.0.0",
            "g@1.0.0",
            "dependencies",
            "resolution",
            "nested",
            "deeper",
            "missing",
        ];
        for section in ["snapshots", "packages", "importers"] {
            let index = section_index(text, 0..text.len(), &line_index, section);
            for key in keys {
                assert_eq!(
                    index.get(key).copied(),
                    entry_span(text, &line_index, section, key),
                    "{section} {key}"
                );
            }
        }
        // Not vacuous: the first of a duplicate wins, the second section sets its own indentation, deeper lines and
        // comments are not entries.
        let snapshots = section_index(text, 0..text.len(), &line_index, "snapshots");
        let found = |key: &str| {
            snapshots
                .get(key)
                .map(|span| &text[span.start as usize..span.end as usize])
        };
        assert_eq!(found("a@1.0.0"), Some("a@1.0.0: {}"));
        assert_eq!(found("b@2.0.0"), Some("'b@2.0.0'"));
        assert_eq!(found("d:e"), Some("\"d:e\": 1"));
        assert_eq!(found("f@1.0.0"), Some("f@1.0.0: {}"));
        assert_eq!(found("g@1.0.0"), None);
        assert_eq!(found("c@3.0.0"), None);
        assert_eq!(found("dependencies"), None);
        assert_eq!(snapshots.len(), 4);
    }

    #[test]
    fn minimum_release_age_is_read_when_it_is_a_whole_number_of_minutes() {
        let read = |text: &str| parse(Path::new("pnpm-workspace.yaml"), text).map(|parsed| parsed.minimum_release_age);
        assert_eq!(read("minimumReleaseAge: 2880\n").ok(), Some(Some(2880)));
        assert_eq!(read("minimumReleaseAge: 0\n").ok(), Some(Some(0)));
        assert_eq!(read("minimumReleaseAge: soon\n").ok(), Some(None));
        assert_eq!(read("minimumReleaseAge: -5\n").ok(), Some(None));
        assert_eq!(read("packages: []\n").ok(), Some(None));
    }

    #[test]
    fn node_linker_is_read_when_it_is_a_string() {
        let read = |text: &str| parse(Path::new("pnpm-workspace.yaml"), text).map(|parsed| parsed.node_linker);
        assert_eq!(read("nodeLinker: hoisted\n").ok(), Some(Some("hoisted".to_owned())));
        assert_eq!(read("nodeLinker: 'isolated'\n").ok(), Some(Some("isolated".to_owned())));
        assert_eq!(read("nodeLinker: 1\n").ok(), Some(None));
        assert_eq!(read("packages: []\n").ok(), Some(None));
    }

    #[test]
    fn public_hoisting_settings_are_read_when_well_typed() {
        let patterns = |text: &str| {
            parse(Path::new("pnpm-workspace.yaml"), text)
                .map(|parsed| parsed.public_hoist_pattern)
                .ok()
        };
        let owned = |items: &[&str]| Some(Some(items.iter().map(|item| (*item).to_owned()).collect::<Vec<_>>()));
        assert_eq!(
            patterns("publicHoistPattern:\n  - '*eslint*'\n  - '!eslint-config-x'\n"),
            owned(&["*eslint*", "!eslint-config-x"])
        );
        assert_eq!(patterns("publicHoistPattern: []\n"), owned(&[]));
        assert_eq!(patterns("publicHoistPattern: '*types*'\n"), owned(&["*types*"]));
        assert_eq!(patterns("publicHoistPattern: [a, 1]\n"), Some(None));
        assert_eq!(patterns("packages: []\n"), Some(None));
        let shamefully = |text: &str| {
            parse(Path::new("pnpm-workspace.yaml"), text)
                .map(|parsed| parsed.shamefully_hoist)
                .ok()
        };
        assert_eq!(shamefully("shamefullyHoist: true\n"), Some(Some(true)));
        assert_eq!(shamefully("shamefullyHoist: false\n"), Some(Some(false)));
        assert_eq!(shamefully("shamefullyHoist: 'yes'\n"), Some(None));
        assert_eq!(shamefully("packages: []\n"), Some(None));
    }
}
