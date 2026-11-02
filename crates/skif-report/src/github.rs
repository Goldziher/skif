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
        Severity::Info => "notice",
    }
}

fn annotation(diagnostic: &Diagnostic, options: &RenderOptions) -> String {
    let mut line = format!(
        "::{} file={}",
        command(diagnostic.severity),
        escape_property(&options.display_path(&diagnostic.location.path))
    );
    if let Some(span) = diagnostic.location.span {
        let _ = write!(
            line,
            ",line={},col={},endLine={},endColumn={}",
            span.start_line, span.start_col, span.end_line, span.end_col
        );
    }
    let title = format!("skif {}", diagnostic.rule);
    let _ = write!(
        line,
        ",title={}::{}",
        escape_property(&title),
        escape_data(&diagnostic.message)
    );
    line
}

fn count(n: usize, noun: &str) -> String {
    format!("{n} {noun}{}", if n == 1 { "" } else { "s" })
}

/// Renders one workflow command per diagnostic, followed by a plain summary line.
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
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escaping_table() {
        assert_eq!(escape_data("a%b\r\nc:d,e"), "a%25b%0D%0Ac:d,e");
        assert_eq!(escape_property("a%b\r\nc:d,e"), "a%25b%0D%0Ac%3Ad%2Ce");
    }
}
