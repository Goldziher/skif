//! The CLI side of fixing: choosing among alternatives interactively and describing the outcome.

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::fmt;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use inquire::InquireError;
use serde_json::{Value, json};
use similar::TextDiff;
use skif::{
    Alternatives, FileChange, FileSystem, FixPlan, FixSafety, FixSelection, Prefer, Report, SkipReason,
    preferred_alternative, slash,
};
use skif_report::{escape_controls, escape_multiline};

use crate::args::{Format, LintArgs};
use crate::env::Env;
use crate::error::{CliError, Kind};
use crate::install::InstallStatus;
use crate::roots;

/// What a fix run did.
pub struct FixRun {
    pub plan: FixPlan,
    pub install: InstallStatus,
    /// The files a `--dry-run` would change, with their diffs; `None` when the plan was written.
    pub preview: Option<Vec<Preview>>,
}

/// One file a dry run would change.
pub struct Preview {
    /// Relative to the root of the run.
    pub path: PathBuf,
    /// `modify`, `create` or `delete`.
    pub kind: &'static str,
    /// A unified diff of the change, with `a/` and `b/` paths.
    pub diff: String,
}

/// The diff of every change in `plan`, reading the current contents of the files under `root` through `fs`.
///
/// # Errors
/// A [`CliError`] when a file the plan modifies or deletes cannot be read (diffing it as empty would show a
/// modification as a creation), or when the plan holds a kind of change this version cannot show.
pub fn preview(plan: &FixPlan, root: &Path, fs: &dyn FileSystem) -> Result<Vec<Preview>, CliError> {
    let current = |path: &Path| {
        let full = root.join(path);
        fs.read_to_string(&full)
            .map(|text| text.to_string())
            .map_err(|error| CliError::io("cannot read", &full, &error))
    };
    plan.changes
        .iter()
        .map(|change| {
            let path = change.path().to_path_buf();
            let shown = slash(&path);
            let (kind, old, new, old_name, new_name) = match change {
                FileChange::Create { contents, .. } => (
                    "create",
                    String::new(),
                    contents.clone(),
                    "/dev/null".to_owned(),
                    format!("b/{shown}"),
                ),
                FileChange::Delete { .. } => (
                    "delete",
                    current(&path)?,
                    String::new(),
                    format!("a/{shown}"),
                    "/dev/null".to_owned(),
                ),
                FileChange::Modify { new_contents, .. } => (
                    "modify",
                    current(&path)?,
                    new_contents.clone(),
                    format!("a/{shown}"),
                    format!("b/{shown}"),
                ),
                _ => {
                    return Err(CliError::new(
                        Kind::Internal,
                        format!("cannot preview the planned change to {shown}"),
                    ));
                }
            };
            let diff = TextDiff::from_lines(&old, &new)
                .unified_diff()
                .context_radius(3)
                .header(&old_name, &new_name)
                .to_string();
            Ok(Preview { path, kind, diff })
        })
        .collect()
}

struct Choice {
    index: usize,
    title: String,
}

impl fmt::Display for Choice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&escape_controls(&self.title))
    }
}

/// Builds the selection for this run. Alternatives are put to the user only when both stdin and stdout are terminals,
/// the format is `human`, neither `--no-prompt` nor `--select` was given, and configuration does not already
/// determine the answer. With `--select` the first alternative (which the rules order by the selected version) is
/// taken; otherwise an alternative that names the version `[policy] prefer` selects is taken; a diagnostic with
/// several fixes and neither stays unfixed (`needs-choice`), as ADR 0008 requires of a non-interactive run.
pub fn selection(
    args: &LintArgs,
    env: Env,
    report: &Report,
    prefer: Prefer,
    rules: Option<&BTreeSet<String>>,
) -> Result<FixSelection, CliError> {
    let safety = if args.fix_unsafe {
        FixSafety::Unsafe
    } else {
        FixSafety::Safe
    };
    let mut selection = FixSelection::new(safety).with_remove_files(args.allow_remove_files);
    if let Some(rules) = rules {
        selection = selection.with_rules(rules.iter().cloned());
    }
    let interactive =
        env.stdin_tty && env.stdout_tty && !args.no_prompt && args.format() == Format::Human && args.select.is_none();
    if !interactive {
        return Ok(selection.with_alternatives(if args.select.is_some() {
            Alternatives::First
        } else {
            Alternatives::Preferred(prefer)
        }));
    }
    let mut selection = selection.with_alternatives(Alternatives::Preferred(prefer));
    for (index, diagnostic) in report.diagnostics.iter().enumerate() {
        if diagnostic.fixes.len() < 2
            || preferred_alternative(&diagnostic.fixes, prefer).is_some()
            || rules.is_some_and(|rules| !rules.contains(diagnostic.rule.as_str()))
        {
            continue;
        }
        let mut options: Vec<Choice> = diagnostic
            .fixes
            .iter()
            .enumerate()
            .filter(|(_, fix)| selection.allows(fix.safety))
            .map(|(index, fix)| Choice {
                index,
                title: fix.title.clone(),
            })
            .collect();
        let picked = match options.len() {
            0 => continue,
            1 => options.pop(),
            _ => {
                let prompt = format!("{}: {}", diagnostic.rule, escape_controls(&diagnostic.message));
                match inquire::Select::new(&prompt, options).prompt() {
                    Ok(choice) => Some(choice),
                    Err(InquireError::OperationCanceled) => None,
                    Err(error) => return Err(CliError::new(Kind::Io, format!("prompt failed: {error}"))),
                }
            }
        };
        if let Some(choice) = picked {
            selection = selection.choose(index, choice.index);
        }
    }
    Ok(selection)
}

/// The rule ids `--fix-type` names, or `None` without the flag. Each value is a rule id of `known`, or a knip issue
/// type, which stands for the rules that report the same findings (`skif_migrate::knip_fix_type_rules`).
pub fn fix_type_rules<'a>(
    names: &[String],
    known: impl IntoIterator<Item = &'a str>,
) -> Result<Option<BTreeSet<String>>, CliError> {
    if names.is_empty() {
        return Ok(None);
    }
    let known: BTreeSet<&str> = known.into_iter().collect();
    let mut rules = BTreeSet::new();
    for name in names.iter().map(|name| name.trim()) {
        if known.contains(name) {
            rules.insert(name.to_owned());
        } else if let Some(mapped) = skif_migrate::knip_fix_type_rules(name) {
            rules.extend(mapped.into_iter().map(str::to_owned));
        } else {
            return Err(CliError::new(
                Kind::Usage,
                format!(
                    "unknown --fix-type `{}`: expected a rule id (see `skif rules`) or a knip issue type ({})",
                    escape_controls(name),
                    skif_migrate::knip_issue_types().join(", ")
                ),
            ));
        }
    }
    Ok(Some(rules))
}

fn skip_parts(reason: &SkipReason) -> (&'static str, String) {
    match reason {
        SkipReason::Safety(_) => ("safety", "needs --fix-unsafe".to_owned()),
        SkipReason::NeedsChoice { alternatives } => ("needs-choice", alternatives.join(", ")),
        SkipReason::Conflict(message) => ("conflict", message.clone()),
        SkipReason::Failed(message) => ("failed", message.clone()),
        SkipReason::RemovesFiles => ("removes-files", "re-run with --allow-remove-files".to_owned()),
        _ => ("skipped", String::new()),
    }
}

fn install_json(status: &InstallStatus) -> Value {
    match status {
        InstallStatus::Ran { command } => json!({"status": "ran", "command": command}),
        InstallStatus::Skipped { reason } => json!({"status": "skipped", "reason": reason}),
        InstallStatus::Failed { command, message } => {
            json!({"status": "failed", "command": command, "message": message})
        }
    }
}

/// A fix run and the root it ran in, relative to the base the paths of the run are reported against.
pub struct RootFix<'a> {
    pub prefix: &'a Path,
    pub run: &'a FixRun,
}

impl FixRun {
    /// A fix run whose install step did not happen because the re-lint failed.
    pub const fn relint_failed(plan: FixPlan) -> Self {
        Self {
            plan,
            install: InstallStatus::Skipped {
                reason: "the re-lint after fixing failed",
            },
            preview: None,
        }
    }

    /// A planned fix run that a `--dry-run` shows instead of writing.
    pub const fn dry_run(plan: FixPlan, preview: Vec<Preview>) -> Self {
        Self {
            plan,
            install: InstallStatus::Skipped {
                reason: "--dry-run was given",
            },
            preview: Some(preview),
        }
    }

    fn changes_json<'a>(&'a self, prefix: &'a Path) -> impl Iterator<Item = Value> + 'a {
        self.preview.iter().flatten().map(move |preview| {
            json!({
                "path": slash(&prefix.join(&preview.path)),
                "kind": preview.kind,
                "diff": preview.diff,
            })
        })
    }

    fn applied_json(&self) -> impl Iterator<Item = Value> + '_ {
        self.plan
            .applied
            .iter()
            .map(|fix| json!({"rule": fix.rule.as_str(), "message": fix.message, "title": fix.title}))
    }

    fn skipped_json(&self) -> impl Iterator<Item = Value> + '_ {
        self.plan.skipped.iter().map(|fix| {
            let (reason, detail) = skip_parts(&fix.reason);
            json!({"rule": fix.rule.as_str(), "message": fix.message, "reason": reason, "detail": detail})
        })
    }

    fn changed_at<'a>(&'a self, prefix: &'a Path) -> impl Iterator<Item = String> + 'a {
        self.plan.paths().map(move |path| slash(&prefix.join(path)))
    }

    /// The `fix` object of the JSON report; a dry run adds `dry-run` and the `changes` with their diffs.
    pub fn to_json(&self) -> Value {
        let mut value = json!({
            "applied": self.applied_json().collect::<Vec<_>>(),
            "skipped": self.skipped_json().collect::<Vec<_>>(),
            "changed": self.changed_at(Path::new("")).collect::<Vec<_>>(),
            "install": install_json(&self.install),
        });
        if self.preview.is_some() {
            value["dry-run"] = Value::Bool(true);
            value["changes"] = Value::Array(self.changes_json(Path::new("")).collect());
        }
        value
    }

    /// A short account of the fix run in the root `prefix` for terminals: the root is named when it is not the base,
    /// and the changed files are shown relative to the base. Text that came from the repository is escaped, so it
    /// cannot drive the terminal.
    pub fn write_human_at(&self, prefix: &Path, out: &mut dyn Write) -> io::Result<()> {
        if !prefix.as_os_str().is_empty() {
            writeln!(out, "{}:", escape_controls(&slash(prefix)))?;
        }
        let files = self.plan.changes.len();
        let (verb, each) = if self.preview.is_some() {
            ("Would fix", "would fix")
        } else {
            ("Fixed", "fixed")
        };
        writeln!(out, "{verb} {} issue(s) in {files} file(s).", self.plan.applied.len())?;
        for path in self.changed_at(prefix) {
            writeln!(out, "  {each} {}", escape_controls(&path))?;
        }
        for skipped in &self.plan.skipped {
            let (reason, detail) = skip_parts(&skipped.reason);
            let detail = if detail.is_empty() {
                String::new()
            } else {
                format!(": {}", escape_controls(&detail))
            };
            writeln!(
                out,
                "  skipped [{}] {} ({reason}{detail})",
                skipped.rule,
                escape_controls(&skipped.message)
            )?;
        }
        for preview in self.preview.iter().flatten() {
            writeln!(out)?;
            out.write_all(escape_multiline(&preview.diff).as_bytes())?;
        }
        writeln!(out)
    }
}

static NOTHING_FIXED: InstallStatus = InstallStatus::Skipped {
    reason: "nothing was fixed",
};

/// The install status that stands for all roots: the failures (all of them, named by root, when several), else a
/// run, else the first skip.
fn merged_install<'a>(runs: &[RootFix<'a>]) -> Cow<'a, InstallStatus> {
    let installs: Vec<(&'a Path, &'a InstallStatus)> =
        runs.iter().map(|root| (root.prefix, &root.run.install)).collect();
    merge_installs(&installs)
}

fn merge_installs<'a>(installs: &[(&Path, &'a InstallStatus)]) -> Cow<'a, InstallStatus> {
    let failed: Vec<(&Path, &str, &str)> = installs
        .iter()
        .filter_map(|(prefix, status)| match status {
            InstallStatus::Failed { command, message } => Some((*prefix, command.as_str(), message.as_str())),
            _ => None,
        })
        .collect();
    if failed.len() > 1 {
        return Cow::Owned(InstallStatus::Failed {
            command: failed
                .iter()
                .map(|(_, command, _)| *command)
                .collect::<Vec<_>>()
                .join("; "),
            message: failed
                .iter()
                .map(|(prefix, _, message)| format!("{}: {message}", roots::display(prefix)))
                .collect::<Vec<_>>()
                .join("; "),
        });
    }
    let statuses = || installs.iter().map(|(_, status)| *status);
    Cow::Borrowed(
        statuses()
            .find(|status| matches!(status, InstallStatus::Failed { .. }))
            .or_else(|| statuses().find(|status| matches!(status, InstallStatus::Ran { .. })))
            .or_else(|| statuses().next())
            .unwrap_or(&NOTHING_FIXED),
    )
}

/// The `fix` object for the roots of a run: the run itself for one root at the base, otherwise the applied and
/// skipped fixes of all roots and the changed files relative to the base.
pub fn fixes_json(runs: &[RootFix<'_>]) -> Value {
    if let [only] = runs
        && only.prefix.as_os_str().is_empty()
    {
        return only.run.to_json();
    }
    let mut value = json!({
        "applied": runs.iter().flat_map(|root| root.run.applied_json()).collect::<Vec<_>>(),
        "skipped": runs.iter().flat_map(|root| root.run.skipped_json()).collect::<Vec<_>>(),
        "changed": runs.iter().flat_map(|root| root.run.changed_at(root.prefix)).collect::<Vec<_>>(),
        "install": install_json(&merged_install(runs)),
    });
    if runs.iter().any(|root| root.run.preview.is_some()) {
        value["dry-run"] = Value::Bool(true);
        value["changes"] = Value::Array(
            runs.iter()
                .flat_map(|root| root.run.changes_json(root.prefix))
                .collect(),
        );
    }
    value
}

/// The human account of every root's fix run, in order.
pub fn write_fixes_human(runs: &[RootFix<'_>], out: &mut dyn Write) -> io::Result<()> {
    runs.iter()
        .try_for_each(|root| root.run.write_human_at(root.prefix, out))
}

#[cfg(test)]
mod install_tests {
    use super::*;

    fn failed(command: &str, message: &str) -> InstallStatus {
        InstallStatus::Failed {
            command: command.to_owned(),
            message: message.to_owned(),
        }
    }

    #[test]
    fn every_failed_install_is_named_by_its_root() {
        let a = failed("npm install", "exit 1");
        let b = failed("pnpm install", "no network");
        let ran = InstallStatus::Ran {
            command: "yarn".to_owned(),
        };
        let merged = merge_installs(&[(Path::new("a"), &a), (Path::new("ok"), &ran), (Path::new("pkgs/b"), &b)]);
        assert_eq!(
            *merged,
            failed("npm install; pnpm install", "a: exit 1; pkgs/b: no network")
        );
    }

    #[test]
    fn one_failure_wins_over_a_run_and_a_run_over_a_skip() {
        let a = failed("npm install", "exit 1");
        let ran = InstallStatus::Ran {
            command: "yarn".to_owned(),
        };
        let skipped = InstallStatus::Skipped { reason: "nothing" };
        assert_eq!(*merge_installs(&[(Path::new("x"), &skipped), (Path::new("a"), &a)]), a);
        assert_eq!(
            *merge_installs(&[(Path::new("x"), &skipped), (Path::new("y"), &ran)]),
            ran
        );
        assert_eq!(*merge_installs(&[(Path::new("x"), &skipped)]), skipped);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod selection_tests {
    use clap::Parser;
    use skif::{Category, Fix, Fixability, Location, MemoryFs, RuleMeta, Severity, diagnostic};

    use super::*;
    use crate::args::Cli;

    static AMBIGUOUS: RuleMeta = RuleMeta::new(
        "demo-ambiguous",
        Category::Manifest,
        Some(Severity::Warning),
        Fixability::Unsafe,
        "demo",
    );

    fn alternative(title: &str) -> Fix {
        Fix::new(title, FixSafety::Unsafe, Vec::new())
    }

    fn report(titles: &[&str]) -> Report {
        let location = Location::new("package.json", None);
        let mut builder = diagnostic(&AMBIGUOUS, "finding", location);
        for title in titles {
            builder = builder.fix(alternative(title));
        }
        Report::new(vec![builder.build()], 1)
    }

    fn terminal() -> Env {
        Env {
            ci: false,
            no_color: false,
            stdin_tty: true,
            stdout_tty: true,
            github_actions: false,
        }
    }

    fn lint_args(flags: &[&str]) -> LintArgs {
        let argv = std::iter::once("skif").chain(flags.iter().copied());
        Cli::try_parse_from(argv).unwrap().lint
    }

    #[test]
    fn no_prompt_leaves_alternatives_to_the_configured_preference_even_on_a_terminal() {
        let report = report(&["pin one", "pin two"]);
        let selection = selection(
            &lint_args(&["--fix-unsafe", "--no-prompt"]),
            terminal(),
            &report,
            Prefer::Lowest,
            None,
        )
        .unwrap();
        assert_eq!(selection.alternatives, Alternatives::Preferred(Prefer::Lowest));
        assert!(selection.choices.is_empty());
    }

    #[test]
    fn a_non_terminal_run_never_asks() {
        let report = report(&["pin one", "pin two"]);
        let piped = Env {
            stdout_tty: false,
            ..terminal()
        };
        let selection = selection(&lint_args(&["--fix-unsafe"]), piped, &report, Prefer::Highest, None).unwrap();
        assert_eq!(selection.alternatives, Alternatives::Preferred(Prefer::Highest));
        assert!(selection.choices.is_empty());
    }

    #[test]
    fn select_takes_the_first_alternative_without_asking() {
        let report = report(&["pin one", "pin two"]);
        let selection = selection(
            &lint_args(&["--fix-unsafe", "--select", "lowest"]),
            terminal(),
            &report,
            Prefer::Highest,
            None,
        )
        .unwrap();
        assert_eq!(selection.alternatives, Alternatives::First);
    }

    #[test]
    fn fix_types_accept_rule_ids_and_knip_issue_types() {
        let known = ["unused-files", "unused-exports"];
        let names = |values: &[&str]| values.iter().map(|value| (*value).to_owned()).collect::<Vec<_>>();
        assert_eq!(fix_type_rules(&[], known).unwrap(), None);
        let rules = fix_type_rules(&names(&["unused-files", "enumMembers"]), known)
            .unwrap()
            .unwrap();
        assert_eq!(
            rules.into_iter().collect::<Vec<_>>(),
            ["unused-enum-members", "unused-files"]
        );
        let error = fix_type_rules(&names(&["nonsense"]), known).unwrap_err();
        assert_eq!(error.kind, Kind::Usage);
        assert!(
            error.message.contains("unknown --fix-type `nonsense`"),
            "{}",
            error.message
        );
    }

    #[test]
    fn a_fix_type_keeps_other_rules_from_asking() {
        let report = report(&["pin one", "pin two"]);
        let only = BTreeSet::from(["unused-files".to_owned()]);
        let selection = selection(
            &lint_args(&["--fix-unsafe", "--fix-type", "unused-files"]),
            terminal(),
            &report,
            Prefer::Highest,
            Some(&only),
        )
        .unwrap();
        assert!(selection.choices.is_empty());
        assert_eq!(selection.rules, Some(only));
    }

    #[test]
    fn allow_remove_files_reaches_the_selection() {
        let report = report(&[]);
        let with = selection(
            &lint_args(&["--fix-unsafe", "--allow-remove-files"]),
            terminal(),
            &report,
            Prefer::Highest,
            None,
        )
        .unwrap();
        assert!(with.remove_files);
        let without = selection(
            &lint_args(&["--fix-unsafe"]),
            terminal(),
            &report,
            Prefer::Highest,
            None,
        )
        .unwrap();
        assert!(!without.remove_files);
        assert!(Cli::try_parse_from(["skif", "--allow-remove-files"]).is_err());
        assert!(Cli::try_parse_from(["skif", "--fix-type", "files"]).is_err());
    }

    #[test]
    fn a_skipped_choice_is_reported_as_needs_choice() {
        let skipped = SkipReason::NeedsChoice {
            alternatives: vec!["pin one".to_owned(), "pin two".to_owned()],
        };
        assert_eq!(skip_parts(&skipped), ("needs-choice", "pin one, pin two".to_owned()));
    }

    fn modify(path: &str, new_contents: &str) -> FixPlan {
        let mut plan = FixPlan::default();
        plan.changes.push(FileChange::Modify {
            path: PathBuf::from(path),
            edits: Vec::new(),
            new_contents: new_contents.to_owned(),
        });
        plan
    }

    #[test]
    fn a_modified_file_is_diffed_against_its_current_contents() {
        let fs = MemoryFs::new().with_file("/ws/package.json", "{\"name\": \"a\"}\n");
        let previews = preview(&modify("package.json", "{\"name\": \"b\"}\n"), Path::new("/ws"), &fs).unwrap();
        assert_eq!(previews[0].kind, "modify");
        assert!(previews[0].diff.contains("--- a/package.json"));
        assert!(previews[0].diff.contains("-{\"name\": \"a\"}"));
    }

    #[test]
    fn a_file_that_cannot_be_read_fails_the_preview() {
        let fs = MemoryFs::new();
        let Err(error) = preview(&modify("package.json", "{}\n"), Path::new("/ws"), &fs) else {
            panic!("a modify of a missing file was previewed");
        };
        assert_eq!(error.kind, Kind::Io);
        assert_eq!(error.path.as_deref(), Some("/ws/package.json"));
    }
}
