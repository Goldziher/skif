use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use skif_core::Diagnostic;

use crate::context::{EnabledRules, LintContext};
use crate::ignore::Suppressions;

/// Collects the diagnostics of one or more rules and applies policy centrally.
///
/// [`emit`](Self::emit) sets the effective severity, drops diagnostics of disabled rules and
/// drops diagnostics matched by `[[ignore]]` entries or `workspace.ignore-packages`.
#[derive(Debug)]
pub struct Emitter<'a> {
    enabled: &'a EnabledRules,
    suppressions: Arc<Suppressions>,
    cancel: Option<&'a AtomicBool>,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> Emitter<'a> {
    /// Creates an emitter applying the policy of `cx`.
    #[must_use]
    pub fn new(cx: &LintContext<'a>) -> Self {
        Self {
            enabled: cx.enabled,
            suppressions: Arc::new(Suppressions::new(cx.workspace, cx.config)),
            cancel: None,
            diagnostics: Vec::new(),
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
            cancel: self.cancel,
            diagnostics: Vec::new(),
        }
    }

    /// Appends the diagnostics collected by `other`.
    pub fn merge(&mut self, other: Self) {
        self.diagnostics.extend(other.diagnostics);
    }

    /// Whether the host asked the run to stop.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_some_and(|flag| flag.load(Ordering::Relaxed))
    }

    /// Reports a diagnostic; it is kept only if its rule is enabled and it is not suppressed.
    pub fn emit(&mut self, mut diagnostic: Diagnostic) {
        let Some(severity) = self.enabled.severity(diagnostic.rule.as_str()) else {
            return;
        };
        diagnostic.severity = severity;
        if self.suppressions.suppresses(&diagnostic) {
            return;
        }
        self.diagnostics.push(diagnostic);
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

    /// The kept diagnostics, in emission order.
    #[must_use]
    pub fn into_diagnostics(self) -> Vec<Diagnostic> {
        self.diagnostics
    }
}
