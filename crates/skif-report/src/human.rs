//! The human-readable renderer.

use std::collections::BTreeMap;
use std::io::{self, Write};

use skif_core::{Diagnostic, Fix, FixSafety, Report, Severity, Span, Summary};

use crate::options::RenderOptions;
use crate::style::Painter;

/// Renders `report` grouped by file, with code frames when sources were supplied via
/// [`RenderOptions::with_source`], followed by a summary line.
///
/// # Errors
///
/// Returns any error produced by writing to `out`.
pub fn render_human(report: &Report, options: &RenderOptions, out: &mut dyn Write) -> io::Result<()> {
    let paint = Painter::new(options.color);
    let mut files: BTreeMap<String, Vec<&Diagnostic>> = BTreeMap::new();
    for diagnostic in &report.diagnostics {
        files
            .entry(options.display_path(&diagnostic.location.path))
            .or_default()
            .push(diagnostic);
    }
    for (path, mut diagnostics) in files {
        diagnostics.sort_by(|a, b| {
            a.rule
                .cmp(&b.rule)
                .then_with(|| start(a).cmp(&start(b)))
                .then_with(|| a.message.cmp(&b.message))
        });
        writeln!(out, "{}", paint.bold(&path))?;
        let source = options.source(&path);
        for diagnostic in diagnostics {
            write_diagnostic(out, paint, options, &path, source, diagnostic)?;
        }
        writeln!(out)?;
    }
    writeln!(out, "{}", summary_line(paint, &report.summary))
}

fn start(diagnostic: &Diagnostic) -> u32 {
    diagnostic.location.span.map_or(0, |span| span.start)
}

fn write_diagnostic(
    out: &mut dyn Write,
    paint: Painter,
    options: &RenderOptions,
    path: &str,
    source: Option<&str>,
    diagnostic: &Diagnostic,
) -> io::Result<()> {
    let span = diagnostic.location.span;
    let frame = span.and_then(|span| {
        source
            .and_then(|text| line_of(text, span.start_line))
            .map(|line| (span, line))
    });
    let width = span.map_or(1, |span| digits(span.start_line));
    let pad = " ".repeat(width);
    let bar = paint.dim("|");
    let label = format!("{}[{}]", severity_label(diagnostic.severity), diagnostic.rule);
    writeln!(
        out,
        "  {}: {}",
        paint.severity(diagnostic.severity, &label),
        paint.bold(&diagnostic.message)
    )?;
    writeln!(
        out,
        "  {pad}{} {}",
        paint.accent("-->"),
        location_text(path, span.as_ref())
    )?;
    if let Some((span, line)) = frame {
        let number = paint.accent(&format!("{:>width$}", span.start_line));
        writeln!(out, "  {pad} {bar}")?;
        writeln!(out, "  {number} {bar} {}", line.replace('\t', " "))?;
        writeln!(
            out,
            "  {pad} {bar} {}{}",
            " ".repeat(to_usize(span.start_col.saturating_sub(1))),
            paint.severity(diagnostic.severity, &"^".repeat(underline_width(&span, line)))
        )?;
    }
    let note = |out: &mut dyn Write, key: &str, value: &str| {
        writeln!(out, "  {pad} {} {}: {value}", paint.accent("="), paint.bold(key))
    };
    for (key, value) in [
        ("package", diagnostic.package.as_deref()),
        ("dependency", diagnostic.dependency.as_deref()),
        ("detail", diagnostic.detail.as_deref()),
        ("help", diagnostic.help.as_deref()),
    ] {
        if let Some(value) = value {
            note(out, key, value)?;
        }
    }
    for related in &diagnostic.related {
        let at = location_text(
            &options.display_path(&related.location.path),
            related.location.span.as_ref(),
        );
        note(out, "note", &format!("{} ({at})", related.message))?;
    }
    for fix in &diagnostic.fixes {
        note(out, &fix_key(fix), &fix.title)?;
    }
    Ok(())
}

fn fix_key(fix: &Fix) -> String {
    match fix.safety {
        FixSafety::Safe => "fix (safe)".to_owned(),
        _ => "fix (unsafe)".to_owned(),
    }
}

const fn severity_label(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        _ => "info",
    }
}

fn location_text(path: &str, span: Option<&Span>) -> String {
    span.map_or_else(
        || path.to_owned(),
        |span| format!("{path}:{}:{}", span.start_line, span.start_col),
    )
}

fn line_of(source: &str, line: u32) -> Option<&str> {
    let index = to_usize(line.checked_sub(1)?);
    source
        .split('\n')
        .nth(index)
        .map(|text| text.strip_suffix('\r').unwrap_or(text))
}

/// Columns are character based, matching [`Span`]; wide glyphs are not measured specially.
fn underline_width(span: &Span, line: &str) -> usize {
    let end = if span.end_line == span.start_line {
        span.end_col
    } else {
        u32::try_from(line.chars().count())
            .unwrap_or(u32::MAX)
            .saturating_add(1)
    };
    to_usize(end.saturating_sub(span.start_col)).max(1)
}

fn digits(n: u32) -> usize {
    n.checked_ilog10().map_or(1, |log| to_usize(log) + 1)
}

fn to_usize(n: u32) -> usize {
    usize::try_from(n).unwrap_or(usize::MAX)
}

fn plural(count: usize, word: &str) -> String {
    format!("{count} {word}{}", if count == 1 { "" } else { "s" })
}

fn summary_line(paint: Painter, summary: &Summary) -> String {
    let counts: Vec<String> = [
        (summary.errors, "error"),
        (summary.warnings, "warning"),
        (summary.infos, "info"),
    ]
    .into_iter()
    .filter(|&(count, _)| count > 0)
    .map(|(count, word)| plural(count, word))
    .collect();
    let scope = if summary.packages > 0 {
        format!(" in {}", plural(summary.packages, "package"))
    } else {
        String::new()
    };
    if counts.is_empty() {
        return paint.success(&format!("No issues found{scope}"));
    }
    let fixable = if summary.fixable > 0 {
        format!(" ({} fixable)", summary.fixable)
    } else {
        String::new()
    };
    paint.bold(&format!("Found {}{scope}{fixable}", counts.join(", ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digits_table() {
        for (n, expected) in [(0, 1), (1, 1), (9, 1), (10, 2), (99, 2), (100, 3)] {
            assert_eq!(digits(n), expected, "{n}");
        }
    }

    #[test]
    fn line_of_handles_crlf_and_bounds() {
        let text = "a\r\nb\n";
        assert_eq!(line_of(text, 1), Some("a"));
        assert_eq!(line_of(text, 2), Some("b"));
        assert_eq!(line_of(text, 4), None);
        assert_eq!(line_of(text, 0), None);
    }
}
