use crate::diagnostic::Diagnostic;
use crate::fix::Fix;
use crate::package_id::PackageId;
use crate::rule::{RuleId, RuleMeta, Severity};
use crate::span::{Location, Related};

/// Builds a [`Diagnostic`] for `meta`.
///
/// The severity is a placeholder; the rule engine sets the effective one when the diagnostic is emitted.
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
            package_id: None,
            dependency: None,
            specifier: None,
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

    /// Identifies the package the diagnostic belongs to; suppressions prefer this over the display name.
    pub const fn package_id(mut self, id: PackageId) -> Self {
        self.inner.package_id = Some(id);
        self
    }

    /// Names the dependency the diagnostic is about (its name; `name@specifier` is still accepted).
    pub fn dependency(mut self, dependency: impl Into<String>) -> Self {
        self.inner.dependency = Some(dependency.into());
        self
    }

    /// Records the raw specifier of the dependency instance the diagnostic is about, so that `[[ignore]]` entries
    /// written as `name@specifier` can match it.
    pub fn specifier(mut self, specifier: impl Into<String>) -> Self {
        self.inner.specifier = Some(specifier.into());
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::fix::{FixOp, FixSafety};
    use crate::rule::{Category, Fixability};
    use crate::span::Span;

    const META: RuleMeta = RuleMeta {
        id: "demo",
        category: Category::Manifest,
        default: Some(Severity::Error),
        fixable: Fixability::None,
        summary: "demo",
    };

    fn location() -> Location {
        Location {
            path: "package.json".into(),
            span: None,
        }
    }

    #[test]
    fn minimal_diagnostic_has_placeholder_severity_and_no_extras() {
        let built = diagnostic(&META, "message", location()).build();
        assert_eq!(built.rule.as_str(), "demo");
        assert_eq!(built.severity, Severity::Warning);
        assert_eq!(built.message, "message");
        assert!(
            built.help.is_none() && built.package.is_none() && built.dependency.is_none()
                && built.specifier.is_none()
                && built.detail.is_none()
        );
        assert!(built.related.is_empty() && built.fixes.is_empty());
        assert!(!built.is_fixable());
    }

    #[test]
    fn every_setter_lands_in_its_field() {
        let related = Related {
            message: "see".into(),
            location: Location {
                path: "other.json".into(),
                span: Some(Span {
                    start: 1,
                    end: 2,
                    start_line: 1,
                    start_col: 2,
                    end_line: 1,
                    end_col: 3,
                }),
            },
        };
        let fix = Fix {
            title: "drop".into(),
            safety: FixSafety::Safe,
            ops: vec![FixOp::DeleteFile { path: "x".into() }],
        };
        let built = diagnostic(&META, "m", location())
            .package("pkg")
            .dependency("react")
            .specifier("^18.0.0")
            .help("do it")
            .detail("d")
            .related(related.clone())
            .fix(fix.clone())
            .fix(fix.clone())
            .build();
        assert_eq!(built.package.as_deref(), Some("pkg"));
        assert_eq!(built.dependency.as_deref(), Some("react"));
        assert_eq!(built.specifier.as_deref(), Some("^18.0.0"));
        assert_eq!(built.help.as_deref(), Some("do it"));
        assert_eq!(built.detail.as_deref(), Some("d"));
        assert_eq!(built.related, [related]);
        assert_eq!(built.fixes, [fix.clone(), fix]);
        assert!(built.is_fixable());
    }
}
