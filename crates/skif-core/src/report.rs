//! Deterministic collection of diagnostics with summary counts.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::diagnostic::Diagnostic;
use crate::incomplete::IncompletePackage;
use crate::rule::Severity;

/// Aggregate counts over a report.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct Summary {
    /// Diagnostics with [`Severity::Error`].
    pub errors: usize,
    /// Diagnostics with [`Severity::Warning`].
    pub warnings: usize,
    /// Diagnostics with [`Severity::Info`].
    pub infos: usize,
    /// Diagnostics that carry at least one fix.
    pub fixable: usize,
    /// Number of packages that were checked.
    pub packages: usize,
}

/// The complete result of a lint run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct Report {
    /// Diagnostics sorted by path, span start, rule and message.
    pub diagnostics: Vec<Diagnostic>,
    /// Counts over `diagnostics`.
    pub summary: Summary,
    /// Packages whose entry files are not all known, so their files and dependencies were not judged unused; sorted
    /// by directory.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub incomplete_packages: Vec<IncompletePackage>,
    /// Files of other projects that the analysed code uses, absolute and sorted. The run does not judge them. Those
    /// below the workspace root belong to a project of their own (a directory below a package that holds a
    /// `package.json`, such as a nested root) and are imported, run by a script or named as an entry by a plugin; a
    /// multi-root host hands them to the root that owns them as extra entries, made relative to it (see
    /// `RunOptions::with_extra_entries` of the `skif` crate). Those outside the workspace root are every file there
    /// the analysed code reaches, directly or through other such files, or that a plugin names; a multi-root host
    /// counts them as used by the root that owns them. Not rendered.
    #[serde(skip)]
    pub other_project_files: Vec<PathBuf>,
    /// Dependencies that the analysed code uses through a `package.json` outside the workspace root (Node.js resolves
    /// them from that project's `node_modules`): the absolute directory of that manifest and the dependency name,
    /// sorted. Those the code below the root uses come from a manifest above the root that lists them when this root
    /// does not; those that files outside the root import (see `other_project_files`) from the innermost manifest at
    /// or above each file that lists them. A multi-root host credits them to the root that owns the manifest, which
    /// cannot see this code. Not rendered.
    #[serde(skip)]
    pub ancestor_dependency_uses: Vec<(PathBuf, String)>,
    /// The entries of configuration lists that `stale-js-config` judged, used or stale, sorted. A list it could not
    /// judge (a rule that consults it did not run, a package whose entry files are not all known) is not here. A
    /// multi-root host decides from them whether an entry of a file several roots read is stale for the whole run
    /// (`settle_shared_config` of the `skif` crate). Not rendered.
    #[serde(skip)]
    #[cfg_attr(feature = "schema", schemars(skip))]
    pub config_judgements: Vec<ConfigJudgement>,
}

/// An entry of a configuration list as `stale-js-config` judged it in one run (see [`Report::config_judgements`]).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub struct ConfigJudgement {
    /// The configuration file the entry is written in, as the location of the finding that reports it stale has it.
    pub file: PathBuf,
    /// The key of the list, as written in the file: `js.ignore-dependencies`, `js.workspaces."tools/*".entry`, or
    /// `js.workspaces` for the keys of that table.
    pub key: String,
    /// The entry, as written.
    pub entry: String,
    /// The directory of the package whose list it is, relative to the root like `file`, for a list a
    /// `[js.workspaces]` table gives a package (`js.workspaces."tools/*".ignore-dependencies`): a glob table is
    /// judged in each package it selects, and each package's list stands on its own. `None` for a top-level list and
    /// for a pattern or key judged over the whole run of the root.
    pub package: Option<PathBuf>,
    /// Whether the entry matched nothing in this run.
    pub stale: bool,
    /// The message of the finding that reports the entry stale, which the run emits only when [`stale`](Self::stale).
    pub message: String,
}

impl ConfigJudgement {
    /// The judgement of `entry` of the list `key` in `file`, with the message of the finding that calls it stale.
    #[must_use]
    pub fn new(
        file: impl Into<PathBuf>,
        key: impl Into<String>,
        entry: impl Into<String>,
        stale: bool,
        message: impl Into<String>,
    ) -> Self {
        Self {
            file: file.into(),
            key: key.into(),
            entry: entry.into(),
            package: None,
            stale,
            message: message.into(),
        }
    }

    /// The judgement, made of the list of the package in `dir` (see [`package`](Self::package)).
    #[must_use]
    pub fn with_package(mut self, dir: impl Into<PathBuf>) -> Self {
        self.package = Some(dir.into());
        self
    }
}

impl Report {
    /// Sorts `diagnostics` deterministically and computes the summary; `packages` is the number of packages checked.
    #[must_use]
    pub fn new(mut diagnostics: Vec<Diagnostic>, packages: usize) -> Self {
        sort(&mut diagnostics);
        let summary = diagnostics.iter().fold(
            Summary {
                packages,
                ..Summary::default()
            },
            count,
        );
        Self {
            diagnostics,
            summary,
            incomplete_packages: Vec::new(),
            other_project_files: Vec::new(),
            ancestor_dependency_uses: Vec::new(),
            config_judgements: Vec::new(),
        }
    }

    /// Sets the packages whose files and dependencies were not judged, sorted by directory then name.
    #[must_use]
    pub fn with_incomplete_packages(mut self, mut packages: Vec<IncompletePackage>) -> Self {
        packages.sort_by(|a, b| a.dir.cmp(&b.dir).then_with(|| a.name.cmp(&b.name)));
        self.incomplete_packages = packages;
        self
    }

    /// Sets the files of other projects the analysed code uses, absolute, sorted and without duplicates.
    #[must_use]
    pub fn with_other_project_files(mut self, mut files: Vec<PathBuf>) -> Self {
        files.sort();
        files.dedup();
        self.other_project_files = files;
        self
    }

    /// Sets the dependencies the analysed code uses through manifests outside the workspace root, sorted and without
    /// duplicates.
    #[must_use]
    pub fn with_ancestor_dependency_uses(mut self, mut uses: Vec<(PathBuf, String)>) -> Self {
        uses.sort();
        uses.dedup();
        self.ancestor_dependency_uses = uses;
        self
    }

    /// Sets the configuration entries `stale-js-config` judged, sorted and without duplicates.
    #[must_use]
    pub fn with_config_judgements(mut self, mut judgements: Vec<ConfigJudgement>) -> Self {
        judgements.sort();
        judgements.dedup();
        self.config_judgements = judgements;
        self
    }

    /// Keeps only the diagnostics `keep` accepts, in their order, and counts the summary again; the number of
    /// packages and everything else stay.
    #[must_use]
    pub fn retain(mut self, keep: impl FnMut(&Diagnostic) -> bool) -> Self {
        self.diagnostics.retain(keep);
        self.recount();
        self
    }

    /// Keeps only the diagnostics `keep` accepts, which it may change in place (without moving them in the order),
    /// and counts the summary again; the number of packages and everything else stay.
    #[must_use]
    pub fn retain_mut(mut self, keep: impl FnMut(&mut Diagnostic) -> bool) -> Self {
        self.diagnostics.retain_mut(keep);
        self.recount();
        self
    }

    fn recount(&mut self) {
        self.summary = self.diagnostics.iter().fold(
            Summary {
                packages: self.summary.packages,
                ..Summary::default()
            },
            count,
        );
    }
}

/// Sorts `diagnostics` stably by path, span start, rule and message, exactly as a stable sort by [`compare`] would.
///
/// A report can hold tens of thousands of diagnostics of a few hundred bytes each, about a few thousand files. So the
/// order is worked out on indices, each distinct path is ranked once instead of being compared at every step, and the
/// diagnostics are then moved into place by swaps: sorting the diagnostics themselves moved each several times and
/// needed a scratch copy of the whole vector.
fn sort(diagnostics: &mut [Diagnostic]) {
    // Neighbours mostly share a file: each run of one path is ranked once.
    let mut runs: Vec<&Path> = Vec::new();
    let mut run_of: Vec<usize> = Vec::with_capacity(diagnostics.len());
    for diagnostic in diagnostics.iter() {
        let path = diagnostic.location.path.as_path();
        if runs.last().is_none_or(|last| last.as_os_str() != path.as_os_str()) {
            runs.push(path);
        }
        run_of.push(runs.len() - 1);
    }
    let mut by_path: Vec<usize> = (0..runs.len()).collect();
    if runs.iter().all(|path| is_plain(path)) {
        by_path.sort_unstable_by(|&a, &b| plain_cmp(runs[a], runs[b]));
    } else {
        by_path.sort_unstable_by(|&a, &b| runs[a].cmp(runs[b]));
    }
    // Equal paths share a rank: `Path` equality is by component, as its order is.
    let mut rank_of_run = vec![0_u32; runs.len()];
    let mut rank = 0_u32;
    for (index, &run) in by_path.iter().enumerate() {
        if index > 0 && runs[by_path[index - 1]] != runs[run] {
            rank += 1;
        }
        rank_of_run[run] = rank;
    }
    let keys: Vec<(u32, u32)> = diagnostics
        .iter()
        .zip(&run_of)
        .map(|(diagnostic, &run)| (rank_of_run[run], start(diagnostic)))
        .collect();
    let mut order: Vec<usize> = (0..diagnostics.len()).collect();
    order.sort_by(|&a, &b| {
        keys[a].cmp(&keys[b]).then_with(|| {
            let (a, b) = (&diagnostics[a], &diagnostics[b]);
            a.rule.cmp(&b.rule).then_with(|| a.message.cmp(&b.message))
        })
    });
    // Position `k` takes the diagnostic at `order[k]`; each cycle of the permutation is walked once.
    for first in 0..order.len() {
        let mut current = first;
        while order[current] != usize::MAX {
            let next = std::mem::replace(&mut order[current], usize::MAX);
            if next == first {
                break;
            }
            diagnostics.swap(current, next);
            current = next;
        }
    }
}

/// Whether `path` is spelled so that [`plain_cmp`] orders it as `Path` does: on a platform whose only separator is
/// `/`, without an empty (`a//b`, `a/`), `.` or `..` component, so its components are exactly its `/`-separated parts.
fn is_plain(path: &Path) -> bool {
    !cfg!(windows)
        && path
            .as_os_str()
            .as_encoded_bytes()
            .split(|&byte| byte == b'/')
            .enumerate()
            .all(|(index, part)| (index == 0 || !part.is_empty()) && part != b"." && part != b"..")
}

/// The order of two [plain](is_plain) paths by component, from their bytes: up to the first difference they share
/// their components, and there the one whose component ends (a `/`, or the end of the path) is the smaller.
fn plain_cmp(a: &Path, b: &Path) -> Ordering {
    let (a, b) = (a.as_os_str().as_encoded_bytes(), b.as_os_str().as_encoded_bytes());
    let key = |byte: u8| if byte == b'/' { 0 } else { u16::from(byte) + 1 };
    match a.iter().zip(b).position(|(x, y)| x != y) {
        Some(index) => key(a[index]).cmp(&key(b[index])),
        None => a.len().cmp(&b.len()),
    }
}

fn start(diagnostic: &Diagnostic) -> u32 {
    diagnostic.location.span.map_or(0, |span| span.start)
}

/// The order of a report, which [`sort`] computes faster; kept as the reference the tests hold it to.
#[cfg(test)]
fn compare(a: &Diagnostic, b: &Diagnostic) -> Ordering {
    a.location
        .path
        .cmp(&b.location.path)
        .then_with(|| start(a).cmp(&start(b)))
        .then_with(|| a.rule.cmp(&b.rule))
        .then_with(|| a.message.cmp(&b.message))
}

fn count(mut summary: Summary, diagnostic: &Diagnostic) -> Summary {
    match diagnostic.severity {
        Severity::Error => summary.errors += 1,
        Severity::Warning => summary.warnings += 1,
        Severity::Info => summary.infos += 1,
    }
    summary.fixable += usize::from(diagnostic.is_fixable());
    summary
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::fix::{Fix, FixOp, FixSafety};
    use crate::rule::RuleId;
    use crate::span::{Location, Span};

    fn diag(rule: &'static str, severity: Severity, path: &str, start: Option<u32>, fixable: bool) -> Diagnostic {
        let span = start.map(|s| Span {
            start: s,
            end: s + 1,
            start_line: 1,
            start_col: s + 1,
            end_line: 1,
            end_col: s + 2,
        });
        let fixes = if fixable {
            vec![Fix {
                title: "f".into(),
                safety: FixSafety::Safe,
                ops: vec![FixOp::DeleteFile { path: path.into() }],
                preference: None,
            }]
        } else {
            Vec::new()
        };
        Diagnostic {
            rule: RuleId::from_static(rule),
            severity,
            message: rule.to_owned(),
            help: None,
            location: Location {
                path: path.into(),
                span,
            },
            related: Vec::new(),
            package: None,
            package_id: None,
            dependency: None,
            specifier: None,
            default_severity: None,
            detail: None,
            fixes,
        }
    }

    fn order(report: &Report) -> Vec<(String, u32, String)> {
        report
            .diagnostics
            .iter()
            .map(|d| (d.location.path.display().to_string(), start(d), d.rule.to_string()))
            .collect()
    }

    fn input() -> Vec<Diagnostic> {
        vec![
            diag("b-rule", Severity::Warning, "z/package.json", Some(4), true),
            diag("a-rule", Severity::Error, "a/package.json", Some(9), false),
            diag("c-rule", Severity::Info, "a/package.json", Some(2), false),
            diag("b-rule", Severity::Error, "a/package.json", Some(9), true),
            diag("a-rule", Severity::Error, "a/package.json", None, false),
        ]
    }

    #[test]
    fn sorts_by_path_start_then_rule() {
        let expected = [
            ("a/package.json", 0, "a-rule"),
            ("a/package.json", 2, "c-rule"),
            ("a/package.json", 9, "a-rule"),
            ("a/package.json", 9, "b-rule"),
            ("z/package.json", 4, "b-rule"),
        ]
        .map(|(p, s, r)| (p.to_owned(), s, r.to_owned()));
        assert_eq!(order(&Report::new(input(), 3)), expected);
    }

    #[test]
    fn order_is_independent_of_input_order() {
        let forward = Report::new(input(), 3);
        let mut reversed = input();
        reversed.reverse();
        assert_eq!(Report::new(reversed, 3), forward);
    }

    #[test]
    fn sorts_as_a_stable_sort_by_compare() {
        // Paths that order differently by component than by bytes (`a-b`, `a.b` and `a/b`, `/` and relative), and a
        // second set with spellings of one path (`a//b`, `./a`) and `..`, which take the general comparison. Missing
        // spans and diagnostics equal in every sort key, told apart by their help, in a scrambled order.
        let plain = [
            "a/b/package.json",
            "a-b/package.json",
            "a.b/package.json",
            "a/package.json",
            "/a/package.json",
            "package.json",
            "a/b",
        ];
        let spelled = [
            "a/b/package.json",
            "a//b/package.json",
            "./a/package.json",
            "../a/package.json",
            "-a/x",
        ];
        for paths in [&plain[..], &spelled[..]] {
            let rules = ["a-rule", "b-rule"];
            let mut state: u32 = 7;
            let mut next = |bound: usize| {
                state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                usize::try_from(state >> 16).unwrap() % bound
            };
            let input: Vec<Diagnostic> = (0..400)
                .map(|index| {
                    let start = [None, Some(0), Some(3), Some(9)][next(4)];
                    let path = paths[next(paths.len())];
                    let mut diagnostic = diag(rules[next(2)], Severity::Error, path, start, false);
                    diagnostic.help = Some(index.to_string());
                    diagnostic
                })
                .collect();
            let mut expected = input.clone();
            expected.sort_by(compare);
            assert_eq!(Report::new(input, 1).diagnostics, expected, "{paths:?}");
        }
    }

    #[test]
    fn plain_paths_compare_by_component() {
        let paths = [
            "", "/a", "a", "a-b", "a.b", "a/b", "a/b/c", "a/bc", "ab", "b", "a/b-c", "a/b/-",
        ];
        if cfg!(windows) {
            // `\` is a separator too, so every path takes the `Path` order there.
            assert!(paths.iter().all(|path| !is_plain(Path::new(path))));
            return;
        }
        for a in paths {
            for b in paths {
                let (a, b) = (Path::new(a), Path::new(b));
                assert!(is_plain(a) && is_plain(b), "{a:?} {b:?}");
                assert_eq!(plain_cmp(a, b), a.cmp(b), "{a:?} {b:?}");
            }
        }
        for path in ["/", "a//b", "a/", "./a", "a/./b", "a/../b", ".."] {
            assert!(!is_plain(Path::new(path)), "{path}");
        }
    }

    #[test]
    fn counts_summary() {
        assert_eq!(
            Report::new(input(), 3).summary,
            Summary {
                errors: 3,
                warnings: 1,
                infos: 1,
                fixable: 2,
                packages: 3
            }
        );
    }

    #[test]
    fn empty_report_has_zero_counts() {
        let report = Report::new(Vec::new(), 7);
        assert_eq!(report.diagnostics, Vec::new());
        assert_eq!(
            report.summary,
            Summary {
                packages: 7,
                ..Summary::default()
            }
        );
    }

    #[test]
    fn retain_counts_the_summary_of_the_kept_diagnostics_again() {
        let report = Report::new(input(), 3)
            .with_other_project_files(vec![PathBuf::from("/ws/build/x.ts")])
            .retain(|diagnostic| diagnostic.rule.as_str() != "b-rule");
        let kept: Vec<&str> = report.diagnostics.iter().map(|d| d.rule.as_str()).collect();
        assert_eq!(kept, ["a-rule", "c-rule", "a-rule"]);
        assert_eq!(
            report.summary,
            Summary {
                errors: 2,
                warnings: 0,
                infos: 1,
                fixable: 0,
                packages: 3
            }
        );
        assert_eq!(report.other_project_files, [PathBuf::from("/ws/build/x.ts")]);
    }

    #[test]
    fn retain_mut_counts_the_summary_of_the_changed_diagnostics_again() {
        let report = Report::new(input(), 3).retain_mut(|diagnostic| {
            diagnostic.severity = Severity::Info;
            diagnostic.rule.as_str() != "a-rule"
        });
        let kept: Vec<(&str, Severity)> = report
            .diagnostics
            .iter()
            .map(|d| (d.rule.as_str(), d.severity))
            .collect();
        assert_eq!(kept, [("c-rule", Severity::Info), ("b-rule", Severity::Info), ("b-rule", Severity::Info)]);
        assert_eq!(
            report.summary,
            Summary {
                errors: 0,
                warnings: 0,
                infos: 3,
                fixable: 2,
                packages: 3
            }
        );
    }

    #[test]
    fn summary_serializes_kebab_case() {
        let summary = Summary {
            errors: 1,
            warnings: 2,
            infos: 3,
            fixable: 4,
            packages: 5,
        };
        assert_eq!(
            serde_json::to_value(summary).unwrap(),
            serde_json::json!({ "errors": 1, "warnings": 2, "infos": 3, "fixable": 4, "packages": 5 })
        );
    }
    #[test]
    fn incomplete_packages_sort_by_directory_and_serialize_kebab_case() {
        use crate::incomplete::{IncompleteCause, IncompleteEntries};

        let reason = |cause, plugin: Option<&str>, file: Option<&str>| IncompleteEntries {
            cause,
            plugin: plugin.map(str::to_owned),
            file: file.map(std::path::PathBuf::from),
        };
        let report = Report::new(Vec::new(), 2).with_incomplete_packages(vec![
            IncompletePackage::new(
                "web".into(),
                "packages/web".into(),
                vec![reason(
                    IncompleteCause::UnknownSpread,
                    Some("vite"),
                    Some("packages/web/vite.config.ts"),
                )],
            ),
            IncompletePackage::new(
                "api".into(),
                "packages/api".into(),
                vec![reason(IncompleteCause::Unspecified, None, None)],
            ),
        ]);
        assert_eq!(
            serde_json::to_value(&report).unwrap()["incomplete-packages"],
            serde_json::json!([
                { "name": "api", "dir": "packages/api", "reasons": [{ "cause": "unspecified" }], "scope": "package" },
                { "name": "web", "dir": "packages/web", "reasons": [
                    { "cause": "unknown-spread", "plugin": "vite", "file": "packages/web/vite.config.ts" }
                ], "scope": "package" },
            ])
        );
        let empty = serde_json::to_value(Report::new(Vec::new(), 1)).unwrap();
        assert!(empty.get("incomplete-packages").is_none());
    }
}
