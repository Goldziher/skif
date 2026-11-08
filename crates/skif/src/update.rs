//! `skif update`: newer published versions for the dependencies the version policies govern (ADR 0057).
//!
//! The facade never opens a connection. The host implements [`Registry`] (the `skif` command line over HTTP, a test
//! from memory), [`plan_updates`] asks it once per dependency name and returns an [`UpdateReport`], and
//! [`apply_updates`] turns the report into a [`FixPlan`] of format-preserving edits for the host to write.
//!
//! ```
//! use std::path::Path;
//! use skif::update::{FetchError, Packument, Published, Registry, UpdateOptions, apply_updates, plan_updates};
//! use skif::{Config, MemoryFs, Workspace};
//!
//! struct Fixed;
//! impl Registry for Fixed {
//!     fn fetch(&self, name: &str) -> Result<Packument, FetchError> {
//!         match name {
//!             "left-pad" => Ok(Packument::new(vec![Published::new("1.0.0", Some(0)), Published::new("1.3.0", Some(0))])),
//!             _ => Err(FetchError::NotFound),
//!         }
//!     }
//! }
//!
//! let fs = MemoryFs::new().with_file("/repo/package.json", r#"{"name": "app", "dependencies": {"left-pad": "^1.0.0"}}"#);
//! let config = Config::default();
//! let workspace = Workspace::discover_with_fs(Path::new("/repo"), &config, &fs)?;
//! let report = plan_updates(&workspace, &config, &Fixed, &UpdateOptions::new(1_800_000_000));
//! assert_eq!(report.updates[0].target, "^1.3.0");
//! let plan = apply_updates(&workspace, &report, &fs)?;
//! let skif::FileChange::Modify { new_contents, .. } = &plan.changes[0] else { unreachable!() };
//! assert!(new_contents.contains(r#""left-pad": "^1.3.0""#));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use skif_core::{Diagnostic, FileSystem, FixSafety, Report, RuleId, Severity};
pub use skif_rules::update::{
    DEFAULT_MINIMUM_RELEASE_AGE, Failure, FetchError, Note, Packument, Published, Registry, STATE, Update,
    UpdateOptions, UpdateReport, is_valid_package_name, minimum_release_age, plan_updates, unix_seconds,
};
use skif_workspace::Workspace;

use crate::fix::{Alternatives, FixError, FixPlan, FixSelection, apply_fixes};

/// The rule ID the entries of a [`FixPlan`] made by [`apply_updates`] carry. It names no lint rule: linting never
/// reads the registry.
pub const UPDATE_ID: RuleId = RuleId::from_static("update");

/// Plans the edits that write every update of `report`, as [`apply_fixes`] plans fixes: per file, format-preserving,
/// with the files read through `fs`. Each [`AppliedFix`](crate::AppliedFix) carries [`UPDATE_ID`] and a message that
/// names the dependency, its current specifier and its target.
///
/// # Errors
/// [`FixError::Plan`] when the edits of a file cannot be combined.
pub fn apply_updates(workspace: &Workspace, report: &UpdateReport, fs: &dyn FileSystem) -> Result<FixPlan, FixError> {
    let diagnostics: Vec<Diagnostic> = report
        .updates
        .iter()
        .map(|update| {
            let mut diagnostic = Diagnostic::new(
                UPDATE_ID,
                Severity::Warning,
                format!("{} {} can be {}", update.dependency, update.current, update.target),
                update.location.clone(),
            );
            diagnostic.detail = Some(STATE.to_owned());
            diagnostic.fixes.push(update.fix.clone());
            diagnostic
        })
        .collect();
    let report = Report::new(diagnostics, 0);
    let selection = FixSelection::new(FixSafety::Unsafe).with_alternatives(Alternatives::First);
    apply_fixes(workspace, &report, &selection, fs)
}
