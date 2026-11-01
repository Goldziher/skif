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
    let document: Value = serde_saphyr::from_str(source).map_err(|error| invalid(error.to_string()))?;
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
    let catalogs = if catalogs.is_empty() { Catalogs::default() } else { catalogs.with_source(path) };
    Ok(PnpmWorkspace { packages, catalogs })
}

fn unquote(token: &str) -> Option<(&str, usize)> {
    let quote = token.chars().next().filter(|c| matches!(c, '\'' | '"'))?;
    let close = token[1..].find(quote)?;
    Some((&token[1..=close], close + 2))
}

/// Finds the span of `pattern` inside the block-style `packages:` list of `pnpm-workspace.yaml`.
pub(crate) fn pattern_span(source: &str, line_index: &LineIndex, pattern: &str) -> Option<Span> {
    let mut in_packages = false;
    let mut offset = 0;
    for line in source.split_inclusive('\n') {
        let content = line.trim_end_matches(['\r', '\n']);
        let trimmed = content.trim_start();
        let indent = content.len() - trimmed.len();
        if indent == 0 && !trimmed.is_empty() && !trimmed.starts_with(['#', '-']) {
            let key = trimmed.split(':').next().unwrap_or_default().trim_matches(['"', '\'']);
            in_packages = key == "packages";
        } else if in_packages && let Some(item) = trimmed.strip_prefix('-') {
            let token = item.trim_start();
            let start = offset + indent + 1 + (item.len() - token.len());
            let (value, length) = unquote(token).unwrap_or_else(|| {
                let end = token.find(" #").unwrap_or(token.len());
                let plain = token[..end].trim_end();
                (plain, plain.len())
            });
            if value == pattern {
                return Some(line_index.span(to_u32(start), to_u32(start + length)));
            }
        }
        offset += line.len();
    }
    None
}
