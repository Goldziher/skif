//! Reading skif's JSON report from a captured run and telling how two reports differ: the top-level sections whose
//! values differ and the findings only one side has.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use serde_json::Value;

use crate::proc::Captured;

/// Findings listed per side before the rest is counted.
const LIST_CAP: usize = 20;

/// A skif run's report and exit code, or why it has none.
pub type Report = Result<(Value, Option<i32>), String>;

/// The JSON report and exit code of a captured skif run, or why it has none: it timed out, printed no JSON or printed
/// an error document.
pub fn read_report(run: &Captured) -> Report {
    if run.timed_out {
        return Err("timed out".to_owned());
    }
    let document: Value = match serde_json::from_slice(&run.stdout) {
        Ok(document) => document,
        Err(error) => return Err(format!("output is not JSON ({error}); {}", run.stderr_summary())),
    };
    if document.get("diagnostics").is_none() {
        let message = document["error"]["message"]
            .as_str()
            .unwrap_or("no diagnostics in the output");
        return Err(format!("failed: {message}"));
    }
    Ok((document, run.code))
}

/// A finding of one side's `diagnostics`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub rule: String,
    pub file: String,
    pub line: Option<u64>,
    pub message: String,
}

impl Finding {
    fn label(&self) -> String {
        let place = self
            .line
            .map_or_else(|| self.file.clone(), |line| format!("{}:{line}", self.file));
        format!("{} {place} {}", self.rule, self.message)
    }
}

/// How two JSON reports differ.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Diff {
    /// The top-level keys whose values differ.
    pub sections: BTreeSet<String>,
    /// Findings the second report has and the first does not.
    pub added: Vec<Finding>,
    /// Findings the first report has and the second does not.
    pub removed: Vec<Finding>,
}

impl Diff {
    pub fn is_empty(&self) -> bool {
        self.sections.is_empty()
    }
}

fn finding(diagnostic: &Value) -> Finding {
    let text = |pointer: &str| {
        diagnostic
            .pointer(pointer)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    Finding {
        rule: text("/rule"),
        file: text("/location/path"),
        line: diagnostic.pointer("/location/span/start-line").and_then(Value::as_u64),
        message: text("/message"),
    }
}

/// The diagnostics as a multiset keyed by their whole JSON, so a change to any field (severity, fixes, detail) is a
/// removed finding and an added one.
fn diagnostic_counts(document: &Value) -> BTreeMap<String, (usize, Finding)> {
    let mut counts = BTreeMap::new();
    for diagnostic in document
        .get("diagnostics")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        counts
            .entry(diagnostic.to_string())
            .or_insert_with(|| (0, finding(diagnostic)))
            .0 += 1;
    }
    counts
}

fn surplus(of: &BTreeMap<String, (usize, Finding)>, against: &BTreeMap<String, (usize, Finding)>) -> Vec<Finding> {
    let mut found = Vec::new();
    for (key, (count, finding)) in of {
        let other = against.get(key).map_or(0, |entry| entry.0);
        found.extend(std::iter::repeat_n(finding.clone(), count.saturating_sub(other)));
    }
    found.sort_by(|a, b| (&a.rule, &a.file, a.line, &a.message).cmp(&(&b.rule, &b.file, b.line, &b.message)));
    found
}

/// Compares two JSON reports, leaving the top-level keys in `ignored` out.
pub fn diff_documents(first: &Value, second: &Value, ignored: &[&str]) -> Diff {
    let keys: BTreeSet<&String> = first
        .as_object()
        .into_iter()
        .chain(second.as_object())
        .flat_map(serde_json::Map::keys)
        .filter(|key| !ignored.contains(&key.as_str()))
        .collect();
    let sections = keys
        .into_iter()
        .filter(|key| first.get(key.as_str()) != second.get(key.as_str()))
        .cloned()
        .collect();
    let (before, after) = (diagnostic_counts(first), diagnostic_counts(second));
    Diff {
        sections,
        added: surplus(&after, &before),
        removed: surplus(&before, &after),
    }
}

fn list(out: &mut String, prefix: &str, items: &[Finding]) {
    for item in items.iter().take(LIST_CAP) {
        let _ = writeln!(out, "    {prefix} {}", item.label());
    }
    if items.len() > LIST_CAP {
        let _ = writeln!(out, "    {prefix} ... and {} more", items.len() - LIST_CAP);
    }
}

/// Appends the differing sections and the findings only one side has (`+` the second report, `-` the first).
pub fn render_diff(out: &mut String, diff: &Diff) {
    let sections: Vec<&str> = diff.sections.iter().map(String::as_str).collect();
    if !sections.is_empty() {
        let _ = writeln!(out, "    sections: {}", sections.join(", "));
    }
    list(out, "+", &diff.added);
    list(out, "-", &diff.removed);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::time::Duration;

    use serde_json::json;

    use super::*;

    fn captured(stdout: &str, timed_out: bool) -> Captured {
        Captured {
            code: Some(1),
            timed_out,
            stdout: stdout.as_bytes().to_vec(),
            stderr: "boom\n".to_owned(),
            elapsed: Duration::ZERO,
        }
    }

    #[test]
    fn a_run_without_a_report_says_why() {
        assert_eq!(read_report(&captured("{}", true)).unwrap_err(), "timed out");
        assert!(read_report(&captured("oops", false)).unwrap_err().contains("not JSON"));
        let error = r#"{"error": {"message": "no workspace"}}"#;
        assert_eq!(
            read_report(&captured(error, false)).unwrap_err(),
            "failed: no workspace"
        );
        let (document, code) = read_report(&captured(r#"{"diagnostics": []}"#, false)).unwrap();
        assert_eq!((document, code), (json!({"diagnostics": []}), Some(1)));
    }

    #[test]
    fn a_diff_names_sections_and_one_sided_findings_and_skips_ignored_keys() {
        let diagnostic = |file: &str| json!({"rule": "unused-file", "message": "m", "location": {"path": file}});
        let first = json!({"skif": "1", "summary": {"n": 2}, "diagnostics": [diagnostic("a.ts"), diagnostic("a.ts")]});
        let second = json!({"skif": "2", "summary": {"n": 2}, "diagnostics": [diagnostic("a.ts"), diagnostic("b.ts")]});
        assert!(diff_documents(&first, &first, &[]).is_empty());
        let diff = diff_documents(&first, &second, &["skif"]);
        assert_eq!(diff.sections.iter().collect::<Vec<_>>(), ["diagnostics"]);
        assert_eq!(
            (diff.added[0].file.as_str(), diff.removed[0].file.as_str()),
            ("b.ts", "a.ts")
        );
        assert_eq!((diff.added.len(), diff.removed.len()), (1, 1));
        assert!(diff_documents(&first, &second, &[]).sections.contains("skif"));
        let mut text = String::new();
        render_diff(&mut text, &diff);
        assert_eq!(
            text,
            "    sections: diagnostics\n    + unused-file b.ts m\n    - unused-file a.ts m\n"
        );
    }
}
