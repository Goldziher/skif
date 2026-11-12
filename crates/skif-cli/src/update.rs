//! `skif update`: propose, or write, newer published versions (ADR 0026). The only command that uses the network.
//!
//! The plan is the library's ([`skif::update::plan_updates`]); this module supplies the HTTP registry, the clock, the
//! output and the writes. Exit codes follow ADR 0024: `0` nothing to do or everything written, `1` updates are
//! available under `--check`, `2` the run is incomplete (a package the registry could not answer for, a failed
//! install) or could not happen at all.
//!
//! `--interactive` asks which updates to write before writing them. The choice is made here, on a copy of the report
//! that keeps only the chosen updates, so the library plans and applies updates the same way with or without it.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use inquire::InquireError;
use serde_json::{Value, json};
use skif::update::{STATE, UpdateOptions, UpdateReport, apply_updates, plan_updates};
use skif::{ConfigOverrides, OsFs, Selector, Target as Root, UpdateTarget, slash};
use skif_report::escape_controls;

use crate::args::{Format, RulesFormat, Target, UpdateArgs};
use crate::env::Env;
use crate::error::{CliError, Kind};
use crate::fix::install_json;
use crate::install::{self, InstallStatus};
use crate::lint::canonical;
use crate::npmrc::Npmrc;
use crate::output::{emit_error, stderr_line, write_stdout};
use crate::registry::HttpRegistry;
use crate::write::write_plan;

const CLEAN: u8 = 0;
const AVAILABLE: u8 = 1;
const INCOMPLETE: u8 = 2;

pub fn run(args: &UpdateArgs, env: Env) -> u8 {
    match execute(args, env) {
        Ok(code) => code,
        Err(error) => {
            emit_error(&error, Format::from(args.format));
            INCOMPLETE
        }
    }
}

const fn target(target: Target) -> UpdateTarget {
    match target {
        Target::Latest => UpdateTarget::Latest,
        Target::Minor => UpdateTarget::Minor,
        Target::Patch => UpdateTarget::Patch,
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_secs()).ok())
        .unwrap_or(0)
}

fn load(args: &UpdateArgs) -> Result<Root, CliError> {
    let start = canonical(&args.path)?;
    let overrides = ConfigOverrides::default()
        .with_config_file(
            args.config
                .as_ref()
                .map(|file| std::path::absolute(file).unwrap_or_else(|_| file.clone())),
        )
        .with_no_config(args.no_config);
    Ok(skif::find_target(&start, &OsFs, &overrides)?)
}

/// What a write run did after planning.
struct Written {
    files: Vec<String>,
    install: InstallStatus,
}

fn execute(args: &UpdateArgs, env: Env) -> Result<u8, CliError> {
    if args.interactive && !(env.stdin_tty && env.stdout_tty) {
        return Err(CliError::new(
            Kind::Usage,
            "--interactive needs a terminal on stdin and stdout; choose updates with --dependencies and --target instead",
        ));
    }
    if !args.check && env.ci {
        return Err(CliError::new(
            Kind::CiRefusal,
            "refusing to update: the CI environment variable is set; use `skif update --check` in CI, or unset CI",
        ));
    }
    let root = load(args)?;
    let options = UpdateOptions::new(now())
        .with_target(target(args.target))
        .with_dependencies(Selector::new(args.dependencies.iter().cloned())?);
    let (npmrc, warnings) = Npmrc::load(&root.root);
    for warning in &warnings {
        stderr_line(&format!("warning: {}", escape_controls(warning)));
    }
    let registry = HttpRegistry::new(npmrc, root.config.update.max_concurrent_requests);
    let mut report = plan_updates(&root.workspace, &root.config, &registry, &options);
    let mut declined = 0;
    if args.interactive && !report.updates.is_empty() {
        let available = report.updates.len();
        report = select(&report, &mut TerminalPicker)?;
        declined = available - report.updates.len();
    }
    let written = if args.check || report.updates.is_empty() {
        None
    } else {
        let plan = apply_updates(&root.workspace, &report, &OsFs)
            .map_err(|error| CliError::new(Kind::Internal, error.to_string()))?;
        write_plan(&plan, &root.root)?;
        let install = if args.no_install {
            InstallStatus::Skipped {
                reason: "--no-install was given",
            }
        } else {
            match root.workspace.package_manager {
                Some(manager) => install::install(manager, &root.root, args.format == RulesFormat::Json),
                None => InstallStatus::Skipped {
                    reason: "no package manager detected",
                },
            }
        };
        Some(Written {
            files: plan.paths().map(slash).collect(),
            install,
        })
    };
    let text = match args.format {
        RulesFormat::Json => json_document(args, &report, written.as_ref(), declined)?,
        RulesFormat::Human => human(args, &report, written.as_ref(), declined),
    };
    write_stdout(text.as_bytes())?;
    let install_failed = written
        .as_ref()
        .is_some_and(|written| matches!(written.install, InstallStatus::Failed { .. }));
    Ok(if !report.failures.is_empty() || install_failed {
        INCOMPLETE
    } else if args.check && !report.updates.is_empty() {
        AVAILABLE
    } else {
        CLEAN
    })
}

fn position(report: &UpdateReport, index: usize) -> (Option<u32>, Option<u32>) {
    report.updates[index]
        .location
        .span
        .map_or((None, None), |span| (Some(span.start_line), Some(span.start_col)))
}

fn json_document(
    args: &UpdateArgs,
    report: &UpdateReport,
    written: Option<&Written>,
    declined: usize,
) -> Result<String, CliError> {
    let updates: Vec<Value> = report
        .updates
        .iter()
        .enumerate()
        .map(|(index, update)| {
            let (line, column) = position(report, index);
            json!({
                "path": slash(&update.location.path),
                "line": line,
                "column": column,
                "package": update.package,
                "dependency": update.dependency,
                "place": update.place,
                "current": update.current,
                "target": update.target,
                "state": STATE,
            })
        })
        .collect();
    let failures: Vec<Value> = report
        .failures
        .iter()
        .map(|failure| json!({"dependency": failure.dependency, "message": failure.error.to_string()}))
        .collect();
    let notes: Vec<Value> = report
        .notes
        .iter()
        .map(|note| json!({"dependency": note.dependency(), "message": note.to_string()}))
        .collect();
    let mut document = json!({
        "version": 1,
        "skif": env!("CARGO_PKG_VERSION"),
        "check": args.check,
        "target": target(args.target).name(),
        "minimumReleaseAge": report.minimum_release_age,
        "queried": report.queried,
        "updates": updates,
        "failures": failures,
        "notes": notes,
    });
    if let Some(object) = document.as_object_mut() {
        if args.interactive {
            object.insert("declined".to_owned(), json!(declined));
        }
        if let Some(written) = written {
            object.insert("changed".to_owned(), json!(written.files));
            object.insert("install".to_owned(), install_json(&written.install));
        }
    }
    let mut text =
        serde_json::to_string_pretty(&document).map_err(|error| CliError::new(Kind::Internal, error.to_string()))?;
    text.push('\n');
    Ok(text)
}

fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// The report for a terminal. Names and specifiers come from the repository and the registry, so they are escaped.
fn human(args: &UpdateArgs, report: &UpdateReport, written: Option<&Written>, declined: usize) -> String {
    let mut out: Vec<u8> = Vec::new();
    let _ = write_human(args, report, written, declined, &mut out);
    String::from_utf8_lossy(&out).into_owned()
}

fn write_human(
    args: &UpdateArgs,
    report: &UpdateReport,
    written: Option<&Written>,
    declined: usize,
    out: &mut dyn std::io::Write,
) -> std::io::Result<()> {
    let escape = |text: &str| escape_controls(text).into_owned();
    if report.updates.is_empty() && declined > 0 {
        writeln!(
            out,
            "No updates chosen: {} available, nothing written.",
            plural(declined, "update", "updates")
        )?;
    } else if report.updates.is_empty() {
        writeln!(
            out,
            "No updates: {} checked (target {}).",
            plural(report.queried, "package", "packages"),
            target(args.target).name()
        )?;
    } else {
        let verb = if written.is_some() { "Updated" } else { "Available" };
        writeln!(
            out,
            "{verb}: {} (target {}, minimum release age {} min).",
            plural(report.updates.len(), "update", "updates"),
            target(args.target).name(),
            report.minimum_release_age
        )?;
        let mut file: Option<&Path> = None;
        for (index, update) in report.updates.iter().enumerate() {
            if file != Some(update.location.path.as_path()) {
                file = Some(update.location.path.as_path());
                writeln!(out, "  {}", escape(&slash(&update.location.path)))?;
            }
            let at = match position(report, index) {
                (Some(line), _) => format!(", line {line}"),
                _ => String::new(),
            };
            writeln!(
                out,
                "    {}  {} -> {}  ({}{at})",
                escape(&update.dependency),
                escape(&update.current),
                escape(&update.target),
                escape(&update.place)
            )?;
        }
        if declined > 0 {
            writeln!(out, "Not chosen: {}.", plural(declined, "update", "updates"))?;
        }
    }
    if !report.failures.is_empty() {
        writeln!(
            out,
            "Could not check {}:",
            plural(report.failures.len(), "package", "packages")
        )?;
        for failure in &report.failures {
            writeln!(
                out,
                "  {}: {}",
                escape(&failure.dependency),
                escape(&failure.error.to_string())
            )?;
        }
    }
    if !report.notes.is_empty() {
        writeln!(out, "Left out:")?;
        for note in &report.notes {
            writeln!(out, "  {}: {}", escape(note.dependency()), escape(&note.to_string()))?;
        }
    }
    if let Some(written) = written {
        for path in &written.files {
            writeln!(out, "  wrote {}", escape(path))?;
        }
        match &written.install {
            InstallStatus::Ran { command } => writeln!(out, "Ran `{command}`.")?,
            InstallStatus::Skipped { reason } => writeln!(out, "Install skipped: {reason}.")?,
            InstallStatus::Failed { message, .. } => writeln!(out, "Install failed: {}", escape(message))?,
        }
    } else if args.check && !report.updates.is_empty() {
        writeln!(out, "Run `skif update` without --check to write them.")?;
    }
    Ok(())
}

/// One line of the `--interactive` list: every instance that moves one dependency from one specifier to one target,
/// so a dependency declared alike in several packages is chosen once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub dependency: String,
    pub current: String,
    pub target: String,
    /// Where each instance is declared: the package (or, for a catalog, the file) and the place.
    pub instances: Vec<String>,
    /// The indexes into [`UpdateReport::updates`] this line stands for.
    pub updates: Vec<usize>,
}

impl fmt::Display for Choice {
    /// Names and specifiers come from the repository and the registry, so they are escaped.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}  {} -> {}  ({})",
            escape_controls(&self.dependency),
            escape_controls(&self.current),
            escape_controls(&self.target),
            escape_controls(&self.instances.join(", "))
        )
    }
}

/// The lines of the `--interactive` list for `report`, sorted by dependency, current specifier and target.
pub fn choices(report: &UpdateReport) -> Vec<Choice> {
    let mut lines: BTreeMap<(&str, &str, &str), Vec<usize>> = BTreeMap::new();
    for (index, update) in report.updates.iter().enumerate() {
        lines
            .entry((&update.dependency, &update.current, &update.target))
            .or_default()
            .push(index);
    }
    lines
        .into_iter()
        .map(|((dependency, current, target), updates)| Choice {
            dependency: dependency.to_owned(),
            current: current.to_owned(),
            target: target.to_owned(),
            instances: updates
                .iter()
                .map(|&index| {
                    let update = &report.updates[index];
                    let owner = update.package.clone().unwrap_or_else(|| slash(&update.location.path));
                    format!("{owner} {}", update.place)
                })
                .collect(),
            updates,
        })
        .collect()
}

/// Asks which lines of the `--interactive` list to write. A trait so the selection can be tested without a terminal.
pub trait Picker {
    /// The indexes into `choices` of the chosen lines; none when the person cancels.
    ///
    /// # Errors
    /// A [`CliError`] when the question cannot be asked.
    fn pick(&mut self, choices: &[Choice]) -> Result<Vec<usize>, CliError>;
}

/// The terminal multi-select, on stderr, with nothing chosen at the start.
struct TerminalPicker;

impl Picker for TerminalPicker {
    fn pick(&mut self, choices: &[Choice]) -> Result<Vec<usize>, CliError> {
        let labels: Vec<String> = choices.iter().map(ToString::to_string).collect();
        let prompt = inquire::MultiSelect::new("Updates to write:", labels)
            .with_help_message("space to choose, enter to write the chosen updates, esc to write nothing");
        match prompt.raw_prompt() {
            Ok(picked) => Ok(picked.into_iter().map(|option| option.index).collect()),
            Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => Ok(Vec::new()),
            Err(error) => Err(CliError::new(Kind::Io, format!("prompt failed: {error}"))),
        }
    }
}

/// A copy of `report` that keeps only the updates of the lines `picker` chooses, in their order; failures and notes
/// are kept as they are.
///
/// # Errors
/// What `picker` returns.
pub fn select(report: &UpdateReport, picker: &mut dyn Picker) -> Result<UpdateReport, CliError> {
    let choices = choices(report);
    let chosen: BTreeSet<usize> = picker
        .pick(&choices)?
        .into_iter()
        .filter_map(|line| choices.get(line))
        .flat_map(|choice| choice.updates.iter().copied())
        .collect();
    let mut selected = report.clone();
    selected.updates = report
        .updates
        .iter()
        .enumerate()
        .filter(|(index, _)| chosen.contains(index))
        .map(|(_, update)| update.clone())
        .collect();
    Ok(selected)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::path::Path;

    use skif::update::{FetchError, Packument, Published, Registry, UpdateOptions, apply_updates, plan_updates};
    use skif::{Config, FileChange, MemoryFs, Workspace};

    use super::{Choice, CliError, Picker, choices, select};

    struct Fixed;

    impl Registry for Fixed {
        fn fetch(&self, name: &str) -> Result<Packument, FetchError> {
            let versions = match name {
                "react" => vec!["18.2.0", "19.0.0"],
                "left-pad" => vec!["1.0.0", "1.3.0"],
                _ => return Err(FetchError::NotFound),
            };
            Ok(Packument::new(
                versions
                    .into_iter()
                    .map(|version| Published::new(version, Some(0)))
                    .collect(),
            ))
        }
    }

    /// Chooses fixed lines and remembers what it was shown.
    struct Scripted {
        lines: Vec<usize>,
        shown: Vec<Choice>,
    }

    impl Picker for Scripted {
        fn pick(&mut self, choices: &[Choice]) -> Result<Vec<usize>, CliError> {
            self.shown = choices.to_vec();
            Ok(self.lines.clone())
        }
    }

    fn fs() -> MemoryFs {
        MemoryFs::new()
            .with_file(
                "/repo/package.json",
                r#"{"name": "root", "private": true, "workspaces": ["packages/*"]}"#,
            )
            .with_file(
                "/repo/packages/a/package.json",
                r#"{"name": "a", "dependencies": {"react": "^18.2.0", "left-pad": "1.0.0"}}"#,
            )
            .with_file(
                "/repo/packages/b/package.json",
                r#"{"name": "b", "devDependencies": {"react": "^18.2.0"}}"#,
            )
    }

    fn plan(fs: &MemoryFs) -> (Workspace, skif::update::UpdateReport) {
        let config = Config::default();
        let workspace = Workspace::discover_with_fs(Path::new("/repo"), &config, fs).unwrap();
        let report = plan_updates(&workspace, &config, &Fixed, &UpdateOptions::new(1_800_000_000));
        (workspace, report)
    }

    fn written(plan: &skif::FixPlan, path: &str) -> Option<String> {
        plan.changes.iter().find_map(|change| match change {
            FileChange::Modify {
                path: at, new_contents, ..
            } if at.ends_with(path) => Some(new_contents.clone()),
            _ => None,
        })
    }

    #[test]
    fn a_dependency_declared_alike_in_several_packages_is_one_line() {
        let fs = fs();
        let (_, report) = plan(&fs);
        assert_eq!(report.updates.len(), 3);
        let lines: Vec<String> = choices(&report).iter().map(ToString::to_string).collect();
        assert_eq!(
            lines,
            [
                "left-pad  1.0.0 -> 1.3.0  (a dependencies)",
                "react  ^18.2.0 -> ^19.0.0  (a dependencies, b devDependencies)",
            ]
        );
    }

    #[test]
    fn only_the_chosen_lines_are_applied() {
        let fs = fs();
        let (workspace, report) = plan(&fs);
        let mut picker = Scripted {
            lines: vec![1],
            shown: Vec::new(),
        };
        let selected = select(&report, &mut picker).unwrap();
        assert_eq!(picker.shown.len(), 2);
        assert!(selected.updates.iter().all(|update| update.dependency == "react"));
        assert_eq!(selected.updates.len(), 2);
        let plan = apply_updates(&workspace, &selected, &fs).unwrap();
        let a = written(&plan, "packages/a/package.json").unwrap();
        assert!(a.contains(r#""react": "^19.0.0""#), "{a}");
        assert!(a.contains(r#""left-pad": "1.0.0""#), "{a}");
        let b = written(&plan, "packages/b/package.json").unwrap();
        assert!(b.contains(r#""react": "^19.0.0""#), "{b}");
    }

    #[test]
    fn choosing_nothing_or_an_unknown_line_applies_nothing() {
        let fs = fs();
        let (_, report) = plan(&fs);
        for lines in [vec![], vec![7]] {
            let mut picker = Scripted {
                lines,
                shown: Vec::new(),
            };
            let selected = select(&report, &mut picker).unwrap();
            assert_eq!(selected.updates.len(), 0);
            assert_eq!(selected.queried, report.queried);
        }
    }
}
