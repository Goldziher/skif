//! Turns the verdicts of an [`Analysis`] into diagnostics.

use std::path::PathBuf;

use skif_core::{Diagnostic, Fix, FixSafety, Location, diagnostic};
use skif_lint::Emitter;

use super::analysis::{Analysis, UnusedGroup};
use super::instances::Instance;
use super::state::{State, Verdict};

/// The configuration file `UnusedGroup` diagnostics point at.
const CONFIG_FILE: &str = "skif.toml";

/// Emits one diagnostic per non-valid instance, then one per unused group.
pub(super) fn emit(analysis: &Analysis<'_>, out: &mut Emitter<'_>) {
    for (id, instance) in analysis.instances.iter() {
        if let Some(verdict) = &analysis.verdicts[id.index()] {
            out.emit(instance_diagnostic(instance, verdict));
        }
    }
    for unused in &analysis.unused_groups {
        out.emit(unused_group_diagnostic(unused));
    }
}

fn instance_diagnostic(instance: &Instance<'_>, verdict: &Verdict) -> Diagnostic {
    let state = verdict.state;
    let package = instance.package.display_name();
    let subject = match instance.kind() {
        Some(kind) => format!(
            "`{}@{}` in `{}` of {package}",
            instance.name,
            instance.raw(),
            kind.field()
        ),
        None => format!("the version `{}` of {package}", instance.raw()),
    };
    let mut builder = diagnostic(
        state.rule(),
        format!("{subject} {}", state.summary()),
        instance.location(),
    )
    .package(package)
    .dependency(&instance.name)
    .detail(state.name());
    let help = match (&verdict.target, state) {
        (_, State::IsBanned) => Some(format!("remove `{}`", instance.name)),
        (Some(target), state) if state.is_fixable() => Some(format!("use `{}`", target.get_raw())),
        _ => None,
    };
    if let Some(help) = help {
        builder = builder.help(help);
    }
    if let Some(fix) = fix_of(instance, verdict) {
        builder = builder.fix(fix);
    }
    builder.build()
}

/// The fix of a fixable state: the entry is removed when banned, and set to the chosen specifier otherwise.
///
/// The specifier is written as the group chose it, never re-rendered. Changing what gets installed is unsafe.
fn fix_of(instance: &Instance<'_>, verdict: &Verdict) -> Option<Fix> {
    if !verdict.state.is_fixable() {
        return None;
    }
    let (title, op) = if verdict.state == State::IsBanned {
        (format!("Remove `{}`", instance.name), instance.remove()?)
    } else {
        let target = verdict.target.as_ref()?.get_raw();
        (
            format!("Set `{}` to `{target}`", instance.name),
            instance.set_specifier(target)?,
        )
    };
    Some(Fix {
        title,
        safety: FixSafety::Unsafe,
        ops: vec![op],
    })
}

fn unused_group_diagnostic(unused: &UnusedGroup) -> Diagnostic {
    let state = State::UnusedGroup;
    let name = unused.label.as_ref().map_or_else(
        || format!("`version-groups[{}]`", unused.index),
        |label| format!("`{label}` (`version-groups[{}]`)", unused.index),
    );
    diagnostic(
        state.rule(),
        format!("the version group {name} owns no dependency"),
        Location {
            path: PathBuf::from(CONFIG_FILE),
            span: None,
        },
    )
    .help("an earlier group may claim its dependencies, or its selectors match nothing")
    .detail(state.name())
    .build()
}
