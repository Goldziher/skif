//! Turns the verdicts of an [`Analysis`] into diagnostics.

use std::path::Path;

use skif_core::{Diagnostic, Fix, FixSafety, Location, Related, diagnostic};
use skif_lint::Emitter;
use skif_specifier::Specifier;

use super::analysis::{Analysis, ConfiguredGroup};
use super::instances::{Instance, InstanceId};
use super::specifiers::is_compatible_upgrade;
use super::state::{State, Verdict};

/// Emits one diagnostic per non-valid instance, then one per unused group and one per unusable pin; the latter two
/// point at `config_file`.
pub(super) fn emit(analysis: &Analysis<'_>, config_file: &Path, out: &mut Emitter<'_>) {
    for (id, instance) in analysis.instances.iter() {
        if let Some(verdict) = &analysis.verdicts[id.index()] {
            out.emit(instance_diagnostic(analysis, id, instance, verdict));
        }
    }
    for unused in &analysis.unused_groups {
        out.emit(group_diagnostic(
            State::UnusedGroup,
            config_file,
            unused,
            "owns no dependency",
            "an earlier group may claim its dependencies, or its selectors match nothing",
        ));
    }
    for invalid in &analysis.invalid_pins {
        out.emit(group_diagnostic(
            State::InvalidPin,
            config_file,
            invalid,
            "has a `pin` that is empty or not a version specifier",
            "set `pin` to the version the dependencies must use, for example `\"1.2.3\"`",
        ));
    }
}

fn instance_diagnostic(
    analysis: &Analysis<'_>,
    id: InstanceId,
    instance: &Instance<'_>,
    verdict: &Verdict,
) -> Diagnostic {
    let state = verdict.state;
    let package = instance.package.display_name();
    let subject = if instance.is_own_version() {
        format!("the version `{}` of {package}", instance.raw())
    } else {
        format!(
            "`{}@{}` in `{}` of {package}",
            instance.name,
            instance.raw(),
            instance.ty().label()
        )
    };
    let mut builder = diagnostic(
        state.rule(),
        format!("{subject} {}", state.summary()),
        instance.location(),
    )
    .package(package)
    .package_id(instance.package.id)
    .dependency(&instance.name)
    .specifier(instance.raw())
    .detail(state.name());
    let help = match (&verdict.target, state) {
        (_, State::IsBanned) => Some(format!("remove `{}`", instance.name)),
        (Some(target), state) if state.is_fixable() => Some(format!("use `{}`", target.get_raw())),
        _ => None,
    };
    if let Some(help) = help {
        builder = builder.help(help);
    }
    if let Some(related) = related_of(analysis, id, verdict) {
        builder = builder.related(related);
    }
    if let Some(fix) = fix_of(instance, verdict) {
        builder = builder.fix(fix);
    }
    builder.build()
}

/// The other instances of the dependency of `id`, in collection order.
fn counterparts<'a, 'w>(analysis: &'a Analysis<'w>, id: InstanceId) -> impl Iterator<Item = &'a Instance<'w>> {
    let instance = analysis.instances.get(id);
    analysis
        .instances
        .named(&instance.name)
        .iter()
        .copied()
        .filter(move |&other| other != id)
        .map(move |other| analysis.instances.get(other))
        .filter(move |other| other.alias_target() == instance.alias_target())
}

/// The instance the finding is about: the one that holds the version `id` should take, or, when the instances
/// cannot be ranked, the first one that differs from it.
fn related_of(analysis: &Analysis<'_>, id: InstanceId, verdict: &Verdict) -> Option<Related> {
    let instance = analysis.instances.get(id);
    let (counterpart, why) = match (&verdict.target, verdict.state) {
        (Some(target), state) if state.is_fixable() && state != State::IsBanned => (
            counterparts(analysis, id).find(|other| other.raw() == target.get_raw())?,
            "holds the version this should align with",
        ),
        (
            None,
            State::NonSemverMismatch
            | State::SameRangeMismatch
            | State::SameMinorMismatch
            | State::SameMinorHasMajorMismatch,
        ) => (
            counterparts(analysis, id).find(|other| other.raw() != instance.raw())?,
            "differs from it",
        ),
        _ => return None,
    };
    Some(Related::new(
        format!(
            "`{}@{}` in {} {why}",
            counterpart.name,
            counterpart.raw(),
            counterpart.package.display_name()
        ),
        counterpart.location(),
    ))
}

/// The fix of a fixable state: the entry is removed when banned, and set to the chosen specifier otherwise.
///
/// The specifier is written as the group chose it, never re-rendered. Its safety follows ADR 0051.
fn fix_of(instance: &Instance<'_>, verdict: &Verdict) -> Option<Fix> {
    if !verdict.state.is_fixable() {
        return None;
    }
    let (title, op, safety) = if verdict.state == State::IsBanned {
        (
            format!("Remove `{}`", instance.name),
            instance.remove()?,
            FixSafety::Unsafe,
        )
    } else {
        let target = verdict.target.as_ref()?;
        (
            format!("Set `{}` to `{}`", instance.name, target.get_raw()),
            instance.set_specifier(target.get_raw())?,
            alignment_safety(instance, verdict.state, target),
        )
    };
    Some(Fix::new(title, safety, vec![op]))
}

/// How safe rewriting `instance` to `target` is (ADR 0051).
///
/// A pin is a version the configuration names, so moving to it is safe. A version the workspace decides (the highest
/// or lowest in use, the version of a snap target or of the workspace package) is safe only as a compatible upgrade
/// that keeps the specifier's shape. Range changes and the fixes of policy conflicts change what a configuration
/// asked for, and stay unsafe.
fn alignment_safety(instance: &Instance<'_>, state: State, target: &Specifier) -> FixSafety {
    let safe = match state {
        State::DiffersToPin => true,
        State::DiffersToHighestOrLowestSemver | State::DiffersToSnapTarget | State::DiffersToLocal => {
            is_compatible_upgrade(&instance.specifier, target)
        }
        _ => false,
    };
    if safe { FixSafety::Safe } else { FixSafety::Unsafe }
}

/// A finding about a configured version group, located in the configuration file.
fn group_diagnostic(state: State, config_file: &Path, group: &ConfiguredGroup, what: &str, help: &str) -> Diagnostic {
    let name = group.label.as_ref().map_or_else(
        || format!("`version-groups[{}]`", group.index),
        |label| format!("`{label}` (`version-groups[{}]`)", group.index),
    );
    diagnostic(
        state.rule(),
        format!("the version group {name} {what}"),
        Location::new(config_file.to_path_buf(), None),
    )
    .help(help)
    .detail(state.name())
    .build()
}
