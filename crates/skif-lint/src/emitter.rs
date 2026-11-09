use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use skif_core::{Diagnostic, IncompletePackage};

use crate::context::{EnabledRules, LintContext};
use crate::ignore::Suppressions;
use crate::scoped::Scopes;

/// Collects the diagnostics of one or more rules and applies policy centrally.
///
/// [`emit`](Self::emit) sets the effective severity (the configured level, else the severity the rule proposed with
/// [`Diagnostic::default_severity`], else the rule's default, then the levels and caps of the `[[overrides]]` that
/// cover the diagnostic and `[lint] max-level`), drops diagnostics of disabled rules and drops diagnostics matched
/// by `[[ignore]]` entries (those of an override only within its scope) or `workspace.ignore-packages`.
#[derive(Debug)]
pub struct Emitter<'a> {
    enabled: &'a EnabledRules,
    suppressions: Arc<Suppressions>,
    scopes: Arc<Scopes>,
    cancel: Option<&'a AtomicBool>,
    diagnostics: Vec<Diagnostic>,
    incomplete_packages: Vec<IncompletePackage>,
    other_project_files: Vec<PathBuf>,
    ancestor_dependency_uses: Vec<(PathBuf, String)>,
}

impl<'a> Emitter<'a> {
    /// Creates an emitter applying the policy of `cx`.
    #[must_use]
    pub fn new(cx: &LintContext<'a>) -> Self {
        Self {
            enabled: cx.enabled,
            suppressions: Arc::new(Suppressions::new(cx.workspace, cx.config)),
            scopes: Arc::new(Scopes::new(cx.workspace, cx.config)),
            cancel: None,
            diagnostics: Vec::new(),
            incomplete_packages: Vec::new(),
            other_project_files: Vec::new(),
            ancestor_dependency_uses: Vec::new(),
        }
    }

    /// Attaches a cancellation flag that [`is_cancelled`](Self::is_cancelled) reports.
    #[must_use]
    pub fn with_cancel(mut self, cancel: Option<&'a AtomicBool>) -> Self {
        self.cancel = cancel;
        self
    }

    /// An empty emitter with the same policy, for collecting in parallel and merging later.
    #[must_use]
    pub fn fork(&self) -> Self {
        Self {
            enabled: self.enabled,
            suppressions: Arc::clone(&self.suppressions),
            scopes: Arc::clone(&self.scopes),
            cancel: self.cancel,
            diagnostics: Vec::new(),
            incomplete_packages: Vec::new(),
            other_project_files: Vec::new(),
            ancestor_dependency_uses: Vec::new(),
        }
    }

    /// Appends the diagnostics, incomplete packages, files of other projects and ancestor dependency uses collected by
    /// `other`.
    pub fn merge(&mut self, other: Self) {
        if self.diagnostics.is_empty() {
            self.diagnostics = other.diagnostics;
        } else {
            self.diagnostics.extend(other.diagnostics);
        }
        self.incomplete_packages.extend(other.incomplete_packages);
        self.other_project_files.extend(other.other_project_files);
        self.ancestor_dependency_uses.extend(other.ancestor_dependency_uses);
    }

    /// Merges `parts` in order, with the same result as calling [`merge`](Self::merge) for each.
    ///
    /// The diagnostics of the largest part stay where they are and the others are moved around them. On a large
    /// workspace one provider can hold tens of thousands of diagnostics, and copying them into a fresh buffer touched
    /// megabytes of new memory for nothing.
    pub fn merge_all(&mut self, mut parts: Vec<Self>) {
        let Some(largest) = (0..parts.len()).max_by_key(|&index| parts[index].diagnostics.len()) else {
            return;
        };
        if parts[largest].diagnostics.len() > self.diagnostics.len() {
            let total = self.diagnostics.len() + parts.iter().map(|part| part.diagnostics.len()).sum::<usize>();
            let mut merged = std::mem::take(&mut parts[largest].diagnostics);
            merged.reserve_exact(total - merged.len());
            let mut front = std::mem::take(&mut self.diagnostics);
            for part in &mut parts[..largest] {
                front.append(&mut part.diagnostics);
            }
            merged.splice(0..0, front);
            self.diagnostics = merged;
        }
        for part in parts {
            self.merge(part);
        }
    }

    /// Whether the host asked the run to stop.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_some_and(|flag| flag.load(Ordering::Relaxed))
    }

    /// Reports a diagnostic; it is kept only if its rule is enabled and it is not suppressed.
    pub fn emit(&mut self, mut diagnostic: Diagnostic) {
        if self.admit(&mut diagnostic) {
            self.diagnostics.push(diagnostic);
        }
    }

    /// Reports `diagnostics` in order, as [`emit`](Self::emit) does one by one.
    ///
    /// The vector is kept as the emitter's own when nothing was emitted before, so a rule that builds many
    /// diagnostics at once hands them over without copying them.
    pub fn emit_all(&mut self, mut diagnostics: Vec<Diagnostic>) {
        diagnostics.retain_mut(|diagnostic| self.admit(diagnostic));
        if self.diagnostics.is_empty() {
            self.diagnostics = diagnostics;
        } else {
            self.diagnostics.append(&mut diagnostics);
        }
    }

    /// Applies the effective severity to `diagnostic` and says whether it is kept: its rule is enabled and it is not
    /// suppressed.
    fn admit(&self, diagnostic: &mut Diagnostic) -> bool {
        let applicable = self.scopes.applicable(diagnostic);
        let Some(severity) = self.scopes.severity(self.enabled, diagnostic, &applicable) else {
            return false;
        };
        diagnostic.severity = severity;
        diagnostic.default_severity = None;
        !(self.suppressions.suppresses(diagnostic) || self.suppressions.suppressed_in_scope(diagnostic, &applicable))
    }

    /// Number of diagnostics kept so far.
    #[must_use]
    pub fn len(&self) -> usize {
        self.diagnostics.len()
    }

    /// Whether no diagnostic has been kept.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.diagnostics.is_empty()
    }

    /// Records a package whose files and dependencies the provider could not judge, so the report can say so
    /// instead of passing the silence off as a clean result.
    pub fn report_incomplete_package(&mut self, package: IncompletePackage) {
        self.incomplete_packages.push(package);
    }

    /// Records a file of another project (such as a nested root) that the analysed code imports, relative to the
    /// workspace root, so a multi-root host can make it an entry of the root that owns it.
    pub fn report_other_project_file(&mut self, path: PathBuf) {
        self.other_project_files.push(path);
    }

    /// Takes the recorded files of other projects, in emission order.
    #[must_use]
    pub fn take_other_project_files(&mut self) -> Vec<PathBuf> {
        std::mem::take(&mut self.other_project_files)
    }

    /// Records that the analysed code uses dependency `name` through the `package.json` in `manifest_dir`, a
    /// directory above the workspace root, so a multi-root host can count it as used by the root that owns that
    /// manifest.
    pub fn report_ancestor_dependency_use(&mut self, manifest_dir: PathBuf, name: String) {
        self.ancestor_dependency_uses.push((manifest_dir, name));
    }

    /// Takes the recorded ancestor dependency uses, in emission order.
    #[must_use]
    pub fn take_ancestor_dependency_uses(&mut self) -> Vec<(PathBuf, String)> {
        std::mem::take(&mut self.ancestor_dependency_uses)
    }

    /// The kept diagnostics, in emission order.
    #[must_use]
    pub fn into_diagnostics(self) -> Vec<Diagnostic> {
        self.diagnostics
    }

    /// The kept diagnostics and the recorded incomplete packages, each in emission order.
    #[must_use]
    pub fn into_parts(self) -> (Vec<Diagnostic>, Vec<IncompletePackage>) {
        (self.diagnostics, self.incomplete_packages)
    }
}
