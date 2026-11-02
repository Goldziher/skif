//! Errors raised while planning or applying edits.

use skif_core::EditError;

use crate::FileKind;

/// Reason a set of [`skif_core::FixOp`]s could not be turned into text edits.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum EditPlanError {
    /// The source is not valid for its file kind.
    #[error("cannot parse {kind} source: {message}")]
    Parse {
        /// Kind of file being edited.
        kind: FileKind,
        /// Parser message.
        message: String,
    },
    /// The path of an operation does not exist in the document.
    #[error("{op}: path not found in document")]
    PathNotFound {
        /// Description of the operation.
        op: String,
    },
    /// The target node cannot take the requested action.
    #[error("{op}: target is not {expected}")]
    WrongNodeKind {
        /// Description of the operation.
        op: String,
        /// What the target would have to be.
        expected: &'static str,
    },
    /// A key an operation would create already exists.
    #[error("{op}: key {key:?} already exists")]
    DuplicateKey {
        /// Description of the operation.
        op: String,
        /// The key that is already present.
        key: String,
    },
    /// The operation is not meaningful for this file kind.
    #[error("{op}: operation is not supported for {kind} files")]
    Unsupported {
        /// Description of the operation.
        op: String,
        /// Kind of file being edited.
        kind: FileKind,
    },
    /// Two operations change overlapping parts of the file.
    #[error("operations conflict: `{first}` overlaps `{second}`")]
    Conflict {
        /// Description of the earlier operation.
        first: String,
        /// Description of the later operation.
        second: String,
    },
    /// The YAML editor rejected an operation.
    #[error("{op}: {message}")]
    Yaml {
        /// Description of the operation.
        op: String,
        /// Editor message.
        message: String,
    },
    /// YAML editing was requested but the `yaml` feature is off.
    #[error("YAML edits require the `yaml` feature of skif-edit")]
    YamlDisabled,
    /// The source is larger than 4 GiB, so offsets do not fit a `u32`.
    #[error("source is too large to address with 32-bit offsets")]
    SourceTooLarge,
    /// The edits apply, but the result is no longer a valid document.
    #[error("the edits would produce invalid {kind}: {message}")]
    InvalidOutput {
        /// Kind of file being edited.
        kind: FileKind,
        /// Parser message.
        message: String,
    },
    /// The planned edits could not be applied.
    #[error(transparent)]
    Apply(#[from] EditError),
}
