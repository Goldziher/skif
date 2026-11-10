//! `# skif-ignore-next-line` and `# skif-ignore-file` comments in toolchain files.
//!
//! Every file kind the toolchain rules read takes `#` comments, so one syntax serves them all:
//!
//! ```text
//! # skif-ignore-next-line foreign-package-manager-command -- fallback for the 1.x release branch
//! # skif-ignore-file toolchain-pin-drift -- the image pins its own Node
//! ```
//!
//! The rule ids are optional (none covers every toolchain rule); the reason follows `--` and is required: a comment
//! without one suppresses nothing, and the finding says so.

/// A suppression comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Directive {
    /// The rule ids it names; empty covers every toolchain rule.
    pub rules: Vec<String>,
    /// The text after `--`, trimmed; `None` when missing or blank.
    pub reason: Option<String>,
    /// The 1-based line of the comment.
    pub line: u32,
    /// The 1-based first and last line it covers: the next line that is neither blank nor a comment, with the lines a
    /// trailing backslash continues it onto. `None` covers the whole file.
    pub covers: Option<(u32, u32)>,
}

const NEXT_LINE: &str = "skif-ignore-next-line";
const FILE: &str = "skif-ignore-file";

fn to_line(index: usize) -> u32 {
    u32::try_from(index + 1).unwrap_or(u32::MAX)
}

fn is_code(line: &str) -> bool {
    let trimmed = line.trim();
    !trimmed.is_empty() && !trimmed.starts_with('#')
}

/// The lines from the next code line after `index` through the lines its trailing backslashes continue onto.
fn next_code_lines(lines: &[&str], index: usize) -> Option<(u32, u32)> {
    let start = (index + 1..lines.len()).find(|&candidate| is_code(lines[candidate]))?;
    let mut end = start;
    while end + 1 < lines.len() && lines[end].trim_end().ends_with('\\') {
        end += 1;
    }
    Some((to_line(start), to_line(end)))
}

/// The directive a comment line holds, without its coverage.
fn parse(line: &str) -> Option<(bool, Vec<String>, Option<String>)> {
    let comment = line.trim().strip_prefix('#')?.trim_start_matches('#').trim_start();
    let (whole_file, rest) = if let Some(rest) = comment.strip_prefix(NEXT_LINE) {
        (false, rest)
    } else {
        (true, comment.strip_prefix(FILE)?)
    };
    if !(rest.is_empty() || rest.starts_with(char::is_whitespace)) {
        return None;
    }
    let (names, reason) = rest.split_once("--").map_or((rest, None), |(names, reason)| (names, Some(reason)));
    let rules = names
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect();
    let reason = reason.map(str::trim).filter(|reason| !reason.is_empty()).map(str::to_owned);
    Some((whole_file, rules, reason))
}

/// Every suppression comment of `source`, in line order.
pub(crate) fn directives(source: &str) -> Vec<Directive> {
    let lines: Vec<&str> = source.lines().collect();
    lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| {
            let (whole_file, rules, reason) = parse(line)?;
            let covers = if whole_file {
                None
            } else {
                Some(next_code_lines(&lines, index)?)
            };
            Some(Directive {
                rules,
                reason,
                line: to_line(index),
                covers,
            })
        })
        .collect()
}

/// The directive that covers a finding of `rule` on `line`: one with a reason when there is one.
pub(crate) fn find<'a>(directives: &'a [Directive], rule: &str, line: u32) -> Option<&'a Directive> {
    let mut covering = directives.iter().filter(|directive| {
        (directive.rules.is_empty() || directive.rules.iter().any(|name| name == rule))
            && directive
                .covers
                .is_none_or(|(first, last)| first <= line && line <= last)
    });
    let first = covering.next()?;
    if first.reason.is_some() {
        return Some(first);
    }
    Some(covering.find(|directive| directive.reason.is_some()).unwrap_or(first))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RULE: &str = "foreign-package-manager-command";

    #[test]
    fn next_line_comments_cover_the_next_code_line_and_its_continuations() {
        let source = "\
# skif-ignore-next-line foreign-package-manager-command -- fallback for 1.x

RUN apt-get update && \\
    npm ci
RUN npm ci
";
        let directives = directives(source);
        assert_eq!(
            directives,
            [Directive {
                rules: vec![RULE.to_owned()],
                reason: Some("fallback for 1.x".to_owned()),
                line: 1,
                covers: Some((3, 4)),
            }]
        );
        assert!(find(&directives, RULE, 4).is_some());
        assert!(find(&directives, RULE, 5).is_none());
        assert!(find(&directives, "toolchain-pin-drift", 4).is_none());
    }

    #[test]
    fn file_comments_rule_lists_and_missing_reasons() {
        let source = "\
#!/bin/sh
# skif-ignore-file -- kept on npm until the app moves
#   skif-ignore-next-line toolchain-pin-drift, foreign-package-manager-command
npm ci
# skif-ignore-next-lines -- not a directive
# skif-ignore-file toolchain-pin-drift --
";
        let directives = directives(source);
        assert_eq!(directives.len(), 3);
        assert_eq!(directives[0].covers, None);
        assert!(directives[0].rules.is_empty());
        assert_eq!(directives[1].rules, ["toolchain-pin-drift", RULE]);
        assert_eq!(directives[1].reason, None);
        assert_eq!(directives[1].covers, Some((4, 4)));
        assert_eq!(directives[2].reason, None, "a blank reason is no reason");
        // The whole-file comment with a reason wins over the line comment without one.
        assert_eq!(find(&directives, RULE, 4).map(|directive| directive.line), Some(2));
        assert_eq!(find(&directives[1..], RULE, 4).map(|directive| directive.line), Some(3));
    }
}
