//! Which exports of which modules are used, following re-export chains.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

use skif_js_syntax::{ExportKind, ImportKind, ImportedKind};

use crate::graph::{ModuleGraph, ModuleId, Target};

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

/// The re-exports of one module, resolved to the modules they come from.
#[derive(Default)]
struct Reexports {
    /// `export { source as name } from`, by exported `name`.
    named: HashMap<String, Vec<(ModuleId, String)>>,
    /// `export * as name from`, by `name`.
    namespaces: HashMap<String, Vec<ModuleId>>,
    /// `export * from`.
    stars: Vec<ModuleId>,
    /// Every target with the name it is re-exported under: `Some((name, source))` for a named re-export.
    all: Vec<(ModuleId, Option<(String, String)>)>,
    /// Names the module defines or re-exports itself, excluding what `export *` forwards.
    defined: HashSet<String>,
}

/// Re-export lookups for every module, computed once so that propagating a name touches only what can answer it.
struct ReexportIndex {
    modules: Vec<Reexports>,
    /// Per module, every name it exposes: its own and those forwarded by `export *`, transitively.
    exposed: Vec<HashSet<String>>,
}

impl ReexportIndex {
    fn build(graph: &ModuleGraph) -> Self {
        let modules: Vec<Reexports> = graph
            .modules()
            .iter()
            .map(|module| {
                let mut sources: HashMap<&str, ModuleId> = HashMap::new();
                for resolved in &module.imports {
                    let Some(import) = module.info.imports.get(resolved.import) else {
                        continue;
                    };
                    if let (ImportKind::ExportFrom, Target::Module(id)) = (import.kind, &resolved.target) {
                        sources.entry(import.specifier.as_str()).or_insert(*id);
                    }
                }
                let mut reexports = Reexports::default();
                for export in &module.info.exports {
                    if !(export.kind == ExportKind::StarReExport && export.name == "*") {
                        reexports.defined.insert(export.name.clone());
                    }
                    let Some(target) = export
                        .from
                        .as_deref()
                        .and_then(|from| sources.get(from))
                        .copied()
                    else {
                        continue;
                    };
                    match export.kind {
                        ExportKind::ReExport => {
                            let source = export.imported_name.clone().unwrap_or_else(|| export.name.clone());
                            reexports
                                .named
                                .entry(export.name.clone())
                                .or_default()
                                .push((target, source.clone()));
                            reexports.all.push((target, Some((export.name.clone(), source))));
                        }
                        ExportKind::StarReExport if export.name == "*" => {
                            reexports.stars.push(target);
                            reexports.all.push((target, None));
                        }
                        ExportKind::StarReExport => {
                            reexports.namespaces.entry(export.name.clone()).or_default().push(target);
                            reexports.all.push((target, None));
                        }
                        _ => {}
                    }
                }
                reexports
            })
            .collect();
        let mut exposed: Vec<HashSet<String>> = modules.iter().map(|m| m.defined.clone()).collect();
        loop {
            let mut changed = false;
            for (index, reexports) in modules.iter().enumerate() {
                for target in &reexports.stars {
                    let additions: Vec<String> = exposed[target.index()]
                        .iter()
                        .filter(|name| name.as_str() != "default" && !exposed[index].contains(*name))
                        .cloned()
                        .collect();
                    if !additions.is_empty() {
                        changed = true;
                        exposed[index].extend(additions);
                    }
                }
            }
            if !changed {
                break;
            }
        }
        Self { modules, exposed }
    }
}

impl ExportUsage {
    /// Propagates every import of the graph to the exports it uses.
    ///
    /// Exports of entry files count as used unless `include_entry_exports` is set.
    #[must_use]
    pub fn compute(graph: &ModuleGraph, include_entry_exports: bool) -> Self {
        let mut usage = Self::default();
        let index = ReexportIndex::build(graph);
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
                Task::All(id) => usage.mark_all(&index, id, &mut queue),
                Task::Name(id, name, access) => usage.mark_name(&index, id, &name, &access, &mut queue),
            }
        }
        usage
    }

    fn mark_all(&mut self, index: &ReexportIndex, id: ModuleId, queue: &mut VecDeque<Task>) {
        if !self.all.insert(id) {
            return;
        }
        for (target, named) in &index.modules[id.index()].all {
            match named {
                Some((_, source)) => queue.push_back(Task::Name(*target, source.clone(), MemberUse::everything())),
                None => queue.push_back(Task::All(*target)),
            }
        }
    }

    fn mark_name(
        &mut self,
        index: &ReexportIndex,
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
        let reexports = &index.modules[id.index()];
        for (target, source) in reexports.named.get(name).into_iter().flatten() {
            queue.push_back(Task::Name(*target, source.clone(), access.clone()));
        }
        for target in reexports.namespaces.get(name).into_iter().flatten() {
            queue.push_back(Task::All(*target));
        }
        if !reexports.defined.contains(name) && name != "default" {
            for target in &reexports.stars {
                if index.exposed[target.index()].contains(name) {
                    queue.push_back(Task::Name(*target, name.to_owned(), access.clone()));
                }
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
