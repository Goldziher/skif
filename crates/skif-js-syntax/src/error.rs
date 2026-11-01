//! Errors returned by this crate.

use skif_core::Span;

/// Why a file could not be turned into a [`crate::ModuleInfo`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SyntaxError {
    /// The parser reported a syntax error.
    #[error("{message} at {}:{}", .span.start_line, .span.start_col)]
    Parse {
        /// First error message.
        message: String,
        /// Location of the first error.
        span: Span,
    },
    /// The file extension is not a supported script or component type.
    #[error("unsupported file type: {extension}")]
    Unsupported {
        /// The extension (without dot), empty if none.
        extension: String,
    },
}
