use skif_core::{Diagnostic, Fix, Location, Related, RuleId, RuleMeta, Severity};

/// Builds a [`Diagnostic`] for `meta`.
///
/// The severity is a placeholder; the [`Emitter`](crate::Emitter) sets the effective one.
pub fn diagnostic(meta: &'static RuleMeta, message: impl Into<String>, location: Location) -> DiagnosticBuilder {
    DiagnosticBuilder {
        inner: Diagnostic {
            rule: RuleId::from_static(meta.id),
            severity: Severity::Warning,
            message: message.into(),
            help: None,
            location,
            related: Vec::new(),
            package: None,
            dependency: None,
            detail: None,
            fixes: Vec::new(),
        },
    }
}

/// Fluent builder returned by [`diagnostic`].
#[derive(Debug, Clone)]
#[must_use]
pub struct DiagnosticBuilder {
    inner: Diagnostic,
}

impl DiagnosticBuilder {
    /// Names the package the diagnostic belongs to (its display name).
    pub fn package(mut self, package: impl Into<String>) -> Self {
        self.inner.package = Some(package.into());
        self
    }

    /// Names the dependency the diagnostic is about: `name` or `name@specifier`.
    pub fn dependency(mut self, dependency: impl Into<String>) -> Self {
        self.inner.dependency = Some(dependency.into());
        self
    }

    /// Adds remediation advice.
    pub fn help(mut self, help: impl Into<String>) -> Self {
        self.inner.help = Some(help.into());
        self
    }

    /// Adds rule-specific machine-readable data.
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.inner.detail = Some(detail.into());
        self
    }

    /// Adds a secondary location.
    pub fn related(mut self, related: Related) -> Self {
        self.inner.related.push(related);
        self
    }

    /// Adds a fix.
    pub fn fix(mut self, fix: Fix) -> Self {
        self.inner.fixes.push(fix);
        self
    }

    /// Finishes the diagnostic.
    #[must_use]
    pub fn build(self) -> Diagnostic {
        self.inner
    }
}
