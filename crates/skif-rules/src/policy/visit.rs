//! The state machine of every version-group policy: which [`State`] each instance of a dependency ends in.
//!
//! The behaviour follows syncpack's version-group visitors (`src/version_group/*.rs`, commit 958d306, MIT,
//! Copyright (c) 2017-2023 Jamie Mason; see `ATTRIBUTIONS.md`). Differences from syncpack:
//!
//! * there is no strict mode: on a dependency that names a workspace package, a `workspace:` specifier, `*`/`latest`
//!   and a `link:`/`file:` path that resolves to the package are always valid, as is any range the package's own
//!   version satisfies (path specifiers are the business of `local-path-dependency`);
//! * a `workspace:` specifier is never rewritten to a registry range, and never the target of one: where the package
//!   has no version to align with, such instances take no part in the alignment;
//! * instances using the `catalog:` protocol are left to the catalog rules: they never get a state here and take no
//!   part in a preferred-semver group, whose other members still align with each other;
//! * `latest` and dist-tags are never the target of an alignment: next to a version they are an unfixable
//!   `NonSemverMismatch`;
//! * a specifier that cannot carry a semver group's range (a git URL, `file:`, `latest`...) has no preferred range,
//!   where syncpack would fail when asked to rewrite it;
//! * specifiers that are byte-identical count as having the same version number, which keeps `latest` valid next to
//!   `latest`.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use skif_config::{DependencyType, Prefer};
use skif_core::DependencyKind;
use skif_specifier::Specifier;
use skif_specifier::semver_range::SemverRange;

use super::groups::{Assignment, Policy};
use super::instances::{Instance, InstanceId, Instances};
use super::specifiers::floor_version;
use super::state::{State, Verdict};

/// A state together with the specifier the instance should have.
type Outcome = Option<(State, Option<Arc<Specifier>>)>;

pub(super) struct Visitor<'a, 'w> {
    instances: &'a Instances<'w>,
    assignment: &'a Assignment<'a>,
    /// The verdicts given so far, in the order they were given; a later one for the same instance replaces it.
    marks: Vec<(InstanceId, Verdict)>,
}

impl<'a, 'w> Visitor<'a, 'w> {
    pub(super) const fn new(instances: &'a Instances<'w>, assignment: &'a Assignment<'a>) -> Self {
        Self {
            instances,
            assignment,
            marks: Vec::new(),
        }
    }

    /// The verdicts given, in order; apply them in order, a later one for an instance replacing an earlier one.
    pub(super) fn into_marks(self) -> Vec<(InstanceId, Verdict)> {
        self.marks
    }

    /// Applies `policy` to the instances `ids` of one dependency.
    pub(super) fn visit(&mut self, policy: &Policy, ids: &[InstanceId]) {
        match policy {
            Policy::Ignored | Policy::InvalidPin => {}
            Policy::Banned => self.banned(ids),
            Policy::Pinned(pin) => self.pinned(ids, pin),
            Policy::SameRange => self.same_range(ids),
            Policy::SameMinor => self.same_minor(ids),
            Policy::SnappedTo(packages) => self.snapped_to(ids, packages),
            Policy::PreferredSemver(prefer) => self.preferred_semver(ids, *prefer),
        }
    }

    fn spec(&self, id: InstanceId) -> &Arc<Specifier> {
        &self.instances.get(id).specifier
    }

    fn is_own_version(&self, id: InstanceId) -> bool {
        self.instances.get(id).is_own_version()
    }

    fn mark(&mut self, id: InstanceId, state: State, target: Option<Arc<Specifier>>) {
        self.marks.push((id, Verdict { state, target }));
    }

    fn mark_outcome(&mut self, id: InstanceId, outcome: Outcome) {
        if let Some((state, target)) = outcome {
            self.mark(id, state, target);
        }
    }

    /// The instances the policies align: those not using the `catalog:` protocol, and not a peer range written as a
    /// compatibility window (`>=17 <19`, `^17 || ^18`), which states what is accepted rather than what is installed.
    fn members(&self, ids: &[InstanceId]) -> Vec<InstanceId> {
        ids.iter()
            .copied()
            .filter(|&id| !self.spec(id).is_catalog() && !self.is_peer_window(id))
            .collect()
    }

    fn is_peer_window(&self, id: InstanceId) -> bool {
        self.instances.get(id).kind() == Some(DependencyKind::PeerDependencies)
            && matches!(**self.spec(id), Specifier::ComplexSemver(_))
    }

    /// The members a group may rewrite to `target`: a `workspace:` specifier stays what it is unless the target is one
    /// too (the protocol is never replaced by a registry version).
    fn rewritable(&self, ids: &[InstanceId], target: Option<&Specifier>) -> Vec<InstanceId> {
        let keeps_protocol = target.is_none_or(|target| !target.is_workspace_protocol());
        self.members(ids)
            .into_iter()
            .filter(|&id| !(keeps_protocol && self.spec(id).is_workspace_protocol()))
            .collect()
    }

    fn equals(&self, id: InstanceId, other: &Specifier) -> bool {
        self.spec(id).get_raw() == other.get_raw()
    }

    /// Whether every instance of `ids` is written the same way. One pass: a member that is a member of `ids` itself
    /// differs from some other member exactly when they are not all equal.
    fn all_equal(&self, ids: &[InstanceId]) -> bool {
        ids.first()
            .is_none_or(|&first| ids.iter().all(|&other| self.equals(other, self.spec(first))))
    }

    fn same_version(&self, id: InstanceId, other: &Specifier) -> bool {
        self.spec(id).has_same_version_number_as(other) || self.equals(id, other)
    }

    fn preferred(&self, id: InstanceId) -> Option<&SemverRange> {
        self.assignment.preferred(id)
    }

    /// Whether the instance already uses the range its semver group asks for.
    fn matches_preferred(&self, id: InstanceId) -> bool {
        self.preferred(id)
            .is_some_and(|range| self.spec(id).get_semver_range().as_ref() == Some(range))
    }

    /// Whether the semver group asks for a range other than `range`.
    fn prefers_other_than(&self, id: InstanceId, range: &SemverRange) -> bool {
        self.preferred(id).is_some_and(|preferred| preferred != range)
    }

    /// The instance's specifier rewritten to its semver group's range.
    fn with_preferred(&self, id: InstanceId) -> Option<Arc<Specifier>> {
        self.spec(id).with_range(self.preferred(id)?)
    }

    /// Whether the semver group's range would accept `other`.
    fn preferred_satisfies(&self, id: InstanceId, other: &Specifier) -> bool {
        self.with_preferred(id)
            .and_then(|specifier| specifier.get_node_range())
            .zip(other.get_node_range())
            .is_some_and(|(ours, theirs)| ours.allows_any(&theirs))
    }

    /// The outcome of an instance on its semver group's range when it already has the required version number but
    /// the group wants a range other than `target`'s: the group wins unless it cannot accept `target`.
    fn range_against_group(&self, id: InstanceId, target: &Specifier, conflicts: [State; 2]) -> Outcome {
        let satisfies = self.preferred_satisfies(id, target);
        match (self.matches_preferred(id), satisfies) {
            (true, true) => None,
            (true, false) => Some((conflicts[0], None)),
            (false, true) => Some((State::SemverRangeMismatch, self.with_preferred(id))),
            (false, false) => Some((conflicts[1], None)),
        }
    }

    fn banned(&mut self, ids: &[InstanceId]) {
        for &id in ids {
            let state = if self.is_own_version(id) {
                State::RefuseToBanLocal
            } else {
                State::IsBanned
            };
            self.mark(id, state, None);
        }
    }

    fn pinned(&mut self, ids: &[InstanceId], pin: &Arc<Specifier>) {
        for id in self.rewritable(ids, Some(&**pin)) {
            let state = if self.is_own_version(id) {
                State::RefuseToPinLocal
            } else if self.equals(id, pin) {
                continue;
            } else if !self.same_version(id, pin) {
                State::DiffersToPin
            } else if pin
                .get_semver_range()
                .is_some_and(|range| self.prefers_other_than(id, &range))
            {
                if self.matches_preferred(id) {
                    State::PinOverridesSemverRange
                } else {
                    State::PinOverridesSemverRangeMismatch
                }
            } else {
                State::DiffersToPin
            };
            self.mark(id, state, Some(Arc::clone(pin)));
        }
    }

    fn same_range(&mut self, ids: &[InstanceId]) {
        let members = self.rewritable(ids, None);
        let (ranged, odd): (Vec<InstanceId>, Vec<InstanceId>) = members
            .iter()
            .copied()
            .partition(|&id| self.spec(id).get_node_range().is_some());
        if !odd.is_empty() && !self.all_equal(&members) {
            for id in odd {
                self.mark(id, State::SameRangeMismatch, None);
            }
        }
        // Equal specifiers agree on every comparison, so each distinct one is judged once against the distinct set:
        // a dependency shared by thousands of packages costs a handful of range comparisons, not millions.
        let mut distinct: Vec<Arc<Specifier>> = Vec::new();
        for &id in &ranged {
            let specifier = self.spec(id);
            if !distinct.iter().any(|seen| seen.get_raw() == specifier.get_raw()) {
                distinct.push(Arc::clone(specifier));
            }
        }
        let mut verdicts: HashMap<String, bool> = HashMap::new();
        for id in ranged {
            let specifier = self.spec(id);
            let satisfies = *verdicts
                .entry(specifier.get_raw().to_owned())
                .or_insert_with(|| specifier.satisfies_all(&distinct));
            if !satisfies {
                self.mark(id, State::SameRangeMismatch, None);
            } else if self.preferred(id).is_some() && !self.matches_preferred(id) {
                let target = self.with_preferred(id);
                self.mark(id, State::SemverRangeMismatch, target);
            }
        }
    }

    fn same_minor(&mut self, ids: &[InstanceId]) {
        let members = self.rewritable(ids, None);
        let versions: Vec<Option<(u64, u64)>> = members
            .iter()
            .map(|&id| self.spec(id).get_node_version().map(|v| (v.major(), v.minor())))
            .collect();
        if versions.iter().all(Option::is_none) {
            self.without_semver(&members);
            return;
        }
        let Some(first) = versions.first().copied().flatten() else {
            self.mark_all(&members, State::SameMinorHasMajorMismatch);
            return;
        };
        let same = |matching: fn(&(u64, u64), &(u64, u64)) -> bool| {
            versions
                .iter()
                .all(|version| version.as_ref().is_some_and(|v| matching(v, &first)))
        };
        if !same(|a, b| a.0 == b.0) {
            self.mark_all(&members, State::SameMinorHasMajorMismatch);
        } else if !same(|a, b| a == b) {
            self.mark_all(&members, State::SameMinorMismatch);
        } else {
            for id in members {
                let outcome = self.at_correct_minor(id);
                self.mark_outcome(id, outcome);
            }
        }
    }

    /// A same-minor group forbids ranges wider than a tilde, and keeps a semver group's range when that is safe.
    ///
    /// An instance that already is the tilde the group forces is valid, whatever the semver group asks for, so the
    /// fix converges.
    fn at_correct_minor(&self, id: InstanceId) -> Outcome {
        let is_safe = |range: &SemverRange| matches!(range, SemverRange::Exact | SemverRange::Patch);
        let tilde = || self.spec(id).with_range(&SemverRange::Patch);
        match self.preferred(id) {
            Some(preferred) if is_safe(preferred) => {
                if self.matches_preferred(id) {
                    None
                } else {
                    Some((State::SemverRangeMismatch, self.with_preferred(id)))
                }
            }
            Some(_) => {
                let target = tilde();
                if target.as_ref().is_some_and(|target| self.equals(id, target)) {
                    return None;
                }
                let state = if self.matches_preferred(id) {
                    State::SameMinorOverridesSemverRange
                } else {
                    State::SameMinorOverridesSemverRangeMismatch
                };
                Some((state, target))
            }
            None => match self.spec(id).get_semver_range() {
                Some(range) if !is_safe(&range) => Some((State::SameMinorOverridesSemverRange, tilde())),
                _ => None,
            },
        }
    }

    fn snapped_to(&mut self, ids: &[InstanceId], packages: &[String]) {
        let Some(&first) = ids.first() else { return };
        let first = self.instances.get(first);
        // The first package of `snap-to` that declares the dependency is the target. Where it declares it in several
        // sections, a production dependency beats a development one beats any other, and a peer range stated as a
        // compatibility window is never the target.
        let target = packages
            .iter()
            .find_map(|package| {
                self.instances
                    .named(&first.name)
                    .iter()
                    .copied()
                    .filter(|&id| {
                        let instance = self.instances.get(id);
                        instance.alias_target() == first.alias_target()
                            && instance.package.name.as_deref() == Some(package.as_str())
                            && !self.is_peer_window(id)
                    })
                    .min_by_key(|&id| rank(self.instances.get(id)))
            })
            .map(|id| Arc::clone(self.spec(id)));
        let Some(target) = target else {
            for id in self.rewritable(ids, None) {
                self.mark(id, State::DependsOnMissingSnapTarget, None);
            }
            return;
        };
        if target.is_catalog() {
            return;
        }
        for id in self.rewritable(ids, Some(&*target)) {
            let outcome = self.snap_state(id, &target);
            self.mark_outcome(id, outcome);
        }
    }

    fn snap_state(&self, id: InstanceId, target: &Arc<Specifier>) -> Outcome {
        let own = self.is_own_version(id);
        if own && !self.equals(id, target) {
            return Some((State::RefuseToSnapLocal, Some(Arc::clone(target))));
        }
        if !self.same_version(id, target) {
            let fix = self
                .preferred(id)
                .and_then(|range| target.with_range(range))
                .unwrap_or_else(|| Arc::clone(target));
            return Some((State::DiffersToSnapTarget, Some(fix)));
        }
        let wants_other_range = target
            .get_semver_range()
            .is_some_and(|range| self.prefers_other_than(id, &range));
        if wants_other_range {
            return self.range_against_group(
                id,
                target,
                [
                    State::MatchConflictsWithSnapTarget,
                    State::MismatchConflictsWithSnapTarget,
                ],
            );
        }
        (!self.equals(id, target)).then(|| (State::DiffersToSnapTarget, Some(Arc::clone(target))))
    }

    fn preferred_semver(&mut self, ids: &[InstanceId], prefer: Prefer) {
        if let Some(local) = ids.iter().rev().copied().find(|&id| self.is_own_version(id)) {
            if is_exact_version(self.spec(local)) {
                self.align_with_local(ids, local);
            } else {
                self.reject_invalid_local(ids);
            }
            return;
        }
        // Without a version to align with, a `workspace:` specifier is neither a target nor a mismatch: the
        // protocol is never rewritten to a registry range, nor a registry range to the protocol.
        let members: Vec<InstanceId> = self
            .members(ids)
            .into_iter()
            .filter(|&id| !self.spec(id).is_workspace_protocol())
            .collect();
        self.align_to_extreme(&members, prefer);
    }

    /// The package's own version is not an exact version: nothing can be aligned with it.
    fn reject_invalid_local(&mut self, ids: &[InstanceId]) {
        for id in self.members(ids) {
            let state = if self.is_own_version(id) {
                State::InvalidLocalVersion
            } else {
                State::DependsOnInvalidLocalPackage
            };
            self.mark(id, state, None);
        }
    }

    /// Every dependent of a workspace package must use the version the package declares.
    fn align_with_local(&mut self, ids: &[InstanceId], local: InstanceId) {
        let local_specifier = Arc::clone(self.spec(local));
        for id in self.members(ids) {
            if !self.is_own_version(id) {
                let outcome = self.local_state(id, local, &local_specifier);
                self.mark_outcome(id, outcome);
            }
        }
    }

    fn local_state(&self, id: InstanceId, local: InstanceId, local_specifier: &Arc<Specifier>) -> Outcome {
        let differs = || Some((State::DiffersToLocal, Some(Arc::clone(local_specifier))));
        let specifier = self.spec(id);
        if specifier.is_link() || matches!(**specifier, Specifier::File(_)) {
            return if self.path_resolves_to(id, local) {
                None
            } else {
                differs()
            };
        }
        if specifier.is_workspace_protocol() || matches!(**specifier, Specifier::Latest(_)) {
            return None;
        }
        if self.same_version(id, local_specifier) && self.prefers_other_than(id, &SemverRange::Exact) {
            return self.range_against_group(
                id,
                local_specifier,
                [State::MatchConflictsWithLocal, State::MismatchConflictsWithLocal],
            );
        }
        let keeps_group_range = self.preferred(id).is_none() || self.matches_preferred(id);
        if self.equals(id, local_specifier) || (keeps_group_range && self.accepts(id, local_specifier)) {
            None
        } else {
            differs()
        }
    }

    /// Whether the range of `id` accepts the exact version `version`.
    fn accepts(&self, id: InstanceId, version: &Specifier) -> bool {
        version
            .get_node_version()
            .zip(self.spec(id).get_node_range())
            .is_some_and(|(version, range)| range.satisfies(&version))
    }

    /// Whether the `link:` or `file:` path of `id`, resolved from its package's directory, is the directory of
    /// `local`'s package.
    fn path_resolves_to(&self, id: InstanceId, local: InstanceId) -> bool {
        let raw = self.spec(id).get_raw();
        let target = ["link:", "file:"]
            .iter()
            .find_map(|prefix| raw.strip_prefix(prefix))
            .unwrap_or(raw);
        let from = self.instances.get(id).package.dir.join(target);
        normalized(&from).is_some_and(|from| Some(from) == normalized(&self.instances.get(local).package.dir))
    }

    /// Aligns every instance on the highest (or lowest) version in use, each carrying its own semver group's range.
    ///
    /// `latest` and dist-tags cannot be ranked: they are never the target, and one that differs from another instance
    /// is a `NonSemverMismatch` that has no fix.
    fn align_to_extreme(&mut self, ids: &[InstanceId], prefer: Prefer) {
        let (floating, ranked): (Vec<InstanceId>, Vec<InstanceId>) =
            ids.iter().copied().partition(|&id| is_floating(self.spec(id)));
        let target = {
            // Ranked by the lowest version each admits, so `^18` never outranks `^18.3.1`. The floor is worked out
            // once per distinct specifier of the dependency, not once per comparison: the instances of one dependency
            // repeat a few specifiers, and a specifier's text decides its floor (a `workspace:` one aside, which
            // depends on what it resolves to).
            let mut floors = Vec::new();
            let mut floor_of = |specifier: &Arc<Specifier>| {
                if matches!(**specifier, Specifier::WorkspaceProtocol(_)) {
                    return floor_version(specifier);
                }
                let raw = specifier.get_raw();
                if let Some((_, floor)) = floors.iter().find(|(known, _): &&(String, _)| known == raw) {
                    return Clone::clone(floor);
                }
                let floor = floor_version(specifier);
                floors.push((raw.to_owned(), floor.clone()));
                floor
            };
            let candidates: Vec<_> = ranked
                .iter()
                .copied()
                .filter(|&id| self.spec(id).get_node_version().is_some())
                .map(|id| {
                    let specifier = self.with_preferred(id).unwrap_or_else(|| Arc::clone(self.spec(id)));
                    (floor_of(&specifier), specifier)
                })
                .collect();
            let candidates = candidates.into_iter();
            let chosen = match prefer {
                Prefer::Lowest => candidates.min_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1))),
                _ => candidates.max_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1))),
            };
            chosen.map(|(_, specifier)| specifier)
        };
        let Some(target) = target else {
            self.without_semver(ids);
            return;
        };
        if !floating.is_empty() && !self.all_equal(ids) {
            for id in floating {
                self.mark(id, State::NonSemverMismatch, None);
            }
        }
        for id in ranked {
            let outcome = self.extreme_state(id, &target);
            self.mark_outcome(id, outcome);
        }
    }

    fn extreme_state(&self, id: InstanceId, target: &Arc<Specifier>) -> Outcome {
        if !self.same_version(id, target) {
            let fix = self
                .preferred(id)
                .and_then(|range| target.with_range(range))
                .unwrap_or_else(|| Arc::clone(target));
            return Some((State::DiffersToHighestOrLowestSemver, Some(fix)));
        }
        let wants_other_range = target
            .get_semver_range()
            .is_some_and(|range| self.prefers_other_than(id, &range));
        if wants_other_range {
            return self.range_against_group(
                id,
                target,
                [
                    State::MatchConflictsWithHighestOrLowestSemver,
                    State::MismatchConflictsWithHighestOrLowestSemver,
                ],
            );
        }
        if self.preferred(id).is_some() && !self.matches_preferred(id) {
            return self
                .preferred_satisfies(id, target)
                .then(|| (State::SemverRangeMismatch, self.with_preferred(id)));
        }
        (!self.equals(id, target)).then(|| (State::DiffersToHighestOrLowestSemver, Some(Arc::clone(target))))
    }

    /// No instance has a semver version: identical specifiers are fine, differing ones cannot be ranked.
    fn without_semver(&mut self, ids: &[InstanceId]) {
        let identical = ids
            .first()
            .is_none_or(|&first| ids.iter().all(|&id| self.equals(id, self.spec(first))));
        if !identical {
            self.mark_all(ids, State::NonSemverMismatch);
        }
    }

    fn mark_all(&mut self, ids: &[InstanceId], state: State) {
        for &id in ids {
            self.mark(id, state, None);
        }
    }
}

/// How early the place of an instance is preferred as the source of a snap target.
fn rank(instance: &Instance<'_>) -> u8 {
    match instance.ty() {
        DependencyType::Local => 0,
        DependencyType::Section(kind) => section_rank(*kind),
        _ => 2,
    }
}

/// How early a section is preferred as the source of a snap target: production, then development, then the rest.
const fn section_rank(kind: DependencyKind) -> u8 {
    match kind {
        DependencyKind::Dependencies => 0,
        DependencyKind::DevDependencies => 1,
        _ => 2,
    }
}

/// Whether the specifier is one exact version. Prerelease tags in capitals and build metadata (`1.0.0-RC.1`,
/// `1.0.0+build.5`) are exact versions even though the specifier parser only knows lowercase tags.
fn is_exact_version(specifier: &Specifier) -> bool {
    if matches!(specifier, Specifier::Exact(_)) {
        return true;
    }
    let raw = specifier.get_raw();
    let core = raw.split(['-', '+']).next().unwrap_or_default();
    core.split('.').count() == 3
        && core
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
        && Specifier::new_node_version(raw).is_some()
}

/// Whether the specifier names a moving target (`latest`, `*`, a dist-tag) rather than a version.
fn is_floating(specifier: &Specifier) -> bool {
    matches!(specifier, Specifier::Latest(_) | Specifier::Tag(_))
}

/// `path` with `.` and `..` components resolved lexically; `None` when a `..` climbs out of the path.
fn normalized(path: &Path) -> Option<PathBuf> {
    let mut resolved = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !resolved.pop() {
                    return None;
                }
            }
            other => resolved.push(other),
        }
    }
    Some(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::analysis::Analysis;
    use crate::testing::{config, workspace};

    const ROOT: &str =
        r#"{"name":"root","version":"0.0.0","private":true,"packageManager":"pnpm@9.0.0","workspaces":["packages/*"]}"#;

    /// `(package, dependency, state)` of every verdict, for a workspace of `members` and a configuration.
    fn states(toml: &str, members: &[(&str, &str)]) -> Vec<(String, String, &'static str)> {
        let config = config(toml);
        let mut files = vec![("package.json".to_owned(), ROOT.to_owned())];
        files.extend(
            members
                .iter()
                .map(|(name, manifest)| (format!("packages/{name}/package.json"), (*manifest).to_owned())),
        );
        let borrowed: Vec<(&str, &str)> = files.iter().map(|(p, c)| (p.as_str(), c.as_str())).collect();
        let workspace = workspace(&borrowed, &config);
        let analysis = Analysis::run(&workspace, &config, true);
        analysis
            .instances
            .iter()
            .filter_map(|(id, instance)| {
                let verdict = analysis.verdicts[id.index()].as_ref()?;
                Some((
                    instance.package.display_name().to_owned(),
                    instance.name.clone(),
                    verdict.state.name(),
                ))
            })
            .collect()
    }

    fn one(package: &str, dependency: &str, state: &'static str) -> (String, String, &'static str) {
        (package.to_owned(), dependency.to_owned(), state)
    }

    fn manifest(name: &str, version: &str, dependencies: &str) -> String {
        format!(r#"{{"name":"{name}","version":"{version}","dependencies":{{{dependencies}}}}}"#)
    }

    #[test]
    fn the_highest_version_wins_and_the_lowest_on_request() {
        let app = manifest("app", "1.0.0", r#""react":"^18.2.0""#);
        let lib = manifest("lib", "1.0.0", r#""react":"^18.3.1""#);
        let members = [("app", app.as_str()), ("lib", lib.as_str())];
        assert_eq!(
            states("", &members),
            [one("app", "react", "DiffersToHighestOrLowestSemver")]
        );
        assert_eq!(
            states("[policy]\nprefer = \"lowest\"\n", &members),
            [one("lib", "react", "DiffersToHighestOrLowestSemver")]
        );
    }

    #[test]
    fn local_packages_win_over_higher_versions() {
        let app = manifest("app", "1.0.0", r#""lib":"^2.0.0""#);
        let tool = manifest("tool", "1.0.0", r#""lib":"workspace:*""#);
        let lib = manifest("lib", "1.0.0", "");
        let members = [("app", app.as_str()), ("lib", lib.as_str()), ("tool", tool.as_str())];
        assert_eq!(states("", &members), [one("app", "lib", "DiffersToLocal")]);
    }

    #[test]
    fn ranges_the_local_version_satisfies_are_valid() {
        let cases = [
            ("^1.0.0", true),
            ("~1.0.0", true),
            ("^1.2.0", false),
            (">=1.0.0 <2.0.0", true),
            ("2.0.0", false),
            ("*", true),
            ("latest", true),
            ("workspace:^", true),
            ("workspace:~", true),
        ];
        for (specifier, valid) in cases {
            let app = manifest("app", "1.0.0", &format!(r#""lib":"{specifier}""#));
            let lib = manifest("lib", "1.0.0", "");
            let members = [("app", app.as_str()), ("lib", lib.as_str())];
            let expected = if valid {
                vec![]
            } else {
                vec![one("app", "lib", "DiffersToLocal")]
            };
            assert_eq!(states("", &members), expected, "{specifier}");
        }
    }

    #[test]
    fn a_semver_group_keeps_deciding_the_range_of_a_dependent_of_a_local_package() {
        let app = manifest("app", "1.0.0", r#""lib":"^1.0.0""#);
        let lib = manifest("lib", "1.0.0", "");
        let members = [("app", app.as_str()), ("lib", lib.as_str())];
        let toml = "[[semver-groups]]\ndependencies = [\"lib\"]\nrange = \"~\"\n";
        assert_eq!(states(toml, &members), [one("app", "lib", "SemverRangeMismatch")]);
    }

    #[test]
    fn file_paths_to_the_local_package_are_valid_and_other_paths_are_not() {
        let good = manifest("good", "1.0.0", r#""lib":"file:../lib""#);
        let bad = manifest("bad", "1.0.0", r#""lib":"file:../other""#);
        let lib = manifest("lib", "1.0.0", "");
        let other = manifest("other", "1.0.0", "");
        let members = [
            ("bad", bad.as_str()),
            ("good", good.as_str()),
            ("lib", lib.as_str()),
            ("other", other.as_str()),
        ];
        assert_eq!(states("", &members), [one("bad", "lib", "DiffersToLocal")]);
    }

    #[test]
    fn latest_and_tags_are_never_the_alignment_target() {
        let cases: [(&str, &str, &[&str]); 5] = [
            ("latest", "^5.4.5", &["NonSemverMismatch"]),
            ("beta", "^5.4.5", &["NonSemverMismatch"]),
            ("latest", "latest", &[]),
            ("beta", "latest", &["NonSemverMismatch", "NonSemverMismatch"]),
            ("^5.4.5", "^5.0.0", &["DiffersToHighestOrLowestSemver"]),
        ];
        for (first, second, expected) in cases {
            let app = manifest("app", "1.0.0", &format!(r#""ts":"{first}""#));
            let lib = manifest("lib", "1.0.0", &format!(r#""ts":"{second}""#));
            let members = [("app", app.as_str()), ("lib", lib.as_str())];
            let actual: Vec<&str> = states("", &members).into_iter().map(|(_, _, state)| state).collect();
            assert_eq!(actual, expected, "{first} {second}");
        }
    }

    #[test]
    fn a_floating_specifier_does_not_stop_the_versions_from_aligning() {
        let app = manifest("app", "1.0.0", r#""ts":"latest""#);
        let lib = manifest("lib", "1.0.0", r#""ts":"^5.4.5""#);
        let tool = manifest("tool", "1.0.0", r#""ts":"^5.0.0""#);
        let members = [("app", app.as_str()), ("lib", lib.as_str()), ("tool", tool.as_str())];
        assert_eq!(
            states("", &members),
            [
                one("app", "ts", "NonSemverMismatch"),
                one("tool", "ts", "DiffersToHighestOrLowestSemver"),
            ]
        );
    }

    #[test]
    fn a_link_that_climbs_out_of_the_workspace_never_names_a_local_package() {
        let root = r#"{"name":"top","version":"0.0.0","private":true,"packageManager":"pnpm@9.0.0","workspaces":["x"],"dependencies":{"x":"link:../x"}}"#;
        let x = manifest("x", "1.0.0", "");
        let files = [("package.json", root), ("x/package.json", x.as_str())];
        let config = config("");
        let workspace = workspace(&files, &config);
        let analysis = Analysis::run(&workspace, &config, true);
        let flagged: Vec<&str> = analysis
            .instances
            .iter()
            .filter_map(|(id, instance)| analysis.verdicts[id.index()].as_ref().map(|_| instance.name.as_str()))
            .collect();
        assert_eq!(
            flagged,
            ["x"],
            "`link:../x` from the root package does not point at the member `x`"
        );
    }

    #[test]
    fn links_are_compared_by_resolved_directory() {
        let good = manifest("good", "1.0.0", r#""lib":"link:../lib""#);
        let bad = manifest("bad", "1.0.0", r#""lib":"link:../other""#);
        let lib = manifest("lib", "1.0.0", "");
        let other = manifest("other", "1.0.0", "");
        let members = [
            ("bad", bad.as_str()),
            ("good", good.as_str()),
            ("lib", lib.as_str()),
            ("other", other.as_str()),
        ];
        assert_eq!(states("", &members), [one("bad", "lib", "DiffersToLocal")]);
    }

    #[test]
    fn invalid_local_versions_block_dependents() {
        let lib = manifest("lib", "^1.0.0", "");
        let app = manifest("app", "1.0.0", r#""lib":"workspace:*""#);
        let members = [("app", app.as_str()), ("lib", lib.as_str())];
        assert_eq!(
            states("", &members),
            [
                one("app", "lib", "DependsOnInvalidLocalPackage"),
                one("lib", "lib", "InvalidLocalVersion"),
            ]
        );
    }

    #[test]
    fn the_workspace_protocol_is_never_aligned_with_registry_ranges() {
        let lib = r#"{"name":"lib"}"#;
        let app = manifest("app", "1.0.0", r#""lib":"workspace:^1.2.0""#);
        let tool = manifest("tool", "1.0.0", r#""lib":"^1.0.0""#);
        let other = manifest("other", "1.0.0", r#""lib":"^1.1.0""#);
        let members = [
            ("app", app.as_str()),
            ("lib", lib),
            ("other", other.as_str()),
            ("tool", tool.as_str()),
        ];
        assert_eq!(
            states("", &members),
            [one("tool", "lib", "DiffersToHighestOrLowestSemver")],
            "only the registry ranges align with each other"
        );
    }

    #[test]
    fn a_package_without_a_version_is_not_a_local_instance() {
        let lib = r#"{"name":"lib"}"#;
        let app = manifest("app", "1.0.0", r#""lib":"workspace:*""#);
        let members = [("app", app.as_str()), ("lib", lib)];
        assert_eq!(
            states("", &members),
            [] as [(std::string::String, std::string::String, &str); 0]
        );
    }

    #[test]
    fn pinned_groups_report_deviations_and_refuse_local_packages() {
        let app = manifest("app", "1.0.0", r#""lib":"1.1.0""#);
        let lib = manifest("lib", "1.0.0", "");
        let members = [("app", app.as_str()), ("lib", lib.as_str())];
        let toml = "[[version-groups]]\ndependencies = [\"lib\"]\npolicy = \"pinned\"\npin = \"1.2.0\"\n";
        assert_eq!(
            states(toml, &members),
            [one("app", "lib", "DiffersToPin"), one("lib", "lib", "RefuseToPinLocal")]
        );
    }

    #[test]
    fn a_pin_beats_a_semver_group() {
        let app = manifest("app", "1.0.0", r#""react":"^18.3.1","zod":"~3.0.0""#);
        let lib = manifest("lib", "1.0.0", r#""react":"~18.3.1","zod":"3.0.0""#);
        let members = [("app", app.as_str()), ("lib", lib.as_str())];
        let toml = "[[version-groups]]\ndependencies = [\"react\"]\npolicy = \"pinned\"\npin = \"18.3.1\"\n\n\
                    [[semver-groups]]\ndependencies = [\"react\"]\nrange = \"^\"\n";
        assert_eq!(
            states(toml, &members),
            [
                one("app", "react", "PinOverridesSemverRange"),
                one("lib", "react", "PinOverridesSemverRangeMismatch"),
                one("lib", "zod", "DiffersToHighestOrLowestSemver"),
            ]
        );
    }

    #[test]
    fn banned_groups_ban_dependencies_but_not_workspace_packages() {
        let app = manifest("app", "1.0.0", r#""lib":"1.0.0""#);
        let lib = manifest("lib", "1.0.0", "");
        let members = [("app", app.as_str()), ("lib", lib.as_str())];
        let toml = "[[version-groups]]\ndependencies = [\"lib\"]\npolicy = \"banned\"\n";
        assert_eq!(
            states(toml, &members),
            [one("app", "lib", "IsBanned"), one("lib", "lib", "RefuseToBanLocal")]
        );
    }

    #[test]
    fn same_range_groups_flag_every_instance_when_one_is_disjoint() {
        let app = manifest("app", "1.0.0", r#""react":"^18.0.0""#);
        let lib = manifest("lib", "1.0.0", r#""react":"^18.2.0""#);
        let tool = manifest("tool", "1.0.0", r#""react":"^17.0.0""#);
        let members = [("app", app.as_str()), ("lib", lib.as_str()), ("tool", tool.as_str())];
        let config = "[[version-groups]]\ndependencies = [\"react\"]\npolicy = \"same-range\"\n";
        assert_eq!(
            states(config, &members),
            [
                one("app", "react", "SameRangeMismatch"),
                one("lib", "react", "SameRangeMismatch"),
                one("tool", "react", "SameRangeMismatch"),
            ]
        );
    }

    #[test]
    fn a_specifier_without_a_range_is_the_only_one_a_same_range_group_flags() {
        let app = manifest("app", "1.0.0", r#""react":"^18.0.0""#);
        let lib = manifest("lib", "1.0.0", r#""react":"^18.2.0""#);
        let tool = manifest("tool", "1.0.0", r#""react":"git+https://x.test/react.git#main""#);
        let members = [("app", app.as_str()), ("lib", lib.as_str()), ("tool", tool.as_str())];
        let config = "[[version-groups]]\ndependencies = [\"react\"]\npolicy = \"same-range\"\n";
        assert_eq!(states(config, &members), [one("tool", "react", "SameRangeMismatch")]);
    }

    #[test]
    fn workspace_dependents_of_a_local_package_take_no_part_in_same_range_and_same_minor_groups() {
        let lib = manifest("lib", "1.2.0", "");
        let app = manifest("app", "1.0.0", r#""lib":"workspace:*""#);
        let tool = manifest("tool", "1.0.0", r#""lib":"workspace:^""#);
        let site = manifest("site", "1.0.0", r#""lib":"workspace:~""#);
        let members = [
            ("app", app.as_str()),
            ("lib", lib.as_str()),
            ("site", site.as_str()),
            ("tool", tool.as_str()),
        ];
        for policy in ["same-range", "same-minor"] {
            let group_toml = format!("[[version-groups]]\npolicy = \"{policy}\"\n");
            assert_eq!(
                states(&group_toml, &members),
                [] as [(std::string::String, std::string::String, &str); 0],
                "{policy}"
            );
        }
    }

    #[test]
    fn registry_dependents_of_a_local_package_still_align_in_a_same_minor_group() {
        let lib = manifest("lib", "1.2.0", "");
        let app = manifest("app", "1.0.0", r#""lib":"~1.2.0""#);
        let tool = manifest("tool", "1.0.0", r#""lib":"workspace:*""#);
        let site = manifest("site", "1.0.0", r#""lib":"~1.3.0""#);
        let members = [
            ("app", app.as_str()),
            ("lib", lib.as_str()),
            ("site", site.as_str()),
            ("tool", tool.as_str()),
        ];
        let group_toml = "[[version-groups]]\ndependencies = [\"lib\"]\npolicy = \"same-minor\"\n";
        let flagged: Vec<String> = states(group_toml, &members)
            .into_iter()
            .map(|(package, _, _)| package)
            .collect();
        assert!(flagged.contains(&"site".to_owned()), "{flagged:?}");
        assert!(!flagged.contains(&"tool".to_owned()), "{flagged:?}");
    }

    #[test]
    fn same_minor_groups_compare_major_before_minor() {
        let group_toml = "[[version-groups]]\ndependencies = [\"ts\"]\npolicy = \"same-minor\"\n";
        let cases: [(&str, &str, &[&'static str]); 5] = [
            ("~5.4.2", "5.4.9", &[]),
            ("~5.4.2", "~5.5.0", &["SameMinorMismatch", "SameMinorMismatch"]),
            (
                "~5.4.2",
                "~4.9.0",
                &["SameMinorHasMajorMismatch", "SameMinorHasMajorMismatch"],
            ),
            ("^5.4.2", "~5.4.2", &["SameMinorOverridesSemverRange"]),
            (
                "git+https://x.test/ts.git#a",
                "git+https://x.test/ts.git#b",
                &["NonSemverMismatch", "NonSemverMismatch"],
            ),
        ];
        for (first, second, expected) in cases {
            let app = manifest("app", "1.0.0", &format!(r#""ts":"{first}""#));
            let lib = manifest("lib", "1.0.0", &format!(r#""ts":"{second}""#));
            let members = [("app", app.as_str()), ("lib", lib.as_str())];
            let actual: Vec<&str> = states(group_toml, &members)
                .into_iter()
                .map(|(_, _, state)| state)
                .collect();
            assert_eq!(actual, expected, "{first} {second}");
        }
    }

    #[test]
    fn a_same_minor_group_forcing_tilde_converges_under_a_caret_semver_group() {
        let toml = "[[version-groups]]\ndependencies = [\"ts\"]\npolicy = \"same-minor\"\n\n\
                    [[semver-groups]]\ndependencies = [\"ts\"]\nrange = \"^\"\n";
        let cases: [(&str, &[&str]); 3] = [
            ("^5.4.2", &["SameMinorOverridesSemverRange"]),
            ("5.4.2", &["SameMinorOverridesSemverRangeMismatch"]),
            ("~5.4.2", &[]),
        ];
        for (specifier, expected) in cases {
            let app = manifest("app", "1.0.0", &format!(r#""ts":"{specifier}""#));
            let lib = manifest("lib", "1.0.0", r#""ts":"~5.4.2""#);
            let members = [("app", app.as_str()), ("lib", lib.as_str())];
            let actual: Vec<&str> = states(toml, &members).into_iter().map(|(_, _, state)| state).collect();
            assert_eq!(actual, expected, "{specifier}");
        }
    }

    #[test]
    fn a_snap_target_in_several_sections_is_the_production_dependency_not_the_peer_window() {
        let app = manifest("app", "1.0.0", r#""react":"^18.2.0""#);
        let lib = r#"{"name":"lib","version":"1.0.0","peerDependencies":{"react":">=17 <19"},"dependencies":{"react":"^18.3.1"}}"#;
        let members = [("app", app.as_str()), ("lib", lib)];
        let toml = "[[version-groups]]\ndependencies = [\"react\"]\npolicy = \"snapped-to\"\nsnap-to = [\"lib\"]\n";
        let flagged = states(toml, &members);
        assert_eq!(flagged, [one("app", "react", "DiffersToSnapTarget")]);
    }

    #[test]
    fn snapped_to_groups_follow_the_target_package() {
        let app = manifest("app", "1.0.0", r#""react":"^18.2.0""#);
        let lib = manifest("lib", "1.0.0", r#""react":"^18.3.1""#);
        let members = [("app", app.as_str()), ("lib", lib.as_str())];
        let snapped = |target: &str| {
            format!(
                "[[version-groups]]\ndependencies = [\"react\"]\npolicy = \"snapped-to\"\nsnap-to = [\"{target}\"]\n"
            )
        };
        assert_eq!(
            states(&snapped("app"), &members),
            [one("lib", "react", "DiffersToSnapTarget")]
        );
        assert_eq!(
            states(&snapped("ghost"), &members),
            [
                one("app", "react", "DependsOnMissingSnapTarget"),
                one("lib", "react", "DependsOnMissingSnapTarget"),
            ]
        );
    }

    #[test]
    fn semver_groups_conflict_with_the_aligned_version() {
        let app = manifest("app", "1.0.0", r#""tiny":"1.0.0""#);
        let lib = manifest("lib", "1.0.0", r#""tiny":"<1.0.0""#);
        let members = [("app", app.as_str()), ("lib", lib.as_str())];
        let toml = "[[semver-groups]]\npackages = [\"lib\"]\nrange = \"<\"\n";
        assert_eq!(
            states(toml, &members),
            [one("lib", "tiny", "MatchConflictsWithHighestOrLowestSemver")]
        );
    }

    #[test]
    fn catalog_references_are_left_to_the_catalog_rules() {
        let app = manifest("app", "1.0.0", r#""react":"catalog:""#);
        let lib = manifest("lib", "1.0.0", r#""react":"^18.3.1""#);
        let tool = manifest("tool", "1.0.0", r#""react":"^17.0.0""#);
        let members = [("app", app.as_str()), ("lib", lib.as_str()), ("tool", tool.as_str())];
        assert_eq!(
            states("", &members),
            [one("tool", "react", "DiffersToHighestOrLowestSemver")],
            "a catalog reference leaves the group, the other members still align"
        );
        let pinned = "[[version-groups]]\ndependencies = [\"react\"]\npolicy = \"pinned\"\npin = \"18.3.1\"\n";
        assert_eq!(
            states(pinned, &members),
            [
                one("lib", "react", "DiffersToPin"),
                one("tool", "react", "DiffersToPin")
            ]
        );
    }

    #[test]
    fn a_shorthand_never_outranks_a_full_version_of_its_line() {
        let app = manifest("app", "1.0.0", r#""react":"^18""#);
        let lib = manifest("lib", "1.0.0", r#""react":"^18.3.1""#);
        let members = [("app", app.as_str()), ("lib", lib.as_str())];
        assert_eq!(
            states("", &members),
            [one("app", "react", "DiffersToHighestOrLowestSemver")]
        );
        assert_eq!(
            states("[policy]\nprefer = \"lowest\"\n", &members),
            [one("lib", "react", "DiffersToHighestOrLowestSemver")]
        );
    }

    #[test]
    fn an_alias_of_another_package_is_not_aligned_with_the_plain_dependency() {
        let app = manifest("app", "1.0.0", r#""foo":"npm:bar@^1.0.0""#);
        let lib = manifest("lib", "1.0.0", r#""foo":"^2.0.0""#);
        let tool = manifest("tool", "1.0.0", r#""foo":"^2.1.0""#);
        let members = [("app", app.as_str()), ("lib", lib.as_str()), ("tool", tool.as_str())];
        assert_eq!(
            states("", &members),
            [one("lib", "foo", "DiffersToHighestOrLowestSemver")]
        );
        let other = manifest("other", "1.0.0", r#""foo":"npm:bar@^1.2.0""#);
        let members = [("app", app.as_str()), ("other", other.as_str())];
        assert_eq!(
            states("", &members),
            [one("app", "foo", "DiffersToHighestOrLowestSemver")],
            "aliases of the same package still align with each other"
        );
    }

    #[test]
    fn prerelease_tags_in_capitals_and_build_metadata_are_exact_local_versions() {
        for version in ["1.0.0-RC.1", "1.0.0+build.5", "1.0.0-beta.1+sha.5114f85"] {
            let lib = manifest("lib", version, "");
            let app = manifest("app", "1.0.0", &format!(r#""lib":"{version}""#));
            let members = [("app", app.as_str()), ("lib", lib.as_str())];
            assert_eq!(states("", &members), [] as [(String, String, &str); 0], "{version}");
        }
        let lib = manifest("lib", "1.0.x", "");
        let app = manifest("app", "1.0.0", r#""lib":"1.0.0""#);
        let members = [("app", app.as_str()), ("lib", lib.as_str())];
        assert_eq!(
            states("", &members),
            [
                one("app", "lib", "DependsOnInvalidLocalPackage"),
                one("lib", "lib", "InvalidLocalVersion"),
            ]
        );
    }

    #[test]
    fn pinned_and_snapped_groups_leave_the_workspace_protocol_alone() {
        let app = manifest("app", "1.0.0", r#""ui":"workspace:*""#);
        let lib = manifest("lib", "1.0.0", r#""ui":"1.1.0""#);
        let members = [("app", app.as_str()), ("lib", lib.as_str())];
        let pinned = "[[version-groups]]\ndependencies = [\"ui\"]\npolicy = \"pinned\"\npin = \"1.2.0\"\n";
        assert_eq!(states(pinned, &members), [one("lib", "ui", "DiffersToPin")]);
        let tool = manifest("tool", "1.0.0", r#""ui":"^1.1.0""#);
        let members = [("app", app.as_str()), ("lib", lib.as_str()), ("tool", tool.as_str())];
        let snapped = "[[version-groups]]\ndependencies = [\"ui\"]\npolicy = \"snapped-to\"\nsnap-to = [\"tool\"]\n";
        assert_eq!(states(snapped, &members), [one("lib", "ui", "DiffersToSnapTarget")]);
        let workspace_pin = "[[version-groups]]\ndependencies = [\"ui\"]\npolicy = \"pinned\"\npin = \"workspace:*\"\n";
        assert_eq!(
            states(workspace_pin, &members),
            [one("lib", "ui", "DiffersToPin"), one("tool", "ui", "DiffersToPin")]
        );
    }

    #[test]
    fn the_snap_target_is_the_first_listed_package_that_declares_the_dependency() {
        let app = manifest("app", "1.0.0", r#""react":"^18.0.0""#);
        let lib = manifest("lib", "1.0.0", r#""react":"^18.2.0""#);
        let tool = manifest("tool", "1.0.0", r#""react":"^18.1.0""#);
        let members = [("app", app.as_str()), ("lib", lib.as_str()), ("tool", tool.as_str())];
        let config =
            "[[version-groups]]\ndependencies = [\"react\"]\npolicy = \"snapped-to\"\nsnap-to = [\"tool\", \"app\"]\n";
        assert_eq!(
            states(config, &members),
            [
                one("app", "react", "DiffersToSnapTarget"),
                one("lib", "react", "DiffersToSnapTarget"),
            ]
        );
    }

    #[test]
    fn peer_ranges_written_as_a_window_are_never_rewritten() {
        let toml = "[policy]\ndependency-types = [\"prod\", \"peer\"]\n";
        let app = r#"{"name":"app","version":"1.0.0","dependencies":{"react":"^18.3.1"},"peerDependencies":{"react":">=17 <19"}}"#;
        let lib = manifest("lib", "1.0.0", r#""react":"^18.3.1""#);
        let members = [("app", app), ("lib", lib.as_str())];
        assert_eq!(states(toml, &members), [] as [(String, String, &str); 0]);
    }

    #[test]
    fn lexical_normalization_resolves_dots() {
        assert_eq!(
            normalized(Path::new("packages/app/../lib/./x")).as_deref(),
            Some(Path::new("packages/lib/x"))
        );
        assert_eq!(normalized(Path::new("")).as_deref(), Some(Path::new("")));
        assert_eq!(normalized(Path::new("a/../..")), None);
        assert_eq!(normalized(Path::new("../x")), None);
    }
}
