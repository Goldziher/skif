//! Which exports of which modules are used, following re-export chains.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

use skif_js_syntax::{ExportKind, ImportKind, ImportedKind};
use skif_workspace::PackageId;

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

/// How a name reached a module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Via {
    /// Imported by name, as the default export or through a use that may touch anything.
    Named,
    /// Read as a member of a namespace (`ns.member`, `export * as ns` followed by `ns.member`).
    Namespace,
}

enum Task {
    Name(ModuleId, String, MemberUse, Via),
    All(ModuleId),
}

/// The exports referenced from other modules, computed once for the whole graph.
#[derive(Debug, Clone, Default)]
pub struct ExportUsage {
    used: HashMap<ModuleId, HashMap<String, MemberUse>>,
    all: HashSet<ModuleId>,
    /// Modules some export of which was reached by name or as a whole.
    reached_by_name: HashSet<ModuleId>,
    /// Modules some export of which was reached as a namespace member.
    reached_by_namespace: HashSet<ModuleId>,
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
                    let Target::Module(id) = &resolved.target else {
                        continue;
                    };
                    // `export { x }` of an imported binding names the source of the `import`, not of an
                    // `export … from`, so any import of the specifier locates the module; an `export … from`
                    // wins when there are several.
                    match import.kind {
                        ImportKind::ExportFrom => {
                            sources.insert(import.specifier.as_str(), *id);
                        }
                        ImportKind::ImportMetaGlob | ImportKind::SideEffect | ImportKind::Mock => {}
                        _ => {
                            sources.entry(import.specifier.as_str()).or_insert(*id);
                        }
                    }
                }
                let mut reexports = Reexports::default();
                for export in &module.info.exports {
                    if !(export.kind == ExportKind::StarReExport && export.name == "*") {
                        reexports.defined.insert(export.name.clone());
                    }
                    let Some(target) = export.from.as_deref().and_then(|from| sources.get(from)).copied() else {
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
                            reexports
                                .namespaces
                                .entry(export.name.clone())
                                .or_default()
                                .push(target);
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
        Self::compute_by(graph, |_| include_entry_exports)
    }

    /// Like [`compute`](Self::compute), with `include_entry_exports` decided per module: it is called with the owner of
    /// each entry module.
    #[must_use]
    pub fn compute_by(graph: &ModuleGraph, include_entry_exports: impl Fn(PackageId) -> bool) -> Self {
        let mut usage = Self::default();
        let index = ReexportIndex::build(graph);
        let mut queue = VecDeque::new();
        for module in graph.modules() {
            if module.is_entry && !include_entry_exports(module.owner) {
                queue.push_back(Task::All(module.id));
            }
            for resolved in &module.imports {
                let Some(import) = module.info.imports.get(resolved.import) else {
                    continue;
                };
                if matches!(
                    import.kind,
                    ImportKind::ExportFrom | ImportKind::SideEffect | ImportKind::Mock
                ) {
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
                        ImportedKind::Default => {
                            queue.push_back(Task::Name(target, "default".to_owned(), access, Via::Named));
                        }
                        ImportedKind::Named(export) => {
                            queue.push_back(Task::Name(target, export.clone(), access, Via::Named));
                        }
                        ImportedKind::Namespace => {
                            if name.escapes || name.local.is_none() {
                                queue.push_back(Task::All(target));
                            } else {
                                queue.extend(name.accessed_members.iter().map(|member| {
                                    Task::Name(target, member.clone(), MemberUse::everything(), Via::Namespace)
                                }));
                            }
                        }
                    }
                }
            }
        }
        while let Some(task) = queue.pop_front() {
            match task {
                Task::All(id) => usage.mark_all(&index, id, &mut queue),
                Task::Name(id, name, access, via) => usage.mark_name(&index, id, &name, &access, via, &mut queue),
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
                Some((_, source)) => {
                    queue.push_back(Task::Name(*target, source.clone(), MemberUse::everything(), Via::Named));
                }
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
        via: Via,
        queue: &mut VecDeque<Task>,
    ) {
        match via {
            Via::Named => self.reached_by_name.insert(id),
            Via::Namespace => self.reached_by_namespace.insert(id),
        };
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
            queue.push_back(Task::Name(*target, source.clone(), access.clone(), via));
        }
        // `export * as name from` (or `import * as name; export { name }`): only the members read through `name`
        // are used, unless the namespace is used as a whole.
        for target in reexports.namespaces.get(name).into_iter().flatten() {
            if access.all || access.members.is_empty() {
                queue.push_back(Task::All(*target));
            } else {
                queue.extend(
                    access
                        .members
                        .iter()
                        .map(|member| Task::Name(*target, member.clone(), MemberUse::everything(), Via::Namespace)),
                );
            }
        }
        if !reexports.defined.contains(name) && name != "default" {
            for target in &reexports.stars {
                if index.exposed[target.index()].contains(name) {
                    queue.push_back(Task::Name(*target, name.to_owned(), access.clone(), via));
                }
            }
        }
    }

    /// Whether another module uses export `name` of `module`.
    #[must_use]
    pub fn is_used(&self, module: ModuleId, name: &str) -> bool {
        self.all.contains(&module) || self.used.get(&module).is_some_and(|names| names.contains_key(name))
    }

    /// Whether every use of `module` reads members of a namespace: it is imported with `import * as ns` or re-exported
    /// with `export * as ns` and only `ns.member` is accessed, never `{ member }` by name and never the whole namespace.
    ///
    /// The unused exports of such a module are `unused-namespace-exports` and `unused-namespace-types`, not
    /// `unused-exports` and `unused-types`.
    #[must_use]
    pub fn is_namespace_only(&self, module: ModuleId) -> bool {
        self.reached_by_namespace.contains(&module)
            && !self.reached_by_name.contains(&module)
            && !self.all.contains(&module)
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
