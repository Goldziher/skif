//! GitHub Actions workflow-command renderer.

use std::fmt::Write as _;
use std::io::{self, Write};
use std::path::Path;

use skif_core::{Diagnostic, Report, Severity};

use crate::options::RenderOptions;

/// Appends `value` escaped as a message (the part after the final `::`): `%`, `\r` and `\n`. A `property` value may
/// not contain `:` or `,` either, so those are escaped too. One pass: no escape produces a character another
/// escape rewrites.
fn push_escaped(line: &mut String, value: &str, property: bool) {
    let mut start = 0;
    for (at, byte) in value.bytes().enumerate() {
        let escape = match byte {
            b'%' => "%25",
            b'\r' => "%0D",
            b'\n' => "%0A",
            b':' if property => "%3A",
            b',' if property => "%2C",
            _ => continue,
        };
        // Every escaped character is ASCII, so `at` and `at + 1` are character boundaries.
        line.push_str(&value[start..at]);
        line.push_str(escape);
        start = at + 1;
    }
    line.push_str(&value[start..]);
}

const fn command(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        _ => "notice",
    }
}

/// Appends the workflow command of `diagnostic`, whose `file` property is already escaped, and a newline.
fn annotation(line: &mut String, diagnostic: &Diagnostic, file: &str) {
    line.push_str("::");
    line.push_str(command(diagnostic.severity));
    line.push_str(" file=");
    line.push_str(file);
    if let Some(span) = diagnostic.location.span {
        let _ = write!(
            line,
            ",line={},col={},endLine={},endColumn={}",
            span.start_line, span.start_col, span.end_line, span.end_col
        );
    }
    line.push_str(",title=");
    push_escaped(line, diagnostic.rule.as_str(), true);
    line.push_str("::");
    push_escaped(line, &diagnostic.message, false);
    line.push('\n');
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
    let mut line = String::new();
    // A report is sorted by path, so the diagnostics of one file follow each other and its `file` property is
    // worked out once for them.
    let mut file: Option<(&Path, String)> = None;
    for diagnostic in &report.diagnostics {
        let path = diagnostic.location.path.as_path();
        if file.as_ref().is_none_or(|(last, _)| last.as_os_str() != path.as_os_str()) {
            let mut escaped = String::new();
            push_escaped(&mut escaped, &options.repo_path(path), true);
            file = Some((path, escaped));
        }
        let escaped = file.as_ref().map_or("", |(_, escaped)| escaped.as_str());
        line.clear();
        annotation(&mut line, diagnostic, escaped);
        out.write_all(line.as_bytes())?;
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

    fn escaped(value: &str, property: bool) -> String {
        let mut line = String::from("::");
        push_escaped(&mut line, value, property);
        line
    }

    #[test]
    fn escaping_table() {
        let cases = [
            ("a%b\r\nc:d,e", false, "::a%25b%0D%0Ac:d,e"),
            ("a%b\r\nc:d,e", true, "::a%25b%0D%0Ac%3Ad%2Ce"),
            ("", true, "::"),
            ("plain", true, "::plain"),
            ("%%", false, "::%25%25"),
            ("r\u{e9}act:\u{65e5}\n", true, "::r\u{e9}act%3A\u{65e5}%0A"),
            (",:", false, "::,:"),
        ];
        for (value, property, expected) in cases {
            assert_eq!(escaped(value, property), expected, "{value:?} {property}");
        }
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
