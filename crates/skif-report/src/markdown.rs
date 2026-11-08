//! Markdown renderers for pull request and merge request comments.

use std::collections::BTreeMap;
use std::io::{self, Write};

use skif_core::{Diagnostic, Report, Severity, Summary};

use crate::options::RenderOptions;

/// An empty HTML comment: GitHub renders nothing for it, but it splits the text around it, so what follows is not
/// read as part of a mention or an autolink.
const BREAK: &str = "<!-- -->";

/// Escapes text for a GitHub-flavoured Markdown table cell: punctuation that would start emphasis, a link, HTML,
/// strikethrough or a new cell is backslash-escaped, control characters are dropped and line breaks become `<br>`.
///
/// Backslash escapes do not stop GitHub's mentions and autolinks, so an empty HTML comment follows each `@` (a
/// scoped package such as `@types/node` would otherwise mention the `types` user or team) and precedes the `:` of
/// `://` and the `.` of `www.`, so URLs and email addresses in messages stay plain text.
fn cell(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    for (index, c) in text.char_indices() {
        match c {
            '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>' | '|' | '~' | '&' | '#' | '!' => {
                out.push('\\');
                out.push(c);
            }
            '@' => {
                out.push('@');
                out.push_str(BREAK);
            }
            ':' if text[index..].starts_with("://") => {
                out.push_str(BREAK);
                out.push(':');
            }
            '.' if out.len() >= 3 && out.as_bytes()[out.len() - 3..].eq_ignore_ascii_case(b"www") => {
                out.push_str(BREAK);
                out.push('.');
            }
            '\n' => out.push_str("<br>"),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

const fn severity(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        _ => "info",
    }
}

fn plural(n: usize, noun: &str) -> String {
    format!("{n} {noun}{}", if n == 1 { "" } else { "s" })
}

fn summary_line(summary: &Summary) -> String {
    format!(
        "{}, {} and {} in {}.",
        plural(summary.errors, "error"),
        plural(summary.warnings, "warning"),
        plural(summary.infos, "info"),
        plural(summary.packages, "package"),
    )
}

/// The diagnostics of `report` grouped by rule ID, rules in ID order and diagnostics in report order.
fn by_rule(report: &Report) -> BTreeMap<&str, Vec<&Diagnostic>> {
    let mut rules: BTreeMap<&str, Vec<&Diagnostic>> = BTreeMap::new();
    for diagnostic in &report.diagnostics {
        rules.entry(diagnostic.rule.as_str()).or_default().push(diagnostic);
    }
    rules
}

fn write_table(diagnostics: &[&Diagnostic], options: &RenderOptions, out: &mut dyn Write) -> io::Result<()> {
    writeln!(out, "| Location | Severity | Message |")?;
    writeln!(out, "| :-- | :-- | :-- |")?;
    for diagnostic in diagnostics {
        let path = options.display_path(&diagnostic.location.path);
        let location = match diagnostic.location.span {
            Some(span) => format!("{path}:{}:{}", span.start_line, span.start_col),
            None => path.into_owned(),
        };
        writeln!(
            out,
            "| {} | {} | {} |",
            cell(&location),
            severity(diagnostic.severity),
            cell(&diagnostic.message)
        )?;
    }
    Ok(())
}

/// Renders a Markdown report for a pull request comment: a heading, a summary line, then one section per rule with
/// a table of its diagnostics (location, severity, message). Rules are in ID order. Text from the repository is
/// escaped so it cannot break the table, inject HTML, mention a user or team (`@types/node`) or become a link.
///
/// # Errors
///
/// Returns any error produced by writing to `out`.
pub fn render_markdown(report: &Report, options: &RenderOptions, out: &mut dyn Write) -> io::Result<()> {
    writeln!(out, "# skif report")?;
    writeln!(out)?;
    writeln!(out, "{}", summary_line(&report.summary))?;
    for (rule, diagnostics) in by_rule(report) {
        writeln!(out)?;
        writeln!(out, "## {rule} ({})", diagnostics.len())?;
        writeln!(out)?;
        write_table(&diagnostics, options, out)?;
    }
    Ok(())
}

/// Renders like [`render_markdown`], with each rule's table folded in a `<details>` element whose summary names the
/// rule and its count, so a long report stays short in a pull request comment.
///
/// # Errors
///
/// Returns any error produced by writing to `out`.
pub fn render_disclosure(report: &Report, options: &RenderOptions, out: &mut dyn Write) -> io::Result<()> {
    writeln!(out, "# skif report")?;
    writeln!(out)?;
    writeln!(out, "{}", summary_line(&report.summary))?;
    for (rule, diagnostics) in by_rule(report) {
        writeln!(out)?;
        writeln!(out, "<details>")?;
        writeln!(out, "<summary>{rule} ({})</summary>", diagnostics.len())?;
        writeln!(out)?;
        write_table(&diagnostics, options, out)?;
        writeln!(out)?;
        writeln!(out, "</details>")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_cannot_break_the_table_or_inject_html() {
        assert_eq!(cell("a|b"), "a\\|b");
        assert_eq!(cell("<details>"), "\\<details\\>");
        assert_eq!(cell("__tests__/*.ts"), "\\_\\_tests\\_\\_/\\*.ts");
        assert_eq!(cell("one\ntwo\u{1b}"), "one<br>two");
        assert_eq!(cell("^18.3.1"), "^18.3.1");
    }

    #[test]
    fn cells_cannot_mention_or_autolink() {
        assert_eq!(cell("@types/node"), "@<!-- -->types/node");
        assert_eq!(cell("see https://x.dev"), "see https<!-- -->://x.dev");
        assert_eq!(cell("WWW.x.dev"), "WWW<!-- -->.x.dev");
        assert_eq!(cell("a.b"), "a.b");
    }
}
