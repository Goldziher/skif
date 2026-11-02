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
}

#[derive(Debug, Default)]
pub(crate) struct PnpmWorkspace {
    pub packages: Option<Vec<String>>,
    pub catalogs: Catalogs,
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
    Ok(PnpmWorkspace { packages, catalogs })
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
