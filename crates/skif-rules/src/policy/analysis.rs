//! One analysis of the whole workspace: instances, group assignment and the state of every instance.

use std::collections::BTreeSet;

use skif_config::Config;
use skif_workspace::{Workspace, map_ordered};

use super::groups::{Group, Groups, Policy};
use super::instances::{InstanceId, Instances};
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
}

impl<'w> Analysis<'w> {
    /// Runs the version groups with the catalog rules owning the dependencies `catalog_owns` selects, as the
    /// provider does.
    #[cfg(test)]
    pub(crate) fn run(workspace: &'w Workspace, config: &Config, catalog_owns: bool) -> Self {
        let instances = Instances::collect(workspace, config, catalog_owns);
        Self::from_instances(workspace, config, instances, true)
    }

    /// Runs the version groups; the dependencies in `owned` are left to the catalog rules (see
    /// [`Instances::collect_with`]). With `sequential` everything runs on the calling thread.
    pub(crate) fn run_with(
        workspace: &'w Workspace,
        config: &Config,
        owned: &BTreeSet<&str>,
        sequential: bool,
    ) -> Self {
        Self::from_instances(
            workspace,
            config,
            Instances::collect_with(
                workspace,
                owned,
                &config.custom_types,
                config.policy.override_reading,
                sequential,
            ),
            sequential,
        )
    }

    fn from_instances(workspace: &'w Workspace, config: &Config, instances: Instances<'w>, sequential: bool) -> Self {
        let (verdicts, unused_groups, invalid_pins) = {
            let groups = Groups::compile(workspace, config);
            let assignment = groups.assign(&instances);
            // Each dependency of each group is visited on its own, in parallel unless the run is `sequential`; the
            // verdicts are applied in the order a single pass would give them.
            let visits: Vec<(&Policy, &[InstanceId])> = groups
                .groups()
                .iter()
                .enumerate()
                .flat_map(|(index, group)| assignment.dependencies(index).map(move |(_, ids)| (&group.policy, ids)))
                .collect();
            let marks: Vec<Vec<(InstanceId, Verdict)>> = map_ordered(&visits, sequential, |&(policy, ids)| {
                let mut visitor = Visitor::new(&instances, &assignment);
                visitor.visit(policy, ids);
                visitor.into_marks()
            });
            let mut verdicts: Vec<Option<Verdict>> = (0..instances.len()).map(|_| None).collect();
            for (id, verdict) in marks.into_iter().flatten() {
                verdicts[id.index()] = Some(verdict);
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
            (verdicts, unused, invalid)
        };
        Self {
            instances,
            verdicts,
            unused_groups,
            invalid_pins,
        }
    }
}
