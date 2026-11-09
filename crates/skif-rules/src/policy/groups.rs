//! Version groups and semver groups: compiled selectors and the assignment of every instance to its group.
//!
//! Groups are evaluated in configuration order and the first group whose selectors match owns the instance
//! (ADR 0009). After the configured version groups sits a catch-all preferred-semver group. The catch-all takes the
//! instances in the sections `[policy] dependency-types` names (production and development dependencies by default)
//! that the catalog rules do not report, and nothing else: a group that names a section itself governs that section
//! whatever the policy default says. A configured group governs every instance its selectors match, a dependency a
//! catalog owns included, whether or not the group names a dependency; only the catch-all leaves those to the
//! catalog rules.
//! Semver groups are independent of version groups: the first matching one decides the range an instance should use.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use skif_config::{
    Config, DependencyType, DependencyTypes, Prefer, Selector, SemverGroup, VersionGroup, VersionPolicy,
};
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
    /// The sections a group that names none governs.
    scope: &'c DependencyTypes,
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
        scope: &'c DependencyTypes,
        specifier_types: &'c [String],
    ) -> Self {
        Self {
            dependencies,
            dependency_types,
            scope,
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
            && (if self.dependency_types.is_empty() {
                in_default_scope(self.scope, instance.ty())
            } else {
                self.dependency_types.matches(instance.ty())
            })
            && identifier_matches(self.specifier_types, instance.specifier.get_config_identifier())
    }
}

/// Whether `[policy] dependency-types` (`scope`) puts the `ty` place under the policies by default: production and
/// development dependencies and a package's own version unless it says otherwise (ADR 0072). Peer and optional ranges
/// state a compatibility window rather than a version to install, so aligning them is opt-in. So are the override
/// sections and the custom types: the scope has to name them.
fn in_default_scope(scope: &DependencyTypes, ty: &DependencyType) -> bool {
    match ty {
        DependencyType::Local => scope.matches(ty),
        DependencyType::Section(kind) if scope.is_empty() => {
            matches!(kind, DependencyKind::Dependencies | DependencyKind::DevDependencies)
        }
        DependencyType::Section(_) => scope.matches(ty),
        _ => scope.names(ty),
    }
}

/// `value` against identifiers where a leading `!` excludes; no plain identifier admits everything not excluded.
pub(crate) fn identifier_matches(patterns: &[String], value: &str) -> bool {
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
    /// Versions may differ; only the semver groups' ranges are checked.
    SemverRangeOnly,
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
    /// `None` for a group that exempts what it matches from a range.
    range: Option<SemverRange>,
}

/// All version groups (the catch-all last) and semver groups of a configuration.
#[allow(
    clippy::struct_field_names,
    reason = "`groups` are the version groups, next to `semver` groups"
)]
pub(crate) struct Groups<'c> {
    groups: Vec<Group<'c>>,
    semver: Vec<Ranged<'c>>,
    scope: &'c DependencyTypes,
}

impl<'c> Groups<'c> {
    pub(crate) fn compile(workspace: &Workspace, config: &'c Config) -> Self {
        // A command-line `--dependency-types` names what the run is about, so it is also the default scope.
        let scope = if config.filter.dependency_types.is_empty() {
            &config.policy.dependency_types
        } else {
            &config.filter.dependency_types
        };
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
                    scope,
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
                    scope,
                    &group.specifier_types,
                ),
                range: group.range.map(convert),
            })
            .collect();
        Self { groups, semver, scope }
    }

    pub(crate) fn groups(&self) -> &[Group<'c>] {
        &self.groups
    }

    /// Whether any group can claim an instance, that is has selectors: [`claim`](Self::claim) is `None` otherwise.
    pub(crate) fn has_selectors(&self) -> bool {
        self.groups.iter().any(|group| group.selectors.is_some())
    }

    /// The configured version group that claims `instance`: the first one whose selectors match it, whatever its
    /// policy. The catalog rules leave a claimed instance to that group, as syncpack's first-match-wins order does.
    pub(crate) fn claim(&self, instance: &Instance<'_>) -> Option<usize> {
        self.groups.iter().position(|group| {
            group
                .selectors
                .as_ref()
                .is_some_and(|selectors| selectors.matches(instance))
        })
    }

    /// Assigns every instance to its owning version group and its preferred semver range.
    pub(crate) fn assign<'i>(&self, instances: &'i Instances<'_>) -> Assignment<'i> {
        let mut preferred = Vec::with_capacity(instances.len());
        let mut owners = Vec::with_capacity(instances.len());
        // Hash maps while every instance is looked up, ordered by dependency once at the end.
        let mut dependencies: Vec<HashMap<Dependency<'i>, Vec<InstanceId>>> =
            self.groups.iter().map(|_| HashMap::new()).collect();
        for (id, instance) in instances.iter() {
            let owner = match self.claim(instance) {
                Some(owner) => owner,
                None if self.governs_by_default(instance) => self.groups.len() - 1,
                None => {
                    preferred.push(None);
                    owners.push(None);
                    continue;
                }
            };
            owners.push(Some(owner));
            dependencies[owner]
                .entry((&*instance.name, instance.alias_target()))
                .or_default()
                .push(id);
            preferred.push(self.preferred_range(instance));
        }
        Assignment {
            preferred,
            owners,
            dependencies: dependencies
                .into_iter()
                .map(|dependencies| dependencies.into_iter().collect())
                .collect(),
        }
    }

    /// Whether the catch-all group takes an instance no configured group claims: it is in a section the policy covers
    /// by default and the catalog rules do not report it.
    fn governs_by_default(&self, instance: &Instance<'_>) -> bool {
        !instance.catalog_owned && in_default_scope(self.scope, instance.ty())
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
            .clone()?;
        instance.specifier.with_range(&range).map(|_| range)
    }
}

/// The owner and preferred range of every instance, and each group's instances by dependency name.
#[derive(Debug)]
pub(crate) struct Assignment<'i> {
    preferred: Vec<Option<SemverRange>>,
    /// The index of the version group that owns each instance; `None` for an instance no group governs.
    owners: Vec<Option<usize>>,
    dependencies: Vec<BTreeMap<Dependency<'i>, Vec<InstanceId>>>,
}

/// A dependency name and, for an `npm:` alias of another package, that package: `foo` installing `bar` is not the
/// `foo` that other packages install, and the two are not aligned with each other.
type Dependency<'i> = (&'i str, Option<&'i str>);

impl<'i> Assignment<'i> {
    /// The semver range `id` must use, when a semver group applies and the specifier can carry it.
    pub(crate) fn preferred(&self, id: InstanceId) -> Option<&SemverRange> {
        self.preferred[id.index()].as_ref()
    }

    /// The index of the version group that owns `id`, the catch-all last; `None` when no group governs it.
    pub(crate) fn owner(&self, id: InstanceId) -> Option<usize> {
        self.owners[id.index()]
    }

    /// The instances `group` owns, by dependency name in byte order.
    pub(crate) fn dependencies(&self, group: usize) -> impl Iterator<Item = (&'i str, &[InstanceId])> {
        self.dependencies[group]
            .iter()
            .map(|((name, _), ids)| (*name, ids.as_slice()))
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
        VersionPolicy::SameRange => Policy::SameRange,
        VersionPolicy::SameMinor => Policy::SameMinor,
        VersionPolicy::SnappedTo => Policy::SnappedTo(group.snap_to.clone()),
        VersionPolicy::PreferredSemver => Policy::PreferredSemver(group.prefer.unwrap_or(default)),
        VersionPolicy::SemverRangeOnly => Policy::SemverRangeOnly,
        // `Ignored`, and any policy this build does not know: leave the group alone.
        _ => Policy::Ignored,
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
        Configured::Caret => SemverRange::Minor,
        Configured::Tilde => SemverRange::Patch,
        Configured::Gt => SemverRange::Gt,
        Configured::Gte => SemverRange::Gte,
        Configured::Lt => SemverRange::Lt,
        Configured::Lte => SemverRange::Lte,
        // `Any`, and any range this build does not know: accept any range.
        _ => SemverRange::Any,
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
        let instances = Instances::collect(&workspace, &config, true);
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
                    instance.name.to_string(),
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
        let instances = Instances::collect(&workspace, &config, true);
        let assignment = Groups::compile(&workspace, &config).assign(&instances);
        let ranges: Vec<(String, Option<String>)> = instances
            .iter()
            .map(|(id, instance)| {
                (
                    instance.name.to_string(),
                    assignment.preferred(id).map(SemverRange::unwrap),
                )
            })
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
        let instances = Instances::collect(&workspace, &config, true);
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
        let instances = Instances::collect(&workspace, &config, true);
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

    const PEERS: &str = r#"{"name":"peers","version":"1.0.0","dependencies":{"react":"^18.0.0"},"peerDependencies":{"react":"^17.0.0 || ^18.0.0"},"optionalDependencies":{"fsevents":"^2.0.0"}}"#;

    /// `(section:dependency, owning group)` of every dependency of `peers` a group governs.
    fn governed(toml: &str) -> Vec<(String, usize)> {
        let config = config(toml);
        let files = [("package.json", ROOT), ("packages/peers/package.json", PEERS)];
        let workspace = workspace(&files, &config);
        let instances = Instances::collect(&workspace, &config, true);
        let groups = Groups::compile(&workspace, &config);
        let assignment = groups.assign(&instances);
        instances
            .iter()
            .filter(|(_, instance)| !instance.is_own_version() && instance.package.name.as_deref() == Some("peers"))
            .filter_map(|(id, instance)| {
                let owner = (0..groups.groups().len())
                    .find(|&group| assignment.dependencies(group).any(|(_, ids)| ids.contains(&id)))?;
                let kind = instance.kind().map_or("version", DependencyKind::token);
                Some((format!("{kind}:{}", instance.name), owner))
            })
            .collect()
    }

    #[test]
    fn the_catch_all_governs_the_sections_the_policy_covers() {
        let prod = || ("prod:react".to_owned(), 0);
        assert_eq!(governed(""), [prod()]);
        assert_eq!(
            governed("[policy]\ndependency-types = [\"prod\", \"peer\"]\n"),
            [prod(), ("peer:react".to_owned(), 0)]
        );
        assert_eq!(
            governed("[policy]\ndependency-types = [\"optional\"]\n"),
            [("optional:fsevents".to_owned(), 0)]
        );
    }

    #[test]
    fn a_group_naming_a_section_governs_it_whatever_the_policy_says() {
        let toml = "[[version-groups]]\ndependency-types = [\"peer\"]\npolicy = \"ignored\"\n\n\
                    [[version-groups]]\ndependency-types = [\"optional\"]\npolicy = \"banned\"\n";
        assert_eq!(
            governed(toml),
            [
                ("prod:react".to_owned(), 2),
                ("peer:react".to_owned(), 0),
                ("optional:fsevents".to_owned(), 1),
            ]
        );
    }

    #[test]
    fn a_group_naming_no_section_covers_the_sections_the_policy_covers() {
        let toml = "[[version-groups]]\ndependencies = [\"react\", \"fsevents\"]\npolicy = \"ignored\"\n";
        assert_eq!(governed(toml), [("prod:react".to_owned(), 0)]);
        let semver = "[[semver-groups]]\nrange = \"~\"\n";
        let config = config(semver);
        let files = [("package.json", ROOT), ("packages/peers/package.json", PEERS)];
        let workspace = workspace(&files, &config);
        let instances = Instances::collect(&workspace, &config, true);
        let assignment = Groups::compile(&workspace, &config).assign(&instances);
        let ranged: Vec<(&str, bool)> = instances
            .iter()
            .filter(|(_, instance)| !instance.is_own_version() && instance.package.name.as_deref() == Some("peers"))
            .map(|(id, instance)| {
                (
                    instance.kind().map_or("", DependencyKind::token),
                    assignment.preferred(id).is_some(),
                )
            })
            .collect();
        assert_eq!(ranged, [("prod", true), ("peer", false), ("optional", false)]);
    }

    const OVERRIDES_ROOT: &str = r#"{"name":"root","version":"0.0.0","private":true,"packageManager":"pnpm@9.0.0","workspaces":["packages/*"],"resolutions":{"lodash":"4.17.20"},"engines":{"node":">=20.0.0"}}"#;
    const OVERRIDES_APP: &str =
        r#"{"name":"app","version":"1.0.0","dependencies":{"lodash":"^4.17.21"},"engines":{"node":">=18.0.0"}}"#;
    const CUSTOM_NODE: &str = "[[custom-types]]\nname = \"node\"\npath = \"engines.node\"\nstrategy = \"version\"\n\n";

    /// `(place:dependency, owning group)` of every instance in the root and app packages that is not an own version.
    fn placed(toml: &str) -> Vec<(String, usize)> {
        let config = config(toml);
        let files = [
            ("package.json", OVERRIDES_ROOT),
            ("packages/app/package.json", OVERRIDES_APP),
        ];
        let workspace = workspace(&files, &config);
        let instances = Instances::collect(&workspace, &config, true);
        let groups = Groups::compile(&workspace, &config);
        let assignment = groups.assign(&instances);
        instances
            .iter()
            .filter(|(_, instance)| !instance.is_own_version())
            .filter_map(|(id, instance)| {
                let owner = (0..groups.groups().len())
                    .find(|&group| assignment.dependencies(group).any(|(_, ids)| ids.contains(&id)))?;
                Some((format!("{}:{}", instance.ty().token(), instance.name), owner))
            })
            .collect()
    }

    #[test]
    fn override_sections_and_custom_types_are_governed_only_when_named() {
        assert_eq!(placed(""), [("prod:lodash".to_owned(), 0)]);
        assert_eq!(placed(CUSTOM_NODE), [("prod:lodash".to_owned(), 0)]);
        let scoped = format!("{CUSTOM_NODE}[policy]\ndependency-types = [\"prod\", \"resolutions\", \"node\"]\n");
        assert_eq!(
            placed(&scoped),
            [
                ("resolutions:lodash".to_owned(), 0),
                ("node:node".to_owned(), 0),
                ("prod:lodash".to_owned(), 0),
                ("node:node".to_owned(), 0),
            ]
        );
        assert_eq!(
            placed("[policy]\ndependency-types = [\"!peer\"]\n"),
            [("prod:lodash".to_owned(), 0)],
            "a negation-only scope does not pull in the override sections"
        );
    }

    #[test]
    fn a_group_naming_an_override_section_or_custom_type_governs_it() {
        let toml = format!(
            "{CUSTOM_NODE}[[version-groups]]\ndependency-types = [\"resolutions\"]\npolicy = \"ignored\"\n\n\
             [[version-groups]]\ndependency-types = [\"node\"]\npolicy = \"banned\"\n"
        );
        assert_eq!(
            placed(&toml),
            [
                ("resolutions:lodash".to_owned(), 0),
                ("node:node".to_owned(), 1),
                ("prod:lodash".to_owned(), 2),
                ("node:node".to_owned(), 1),
            ]
        );
    }

    #[test]
    fn local_is_a_type_of_its_own() {
        let owners = ownership("[[version-groups]]\ndependency-types = [\"local\"]\npolicy = \"ignored\"\n");
        assert_eq!(owner_of(&owners, "lib:version", "lib"), 0);
        assert_eq!(owner_of(&owners, "app:prod", "react"), 1);
        let owners = ownership("[[version-groups]]\ndependency-types = [\"!local\"]\npolicy = \"ignored\"\n");
        assert_eq!(owner_of(&owners, "lib:version", "lib"), 1);
        assert_eq!(owner_of(&owners, "app:prod", "react"), 0);
    }

    #[test]
    fn an_ignored_semver_group_stops_the_search_without_a_range() {
        let config = config(
            "[[semver-groups]]\ndependencies = [\"react\"]\nignored = true\n\n[[semver-groups]]\nrange = \"^\"\n",
        );
        let workspace = workspace(&files(), &config);
        let instances = Instances::collect(&workspace, &config, true);
        let assignment = Groups::compile(&workspace, &config).assign(&instances);
        let range_of = |name: &str| {
            instances
                .iter()
                .find(|(_, instance)| instance.name == name && !instance.is_own_version())
                .and_then(|(id, _)| assignment.preferred(id).map(SemverRange::unwrap))
        };
        assert_eq!(range_of("react"), None);
        assert_eq!(range_of("typescript").as_deref(), Some("^"));
    }
}
