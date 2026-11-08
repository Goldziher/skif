//! The compact renderer: one line per diagnostic.

use std::io::{self, Write};

use skif_core::{Diagnostic, Report};

use crate::options::RenderOptions;
use crate::text::escape_controls;

/// One diagnostic as `path:line:col rule message`, or `path rule message` without a span; control characters in
/// the path and message are escaped so a diagnostic never spans lines.
pub(crate) fn compact_line(diagnostic: &Diagnostic, path: &str) -> String {
    let location = match diagnostic.location.span {
        Some(span) => format!("{path}:{}:{}", span.start_line, span.start_col),
        None => path.to_owned(),
    };
    format!(
        "{} {} {}",
        escape_controls(&location),
        diagnostic.rule.as_str(),
        escape_controls(&diagnostic.message)
    )
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
