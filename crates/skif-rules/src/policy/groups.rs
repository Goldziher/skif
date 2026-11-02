//! Version groups and semver groups: compiled selectors and the assignment of every instance to its group.
//!
//! Groups are evaluated in configuration order and the first group whose selectors match owns the instance
//! (ADR 0009). After the configured version groups sits a catch-all preferred-semver group, so every instance has an
//! owner. Semver groups are independent of version groups: the first matching one decides the range an instance
//! should use, after a built-in group that exempts the packages' own versions.

use std::collections::BTreeMap;
use std::sync::Arc;

use skif_config::{Config, DependencyTypes, Prefer, Selector, SemverGroup, VersionGroup, VersionPolicy};
use skif_core::DependencyKind;
use skif_specifier::Specifier;
use skif_specifier::semver_range::SemverRange;
use skif_workspace::Workspace;

use super::instances::{Instance, InstanceId, Instances};

/// The four selectors shared by version groups and semver groups, compiled against one workspace.
///
/// An empty selector matches everything; the exceptions are negation-only selectors, which match everything except
/// what they name.
#[derive(Debug)]
pub(crate) struct Selectors<'c> {
    dependencies: &'c Selector,
    dependency_types: &'c DependencyTypes,
    /// Whether `dependency-types` admits a package's own version, which belongs to no dependency section.
    admits_own_version: bool,
    specifier_types: &'c [String],
    /// Per package index, whether the `packages` selector admits it.
    packages: Vec<bool>,
}

impl<'c> Selectors<'c> {
    pub(crate) fn new(
        workspace: &Workspace,
        dependencies: &'c Selector,
        packages: &Selector,
        dependency_types: &'c DependencyTypes,
        specifier_types: &'c [String],
    ) -> Self {
        Self {
            dependencies,
            dependency_types,
            admits_own_version: dependency_types.tokens().iter().all(|token| token.starts_with('!')),
            specifier_types,
            packages: workspace
                .iter()
                .map(|(_, package)| packages.is_empty() || package.is_selected_by(packages))
                .collect(),
        }
    }

    pub(crate) fn matches(&self, instance: &Instance<'_>) -> bool {
        self.packages.get(instance.package.id.index()).copied().unwrap_or(false)
            && (self.dependencies.is_empty() || self.dependencies.matches(&instance.name, instance.is_local))
            && instance.kind().map_or(self.admits_own_version, |kind: DependencyKind| {
                self.dependency_types.matches(kind)
            })
            && identifier_matches(self.specifier_types, instance.specifier.get_config_identifier())
    }
}

/// `value` against identifiers where a leading `!` excludes; no plain identifier admits everything not excluded.
fn identifier_matches(patterns: &[String], value: &str) -> bool {
    let named = |negated: bool| {
        patterns
            .iter()
            .filter(move |pattern| pattern.starts_with('!') == negated)
            .map(|pattern| pattern.trim_start_matches('!'))
    };
    let mut included = named(false).peekable();
    let admitted = included.peek().is_none() || included.any(|identifier| identifier == value);
    admitted && !named(true).any(|identifier| identifier == value)
}

/// What a version group requires of the instances it owns.
#[derive(Debug, Clone)]
pub(crate) enum Policy {
    Pinned(Arc<Specifier>),
    Banned,
    Ignored,
    /// A `pinned` group whose `pin` is empty or unparseable: it owns its instances but changes none of them.
    InvalidPin,
    SameRange,
    SameMinor,
    /// The names of the packages whose declaration is the target.
    SnappedTo(Vec<String>),
    PreferredSemver(Prefer),
}

/// A compiled `[[version-groups]]` entry, or the catch-all after them.
#[derive(Debug)]
pub(crate) struct Group<'c> {
    /// Position in `[[version-groups]]`; `None` for the catch-all.
    pub(crate) configured: Option<usize>,
    pub(crate) label: Option<&'c str>,
    pub(crate) policy: Policy,
    selectors: Option<Selectors<'c>>,
}

struct Ranged<'c> {
    selectors: Selectors<'c>,
    range: SemverRange,
}

/// All version groups (the catch-all last) and semver groups of a configuration.
pub(crate) struct Groups<'c> {
    groups: Vec<Group<'c>>,
    semver: Vec<Ranged<'c>>,
}

impl<'c> Groups<'c> {
    pub(crate) fn compile(workspace: &Workspace, config: &'c Config) -> Self {
        let mut groups: Vec<Group<'c>> = config
            .version_groups
            .iter()
            .enumerate()
            .map(|(index, group)| Group {
                configured: Some(index),
                label: group.label.as_deref(),
                policy: policy_of(group, config.policy.prefer),
                selectors: Some(Selectors::new(
                    workspace,
                    &group.dependencies,
                    &group.packages,
                    &group.dependency_types,
                    &group.specifier_types,
                )),
            })
            .collect();
        groups.push(Group {
            configured: None,
            label: None,
            policy: Policy::PreferredSemver(config.policy.prefer),
            selectors: None,
        });
        let semver = config
            .semver_groups
            .iter()
            .map(|group: &SemverGroup| Ranged {
                selectors: Selectors::new(
                    workspace,
                    &group.dependencies,
                    &group.packages,
                    &group.dependency_types,
                    &group.specifier_types,
                ),
                range: convert(group.range),
            })
            .collect();
        Self { groups, semver }
    }

    pub(crate) fn groups(&self) -> &[Group<'c>] {
        &self.groups
    }

    /// Assigns every instance to its owning version group and its preferred semver range.
    pub(crate) fn assign<'i>(&self, instances: &'i Instances<'_>) -> Assignment<'i> {
        let mut preferred = Vec::with_capacity(instances.len());
        let mut dependencies: Vec<BTreeMap<&'i str, Vec<InstanceId>>> =
            self.groups.iter().map(|_| BTreeMap::new()).collect();
        for (id, instance) in instances.iter() {
            let owner = self
                .groups
                .iter()
                .position(|group| {
                    group
                        .selectors
                        .as_ref()
                        .is_none_or(|selectors| selectors.matches(instance))
                })
                .unwrap_or(self.groups.len() - 1);
            dependencies[owner].entry(instance.name.as_str()).or_default().push(id);
            preferred.push(self.preferred_range(instance));
        }
        Assignment {
            preferred,
            dependencies,
        }
    }

    /// The range of the first matching semver group, if the specifier can carry one.
    fn preferred_range(&self, instance: &Instance<'_>) -> Option<SemverRange> {
        if instance.is_own_version() {
            return None;
        }
        let range = self
            .semver
            .iter()
            .find(|group| group.selectors.matches(instance))?
            .range
            .clone();
        instance.specifier.with_range(&range).map(|_| range)
    }
}

/// The owner and preferred range of every instance, and each group's instances by dependency name.
#[derive(Debug)]
pub(crate) struct Assignment<'i> {
    preferred: Vec<Option<SemverRange>>,
    dependencies: Vec<BTreeMap<&'i str, Vec<InstanceId>>>,
}

impl<'i> Assignment<'i> {
    /// The semver range `id` must use, when a semver group applies and the specifier can carry it.
    pub(crate) fn preferred(&self, id: InstanceId) -> Option<&SemverRange> {
        self.preferred[id.index()].as_ref()
    }

    /// The instances `group` owns, by dependency name in byte order.
    pub(crate) fn dependencies(&self, group: usize) -> impl Iterator<Item = (&'i str, &[InstanceId])> {
        self.dependencies[group]
            .iter()
            .map(|(name, ids)| (*name, ids.as_slice()))
    }

    /// Whether `group` owns at least one instance.
    pub(crate) fn is_used(&self, group: usize) -> bool {
        !self.dependencies[group].is_empty()
    }
}

fn policy_of(group: &VersionGroup, default: Prefer) -> Policy {
    match group.policy {
        VersionPolicy::Pinned => pinned(group.pin.as_deref().unwrap_or_default()),
        VersionPolicy::Banned => Policy::Banned,
        VersionPolicy::Ignored => Policy::Ignored,
        VersionPolicy::SameRange => Policy::SameRange,
        VersionPolicy::SameMinor => Policy::SameMinor,
        VersionPolicy::SnappedTo => Policy::SnappedTo(group.snap_to.clone()),
        VersionPolicy::PreferredSemver => Policy::PreferredSemver(group.prefer.unwrap_or(default)),
    }
}

/// A pin is usable when it is a non-empty string the specifier parser understands.
fn pinned(pin: &str) -> Policy {
    let specifier = Specifier::new(pin);
    if pin.trim().is_empty() || matches!(*specifier, Specifier::Unsupported(_) | Specifier::None) {
        Policy::InvalidPin
    } else {
        Policy::Pinned(specifier)
    }
}

fn convert(range: skif_config::SemverRange) -> SemverRange {
    use skif_config::SemverRange as Configured;
    match range {
        Configured::Exact => SemverRange::Exact,
        Configured::Any => SemverRange::Any,
        Configured::Caret => SemverRange::Minor,
        Configured::Tilde => SemverRange::Patch,
        Configured::Gt => SemverRange::Gt,
        Configured::Gte => SemverRange::Gte,
        Configured::Lt => SemverRange::Lt,
        Configured::Lte => SemverRange::Lte,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{config, workspace};

    const ROOT: &str =
        r#"{"name":"root","version":"0.0.0","private":true,"packageManager":"pnpm@9.0.0","workspaces":["packages/*"]}"#;
    const APP: &str = r#"{"name":"app","version":"1.0.0","dependencies":{"react":"^18.0.0","lib":"1.0.0"},"devDependencies":{"typescript":"^5.0.0","left-pad":"git+https://x.test/left-pad.git"}}"#;
    const LIB: &str = r#"{"name":"lib","version":"1.0.0","dependencies":{"react":"^18.0.0"}}"#;

    fn files() -> [(&'static str, &'static str); 3] {
        [
            ("package.json", ROOT),
            ("packages/app/package.json", APP),
            ("packages/lib/package.json", LIB),
        ]
    }

    /// The group index owning each `(package, dependency, is own version)`.
    fn ownership(toml: &str) -> Vec<(String, String, usize)> {
        let config = config(toml);
        let workspace = workspace(&files(), &config);
        let instances = Instances::collect(&workspace, &config);
        let groups = Groups::compile(&workspace, &config);
        let assignment = groups.assign(&instances);
        let owner = |id| {
            (0..groups.groups.len())
                .find(|&group| assignment.dependencies(group).any(|(_, ids)| ids.contains(&id)))
                .unwrap()
        };
        instances
            .iter()
            .map(|(id, instance)| {
                let kind = instance.kind().map_or("version", DependencyKind::token);
                (
                    format!("{}:{kind}", instance.package.display_name()),
                    instance.name.clone(),
                    owner(id),
                )
            })
            .collect()
    }

    fn owner_of(owners: &[(String, String, usize)], package_kind: &str, name: &str) -> usize {
        owners
            .iter()
            .find(|(owner, dependency, _)| owner == package_kind && dependency == name)
            .map(|(_, _, group)| *group)
            .unwrap()
    }

    #[test]
    fn the_first_matching_group_owns_the_instance() {
        let owners = ownership(
            "[[version-groups]]\ndependencies = [\"react\"]\npackages = [\"app\"]\npolicy = \"ignored\"\n\n\
             [[version-groups]]\ndependencies = [\"react\"]\npolicy = \"banned\"\n",
        );
        assert_eq!(owner_of(&owners, "app:prod", "react"), 0);
        assert_eq!(owner_of(&owners, "lib:prod", "react"), 1);
        assert_eq!(
            owner_of(&owners, "app:dev", "typescript"),
            2,
            "unclaimed instances reach the catch-all"
        );
    }

    #[test]
    fn selectors_combine_with_and_and_support_negation() {
        let toml =
            "[[version-groups]]\ndependencies = [\"!react\"]\ndependency-types = [\"dev\"]\npolicy = \"ignored\"\n";
        let owners = ownership(toml);
        assert_eq!(owner_of(&owners, "app:dev", "typescript"), 0);
        assert_eq!(owner_of(&owners, "app:prod", "lib"), 1, "wrong section");
        assert_eq!(owner_of(&owners, "app:dev", "left-pad"), 0);
        assert_eq!(
            owner_of(&owners, "lib:version", "lib"),
            1,
            "an own version belongs to no section, so a plain dependency-types list never selects it"
        );
    }

    #[test]
    fn negation_only_dependency_types_admit_own_versions() {
        let owners = ownership("[[version-groups]]\ndependency-types = [\"!dev\"]\npolicy = \"ignored\"\n");
        assert_eq!(owner_of(&owners, "lib:version", "lib"), 0);
        assert_eq!(owner_of(&owners, "app:dev", "typescript"), 1);
    }

    #[test]
    fn local_selector_picks_workspace_packages_including_their_own_versions() {
        let owners = ownership("[[version-groups]]\ndependencies = [\"$LOCAL\"]\npolicy = \"ignored\"\n");
        assert_eq!(owner_of(&owners, "app:prod", "lib"), 0);
        assert_eq!(owner_of(&owners, "lib:version", "lib"), 0);
        assert_eq!(owner_of(&owners, "app:prod", "react"), 1);
    }

    #[test]
    fn specifier_types_select_by_kind_of_specifier() {
        let owners = ownership("[[version-groups]]\nspecifier-types = [\"git\"]\npolicy = \"banned\"\n");
        assert_eq!(owner_of(&owners, "app:dev", "left-pad"), 0);
        assert_eq!(owner_of(&owners, "app:dev", "typescript"), 1);
        let owners = ownership("[[version-groups]]\nspecifier-types = [\"!range\"]\npolicy = \"banned\"\n");
        assert_eq!(owner_of(&owners, "app:dev", "typescript"), 1);
        assert_eq!(owner_of(&owners, "app:dev", "left-pad"), 0);
    }

    #[test]
    fn identifier_patterns_follow_include_then_exclude() {
        let patterns = |list: &[&str]| list.iter().map(|p| (*p).to_owned()).collect::<Vec<_>>();
        assert!(identifier_matches(&[], "range"));
        assert!(identifier_matches(&patterns(&["range", "exact"]), "exact"));
        assert!(!identifier_matches(&patterns(&["range"]), "exact"));
        assert!(identifier_matches(&patterns(&["!range"]), "exact"));
        assert!(!identifier_matches(&patterns(&["!range"]), "range"));
        assert!(!identifier_matches(&patterns(&["range", "!range"]), "range"));
    }

    #[test]
    fn semver_groups_pick_the_first_match_and_spare_own_versions() {
        let config = config(
            "[[semver-groups]]\ndependencies = [\"react\"]\nrange = \"~\"\n\n[[semver-groups]]\nrange = \"^\"\n",
        );
        let workspace = workspace(&files(), &config);
        let instances = Instances::collect(&workspace, &config);
        let assignment = Groups::compile(&workspace, &config).assign(&instances);
        let ranges: Vec<(String, Option<String>)> = instances
            .iter()
            .map(|(id, instance)| (instance.name.clone(), assignment.preferred(id).map(SemverRange::unwrap)))
            .collect();
        let range_of = |name: &str| ranges.iter().find(|(n, _)| n == name).and_then(|(_, r)| r.clone());
        assert_eq!(range_of("react").as_deref(), Some("~"));
        assert_eq!(range_of("typescript").as_deref(), Some("^"));
        let own = instances
            .iter()
            .find(|(_, i)| i.is_own_version() && i.name == "app")
            .map(|(id, _)| assignment.preferred(id).is_none());
        assert_eq!(own, Some(true));
    }

    #[test]
    fn a_range_the_specifier_cannot_carry_is_not_preferred() {
        let config = config("[[semver-groups]]\nrange = \"^\"\n");
        let workspace = workspace(&files(), &config);
        let instances = Instances::collect(&workspace, &config);
        let assignment = Groups::compile(&workspace, &config).assign(&instances);
        let git = instances
            .iter()
            .find(|(_, i)| i.name == "left-pad")
            .map(|(id, _)| id)
            .unwrap();
        assert!(assignment.preferred(git).is_none());
    }

    #[test]
    fn unused_groups_are_visible_in_the_assignment() {
        let config = config(
            "[[version-groups]]\ndependencies = [\"nothing\"]\npolicy = \"banned\"\n\n\
             [[version-groups]]\ndependencies = [\"react\"]\npolicy = \"ignored\"\n",
        );
        let workspace = workspace(&files(), &config);
        let instances = Instances::collect(&workspace, &config);
        let groups = Groups::compile(&workspace, &config);
        let assignment = groups.assign(&instances);
        let used: Vec<bool> = (0..groups.groups().len())
            .map(|group| assignment.is_used(group))
            .collect();
        assert_eq!(used, [false, true, true]);
    }

    #[test]
    fn group_prefer_overrides_the_policy_default() {
        let config = config(
            "[policy]\nprefer = \"lowest\"\n\n[[version-groups]]\ndependencies = [\"react\"]\npolicy = \"preferred-semver\"\nprefer = \"highest\"\n",
        );
        let workspace = workspace(&files(), &config);
        let groups = Groups::compile(&workspace, &config);
        let prefers: Vec<Prefer> = groups
            .groups()
            .iter()
            .filter_map(|group| match group.policy {
                Policy::PreferredSemver(prefer) => Some(prefer),
                _ => None,
            })
            .collect();
        assert_eq!(prefers, [Prefer::Highest, Prefer::Lowest]);
    }
}
