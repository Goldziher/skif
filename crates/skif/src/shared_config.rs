//! Deciding the `stale-js-config` findings about a configuration file several roots of a multi-root run read, over
//! the final reports of those roots (ADR 0004).

use std::collections::{BTreeMap, BTreeSet};

use skif_core::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use skif_core::{ConfigJudgement, Diagnostic, Report, Severity};

use crate::path::join_lexical;

/// The rule whose findings are decided across roots.
const STALE_JS_CONFIG: &str = "stale-js-config";

/// Decides the `stale-js-config` findings about configuration files several roots read, across the reports of those
/// roots, so a multi-root run reports what a single run over all of them would.
///
/// Each item is a root directory and its report, with paths relative to it, as [`Linter::run`](crate::Linter::run)
/// returns it; the roots come in path order, as [`discover_targets`](crate::discover_targets) lists them. Call it on
/// the reports the run shows, last: after the lint that follows fixing (of every root whose configuration a fix
/// wrote, not only the roots it changed), and after narrowing a report to the findings the caller selects. A report it
/// decided keeps every judgement, so deciding again changes nothing.
///
/// An entry of a configuration list that two or more roots judged ([`Report::config_judgements`]) is grouped by the
/// absolute file, the key as written, the entry and, for a list a `[js.workspaces]` table gives a package, the
/// absolute directory of that package. It is stale when every root that judged it found it stale: a root that used it
/// takes the finding out of every report, and a root that could not judge it has no say. A stale entry is reported
/// once, by the first root whose report still holds the finding, at the highest severity among the copies; the other
/// copies are taken out. Any other `stale-js-config` finding at the same absolute file with the same message (a
/// redundant pattern, a tag hint) is likewise kept by the first root that reports it, at the highest severity.
///
/// A report's summary is counted again when its findings change; everything else stays.
pub fn settle_shared_config<'a>(reports: impl IntoIterator<Item = (&'a Path, &'a mut Report)>) {
    let mut roots: Vec<(&Path, &mut Report)> = reports.into_iter().collect();
    let decisions = {
        let views: Vec<(&Path, &Report)> = roots.iter().map(|(root, report)| (*root, &**report)).collect();
        decide(&views)
    };
    for ((_, report), decision) in roots.iter_mut().zip(decisions) {
        if decision.is_empty() {
            continue;
        }
        let taken = std::mem::replace(&mut **report, Report::new(Vec::new(), 0));
        **report = taken.retain_mut(|diagnostic| decision.apply(diagnostic));
    }
}

/// What the decision does to a `stale-js-config` finding of a root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    /// Another root reports it, or a root used the entry it calls stale.
    Drop,
    /// The root reports it for the run, at this severity, which another root's copy has.
    Raise(Severity),
}

/// The actions on the `stale-js-config` findings of one root, by location path and message.
#[derive(Debug, Default)]
struct Decision(HashMap<PathBuf, HashMap<String, Action>>);

impl Decision {
    fn set(&mut self, path: &Path, message: &str, action: Action) {
        self.0
            .entry(path.to_path_buf())
            .or_default()
            .insert(message.to_owned(), action);
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Applies the action on `diagnostic`, if any; whether it stays in the report.
    fn apply(&self, diagnostic: &mut Diagnostic) -> bool {
        if diagnostic.rule.as_str() != STALE_JS_CONFIG {
            return true;
        }
        let action = self
            .0
            .get(diagnostic.location.path.as_path())
            .and_then(|messages| messages.get(diagnostic.message.as_str()));
        match action {
            Some(Action::Drop) => false,
            Some(Action::Raise(severity)) => {
                diagnostic.severity = *severity;
                true
            }
            None => true,
        }
    }
}

/// The `stale-js-config` findings of one report, by location path and message, with the highest severity among them.
type Copies<'r> = HashMap<(&'r Path, &'r str), Severity>;

fn copies_of(report: &Report) -> Copies<'_> {
    let mut copies: Copies<'_> = HashMap::default();
    for diagnostic in stale_findings(report) {
        let severity = copies
            .entry((diagnostic.location.path.as_path(), diagnostic.message.as_str()))
            .or_insert(diagnostic.severity);
        *severity = (*severity).max(diagnostic.severity);
    }
    copies
}

fn stale_findings(report: &Report) -> impl Iterator<Item = &Diagnostic> {
    report
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.rule.as_str() == STALE_JS_CONFIG)
}

/// One copy of a finding: the root that reports it, its location path and message there, and its severity.
type Found<'r> = (usize, &'r Path, &'r str, Severity);

/// The decision of each root of `roots`, in order.
fn decide(roots: &[(&Path, &Report)]) -> Vec<Decision> {
    let copies: Vec<Copies<'_>> = roots.iter().map(|(_, report)| copies_of(report)).collect();
    let mut decisions: Vec<Decision> = roots.iter().map(|_| Decision::default()).collect();
    for judgements in judged_entries(roots).values() {
        decide_entry(judgements, &copies, &mut decisions);
    }
    for found in other_findings(roots, &copies).values() {
        keep_first(found, &mut decisions);
    }
    decisions
}

/// An entry of a configuration list as several roots may judge it: the absolute file, the key as written, the entry
/// and the absolute directory of the package whose list it is.
type EntryKey<'r> = (PathBuf, &'r str, &'r str, Option<PathBuf>);

/// The judgements of every root, grouped by the entry they judge; each group in the order of the roots.
fn judged_entries<'r>(roots: &[(&Path, &'r Report)]) -> BTreeMap<EntryKey<'r>, Vec<(usize, &'r ConfigJudgement)>> {
    let mut entries: BTreeMap<EntryKey<'r>, Vec<(usize, &'r ConfigJudgement)>> = BTreeMap::new();
    for (index, (root, report)) in roots.iter().enumerate() {
        for judgement in &report.config_judgements {
            let key = (
                join_lexical(root, &judgement.file),
                judgement.key.as_str(),
                judgement.entry.as_str(),
                judgement.package.as_deref().map(|dir| join_lexical(root, dir)),
            );
            entries.entry(key).or_default().push((index, judgement));
        }
    }
    entries
}

/// Decides one entry that two or more roots judged: dropped everywhere when a root used it, else kept by the first
/// root that reports it, at the highest severity of the copies.
fn decide_entry<'r>(judgements: &[(usize, &'r ConfigJudgement)], copies: &[Copies<'r>], decisions: &mut [Decision]) {
    let judging: BTreeSet<usize> = judgements.iter().map(|(index, _)| *index).collect();
    if judging.len() < 2 {
        return;
    }
    let reported: Vec<Found<'r>> = judgements
        .iter()
        .filter(|(_, judgement)| judgement.stale)
        .filter_map(|&(index, judgement)| {
            let (file, message) = (judgement.file.as_path(), judgement.message.as_str());
            copies[index]
                .get(&(file, message))
                .map(|severity| (index, file, message, *severity))
        })
        .collect();
    if judgements.iter().any(|(_, judgement)| !judgement.stale) {
        for &(index, file, message, _) in &reported {
            decisions[index].set(file, message, Action::Drop);
        }
        return;
    }
    keep_first(&reported, decisions);
}

/// Keeps the first of the copies of one finding, at the highest severity among them, and drops the others.
fn keep_first(found: &[Found<'_>], decisions: &mut [Decision]) {
    let Some(&(keeper, file, message, severity)) = found.first() else {
        return;
    };
    let highest = found.iter().map(|copy| copy.3).max().unwrap_or(severity);
    if highest > severity {
        decisions[keeper].set(file, message, Action::Raise(highest));
    }
    for &(index, file, message, _) in &found[1..] {
        if index != keeper {
            decisions[index].set(file, message, Action::Drop);
        }
    }
}

/// The `stale-js-config` findings no judgement of their root accounts for, grouped by absolute file and message; each
/// group holds one copy per root, in the order of the roots.
fn other_findings<'r>(
    roots: &[(&Path, &'r Report)],
    copies: &[Copies<'r>],
) -> BTreeMap<(PathBuf, &'r str), Vec<Found<'r>>> {
    let mut found: BTreeMap<(PathBuf, &'r str), Vec<Found<'r>>> = BTreeMap::new();
    for (index, ((root, report), copies)) in roots.iter().zip(copies).enumerate() {
        let judged: HashSet<(&Path, &str)> = report
            .config_judgements
            .iter()
            .map(|judgement| (judgement.file.as_path(), judgement.message.as_str()))
            .collect();
        for (&(path, message), &severity) in copies {
            if judged.contains(&(path, message)) {
                continue;
            }
            found
                .entry((join_lexical(root, path), message))
                .or_default()
                .push((index, path, message, severity));
        }
    }
    found
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use skif_core::{Location, RuleId};

    use super::*;

    const OUTER: &str = "../etc/skif.toml";
    const INNER: &str = "../../../etc/skif.toml";
    const ROOTS: [&str; 3] = ["/r", "/r/tools/a", "/r/tools/b"];
    const ENTRY_KEY: &str = "js.workspaces.\"tools/*\".entry";
    const ENTRY: &str = "`src/extra.js` in `js.workspaces.\"tools/*\".entry` matched nothing";
    const DEPENDENCIES: &str = "js.ignore-dependencies";
    const DEPENDENCY: &str = "`left-pad` in `js.ignore-dependencies` matched nothing";
    const TABLE_KEY: &str = "js.workspaces.\"tools/*\".ignore-dependencies";
    const HINT: &str = "`src/main.js` in `js.entry` is already an entry of a plugin, `package.json` or a script";

    fn finding(path: &str, message: &str, severity: Severity) -> Diagnostic {
        Diagnostic::new(
            RuleId::from_static(STALE_JS_CONFIG),
            severity,
            message,
            Location::new(path, None),
        )
    }

    /// A report with the warnings `reported` and the judgements `judged`, as `(file, key, entry, stale, message)`.
    fn report(reported: &[(&str, &str)], judged: &[(&str, &str, &str, bool, &str)]) -> Report {
        let diagnostics = reported
            .iter()
            .map(|(path, message)| finding(path, message, Severity::Warning))
            .collect();
        let judgements = judged
            .iter()
            .map(|&(file, key, entry, is_stale, message)| ConfigJudgement::new(file, key, entry, is_stale, message))
            .collect();
        Report::new(diagnostics, 1).with_config_judgements(judgements)
    }

    /// Settles `reports`, linted at [`ROOTS`] in order, and returns the findings each keeps as `(path, message)`.
    fn settled(mut reports: Vec<Report>) -> Vec<Vec<(String, String)>> {
        settle_shared_config(ROOTS.map(Path::new).into_iter().zip(reports.iter_mut()));
        reports.iter().map(kept).collect()
    }

    fn kept(report: &Report) -> Vec<(String, String)> {
        report
            .diagnostics
            .iter()
            .map(|diagnostic| (skif_core::slash(&diagnostic.location.path), diagnostic.message.clone()))
            .collect()
    }

    fn one(path: &str, message: &str) -> Vec<(String, String)> {
        vec![(path.to_owned(), message.to_owned())]
    }

    fn none() -> Vec<(String, String)> {
        Vec::new()
    }

    fn stale_dependency(path: &str) -> (&str, &str, &str, bool, &str) {
        (path, DEPENDENCIES, "left-pad", true, DEPENDENCY)
    }

    #[test]
    fn an_entry_of_a_glob_table_one_root_uses_is_stale_nowhere() {
        let reports = vec![
            report(&[], &[]),
            report(&[], &[(INNER, ENTRY_KEY, "src/extra.js", false, ENTRY)]),
            report(&[(INNER, ENTRY)], &[(INNER, ENTRY_KEY, "src/extra.js", true, ENTRY)]),
        ];
        assert_eq!(settled(reports), [none(), none(), none()]);
    }

    #[test]
    fn a_root_that_could_not_judge_an_entry_does_not_hide_it() {
        let reports = vec![
            report(&[(OUTER, DEPENDENCY)], &[stale_dependency(OUTER)]),
            report(&[], &[]),
            report(&[], &[]),
        ];
        assert_eq!(settled(reports), [one(OUTER, DEPENDENCY), none(), none()]);
    }

    #[test]
    fn an_entry_stale_in_every_root_that_judged_it_is_reported_by_the_first() {
        let reports = vec![
            report(&[(OUTER, DEPENDENCY)], &[stale_dependency(OUTER)]),
            report(&[(INNER, DEPENDENCY)], &[stale_dependency(INNER)]),
            report(&[(INNER, DEPENDENCY)], &[stale_dependency(INNER)]),
        ];
        assert_eq!(settled(reports), [one(OUTER, DEPENDENCY), none(), none()]);
    }

    #[test]
    fn a_stale_entry_is_kept_by_the_first_root_whose_report_still_holds_it() {
        // The outer root judged the entry stale but its report lost the finding (an `[[ignore]]` entry or a filter).
        let reports = vec![
            report(&[], &[stale_dependency(OUTER)]),
            report(&[(INNER, DEPENDENCY)], &[stale_dependency(INNER)]),
            report(&[(INNER, DEPENDENCY)], &[stale_dependency(INNER)]),
        ];
        assert_eq!(settled(reports), [none(), one(INNER, DEPENDENCY), none()]);
    }

    #[test]
    fn a_stale_entry_no_report_holds_any_more_is_not_brought_back() {
        let reports = vec![
            report(&[], &[stale_dependency(OUTER)]),
            report(&[], &[stale_dependency(INNER)]),
            report(&[], &[]),
        ];
        assert_eq!(settled(reports), [none(), none(), none()]);
    }

    #[test]
    fn the_kept_copy_takes_the_highest_severity_and_the_summary_counts_it() {
        let mut reports = [
            report(&[(OUTER, DEPENDENCY)], &[stale_dependency(OUTER)]),
            Report::new(vec![finding(INNER, DEPENDENCY, Severity::Error)], 1).with_config_judgements(vec![
                ConfigJudgement::new(INNER, DEPENDENCIES, "left-pad", true, DEPENDENCY),
            ]),
        ];
        settle_shared_config(ROOTS.map(Path::new).into_iter().zip(reports.iter_mut()));
        let severities: Vec<Vec<Severity>> = reports
            .iter()
            .map(|report| report.diagnostics.iter().map(|d| d.severity).collect())
            .collect();
        assert_eq!(severities, [vec![Severity::Error], vec![]]);
        assert_eq!((reports[0].summary.errors, reports[0].summary.warnings), (1, 0));
        assert_eq!((reports[1].summary.errors, reports[1].summary.warnings), (0, 0));
    }

    #[test]
    fn each_package_of_a_glob_table_has_its_own_list() {
        // `tools/a` uses its `left-pad` entry, `tools/b` does not: the shared table line is stale for `tools/b`.
        let message_b = "`left-pad` in `js.workspaces.\"tools/b\".ignore-dependencies` matched nothing";
        let message_a = "`left-pad` in `js.workspaces.\"tools/a\".ignore-dependencies` matched nothing";
        let judged =
            |is_stale, message| ConfigJudgement::new(INNER, TABLE_KEY, "left-pad", is_stale, message).with_package("");
        let mut reports = [
            report(&[], &[]),
            Report::new(Vec::new(), 1).with_config_judgements(vec![judged(false, message_a)]),
            Report::new(vec![finding(INNER, message_b, Severity::Warning)], 1)
                .with_config_judgements(vec![judged(true, message_b)]),
        ];
        settle_shared_config(ROOTS.map(Path::new).into_iter().zip(reports.iter_mut()));
        assert_eq!(
            reports.iter().map(kept).collect::<Vec<_>>(),
            [none(), none(), one(INNER, message_b)]
        );
    }

    #[test]
    fn the_same_package_judged_by_two_roots_is_one_list() {
        let message = "`left-pad` in `js.workspaces.\"tools/a\".ignore-dependencies` matched nothing";
        let judged = |file, dir, is_stale| {
            ConfigJudgement::new(file, TABLE_KEY, "left-pad", is_stale, message).with_package(dir)
        };
        let mut reports = [
            Report::new(Vec::new(), 1).with_config_judgements(vec![judged(OUTER, "tools/a", false)]),
            Report::new(vec![finding(INNER, message, Severity::Warning)], 1)
                .with_config_judgements(vec![judged(INNER, "", true)]),
        ];
        settle_shared_config(ROOTS.map(Path::new).into_iter().zip(reports.iter_mut()));
        assert_eq!(reports.iter().map(kept).collect::<Vec<_>>(), [none(), none()]);
    }

    #[test]
    fn other_findings_at_a_shared_file_are_reported_by_the_first_root_alone() {
        let reports = vec![
            report(&[(OUTER, HINT), (OUTER, HINT)], &[]),
            report(&[(INNER, HINT)], &[]),
            report(&[("package.json", HINT)], &[]),
        ];
        assert_eq!(
            settled(reports),
            [
                vec![(OUTER.to_owned(), HINT.to_owned()), (OUTER.to_owned(), HINT.to_owned())],
                none(),
                one("package.json", HINT)
            ]
        );
    }

    #[test]
    fn settling_reads_only_the_reports_it_is_given() {
        // After a fix rewrote the shared file, both roots are linted again and neither judges `left-pad`: nothing an
        // earlier decision took out of a report comes back.
        let first = settled(vec![
            report(&[(OUTER, DEPENDENCY)], &[stale_dependency(OUTER)]),
            report(&[(INNER, DEPENDENCY)], &[stale_dependency(INNER)]),
            report(&[], &[]),
        ]);
        assert_eq!(first, [one(OUTER, DEPENDENCY), none(), none()]);
        let relinted = settled(vec![report(&[], &[]), report(&[], &[]), report(&[], &[])]);
        assert_eq!(relinted, [none(), none(), none()]);
    }

    #[test]
    fn deciding_decided_reports_again_changes_nothing() {
        let mut reports = vec![
            report(&[(OUTER, DEPENDENCY), (OUTER, HINT)], &[stale_dependency(OUTER)]),
            Report::new(
                vec![
                    finding(INNER, DEPENDENCY, Severity::Error),
                    finding(INNER, HINT, Severity::Warning),
                ],
                1,
            )
            .with_config_judgements(vec![ConfigJudgement::new(
                INNER,
                DEPENDENCIES,
                "left-pad",
                true,
                DEPENDENCY,
            )]),
        ];
        let roots = ROOTS.map(Path::new);
        settle_shared_config(roots.into_iter().zip(reports.iter_mut()));
        let once = reports.clone();
        settle_shared_config(roots.into_iter().zip(reports.iter_mut()));
        assert_eq!(reports, once);
        assert_eq!(
            kept(&reports[0]),
            [
                (OUTER.to_owned(), DEPENDENCY.to_owned()),
                (OUTER.to_owned(), HINT.to_owned())
            ]
        );
        assert_eq!(kept(&reports[1]), none());
    }

    #[test]
    fn findings_of_other_rules_are_left_alone() {
        let other = Diagnostic::new(
            RuleId::from_static("unused-files"),
            Severity::Warning,
            DEPENDENCY,
            Location::new(INNER, None),
        );
        let mut reports = [
            report(&[(OUTER, DEPENDENCY)], &[stale_dependency(OUTER)]),
            Report::new(vec![other, finding(INNER, DEPENDENCY, Severity::Warning)], 1).with_config_judgements(vec![
                ConfigJudgement::new(INNER, DEPENDENCIES, "left-pad", true, DEPENDENCY),
            ]),
        ];
        settle_shared_config(ROOTS.map(Path::new).into_iter().zip(reports.iter_mut()));
        let rules: Vec<&str> = reports[1].diagnostics.iter().map(|d| d.rule.as_str()).collect();
        assert_eq!(rules, ["unused-files"]);
    }
}
