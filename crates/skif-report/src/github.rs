//! GitHub Actions workflow-command renderer.

use std::fmt::Write as _;
use std::io::{self, Write};

use skif_core::{Diagnostic, Report, Severity};

use crate::options::RenderOptions;

/// Escapes a message (the part after the final `::`).
fn escape_data(value: &str) -> String {
    value.replace('%', "%25").replace('\r', "%0D").replace('\n', "%0A")
}

/// Escapes a property value, which additionally may not contain `:` or `,`.
fn escape_property(value: &str) -> String {
    escape_data(value).replace(':', "%3A").replace(',', "%2C")
}

const fn command(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        _ => "notice",
    }
}

fn annotation(diagnostic: &Diagnostic, options: &RenderOptions) -> String {
    let mut line = format!(
        "::{} file={}",
        command(diagnostic.severity),
        escape_property(&options.repo_path(&diagnostic.location.path))
    );
    if let Some(span) = diagnostic.location.span {
        let _ = write!(
            line,
            ",line={},col={},endLine={},endColumn={}",
            span.start_line, span.start_col, span.end_line, span.end_col
        );
    }
    let _ = write!(
        line,
        ",title={}::{}",
        escape_property(diagnostic.rule.as_str()),
        escape_data(&diagnostic.message)
    );
    line
}

fn count(n: usize, noun: &str) -> String {
    format!("{n} {noun}{}", if n == 1 { "" } else { "s" })
}

/// How many annotations of one level GitHub shows for a step; it drops the rest without saying so.
const SHOWN_PER_LEVEL: usize = 10;

/// Renders one workflow command per diagnostic, followed by a plain summary line, and a note when GitHub will not
/// show every annotation.
///
/// Errors map to `::error`, warnings to `::warning` and infos to `::notice`.
///
/// # Errors
///
/// Returns any error produced by writing to `out`.
pub fn render_github(report: &Report, options: &RenderOptions, out: &mut dyn Write) -> io::Result<()> {
    for diagnostic in &report.diagnostics {
        writeln!(out, "{}", annotation(diagnostic, options))?;
    }
    let summary = &report.summary;
    writeln!(
        out,
        "skif: {}, {}, {} in {}",
        count(summary.errors, "error"),
        count(summary.warnings, "warning"),
        count(summary.infos, "info"),
        count(summary.packages, "package"),
    )?;
    if [summary.errors, summary.warnings, summary.infos]
        .iter()
        .any(|&level| level > SHOWN_PER_LEVEL)
    {
        writeln!(
            out,
            "skif: GitHub shows at most {SHOWN_PER_LEVEL} annotations of each level per step; the others appear only in this log"
        )?;
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn escaping_table() {
        assert_eq!(escape_data("a%b\r\nc:d,e"), "a%25b%0D%0Ac:d,e");
        assert_eq!(escape_property("a%b\r\nc:d,e"), "a%25b%0D%0Ac%3Ad%2Ce");
    }

    fn rendered(errors: usize) -> String {
        use skif_core::{Location, RuleId};

        let diagnostics = (0..errors)
            .map(|index| {
                Diagnostic::new(
                    RuleId::from_static("demo"),
                    Severity::Error,
                    "m",
                    Location::new(format!("p{index}/package.json"), None),
                )
            })
            .collect();
        let mut out = Vec::new();
        render_github(&Report::new(diagnostics, 1), &RenderOptions::default(), &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn annotations_beyond_the_github_cap_are_announced() {
        assert!(!rendered(10).contains("GitHub shows at most"));
        let capped = rendered(11);
        assert!(capped.ends_with("the others appear only in this log\n"), "{capped}");
    }
}
