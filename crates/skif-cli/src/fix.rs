//! The CLI side of fixing: choosing among alternatives interactively and describing the outcome.

use std::borrow::Cow;
use std::fmt;
use std::io::{self, Write};
use std::path::Path;

use inquire::InquireError;
use serde_json::{Value, json};
use skif::{Alternatives, FixPlan, FixSafety, FixSelection, Prefer, Report, SkipReason, preferred_alternative, slash};
use skif_report::escape_controls;

use crate::args::{Format, LintArgs};
use crate::env::Env;
use crate::error::{CliError, Kind};
use crate::install::InstallStatus;
use crate::roots;

/// What a fix run did.
pub struct FixRun {
    pub plan: FixPlan,
    pub install: InstallStatus,
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
pub fn selection(args: &LintArgs, env: Env, report: &Report, prefer: Prefer) -> Result<FixSelection, CliError> {
    let safety = if args.fix_unsafe {
        FixSafety::Unsafe
    } else {
        FixSafety::Safe
    };
    let selection = FixSelection::new(safety);
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
        if diagnostic.fixes.len() < 2 || preferred_alternative(&diagnostic.fixes, prefer).is_some() {
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

fn skip_parts(reason: &SkipReason) -> (&'static str, String) {
    match reason {
        SkipReason::Safety(_) => ("safety", "needs --fix-unsafe".to_owned()),
        SkipReason::NeedsChoice { alternatives } => ("needs-choice", alternatives.join(", ")),
        SkipReason::Conflict(message) => ("conflict", message.clone()),
        SkipReason::Failed(message) => ("failed", message.clone()),
        _ => ("skipped", String::new()),
    }
}

pub fn install_json(status: &InstallStatus) -> Value {
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
        }
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

    /// The `fix` object of the JSON report.
    pub fn to_json(&self) -> Value {
        json!({
            "applied": self.applied_json().collect::<Vec<_>>(),
            "skipped": self.skipped_json().collect::<Vec<_>>(),
            "changed": self.changed_at(Path::new("")).collect::<Vec<_>>(),
            "install": install_json(&self.install),
        })
    }

    /// A short account of the fix run in the root `prefix` for terminals: the root is named when it is not the base,
    /// and the changed files are shown relative to the base. Text that came from the repository is escaped, so it
    /// cannot drive the terminal.
    pub fn write_human_at(&self, prefix: &Path, out: &mut dyn Write) -> io::Result<()> {
        if !prefix.as_os_str().is_empty() {
            writeln!(out, "{}:", escape_controls(&slash(prefix)))?;
        }
        let files = self.plan.changes.len();
        writeln!(out, "Fixed {} issue(s) in {files} file(s).", self.plan.applied.len())?;
        for path in self.changed_at(prefix) {
            writeln!(out, "  fixed {}", escape_controls(&path))?;
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
    json!({
        "applied": runs.iter().flat_map(|root| root.run.applied_json()).collect::<Vec<_>>(),
        "skipped": runs.iter().flat_map(|root| root.run.skipped_json()).collect::<Vec<_>>(),
        "changed": runs.iter().flat_map(|root| root.run.changed_at(root.prefix)).collect::<Vec<_>>(),
        "install": install_json(&merged_install(runs)),
    })
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
    use skif::{Category, Fix, Fixability, Location, RuleMeta, Severity, diagnostic};

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
        let selection = selection(&lint_args(&["--fix-unsafe"]), piped, &report, Prefer::Highest).unwrap();
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
        )
        .unwrap();
        assert_eq!(selection.alternatives, Alternatives::First);
    }

    #[test]
    fn a_skipped_choice_is_reported_as_needs_choice() {
        let skipped = SkipReason::NeedsChoice {
            alternatives: vec!["pin one".to_owned(), "pin two".to_owned()],
        };
        assert_eq!(skip_parts(&skipped), ("needs-choice", "pin one, pin two".to_owned()));
    }
}
