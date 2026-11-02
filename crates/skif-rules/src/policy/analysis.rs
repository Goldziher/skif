//! One analysis of the whole workspace: instances, group assignment and the state of every instance.

use skif_config::Config;
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
}

impl<'w> Analysis<'w> {
    pub(crate) fn run(workspace: &'w Workspace, config: &Config) -> Self {
        let instances = Instances::collect(workspace, config);
        let (verdicts, unused_groups, invalid_pins) = {
            let groups = Groups::compile(workspace, config);
            let assignment = groups.assign(&instances);
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
            let unused = groups
                .groups()
                .iter()
                .enumerate()
                .filter(|&(index, group)| !assignment.is_used(index) && !is_invalid(group))
                .filter_map(|(_, group)| configured(group))
                .collect();
            let invalid = groups
                .groups()
                .iter()
                .filter(|group| is_invalid(*group))
                .filter_map(configured)
                .collect();
            (visitor.into_verdicts(), unused, invalid)
        };
        Self {
            instances,
            verdicts,
            unused_groups,
            invalid_pins,
        }
    }
}
