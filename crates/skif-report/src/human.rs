//! The human-readable renderer.

use std::collections::BTreeMap;
use std::io::{self, Write};

use skif_core::{Diagnostic, Fix, FixSafety, Report, Severity, Span, Summary};

use crate::options::RenderOptions;
use crate::style::Painter;
use crate::text::{escape_controls, frame};

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
            .entry(options.display_path(&diagnostic.location.path).into_owned())
            .or_default()
            .push(diagnostic);
    }
    for (path, mut diagnostics) in files {
        diagnostics.sort_by(|a, b| {
            start(a)
                .cmp(&start(b))
                .then_with(|| a.rule.cmp(&b.rule))
                .then_with(|| a.message.cmp(&b.message))
        });
        writeln!(out, "{}", paint.bold(&escape_controls(&path)))?;
        for diagnostic in diagnostics {
            write_diagnostic(out, paint, options, &path, diagnostic)?;
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
    diagnostic: &Diagnostic,
) -> io::Result<()> {
    let span = diagnostic.location.span;
    let framed = span.and_then(|span| {
        options
            .source_line(path, span.start_line)
            .map(|line| (span, frame(line, span.start_col, same_line_end(&span))))
    });
    let width = span.map_or(1, |span| digits(span.start_line));
    let pad = " ".repeat(width);
    let bar = paint.dim("|");
    let label = format!("{}[{}]", severity_label(diagnostic.severity), diagnostic.rule);
    writeln!(
        out,
        "  {}: {}",
        paint.severity(diagnostic.severity, &escape_controls(&label)),
        paint.bold(&escape_controls(&diagnostic.message))
    )?;
    writeln!(
        out,
        "  {pad}{} {}",
        paint.accent("-->"),
        escape_controls(&location_text(path, span.as_ref()))
    )?;
    if let Some((span, framed)) = framed {
        let number = paint.accent(&format!("{:>width$}", span.start_line));
        writeln!(out, "  {pad} {bar}")?;
        writeln!(out, "  {number} {bar} {}", framed.text)?;
        writeln!(
            out,
            "  {pad} {bar} {}{}",
            " ".repeat(framed.pad),
            paint.severity(diagnostic.severity, &"^".repeat(framed.carets))
        )?;
    }
    let note = |out: &mut dyn Write, key: &str, value: &str| {
        writeln!(
            out,
            "  {pad} {} {}: {}",
            paint.accent("="),
            paint.bold(key),
            escape_controls(value)
        )
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

/// The exclusive end column of a span that stays on its first line; `None` when it continues on later lines.
const fn same_line_end(span: &Span) -> Option<u32> {
    if span.end_line == span.start_line {
        Some(span.end_col)
    } else {
        None
    }
}

fn digits(n: u32) -> usize {
    n.checked_ilog10()
        .map_or(1, |log| usize::try_from(log).unwrap_or(usize::MAX).saturating_add(1))
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
}
