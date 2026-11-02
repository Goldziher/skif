//! One analysis of the whole workspace: instances, group assignment and the state of every instance.

use skif_config::Config;
use skif_workspace::Workspace;

use super::groups::Groups;
use super::instances::Instances;
use super::state::Verdict;
use super::visit::Visitor;

/// A configured version group that owns no instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnusedGroup {
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
    pub(crate) unused_groups: Vec<UnusedGroup>,
}

impl<'w> Analysis<'w> {
    pub(crate) fn run(workspace: &'w Workspace, config: &Config) -> Self {
        let instances = Instances::collect(workspace, config);
        let (verdicts, unused_groups) = {
            let groups = Groups::compile(workspace, config);
            let assignment = groups.assign(&instances);
            let mut visitor = Visitor::new(&instances, &assignment);
            for (index, group) in groups.groups().iter().enumerate() {
                for (_, ids) in assignment.dependencies(index) {
                    visitor.visit(&group.policy, ids);
                }
            }
            let unused = groups
                .groups()
                .iter()
                .enumerate()
                .filter(|&(index, _)| !assignment.is_used(index))
                .filter_map(|(_, group)| {
                    Some(UnusedGroup {
                        index: group.configured?,
                        label: group.label.map(str::to_owned),
                    })
                })
                .collect();
            (visitor.into_verdicts(), unused)
        };
        Self {
            instances,
            verdicts,
            unused_groups,
        }
    }
}
