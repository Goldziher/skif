//! `skif list` and `skif json`: every dependency instance the version groups govern, with its group and state.

use std::fmt::Write as _;

use serde_json::{Value, json};
use skif::{CachedFs, Config, ConfigOverrides, InstanceFilter, InstanceStatus, OsFs, Severity, StatusType, slash};
use skif_report::escape_controls;

use crate::args::{FilterArgs, Format, InstancesArgs, ListArgs, Show};
use crate::error::{CliError, Kind};
use crate::lint::canonical;
use crate::output::{emit_error, stderr_line, write_stdout};
use crate::roots::{self, Linters, Resolved};

const CLEAN: u8 = 0;
const PROBLEMS: u8 = 1;
const COULD_NOT_RUN: u8 = 2;

/// The instances of every root, with paths relative to the base of the run, and the roots that failed.
struct Listing {
    statuses: Vec<InstanceStatus>,
    failures: Vec<roots::RootError>,
}

fn overrides(args: &InstancesArgs) -> ConfigOverrides {
    ConfigOverrides::default()
        // The listing needs the policy analysis only.
        .with_no_js(true)
        .with_config_file(args.config.as_ref().map(|file| crate::lint::absolute_file(file)))
        .with_no_config(args.no_config)
}

/// Narrows what the version-group policies of `config` see to the instances `args` select.
pub fn apply_filter(args: &FilterArgs, config: &mut Config) -> Result<(), CliError> {
    let filter = InstanceFilter::new(
        &args.dependencies,
        &args.dependency_types,
        &args.specifier_types,
        &args.source,
    )?;
    if let Some(name) = filter.unknown_custom_type(config) {
        return Err(CliError::new(
            Kind::Usage,
            format!(
                "--dependency-types: unknown dependency type \"{name}\" (expected prod, dev, peer, optional, local, resolutions, overrides, pnpm-overrides, or the name of a `[[custom-types]]` entry)"
            ),
        ));
    }
    config.filter = filter;
    Ok(())
}

fn collect(args: &InstancesArgs) -> Result<Listing, CliError> {
    let fs = CachedFs::new(OsFs);
    let start = canonical(&args.path)?;
    let Resolved {
        base,
        mut targets,
        failures,
        ..
    } = roots::resolve(&start, args.recursive, &fs, &overrides(args))?;
    for target in &mut targets {
        apply_filter(&args.filter, &mut target.config)?;
    }
    let linters = Linters::build(&targets)?;
    let runs = roots::lint_targets(&base, targets, &linters)?;
    let mut statuses = Vec::new();
    for run in &runs {
        for mut status in linters.get(run.linter).instances(&run.workspace, &run.report) {
            if !run.prefix.as_os_str().is_empty() {
                status.location.path = run.prefix.join(&status.location.path);
            }
            statuses.push(status);
        }
    }
    Ok(Listing { statuses, failures })
}

/// Exit 1 when an instance is reported at error severity, as a lint run would; 2 when a root could not be read.
fn finish(listing: &Listing) -> u8 {
    for failure in &listing.failures {
        stderr_line(&format!("error: {}", escape_controls(&failure.describe())));
    }
    if !listing.failures.is_empty() {
        COULD_NOT_RUN
    } else if listing
        .statuses
        .iter()
        .any(|status| status.severity == Some(Severity::Error))
    {
        PROBLEMS
    } else {
        CLEAN
    }
}

fn severity_name(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        _ => "info",
    }
}

/// One NDJSON line: syncpack's `json` fields where skif has them, plus where the instance is and its group.
fn instance_json(status: &InstanceStatus) -> Value {
    json!({
        "dependency": status.dependency,
        "dependencyType": status.dependency_type,
        "property": status.property,
        "package": slash(&status.location.path),
        "packageName": status.package,
        "versionGroup": status.group.variant,
        "versionGroupIndex": status.group.index,
        "versionGroupLabel": status.group.label,
        "preferredSemverRange": status.preferred_range,
        "statusCode": status.status_code,
        "statusType": status.status_type.name(),
        "rule": status.rule,
        "severity": status.severity.map(severity_name),
        "isLocal": status.is_local,
        "actual": { "raw": status.specifier, "type": status.specifier_type },
        "expected": status.expected.as_ref().map(|raw| json!({ "raw": raw, "type": status.expected_type })),
    })
}

fn render_json(statuses: &[InstanceStatus]) -> String {
    let mut text = String::new();
    for status in statuses {
        text.push_str(&instance_json(status).to_string());
        text.push('\n');
    }
    text
}

/// Runs `skif json`.
pub fn json(args: &InstancesArgs) -> u8 {
    match collect(args) {
        Ok(listing) => match write_stdout(render_json(&listing.statuses).as_bytes()) {
            Ok(()) => finish(&listing),
            Err(error) => {
                emit_error(&error, Format::Json);
                COULD_NOT_RUN
            }
        },
        Err(error) => {
            emit_error(&error, Format::Json);
            COULD_NOT_RUN
        }
    }
}

/// What `--show` turned on.
#[allow(clippy::struct_excessive_bools, reason = "each bool is one `--show` value")]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Shown {
    instances: bool,
    statuses: bool,
    hints: bool,
    ignored: bool,
}

impl Shown {
    fn of(values: &[Show]) -> Self {
        let mut shown = Self::default();
        for value in values {
            match value {
                Show::Instances => shown.instances = true,
                Show::Statuses => shown.statuses = true,
                Show::Hints => shown.hints = true,
                Show::Ignored => shown.ignored = true,
                Show::All => {
                    shown = Self {
                        instances: true,
                        statuses: true,
                        hints: true,
                        ignored: true,
                    };
                }
                Show::None => shown = Self::default(),
            }
        }
        shown
    }
}

fn group_heading(status: &InstanceStatus) -> String {
    let variant = status.group.variant;
    match (status.group.index, &status.group.label) {
        (Some(index), Some(label)) => format!("version-groups[{index}] {:?} ({variant})", escape_controls(label)),
        (Some(index), None) => format!("version-groups[{index}] ({variant})"),
        (None, _) if variant == "Catalog" => "catalog".to_owned(),
        (None, _) => format!("default ({variant})"),
    }
}

/// The mark of a dependency or an instance: the worst reported severity, `ok` when every state is valid, and `-`
/// for a state that is not valid but not reported (its rule is off or ignored).
fn mark<'a>(statuses: impl Iterator<Item = &'a InstanceStatus> + Clone) -> &'static str {
    if let Some(worst) = statuses.clone().filter_map(|status| status.severity).max() {
        return severity_name(worst);
    }
    if statuses
        .into_iter()
        .all(|status| status.status_type == StatusType::Valid)
    {
        "ok"
    } else {
        "-"
    }
}

fn render_list(statuses: &[InstanceStatus], shown: Shown) -> String {
    let mut text = String::new();
    let mut rest = statuses;
    let mut first = true;
    while let Some(head) = rest.first() {
        let in_group = rest.iter().take_while(|status| status.group == head.group).count();
        let (group, tail) = rest.split_at(in_group);
        rest = tail;
        if head.group.variant == "Ignored" && !shown.ignored {
            continue;
        }
        if !first {
            text.push('\n');
        }
        first = false;
        let _ = writeln!(text, "{}", group_heading(head));
        let mut members = group;
        while let Some(dependency) = members.first() {
            let count = members
                .iter()
                .take_while(|status| status.dependency == dependency.dependency)
                .count();
            let (instances, tail) = members.split_at(count);
            members = tail;
            write_dependency(&mut text, instances, shown);
        }
    }
    text
}

fn distinct<'a>(values: impl Iterator<Item = &'a str>) -> Vec<&'a str> {
    let mut seen: Vec<&str> = Vec::new();
    for value in values {
        if !seen.contains(&value) {
            seen.push(value);
        }
    }
    seen
}

fn write_dependency(text: &mut String, instances: &[InstanceStatus], shown: Shown) {
    let head = &instances[0];
    let specifiers = distinct(instances.iter().map(|status| status.specifier.as_str()));
    let _ = write!(
        text,
        "  {:<7} {} {}",
        mark(instances.iter()),
        escape_controls(&head.dependency),
        escape_controls(&specifiers.join(", "))
    );
    if shown.hints && head.is_local {
        text.push_str(" (workspace package)");
    }
    if shown.statuses {
        let invalid = distinct(
            instances
                .iter()
                .filter(|status| status.status_type != StatusType::Valid)
                .map(|status| status.status_code.as_str()),
        );
        let codes = if invalid.is_empty() {
            distinct(instances.iter().map(|status| status.status_code.as_str()))
        } else {
            invalid
        };
        let _ = write!(text, " [{}]", codes.join(", "));
    }
    text.push('\n');
    if shown.instances {
        for status in instances {
            let _ = write!(
                text,
                "    {:<7} {} in {} of {} ({})",
                mark(std::iter::once(status)),
                escape_controls(&status.specifier),
                escape_controls(&status.property),
                escape_controls(&status.package),
                escape_controls(&slash(&status.location.path)),
            );
            if shown.statuses {
                let _ = write!(text, " [{}]", status.status_code);
            }
            if status.status_type != StatusType::Valid
                && let Some(expected) = &status.expected
            {
                let _ = write!(text, " -> {}", escape_controls(expected));
            }
            text.push('\n');
        }
    }
}

/// Runs `skif list`.
pub fn list(args: &ListArgs) -> u8 {
    let shown = Shown::of(&args.show);
    match collect(&args.instances) {
        Ok(listing) => match write_stdout(render_list(&listing.statuses, shown).as_bytes()) {
            Ok(()) => finish(&listing),
            Err(error) => {
                emit_error(&error, Format::Human);
                COULD_NOT_RUN
            }
        },
        Err(error) => {
            emit_error(&error, Format::Human);
            COULD_NOT_RUN
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn show_values_combine_and_all_and_none_reset() {
        assert_eq!(
            Shown::of(&[Show::Hints, Show::Statuses]),
            Shown {
                hints: true,
                statuses: true,
                ..Shown::default()
            }
        );
        assert!(Shown::of(&[Show::All]).ignored);
        assert_eq!(Shown::of(&[Show::All, Show::None]), Shown::default());
    }

    #[test]
    fn values_keep_their_first_order() {
        assert_eq!(distinct(["b", "a", "b"].into_iter()), ["b", "a"]);
    }
}
