//! Core diagnostic, fix and report types for skif.
//!
//! The crate is library-first and deliberately free of side effects:
//!
//! - **No IO outside [`fs`].** Nothing else here reads or writes files. Paths are workspace-relative [`std::path::PathBuf`]s that the
//!   caller resolves; fixes are described as data ([`FixOp`]) and only [`apply_text_edits`] touches text, in memory.
//! - **Plain serde data.** Every type is `Serialize`/`Deserialize` with kebab-case names, so reports can be emitted
//!   as JSON and fixes can cross process boundaries unchanged. The `schema` feature adds `schemars` derives.
//! - **Poly-compatible spans.** A [`Span`] is a half-open UTF-8 byte range plus 1-based line and column numbers
//!   (columns count Unicode scalar values). [`LineIndex`] converts offsets, including UTF-16 columns for SARIF.
//!
//! The [`fs`] module holds the read-only [`FileSystem`] abstraction with its disk ([`OsFs`]) and in-memory
//! ([`MemoryFs`]) implementations; it is the only code that touches the disk.
//!
//! A rule produces [`Diagnostic`]s; each may carry alternative [`Fix`]es (the first is preferred) made of
//! structured [`FixOp`]s. [`Report::new`] sorts diagnostics deterministically and computes a [`Summary`].
//!
//! ```
//! use skif_core::{Diagnostic, Location, Report, RuleId, Severity};
//!
//! let diagnostic = Diagnostic {
//!     rule: RuleId::new("unused-dependency").unwrap_or_else(|_| RuleId::from_static("fallback")),
//!     severity: Severity::Warning,
//!     message: "dependency is never imported".to_owned(),
//!     help: None,
//!     location: Location { path: "package.json".into(), span: None },
//!     related: Vec::new(),
//!     package: None,
//!     package_id: None,
//!     dependency: None,
//!     detail: None,
//!     fixes: Vec::new(),
//! };
//! let report = Report::new(vec![diagnostic], 1);
//! assert_eq!(report.summary.warnings, 1);
//! ```

pub mod builder;
pub mod dependency;
pub mod diagnostic;
pub mod fix;
pub mod fs;
pub mod package_id;
pub mod report;
pub mod rule;
pub mod span;

pub use builder::{DiagnosticBuilder, diagnostic};
pub use dependency::DependencyKind;
pub use diagnostic::Diagnostic;
pub use fix::{
    EditError, FileChange, Fix, FixOp, FixSafety, InsertPosition, JsonAction, PathSeg, TextEdit, YamlAction,
    apply_text_edits,
};
pub use fs::{DirEntry, FileSystem, MemoryFs, OsFs};
pub use package_id::PackageId;
pub use report::{Report, Summary};
pub use rule::{Category, Fixability, InvalidRuleId, RuleId, RuleMeta, Severity};
pub use span::{LineIndex, Location, Related, Span};

const _: () = {
    fn assert_send_sync<T: Send + Sync>() {}
    let _ = assert_send_sync::<Diagnostic>;
    let _ = assert_send_sync::<DiagnosticBuilder>;
    let _ = assert_send_sync::<Report>;
    let _ = assert_send_sync::<Fix>;
    let _ = assert_send_sync::<FixOp>;
    let _ = assert_send_sync::<FileChange>;
    let _ = assert_send_sync::<RuleMeta>;
    let _ = assert_send_sync::<Span>;
    let _ = assert_send_sync::<PackageId>;
    let _ = assert_send_sync::<LineIndex>;
    let _ = assert_send_sync::<MemoryFs>;
    let _ = assert_send_sync::<OsFs>;
};
