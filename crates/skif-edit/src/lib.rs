//! Format-preserving JSON and YAML edits for skif fixes.
//!
//! [`plan_edits`] turns structural [`FixOp`]s into byte-level [`TextEdit`]s over the original source, so every byte a
//! fix does not touch (indentation, comments, key order, line endings) stays identical. [`apply`] plans and applies.
//!
//! [`format_package_json`] formats a `package.json` as syncpack's `format` does (ADR 0009).
//!
//! JSON (including JSONC comments) is located with `jsonc-parser` byte ranges. YAML needs the `yaml` feature and is
//! edited with `yamlpatch`; see [`FileKind::Yaml`].

#![warn(missing_docs)]

mod describe;
mod error;
mod json;
mod package_json;
#[cfg(feature = "yaml")]
mod yaml;

use std::fmt;
use std::path::Path;

pub use error::EditPlanError;
pub use package_json::{
    DEFAULT_SORT_AZ, DEFAULT_SORT_EXPORTS, DEFAULT_SORT_FIRST, FormatIssue, FormatOptions, Formatted,
    format_package_json, minimal_edit,
};
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
/// A caller planning several sets of operations against one file parses it once with [`Document::parse`] instead.
///
/// # Errors
///
/// Returns [`EditPlanError`] when the source does not parse, a path does not exist, an action does not fit the
/// target, or two operations conflict.
pub fn plan_edits(source: &str, kind: FileKind, ops: &[&FixOp]) -> Result<Vec<TextEdit>, EditPlanError> {
    Document::parse(source, kind)?.plan(ops)
}

/// Plans `ops` with [`plan_edits`] and applies them to `source`.
///
/// # Errors
///
/// Returns the errors of [`plan_edits`], or [`EditPlanError::Apply`] if the edits cannot be applied.
pub fn apply(source: &str, kind: FileKind, ops: &[&FixOp]) -> Result<String, EditPlanError> {
    Document::parse(source, kind)?
        .plan_checked(ops)
        .map(|(_, output)| output)
}

/// A JSON or YAML file parsed once, so several sets of operations are planned against it without parsing it again:
/// [`Document::plan`] of each set gives what [`plan_edits`] gives for it.
///
/// A JSON document keeps its syntax tree. A YAML document keeps only its text, as `yamlpatch` applies each operation
/// to a document of its own.
pub struct Document<'s> {
    source: &'s str,
    parsed: Parsed<'s>,
}

enum Parsed<'s> {
    Json(json::Parsed<'s>),
    Yaml,
}

impl<'s> Document<'s> {
    /// Parses `source`, a file of `kind`.
    ///
    /// # Errors
    ///
    /// Returns [`EditPlanError::Parse`] when a JSON source does not parse. A YAML source is parsed by each
    /// [`Document::plan`], which returns its errors.
    pub fn parse(source: &'s str, kind: FileKind) -> Result<Self, EditPlanError> {
        let parsed = match kind {
            FileKind::Json => Parsed::Json(json::parse(source)?),
            FileKind::Yaml => Parsed::Yaml,
        };
        Ok(Self { source, parsed })
    }

    /// The text the document was parsed from.
    #[must_use]
    pub const fn source(&self) -> &'s str {
        self.source
    }

    /// The text edits `ops` amount to over the source, as [`plan_edits`] gives them.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`plan_edits`].
    pub fn plan(&self, ops: &[&FixOp]) -> Result<Vec<TextEdit>, EditPlanError> {
        self.plan_checked(ops).map(|(edits, _)| edits)
    }

    /// Plans the edits, applies them and checks that the result still parses.
    fn plan_checked(&self, ops: &[&FixOp]) -> Result<(Vec<TextEdit>, String), EditPlanError> {
        let mut planned = match &self.parsed {
            Parsed::Json(parsed) => parsed.plan(ops)?,
            Parsed::Yaml => plan_yaml(self.source, ops)?,
        };
        planned.sort_by_key(|edit| (edit.start, edit.end, edit.op));
        ensure_disjoint(&planned, ops)?;
        let edits = planned
            .into_iter()
            .map(|edit| {
                Ok(TextEdit::new(
                    u32::try_from(edit.start).map_err(|_| EditPlanError::SourceTooLarge)?,
                    u32::try_from(edit.end).map_err(|_| EditPlanError::SourceTooLarge)?,
                    edit.replacement,
                ))
            })
            .collect::<Result<Vec<_>, EditPlanError>>()?;
        let output = apply_text_edits(self.source, &edits)?;
        if !edits.is_empty() {
            match &self.parsed {
                Parsed::Json(parsed) => parsed.validate(&output)?,
                Parsed::Yaml => validate_yaml(&output)?,
            }
        }
        Ok((edits, output))
    }
}

impl fmt::Debug for Document<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self.parsed {
            Parsed::Json(_) => FileKind::Json,
            Parsed::Yaml => FileKind::Yaml,
        };
        f.debug_struct("Document").field("kind", &kind).finish_non_exhaustive()
    }
}

#[cfg(feature = "yaml")]
fn validate_yaml(after: &str) -> Result<(), EditPlanError> {
    yaml::validate(after)
}

#[cfg(not(feature = "yaml"))]
fn validate_yaml(_after: &str) -> Result<(), EditPlanError> {
    Err(EditPlanError::YamlDisabled)
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

const _: () = {
    fn assert_send_sync<T: Send + Sync>() {}
    let _ = assert_send_sync::<EditPlanError>;
    let _ = assert_send_sync::<FileKind>;
};
