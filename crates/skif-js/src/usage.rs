//! Which exports of which modules are used, following re-export chains.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

use skif_js_syntax::{Export, ExportKind, ImportKind, ImportedKind};

use crate::graph::{Module, ModuleGraph, ModuleId, Target};

/// How the members of one export (an enum's or a namespace's) are used.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemberUse {
    /// Members read by name.
    pub members: BTreeSet<String>,
    /// The export is used in a way that may touch any member.
    pub all: bool,
}

impl MemberUse {
    fn everything() -> Self {
        Self {
            members: BTreeSet::new(),
            all: true,
        }
    }
}

enum Task {
    Name(ModuleId, String, MemberUse),
    All(ModuleId),
}

/// The exports referenced from other modules, computed once for the whole graph.
#[derive(Debug, Clone, Default)]
pub struct ExportUsage {
    used: HashMap<ModuleId, HashMap<String, MemberUse>>,
    all: HashSet<ModuleId>,
}

fn reexport_target(module: &Module, export: &Export) -> Option<ModuleId> {
    export.from.as_deref().and_then(|from| module.reexport_source(from))
}

impl ExportUsage {
    /// Propagates every import of the graph to the exports it uses.
    ///
    /// Exports of entry files count as used unless `include_entry_exports` is set.
    #[must_use]
    pub fn compute(graph: &ModuleGraph, include_entry_exports: bool) -> Self {
        let mut usage = Self::default();
        let mut queue = VecDeque::new();
        for module in graph.modules() {
            if module.is_entry && !include_entry_exports {
                queue.push_back(Task::All(module.id));
            }
            for resolved in &module.imports {
                let Some(import) = module.info.imports.get(resolved.import) else {
                    continue;
                };
                if matches!(import.kind, ImportKind::ExportFrom | ImportKind::SideEffect) {
                    continue;
                }
                let target = match &resolved.target {
                    Target::Module(id) => *id,
                    Target::Modules(ids) => {
                        queue.extend(ids.iter().map(|id| Task::All(*id)));
                        continue;
                    }
                    _ => continue,
                };
                if import.names.is_empty() {
                    queue.push_back(Task::All(target));
                    continue;
                }
                for name in &import.names {
                    let access = MemberUse {
                        members: name.accessed_members.iter().cloned().collect(),
                        all: name.escapes,
                    };
                    match &name.kind {
                        ImportedKind::Default => queue.push_back(Task::Name(target, "default".to_owned(), access)),
                        ImportedKind::Named(export) => queue.push_back(Task::Name(target, export.clone(), access)),
                        ImportedKind::Namespace => {
                            if name.escapes || name.local.is_none() {
                                queue.push_back(Task::All(target));
                            } else {
                                queue.extend(
                                    name.accessed_members
                                        .iter()
                                        .map(|member| Task::Name(target, member.clone(), MemberUse::everything())),
                                );
                            }
                        }
                    }
                }
            }
        }
        while let Some(task) = queue.pop_front() {
            match task {
                Task::All(id) => usage.mark_all(graph, id, &mut queue),
                Task::Name(id, name, access) => usage.mark_name(graph, id, &name, &access, &mut queue),
            }
        }
        usage
    }

    fn mark_all(&mut self, graph: &ModuleGraph, id: ModuleId, queue: &mut VecDeque<Task>) {
        if !self.all.insert(id) {
            return;
        }
        let module = graph.module(id);
        for export in &module.info.exports {
            let Some(target) = reexport_target(module, export) else {
                continue;
            };
            match export.kind {
                ExportKind::ReExport => {
                    let name = export.imported_name.clone().unwrap_or_else(|| export.name.clone());
                    queue.push_back(Task::Name(target, name, MemberUse::everything()));
                }
                ExportKind::StarReExport => queue.push_back(Task::All(target)),
                _ => {}
            }
        }
    }

    fn mark_name(
        &mut self,
        graph: &ModuleGraph,
        id: ModuleId,
        name: &str,
        access: &MemberUse,
        queue: &mut VecDeque<Task>,
    ) {
        if self.all.contains(&id) {
            return;
        }
        let slot = self.used.entry(id).or_default();
        let fresh = !slot.contains_key(name);
        let entry = slot.entry(name.to_owned()).or_default();
        let before = (entry.members.len(), entry.all);
        entry.members.extend(access.members.iter().cloned());
        entry.all |= access.all;
        if !fresh && (entry.members.len(), entry.all) == before {
            return;
        }
        let module = graph.module(id);
        let defines_own = module
            .info
            .exports
            .iter()
            .any(|export| export.name == name && !(export.kind == ExportKind::StarReExport && export.name == "*"));
        for export in &module.info.exports {
            let Some(target) = reexport_target(module, export) else {
                continue;
            };
            match export.kind {
                ExportKind::ReExport if export.name == name => {
                    let source = export.imported_name.clone().unwrap_or_else(|| export.name.clone());
                    queue.push_back(Task::Name(target, source, access.clone()));
                }
                ExportKind::StarReExport if export.name == "*" => {
                    if !defines_own && name != "default" {
                        queue.push_back(Task::Name(target, name.to_owned(), access.clone()));
                    }
                }
                ExportKind::StarReExport if export.name == name => queue.push_back(Task::All(target)),
                _ => {}
            }
        }
    }

    /// Whether another module uses export `name` of `module`.
    #[must_use]
    pub fn is_used(&self, module: ModuleId, name: &str) -> bool {
        self.all.contains(&module) || self.used.get(&module).is_some_and(|names| names.contains_key(name))
    }

    /// Whether member `member` of export `name` of `module` is used, or the export is used as a whole.
    #[must_use]
    pub fn is_member_used(&self, module: ModuleId, name: &str, member: &str) -> bool {
        if self.all.contains(&module) {
            return true;
        }
        self.used
            .get(&module)
            .and_then(|names| names.get(name))
            .is_some_and(|usage| usage.all || usage.members.contains(member))
    }
}
