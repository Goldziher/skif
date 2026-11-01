//! Format-preserving JSON and YAML edits for skif fixes.
//!
//! [`plan_edits`] turns structural [`FixOp`]s into byte-level [`TextEdit`]s over the original source, so every byte a
//! fix does not touch (indentation, comments, key order, line endings) stays identical. [`apply`] plans and applies.
//!
//! JSON (including JSONC comments) is located with `jsonc-parser` byte ranges. YAML needs the `yaml` feature and is
//! edited with `yamlpatch`; see [`FileKind::Yaml`].

mod describe;
mod error;
mod json;
#[cfg(feature = "yaml")]
mod yaml;

use std::fmt;
use std::path::Path;

pub use error::EditPlanError;
use skif_core::{FixOp, TextEdit, apply_text_edits};

/// Format of a file that can be edited.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FileKind {
    /// JSON or JSONC, such as `package.json`.
    Json,
    /// YAML, such as `pnpm-workspace.yaml`. Requires the `yaml` feature.
    Yaml,
}

impl FileKind {
    /// Infers the kind from a file extension; `None` when the file is neither JSON nor YAML.
    #[must_use]
    pub fn from_path(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        match extension.as_str() {
            "json" | "jsonc" => Some(Self::Json),
            "yaml" | "yml" => Some(Self::Yaml),
            _ => None,
        }
    }
}

impl fmt::Display for FileKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Json => "JSON",
            Self::Yaml => "YAML",
        })
    }
}

/// A byte-range replacement that still carries the operation that produced it.
pub(crate) struct Planned {
    pub op: usize,
    pub start: usize,
    pub end: usize,
    pub replacement: String,
}

/// Translates `ops`, all aimed at one file of `kind`, into text edits over `source`.
///
/// The edits are sorted by position and never overlap. Operations whose edits would overlap are a conflict and fail
/// the whole plan, naming both operations; resolving that (dropping the later fix) is the caller's decision.
/// [`FixOp::Text`] edits are passed through unchanged.
///
/// # Errors
///
/// Returns [`EditPlanError`] when the source does not parse, a path does not exist, an action does not fit the
/// target, or two operations conflict.
pub fn plan_edits(source: &str, kind: FileKind, ops: &[&FixOp]) -> Result<Vec<TextEdit>, EditPlanError> {
    let mut planned = match kind {
        FileKind::Json => json::plan(source, ops)?,
        FileKind::Yaml => plan_yaml(source, ops)?,
    };
    planned.sort_by_key(|edit| (edit.start, edit.end, edit.op));
    ensure_disjoint(&planned, ops)?;
    planned
        .into_iter()
        .map(|edit| {
            Ok(TextEdit {
                start: u32::try_from(edit.start).map_err(|_| EditPlanError::SourceTooLarge)?,
                end: u32::try_from(edit.end).map_err(|_| EditPlanError::SourceTooLarge)?,
                replacement: edit.replacement,
            })
        })
        .collect()
}

/// Plans `ops` with [`plan_edits`] and applies them to `source`.
///
/// # Errors
///
/// Returns the errors of [`plan_edits`], or [`EditPlanError::Apply`] if the edits cannot be applied.
pub fn apply(source: &str, kind: FileKind, ops: &[&FixOp]) -> Result<String, EditPlanError> {
    let edits = plan_edits(source, kind, ops)?;
    Ok(apply_text_edits(source, &edits)?)
}

#[cfg(feature = "yaml")]
fn plan_yaml(source: &str, ops: &[&FixOp]) -> Result<Vec<Planned>, EditPlanError> {
    yaml::plan(source, ops)
}

#[cfg(not(feature = "yaml"))]
fn plan_yaml(_source: &str, _ops: &[&FixOp]) -> Result<Vec<Planned>, EditPlanError> {
    Err(EditPlanError::YamlDisabled)
}

/// Checks position-sorted edits for overlaps between edits of different operations.
fn ensure_disjoint(sorted: &[Planned], ops: &[&FixOp]) -> Result<(), EditPlanError> {
    let mut reach: Option<(usize, usize)> = None;
    for edit in sorted {
        if let Some((owner, end)) = reach
            && owner != edit.op
            && edit.start < end
        {
            let (first, second) = if owner < edit.op {
                (owner, edit.op)
            } else {
                (edit.op, owner)
            };
            return Err(EditPlanError::Conflict {
                first: describe::op(ops[first]),
                second: describe::op(ops[second]),
            });
        }
        if reach.is_none_or(|(_, end)| edit.end > end) {
            reach = Some((edit.op, edit.end));
        }
    }
    Ok(())
}
