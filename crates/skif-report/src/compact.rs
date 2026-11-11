//! The compact renderer: one line per diagnostic.

use std::fmt;
use std::io::{self, Write};

use skif_core::{Diagnostic, Report};

use crate::options::RenderOptions;
use crate::text::escape_controls;

/// One diagnostic as `path:line:col rule message`, or `path rule message` without a span; control characters in
/// the path and message are escaped so a diagnostic never spans lines. Written straight to the formatter.
pub(crate) struct CompactLine<'a> {
    diagnostic: &'a Diagnostic,
    path: &'a str,
}

impl fmt::Display for CompactLine<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The line and column are digits, so escaping the path alone escapes the whole location.
        f.write_str(&escape_controls(self.path))?;
        if let Some(span) = self.diagnostic.location.span {
            write!(f, ":{}:{}", span.start_line, span.start_col)?;
        }
        write!(
            f,
            " {} {}",
            self.diagnostic.rule.as_str(),
            escape_controls(&self.diagnostic.message)
        )
    }
}

/// The compact line of `diagnostic`, whose file is displayed as `path`.
pub(crate) const fn compact_line<'a>(diagnostic: &'a Diagnostic, path: &'a str) -> CompactLine<'a> {
    CompactLine { diagnostic, path }
}

/// Renders one line per diagnostic, `path:line:col rule message`, in report order and with no summary, for
/// `grep` and editors that jump to a location.
///
/// # Errors
///
/// Returns any error produced by writing to `out`.
pub fn render_compact(report: &Report, options: &RenderOptions, out: &mut dyn Write) -> io::Result<()> {
    for diagnostic in &report.diagnostics {
        let path = options.display_path(&diagnostic.location.path);
        writeln!(out, "{}", compact_line(diagnostic, &path))?;
    }
    Ok(())
}
