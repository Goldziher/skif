//! Deterministic collection of diagnostics with summary counts.

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

use crate::diagnostic::Diagnostic;
use crate::rule::Severity;

/// Aggregate counts over a report.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct Summary {
    /// Diagnostics with [`Severity::Error`].
    pub errors: usize,
    /// Diagnostics with [`Severity::Warning`].
    pub warnings: usize,
    /// Diagnostics with [`Severity::Info`].
    pub infos: usize,
    /// Diagnostics that carry at least one fix.
    pub fixable: usize,
    /// Number of packages that were checked.
    pub packages: usize,
}

/// The complete result of a lint run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct Report {
    /// Diagnostics sorted by path, span start, rule and message.
    pub diagnostics: Vec<Diagnostic>,
    /// Counts over `diagnostics`.
    pub summary: Summary,
}

impl Report {
    /// Sorts `diagnostics` deterministically and computes the summary; `packages` is the number of packages checked.
    #[must_use]
    pub fn new(mut diagnostics: Vec<Diagnostic>, packages: usize) -> Self {
        diagnostics.sort_by(compare);
        let summary = diagnostics.iter().fold(
            Summary {
                packages,
                ..Summary::default()
            },
            count,
        );
        Self { diagnostics, summary }
    }
}

fn start(diagnostic: &Diagnostic) -> u32 {
    diagnostic.location.span.map_or(0, |span| span.start)
}

fn compare(a: &Diagnostic, b: &Diagnostic) -> Ordering {
    // Most neighbours share a file; equal bytes are equal paths, without comparing them component by component.
    let path = if a.location.path.as_os_str() == b.location.path.as_os_str() {
        Ordering::Equal
    } else {
        a.location.path.cmp(&b.location.path)
    };
    path.then_with(|| start(a).cmp(&start(b)))
        .then_with(|| a.rule.cmp(&b.rule))
        .then_with(|| a.message.cmp(&b.message))
}

fn count(mut summary: Summary, diagnostic: &Diagnostic) -> Summary {
    match diagnostic.severity {
        Severity::Error => summary.errors += 1,
        Severity::Warning => summary.warnings += 1,
        Severity::Info => summary.infos += 1,
    }
    summary.fixable += usize::from(diagnostic.is_fixable());
    summary
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::fix::{Fix, FixOp, FixSafety};
    use crate::rule::RuleId;
    use crate::span::{Location, Span};

    fn diag(rule: &'static str, severity: Severity, path: &str, start: Option<u32>, fixable: bool) -> Diagnostic {
        let span = start.map(|s| Span {
            start: s,
            end: s + 1,
            start_line: 1,
            start_col: s + 1,
            end_line: 1,
            end_col: s + 2,
        });
        let fixes = if fixable {
            vec![Fix {
                title: "f".into(),
                safety: FixSafety::Safe,
                ops: vec![FixOp::DeleteFile { path: path.into() }],
                preference: None,
            }]
        } else {
            Vec::new()
        };
        Diagnostic {
            rule: RuleId::from_static(rule),
            severity,
            message: rule.to_owned(),
            help: None,
            location: Location {
                path: path.into(),
                span,
            },
            related: Vec::new(),
            package: None,
            package_id: None,
            dependency: None,
            specifier: None,
            default_severity: None,
            detail: None,
            fixes,
        }
    }

    fn order(report: &Report) -> Vec<(String, u32, String)> {
        report
            .diagnostics
            .iter()
            .map(|d| (d.location.path.display().to_string(), start(d), d.rule.to_string()))
            .collect()
    }

    fn input() -> Vec<Diagnostic> {
        vec![
            diag("b-rule", Severity::Warning, "z/package.json", Some(4), true),
            diag("a-rule", Severity::Error, "a/package.json", Some(9), false),
            diag("c-rule", Severity::Info, "a/package.json", Some(2), false),
            diag("b-rule", Severity::Error, "a/package.json", Some(9), true),
            diag("a-rule", Severity::Error, "a/package.json", None, false),
        ]
    }

    #[test]
    fn sorts_by_path_start_then_rule() {
        let expected = [
            ("a/package.json", 0, "a-rule"),
            ("a/package.json", 2, "c-rule"),
            ("a/package.json", 9, "a-rule"),
            ("a/package.json", 9, "b-rule"),
            ("z/package.json", 4, "b-rule"),
        ]
        .map(|(p, s, r)| (p.to_owned(), s, r.to_owned()));
        assert_eq!(order(&Report::new(input(), 3)), expected);
    }

    #[test]
    fn order_is_independent_of_input_order() {
        let forward = Report::new(input(), 3);
        let mut reversed = input();
        reversed.reverse();
        assert_eq!(Report::new(reversed, 3), forward);
    }

    #[test]
    fn counts_summary() {
        assert_eq!(
            Report::new(input(), 3).summary,
            Summary {
                errors: 3,
                warnings: 1,
                infos: 1,
                fixable: 2,
                packages: 3
            }
        );
    }

    #[test]
    fn empty_report_has_zero_counts() {
        let report = Report::new(Vec::new(), 7);
        assert_eq!(report.diagnostics, Vec::new());
        assert_eq!(
            report.summary,
            Summary {
                packages: 7,
                ..Summary::default()
            }
        );
    }

    #[test]
    fn summary_serializes_kebab_case() {
        let summary = Summary {
            errors: 1,
            warnings: 2,
            infos: 3,
            fixable: 4,
            packages: 5,
        };
        assert_eq!(
            serde_json::to_value(summary).unwrap(),
            serde_json::json!({ "errors": 1, "warnings": 2, "infos": 3, "fixable": 4, "packages": 5 })
        );
    }
}
