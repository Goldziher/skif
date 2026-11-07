//! One analysis of the whole workspace: instances, group assignment and the state of every instance.

use std::collections::BTreeSet;

use skif_config::Config;
use skif_specifier::semver_range::SemverRange;
use skif_workspace::Workspace;

use super::groups::{Group, Groups, Policy};
use super::instances::Instances;
use super::state::Verdict;
use super::visit::Visitor;

/// A configured version group named in a configuration-level finding (it owns no instance, or cannot be applied).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConfiguredGroup {
    /// Position in `[[version-groups]]`.
    pub(crate) index: usize,
    pub(crate) label: Option<String>,
}

/// The result of running every version group over the workspace once.
#[derive(Debug)]
pub(crate) struct Analysis<'w> {
    pub(crate) instances: Instances<'w>,
    /// Indexed by [`InstanceId::index`](super::instances::InstanceId::index); `None` for valid instances.
    pub(crate) verdicts: Vec<Option<Verdict>>,
    pub(crate) unused_groups: Vec<ConfiguredGroup>,
    /// `pinned` groups whose `pin` cannot be applied.
    pub(crate) invalid_pins: Vec<ConfiguredGroup>,
    /// Every version group, the catch-all last, as `skif list` names it.
    pub(crate) groups: Vec<GroupSummary>,
    /// Indexed like `verdicts`: the position in `groups` of the group that owns the instance, if any.
    pub(crate) owners: Vec<Option<usize>>,
    /// Indexed like `verdicts`: the range the instance's semver group asks for, if any.
    pub(crate) preferred: Vec<Option<String>>,
}

/// A version group as the instance listing shows it.
#[derive(Debug, Clone)]
pub(crate) struct GroupSummary {
    /// Position in `[[version-groups]]`; `None` for the catch-all.
    pub(crate) configured: Option<usize>,
    pub(crate) label: Option<String>,
    pub(crate) policy: Policy,
}

impl<'w> Analysis<'w> {
    /// Runs the version groups with the catalog rules owning the dependencies `catalog_owns` selects, as the
    /// provider does.
    #[cfg(test)]
    pub(crate) fn run(workspace: &'w Workspace, config: &Config, catalog_owns: bool) -> Self {
        let instances = Instances::collect(workspace, config, catalog_owns);
        Self::from_instances(workspace, config, instances)
    }

    /// Runs the version groups; the dependencies in `owned` are left to the catalog rules (see
    /// [`Instances::collect_with`]).
    pub(crate) fn run_with(workspace: &'w Workspace, config: &Config, owned: &BTreeSet<&str>) -> Self {
        Self::from_instances(
            workspace,
            config,
            Instances::collect_with(workspace, owned, &config.custom_types, config.policy.override_reading),
        )
    }

    fn from_instances(workspace: &'w Workspace, config: &Config, instances: Instances<'w>) -> Self {
        let (verdicts, unused_groups, invalid_pins, summaries, owners, preferred) = {
            let groups = Groups::compile(workspace, config);
            let assignment = groups.assign(&instances);
            let owners: Vec<Option<usize>> = instances.iter().map(|(id, _)| assignment.owner(id)).collect();
            let preferred: Vec<Option<String>> = instances
                .iter()
                .map(|(id, _)| assignment.preferred(id).map(SemverRange::unwrap))
                .collect();
            let summaries = groups
                .groups()
                .iter()
                .map(|group| GroupSummary {
                    configured: group.configured,
                    label: group.label.map(str::to_owned),
                    policy: group.policy.clone(),
                })
                .collect();
            let mut visitor = Visitor::new(&instances, &assignment);
            for (index, group) in groups.groups().iter().enumerate() {
                for (_, ids) in assignment.dependencies(index) {
                    visitor.visit(&group.policy, ids);
                }
            }
            let configured = |group: &Group<'_>| {
                Some(ConfiguredGroup {
                    index: group.configured?,
                    label: group.label.map(str::to_owned),
                })
            };
            let is_invalid = |group: &Group<'_>| matches!(group.policy, Policy::InvalidPin);
            // A banned group that matches nothing is the state it asks for, not a misconfiguration.
            let reportable = |group: &Group<'_>| !is_invalid(group) && !matches!(group.policy, Policy::Banned);
            let unused = groups
                .groups()
                .iter()
                .enumerate()
                .filter(|&(index, group)| !assignment.is_used(index) && reportable(group))
                .filter_map(|(_, group)| configured(group))
                .collect();
            let invalid = groups
                .groups()
                .iter()
                .filter(|group| is_invalid(group))
                .filter_map(configured)
                .collect();
            (visitor.into_verdicts(), unused, invalid, summaries, owners, preferred)
        };
        Self {
            instances,
            verdicts,
            unused_groups,
            invalid_pins,
            groups: summaries,
            owners,
            preferred,
        }
    }
}
