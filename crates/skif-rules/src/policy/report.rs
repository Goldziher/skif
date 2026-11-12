//! Turns the verdicts of an [`Analysis`] into diagnostics.

use skif_core::HashMap;
use std::path::Path;

use skif_core::{Diagnostic, Fix, FixSafety, Location, Related, diagnostic};
use skif_lint::Emitter;
use skif_specifier::Specifier;
use skif_workspace::map_ordered;

use super::analysis::{Analysis, ConfiguredGroup};
use super::instances::{Instance, InstanceId};
use super::specifiers::is_compatible_upgrade;
use super::state::{State, Verdict};

/// Emits one diagnostic per non-valid instance, then one per unused group and one per unusable pin; the latter two
/// point at `config_file`.
///
/// Building the instance diagnostics (messages, locations, the counterpart, the fix) is most of the provider's time on
/// a large workspace with many findings, so they are built in parallel (unless the run is `sequential`) and then
/// emitted in instance order.
pub(super) fn emit(analysis: &Analysis<'_>, config_file: &Path, out: &mut Emitter<'_>, sequential: bool) {
    let findings: Vec<(InstanceId, &Instance<'_>, &Verdict)> = analysis
        .instances
        .iter()
        .filter_map(|(id, instance)| Some((id, instance, analysis.verdicts[id.index()].as_ref()?)))
        .collect();
    // The holders are only looked up for a finding: a workspace without one skips indexing every instance.
    if !findings.is_empty() {
        let holders = Holders::index(analysis, sequential);
        let diagnostics: Vec<Diagnostic> = map_ordered(&findings, sequential, |&(id, instance, verdict)| {
            instance_diagnostic(analysis, &holders, id, instance, verdict)
        });
        out.emit_all(diagnostics);
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

/// The instances of each dependency (and alias target) that hold each specifier: the first two in collection order,
/// so the one an aligned finding points at is found without scanning every instance of the dependency.
struct Holders<'a> {
    by_name: HashMap<&'a str, NameHolders<'a>>,
}

/// The holders of one dependency.
#[derive(Default)]
struct NameHolders<'a> {
    /// The first two instances holding each specifier.
    by_specifier: HashMap<HolderKey<'a>, [Option<InstanceId>; 2]>,
    /// Per alias target, the first instance and the first one written differently from it.
    first_two_written: HashMap<Option<&'a str>, (InstanceId, Option<InstanceId>)>,
}

/// The package an alias installs (if another one) and a specifier.
type HolderKey<'a> = (Option<&'a str>, &'a str);

impl<'a> Holders<'a> {
    /// Indexes each dependency on its own, in parallel unless the run is `sequential`: one map of every instance was a
    /// visible serial step on a large workspace.
    fn index(analysis: &'a Analysis<'_>, sequential: bool) -> Self {
        let names: Vec<(&'a str, &'a [InstanceId])> = analysis.instances.names().collect();
        let maps = map_ordered(&names, sequential, |&(_, ids)| {
            let mut holders = NameHolders::default();
            for &id in ids {
                let instance = analysis.instances.get(id);
                let target = instance.alias_target();
                let slots = holders.by_specifier.entry((target, instance.raw())).or_default();
                if let Some(slot) = slots.iter_mut().find(|slot| slot.is_none()) {
                    *slot = Some(id);
                }
                let (first, differing) = holders.first_two_written.entry(target).or_insert((id, None));
                if differing.is_none() && analysis.instances.get(*first).raw() != instance.raw() {
                    *differing = Some(id);
                }
            }
            holders
        });
        Self {
            by_name: names.iter().map(|&(name, _)| name).zip(maps).collect(),
        }
    }

    /// The first instance other than `id` of the dependency of `instance` (and its alias target) that holds `raw`, in
    /// collection order.
    fn other(&self, id: InstanceId, instance: &Instance<'_>, raw: &str) -> Option<InstanceId> {
        self.by_name
            .get(&*instance.name)?
            .by_specifier
            .get(&(instance.alias_target(), raw))?
            .iter()
            .flatten()
            .copied()
            .find(|&other| other != id)
    }

    /// The first instance of the dependency of `instance` (and its alias target) written differently from it, in
    /// collection order: the first instance when that differs, otherwise the first one that differs from the first.
    fn differing(&self, analysis: &Analysis<'_>, instance: &Instance<'_>) -> Option<InstanceId> {
        let &(first, differing) = self
            .by_name
            .get(&*instance.name)?
            .first_two_written
            .get(&instance.alias_target())?;
        if analysis.instances.get(first).raw() == instance.raw() {
            differing
        } else {
            Some(first)
        }
    }
}

fn instance_diagnostic(
    analysis: &Analysis<'_>,
    holders: &Holders<'_>,
    id: InstanceId,
    instance: &Instance<'_>,
    verdict: &Verdict,
) -> Diagnostic {
    let state = verdict.state;
    let package = instance.package.display_name();
    let message = if instance.is_own_version() {
        concat(&["the version `", instance.raw(), "` of ", package, " ", state.summary()])
    } else {
        concat(&[
            "`",
            &instance.name,
            "@",
            instance.raw(),
            "` in `",
            instance.ty().label(),
            "` of ",
            package,
            " ",
            state.summary(),
        ])
    };
    let mut builder = diagnostic(state.rule(), message, instance.location())
        .package(package)
        .package_id(instance.package.id)
        .dependency(&*instance.name)
        .specifier(instance.raw())
        .detail(state.name());
    let help = match (&verdict.target, state) {
        (_, State::IsBanned) => Some(concat(&["remove `", &instance.name, "`"])),
        (Some(target), state) if state.is_fixable() => Some(concat(&["use `", target.get_raw(), "`"])),
        _ => None,
    };
    if let Some(help) = help {
        builder = builder.help(help);
    }
    if let Some(related) = related_of(analysis, holders, id, verdict) {
        builder = builder.related(related);
    }
    if let Some(fix) = fix_of(instance, verdict) {
        builder = builder.fix(fix);
    }
    builder.build()
}

/// `parts` joined, in one allocation: the messages of tens of thousands of findings are built this way, and
/// `format!` was a visible part of building them.
fn concat(parts: &[&str]) -> String {
    let mut text = String::with_capacity(parts.iter().map(|part| part.len()).sum());
    for part in parts {
        text.push_str(part);
    }
    text
}

/// The instance the finding is about: the one that holds the version `id` should take, or, when the instances
/// cannot be ranked, the first one that differs from it.
fn related_of(analysis: &Analysis<'_>, holders: &Holders<'_>, id: InstanceId, verdict: &Verdict) -> Option<Related> {
    let instance = analysis.instances.get(id);
    let (counterpart, why) = match (&verdict.target, verdict.state) {
        (Some(target), state) if state.is_fixable() && state != State::IsBanned => (
            analysis.instances.get(holders.other(id, instance, target.get_raw())?),
            "holds the version this should align with",
        ),
        (
            None,
            State::NonSemverMismatch
            | State::SameRangeMismatch
            | State::SameMinorMismatch
            | State::SameMinorHasMajorMismatch,
        ) => (
            analysis.instances.get(holders.differing(analysis, instance)?),
            "differs from it",
        ),
        _ => return None,
    };
    Some(Related::new(
        concat(&[
            "`",
            &counterpart.name,
            "@",
            counterpart.raw(),
            "` in ",
            counterpart.package.display_name(),
            " ",
            why,
        ]),
        counterpart.location(),
    ))
}

/// The fix of a fixable state: the entry is removed when banned, and set to the chosen specifier otherwise.
///
/// The specifier is written as the group chose it, never re-rendered. Its safety follows ADR 0009.
fn fix_of(instance: &Instance<'_>, verdict: &Verdict) -> Option<Fix> {
    if !verdict.state.is_fixable() {
        return None;
    }
    let (title, op, safety) = if verdict.state == State::IsBanned {
        (
            concat(&["Remove `", &instance.name, "`"]),
            instance.remove()?,
            FixSafety::Unsafe,
        )
    } else {
        let target = verdict.target.as_ref()?;
        let raw = target.get_raw();
        (
            concat(&["Set `", &instance.name, "` to `", raw, "`"]),
            instance.set_specifier(raw)?,
            alignment_safety(instance, verdict.state, target),
        )
    };
    Some(Fix::new(title, safety, vec![op]))
}

/// How safe rewriting `instance` to `target` is (ADR 0009).
///
/// Whatever chose the target (a pin, the highest or lowest version in use, a snap target, the workspace package),
/// moving to it is safe only as a compatible upgrade that keeps the specifier's shape: a pin can name another major,
/// a lower version or another range as easily as the workspace can. Range changes and the fixes of policy conflicts
/// change what a configuration asked for, and stay unsafe.
fn alignment_safety(instance: &Instance<'_>, state: State, target: &Specifier) -> FixSafety {
    let safe = match state {
        State::DiffersToPin
        | State::DiffersToHighestOrLowestSemver
        | State::DiffersToSnapTarget
        | State::DiffersToLocal => is_compatible_upgrade(&instance.specifier, target),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{config, lint, root};

    type Row = (String, String, FixSafety);

    fn manifest(name: &str, react: &str) -> String {
        format!(r#"{{"name":"{name}","version":"1.0.0","private":true,"dependencies":{{"react":"{react}"}}}}"#)
    }

    /// The package, state and fix safety of each `version-mismatch` in the workspace `files`, by package.
    fn rows(files: &[(String, String)], toml: &str) -> Vec<Row> {
        let borrowed: Vec<(&str, &str)> = files
            .iter()
            .map(|(path, text)| (path.as_str(), text.as_str()))
            .collect();
        let mut rows: Vec<Row> = lint("version-mismatch", &borrowed, &config(toml))
            .into_iter()
            .map(|diagnostic| {
                (
                    diagnostic.package.clone().unwrap_or_default(),
                    diagnostic.detail.clone().unwrap_or_default(),
                    diagnostic.fixes[0].safety,
                )
            })
            .collect();
        rows.sort_by(|left, right| (&left.0, &left.1).cmp(&(&right.0, &right.1)));
        rows
    }

    /// [`rows`] for a workspace whose `app`, `lib` and `tool` packages depend on `react` at the given specifiers.
    fn safety(config_toml: &str, app: &str, lib: &str, tool: &str) -> Vec<Row> {
        let files = [
            ("package.json".to_owned(), root("")),
            ("packages/app/package.json".to_owned(), manifest("app", app)),
            ("packages/lib/package.json".to_owned(), manifest("lib", lib)),
            ("packages/tool/package.json".to_owned(), manifest("tool", tool)),
        ];
        rows(&files, config_toml)
    }

    fn row(package: &str, state: &str, safety: FixSafety) -> Row {
        (package.to_owned(), state.to_owned(), safety)
    }

    #[test]
    fn a_pin_is_safe_only_as_a_compatible_upgrade() {
        let pinned = |pin: &str| {
            format!("[[version-groups]]\ndependencies = [\"react\"]\npolicy = \"pinned\"\npin = \"{pin}\"\n")
        };
        // A later patch is safe; a cross-major move and a downgrade are not.
        assert_eq!(
            safety(&pinned("18.3.1"), "18.3.0", "17.0.2", "18.3.2"),
            [
                row("app", "DiffersToPin", FixSafety::Safe),
                row("lib", "DiffersToPin", FixSafety::Unsafe),
                row("tool", "DiffersToPin", FixSafety::Unsafe),
            ]
        );
        // A change of range and a move away from a prerelease are not safe either.
        assert_eq!(
            safety(&pinned("^18.3.1"), "18.3.1", "^18.2.0", "^18.3.1-rc.0"),
            [
                row("app", "DiffersToPin", FixSafety::Unsafe),
                row("lib", "DiffersToPin", FixSafety::Safe),
                row("tool", "DiffersToPin", FixSafety::Unsafe),
            ]
        );
        // Nor is a move to a prerelease.
        assert_eq!(
            safety(&pinned("^19.0.0-rc.1"), "^19.0.0-rc.0", "^19.0.0-rc.1", "^19.0.0-rc.1"),
            [row("app", "DiffersToPin", FixSafety::Unsafe)]
        );
    }

    #[test]
    fn a_snap_target_is_safe_only_as_a_compatible_upgrade() {
        let toml = "[[version-groups]]\ndependencies = [\"react\"]\npolicy = \"snapped-to\"\nsnap-to = [\"app\"]\n";
        assert_eq!(
            safety(toml, "^18.3.1", "^18.2.0", "^19.0.0"),
            [
                row("lib", "DiffersToSnapTarget", FixSafety::Safe),
                row("tool", "DiffersToSnapTarget", FixSafety::Unsafe),
            ]
        );
    }

    #[test]
    fn prefer_lowest_never_downgrades_on_its_own() {
        assert_eq!(
            safety("[policy]\nprefer = \"lowest\"\n", "^18.2.0", "^18.3.1", "^18.2.0"),
            [row("lib", "DiffersToHighestOrLowestSemver", FixSafety::Unsafe)]
        );
    }

    #[test]
    fn the_workspace_package_version_is_safe_only_as_a_compatible_upgrade() {
        let files = [
            ("package.json".to_owned(), root("")),
            (
                "packages/react/package.json".to_owned(),
                r#"{"name":"react","version":"18.3.1","private":true}"#.to_owned(),
            ),
            ("packages/app/package.json".to_owned(), manifest("app", "18.3.0")),
            ("packages/tool/package.json".to_owned(), manifest("tool", "17.0.2")),
        ];
        assert_eq!(
            rows(&files, ""),
            [
                row("app", "DiffersToLocal", FixSafety::Safe),
                row("tool", "DiffersToLocal", FixSafety::Unsafe),
            ]
        );
    }

    #[test]
    fn a_finding_without_a_target_points_at_the_first_instance_written_differently() {
        let files = [
            ("package.json".to_owned(), root("")),
            ("packages/a/package.json".to_owned(), manifest("a", "^17.0.0")),
            ("packages/b/package.json".to_owned(), manifest("b", "^17.0.0")),
            ("packages/c/package.json".to_owned(), manifest("c", "^18.0.0")),
            ("packages/d/package.json".to_owned(), manifest("d", "^17.0.0")),
        ];
        let borrowed: Vec<(&str, &str)> = files
            .iter()
            .map(|(path, text)| (path.as_str(), text.as_str()))
            .collect();
        let toml = "[[version-groups]]\ndependencies = [\"react\"]\npolicy = \"same-range\"\n";
        let related: Vec<(String, String)> = lint("version-mismatch", &borrowed, &config(toml))
            .into_iter()
            .map(|diagnostic| {
                (
                    diagnostic.package.clone().unwrap_or_default(),
                    diagnostic.related[0].message.clone(),
                )
            })
            .collect();
        let row = |package: &str, message: &str| (package.to_owned(), message.to_owned());
        assert_eq!(
            related,
            [
                row("a", "`react@^18.0.0` in c differs from it"),
                row("b", "`react@^18.0.0` in c differs from it"),
                row("c", "`react@^17.0.0` in a differs from it"),
                row("d", "`react@^18.0.0` in c differs from it"),
            ]
        );
    }
}
