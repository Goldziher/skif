//! The CLI side of fixing: choosing among alternatives interactively and describing the outcome.

use std::fmt;
use std::io::{self, Write};

use inquire::InquireError;
use serde_json::{Value, json};
use skif::{Alternatives, FixPlan, FixSafety, FixSelection, Report, SkipReason};

use crate::args::{Format, LintArgs};
use crate::env::Env;
use crate::error::{CliError, Kind, slash};
use crate::install::InstallStatus;

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
        f.write_str(&self.title)
    }
}

const fn allows(ceiling: FixSafety, safety: FixSafety) -> bool {
    matches!(ceiling, FixSafety::Unsafe) || matches!(safety, FixSafety::Safe)
}

/// Builds the selection for this run. Alternatives are put to the user only when both stdin and stdout are terminals,
/// the format is `human`, neither `--no-prompt` nor `--select` was given; with `--select` the first alternative (which
/// the selection orders first) is taken; otherwise a diagnostic with several fixes stays unfixed (`needs-choice`),
/// as ADR 0008 requires of a non-interactive run.
pub fn selection(args: &LintArgs, env: Env, report: &Report) -> Result<FixSelection, CliError> {
    let safety = if args.fix_unsafe {
        FixSafety::Unsafe
    } else {
        FixSafety::Safe
    };
    let selection = FixSelection::new(safety);
    let interactive =
        env.stdin_tty && env.stdout_tty && !args.no_prompt && args.format() == Format::Human && args.select.is_none();
    if !interactive {
        return Ok(if args.select.is_some() {
            selection.with_alternatives(Alternatives::First)
        } else {
            selection
        });
    }
    let mut selection = selection;
    for (index, diagnostic) in report.diagnostics.iter().enumerate() {
        if diagnostic.fixes.len() < 2 {
            continue;
        }
        let mut options: Vec<Choice> = diagnostic
            .fixes
            .iter()
            .enumerate()
            .filter(|(_, fix)| allows(safety, fix.safety))
            .map(|(index, fix)| Choice {
                index,
                title: fix.title.clone(),
            })
            .collect();
        let picked = match options.len() {
            0 => continue,
            1 => options.pop(),
            _ => {
                let prompt = format!("{}: {}", diagnostic.rule, diagnostic.message);
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

fn install_json(status: &InstallStatus) -> Value {
    match status {
        InstallStatus::Ran { command } => json!({"status": "ran", "command": command}),
        InstallStatus::Skipped { reason } => json!({"status": "skipped", "reason": reason}),
        InstallStatus::Failed { command, message } => {
            json!({"status": "failed", "command": command, "message": message})
        }
    }
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

    /// The `fix` object of the JSON report.
    pub fn to_json(&self) -> Value {
        let applied: Vec<Value> = self
            .plan
            .applied
            .iter()
            .map(|fix| json!({"rule": fix.rule.as_str(), "message": fix.message, "title": fix.title}))
            .collect();
        let skipped: Vec<Value> = self
            .plan
            .skipped
            .iter()
            .map(|fix| {
                let (reason, detail) = skip_parts(&fix.reason);
                json!({"rule": fix.rule.as_str(), "message": fix.message, "reason": reason, "detail": detail})
            })
            .collect();
        let changed: Vec<String> = self.plan.paths().map(slash).collect();
        json!({
            "applied": applied,
            "skipped": skipped,
            "changed": changed,
            "install": install_json(&self.install),
        })
    }

    /// A short account of the fix run for terminals.
    pub fn write_human(&self, out: &mut dyn Write) -> io::Result<()> {
        let files = self.plan.changes.len();
        writeln!(out, "Fixed {} issue(s) in {files} file(s).", self.plan.applied.len())?;
        for path in self.plan.paths() {
            writeln!(out, "  fixed {}", slash(path))?;
        }
        for skipped in &self.plan.skipped {
            let (reason, detail) = skip_parts(&skipped.reason);
            let detail = if detail.is_empty() {
                String::new()
            } else {
                format!(": {detail}")
            };
            writeln!(
                out,
                "  skipped [{}] {} ({reason}{detail})",
                skipped.rule, skipped.message
            )?;
        }
        writeln!(out)
    }
}
