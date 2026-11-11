//! The human-readable renderer.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;
use std::io::{self, Write};

use skif_core::{Diagnostic, Fix, FixSafety, IncompleteScope, Report, Severity, Span, Summary};

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
    // Keyed by the displayed path, borrowed from the diagnostic whenever displaying it changes nothing.
    let mut files: BTreeMap<Cow<'_, str>, Vec<&Diagnostic>> = BTreeMap::new();
    for diagnostic in &report.diagnostics {
        files
            .entry(options.display_path(&diagnostic.location.path))
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
        let shown = escape_controls(&path);
        writeln!(out, "{}", paint.bold(&shown))?;
        for diagnostic in diagnostics {
            write_diagnostic(out, paint, options, (&*path, &*shown), diagnostic)?;
        }
        writeln!(out)?;
    }
    if !report.incomplete_packages.is_empty() {
        let names: Vec<String> = report
            .incomplete_packages
            .iter()
            .map(|package| {
                let name = escape_controls(&package.name);
                match &package.scope {
                    IncompleteScope::Within(globs) if !globs.is_empty() => {
                        let globs: Vec<String> = globs
                            .iter()
                            .map(|glob| format!("`{}`", escape_controls(glob)))
                            .collect();
                        format!("{name} (files matching {})", globs.join(" or "))
                    }
                    _ => name.into_owned(),
                }
            })
            .collect();
        let line = format!(
            "note: unused files and dependencies were not fully checked in {} ({}): their entry files are not all known; \
             `skif explain-usage <file>` says why",
            plural(names.len(), "package"),
            names.join(", ")
        );
        writeln!(out, "{}", paint.dim(line))?;
    }
    writeln!(out, "{}", summary_line(paint, &report.summary))
}

fn start(diagnostic: &Diagnostic) -> u32 {
    diagnostic.location.span.map_or(0, |span| span.start)
}

/// Spaces for the widest line number, a `u32`.
const PAD: &str = "          ";

/// `{severity}[{rule}]`, with the rule escaped as `escape_controls` escapes the whole label: the rest is plain.
struct Label<'a>(Severity, &'a str);

impl fmt::Display for Label<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}[{}]", severity_label(self.0), escape_controls(self.1))
    }
}

/// `:{line}:{column}` of a span's start, or nothing without one.
struct Position<'a>(Option<&'a Span>);

impl fmt::Display for Position<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(span) => write!(f, ":{}:{}", span.start_line, span.start_col),
            None => Ok(()),
        }
    }
}

/// `count` copies of a character.
struct Repeat(char, usize);

impl fmt::Display for Repeat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (0..self.1).try_for_each(|_| fmt::Write::write_char(f, self.0))
    }
}

/// A line number right-aligned to `width`.
struct Number(u32, usize);

impl fmt::Display for Number {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:>1$}", self.0, self.1)
    }
}

/// Writes one diagnostic of the file at `path`, which is displayed (control characters escaped) as `shown`.
fn write_diagnostic(
    out: &mut dyn Write,
    paint: Painter,
    options: &RenderOptions,
    (path, shown): (&str, &str),
    diagnostic: &Diagnostic,
) -> io::Result<()> {
    let span = diagnostic.location.span;
    let framed = span.and_then(|span| {
        options
            .source_line(path, span.start_line)
            .map(|line| (span, frame(line, span.start_col, same_line_end(&span))))
    });
    let width = span.map_or(1, |span| digits(span.start_line));
    let pad = PAD.get(..width).unwrap_or(PAD);
    let bar = paint.dim("|");
    writeln!(
        out,
        "  {}: {}",
        paint.severity(
            diagnostic.severity,
            Label(diagnostic.severity, diagnostic.rule.as_str())
        ),
        paint.bold(escape_controls(&diagnostic.message))
    )?;
    writeln!(out, "  {pad}{} {shown}{}", paint.accent("-->"), Position(span.as_ref()))?;
    if let Some((span, framed)) = framed {
        let number = paint.accent(Number(span.start_line, width));
        writeln!(out, "  {pad} {bar}")?;
        writeln!(out, "  {number} {bar} {}", framed.text)?;
        writeln!(
            out,
            "  {pad} {bar} {}{}",
            Repeat(' ', framed.pad),
            paint.severity(diagnostic.severity, Repeat('^', framed.carets))
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
        note(out, fix_key(fix), &fix.title)?;
    }
    Ok(())
}

const fn fix_key(fix: &Fix) -> &'static str {
    match fix.safety {
        FixSafety::Safe => "fix (safe)",
        _ => "fix (unsafe)",
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
        return paint.success(format!("No issues found{scope}")).to_string();
    }
    let fixable = if summary.fixable > 0 {
        format!(" ({} fixable)", summary.fixable)
    } else {
        String::new()
    };
    paint
        .bold(format!("Found {}{scope}{fixable}", counts.join(", ")))
        .to_string()
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
