//! `skif update`: propose, or write, newer published versions (ADR 0050). The only command that uses the network.
//!
//! The plan is the library's ([`skif::update::plan_updates`]); this module supplies the HTTP registry, the clock, the
//! output and the writes. Exit codes follow ADR 0016: `0` nothing to do or everything written, `1` updates are
//! available under `--check`, `2` the run is incomplete (a package the registry could not answer for, a failed
//! install) or could not happen at all.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

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
    let report = plan_updates(&root.workspace, &root.config, &registry, &options);
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
        RulesFormat::Json => json_document(args, &report, written.as_ref())?,
        RulesFormat::Human => human(args, &report, written.as_ref()),
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

fn json_document(args: &UpdateArgs, report: &UpdateReport, written: Option<&Written>) -> Result<String, CliError> {
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
    if let (Some(written), Some(object)) = (written, document.as_object_mut()) {
        object.insert("changed".to_owned(), json!(written.files));
        object.insert("install".to_owned(), install_json(&written.install));
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
fn human(args: &UpdateArgs, report: &UpdateReport, written: Option<&Written>) -> String {
    let mut out: Vec<u8> = Vec::new();
    let _ = write_human(args, report, written, &mut out);
    String::from_utf8_lossy(&out).into_owned()
}

fn write_human(
    args: &UpdateArgs,
    report: &UpdateReport,
    written: Option<&Written>,
    out: &mut dyn std::io::Write,
) -> std::io::Result<()> {
    let escape = |text: &str| escape_controls(text).into_owned();
    if report.updates.is_empty() {
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
