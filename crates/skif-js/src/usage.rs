//! Which exports of which modules are used, following re-export chains.

use std::collections::{BTreeSet, VecDeque};

use skif_core::{HashMap, HashSet};
use skif_js_syntax::{ExportKind, ImportKind, ImportedKind, ImportedName, ModuleInfo};
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

/// How a name reached a module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Via {
    /// Imported by name, as the default export or through a use that may touch anything.
    Named,
    /// Read as a member of a namespace (`ns.member`, `export * as ns` followed by `ns.member`).
    Namespace,
}

/// How a use reads the members of an export while it propagates: [`MemberUse`] with the names borrowed from the graph
/// and the re-export index, so passing a use on along a re-export chain allocates nothing.
#[derive(Debug, Clone, Default)]
struct Access<'a> {
    members: BTreeSet<&'a str>,
    all: bool,
}

impl Access<'_> {
    fn everything() -> Self {
        Self {
            members: BTreeSet::new(),
            all: true,
        }
    }
}

enum Task<'a> {
    Name(ModuleId, &'a str, Access<'a>, Via),
    All(ModuleId),
    /// Every value export of a module: a use of the module object as a whole, which holds none of its types.
    Values(ModuleId),
}

/// The exports referenced from other modules, computed once for the whole graph.
#[derive(Debug, Clone, Default)]
pub struct ExportUsage {
    used: HashMap<ModuleId, HashMap<String, MemberUse>>,
    all: HashSet<ModuleId>,
    /// Modules whose value exports were all used by a use of the module object as a whole.
    values: HashSet<ModuleId>,
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
    /// Per module, the names `export *` forwards to it, transitively, that it does not define itself. With the names
    /// it defines, they are every name it exposes ([`Self::exposes`]).
    forwarded: Vec<HashSet<String>>,
}

impl ReexportIndex {
    fn build(graph: &ModuleGraph) -> Self {
        let modules: Vec<Reexports> = graph
            .modules()
            .iter()
            .map(|module| {
                let mut sources: HashMap<&str, ModuleId> = HashMap::default();
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
                        ImportKind::ImportMetaGlob
                        | ImportKind::SourceGlob
                        | ImportKind::SideEffect
                        | ImportKind::Mock
                        | ImportKind::Resolve => {}
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
        let defined: Vec<&HashSet<String>> = modules.iter().map(|reexports| &reexports.defined).collect();
        let stars: Vec<Vec<usize>> = modules
            .iter()
            .map(|reexports| reexports.stars.iter().map(|target| target.index()).collect())
            .collect();
        let forwarded = forwarded_names(&defined, &stars);
        Self { modules, forwarded }
    }

    /// Whether module `id` exposes `name`: defines or re-exports it itself, or forwards it with `export *`.
    fn exposes(&self, id: ModuleId, name: &str) -> bool {
        self.modules[id.index()].defined.contains(name) || self.forwarded[id.index()].contains(name)
    }
}

/// For every module, the names its `export *` re-exports forward to it, transitively, other than `default` and the
/// names it defines. `defined` holds the names each module defines, `stars` the modules each one re-exports with
/// `export *`, by index. Only the modules with an `export *` gain names, so only those are revisited until nothing
/// changes.
fn forwarded_names(defined: &[&HashSet<String>], stars: &[Vec<usize>]) -> Vec<HashSet<String>> {
    let starred: Vec<usize> = stars
        .iter()
        .enumerate()
        .filter(|(_, targets)| !targets.is_empty())
        .map(|(index, _)| index)
        .collect();
    let mut forwarded: Vec<HashSet<String>> = vec![HashSet::default(); defined.len()];
    loop {
        let mut changed = false;
        for &index in &starred {
            for &target in &stars[index] {
                let additions: Vec<String> = defined[target]
                    .iter()
                    .chain(&forwarded[target])
                    .filter(|name| {
                        name.as_str() != "default"
                            && !defined[index].contains(*name)
                            && !forwarded[index].contains(*name)
                    })
                    .cloned()
                    .collect();
                if !additions.is_empty() {
                    changed = true;
                    forwarded[index].extend(additions);
                }
            }
        }
        if !changed {
            break;
        }
    }
    forwarded
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
                // A resolve call takes the path of the module and loads none of it.
                if matches!(
                    import.kind,
                    ImportKind::ExportFrom | ImportKind::SideEffect | ImportKind::Mock | ImportKind::Resolve
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
                    queue.push_back(if loads_module_value(import.kind) {
                        Task::Values(target)
                    } else {
                        Task::All(target)
                    });
                    continue;
                }
                for name in &import.names {
                    let access = Access {
                        members: name.accessed_members.iter().map(String::as_str).collect(),
                        all: name.escapes,
                    };
                    match &name.kind {
                        ImportedKind::Default => {
                            queue.push_back(Task::Name(target, "default", access, Via::Named));
                        }
                        ImportedKind::Named(export) => {
                            queue.push_back(Task::Name(target, export, access, Via::Named));
                        }
                        // A namespace without a local binding that does not escape (a bare `typeof import("x")`)
                        // reads no member, so it credits nothing. A member read only through its own members
                        // (`ns.E.A`) uses those, otherwise all of it. A namespace used as a whole is the module
                        // object, which holds the values only: its types are used only when read by name
                        // (`ns.Type`), or when the importer exports the namespace, whose importers can read any.
                        ImportedKind::Namespace => {
                            if name.escapes && exports_binding(&module.info, name) {
                                queue.push_back(Task::All(target));
                            } else {
                                if name.escapes {
                                    queue.push_back(Task::Values(target));
                                }
                                queue.extend(name.accessed_members.iter().map(|member| {
                                    let access =
                                        name.nested_members
                                            .get(member)
                                            .map_or_else(Access::everything, |inner| Access {
                                                members: inner.iter().map(String::as_str).collect(),
                                                all: false,
                                            });
                                    Task::Name(target, member, access, Via::Namespace)
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
                Task::Values(id) => usage.mark_values(graph, &index, id, &mut queue),
                Task::Name(id, name, access, via) => usage.mark_name(&index, id, name, &access, via, &mut queue),
            }
        }
        usage
    }

    fn mark_all<'a>(&mut self, index: &'a ReexportIndex, id: ModuleId, queue: &mut VecDeque<Task<'a>>) {
        if !self.all.insert(id) {
            return;
        }
        for (target, named) in &index.modules[id.index()].all {
            match named {
                Some((_, source)) => {
                    queue.push_back(Task::Name(*target, source, Access::everything(), Via::Named));
                }
                None => queue.push_back(Task::All(*target)),
            }
        }
    }

    /// Marks every value export of `id` used, and those of the modules it re-exports with `export *`. A re-export
    /// that is not type-only counts as a value: its source decides what it is.
    fn mark_values<'a>(
        &mut self,
        graph: &'a ModuleGraph,
        index: &'a ReexportIndex,
        id: ModuleId,
        queue: &mut VecDeque<Task<'a>>,
    ) {
        if self.all.contains(&id) || !self.values.insert(id) {
            return;
        }
        for export in &graph.module(id).info.exports {
            let star = export.kind == ExportKind::StarReExport && export.name == "*";
            if !star && !export.is_type {
                queue.push_back(Task::Name(id, export.name.as_str(), Access::everything(), Via::Named));
            }
        }
        for target in &index.modules[id.index()].stars {
            queue.push_back(Task::Values(*target));
        }
    }

    fn mark_name<'a>(
        &mut self,
        index: &'a ReexportIndex,
        id: ModuleId,
        name: &'a str,
        access: &Access<'a>,
        via: Via,
        queue: &mut VecDeque<Task<'a>>,
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
        if fresh {
            slot.insert(name.to_owned(), MemberUse::default());
        }
        let Some(entry) = slot.get_mut(name) else {
            return;
        };
        let before = (entry.members.len(), entry.all);
        for member in &access.members {
            if !entry.members.contains(*member) {
                entry.members.insert((*member).to_owned());
            }
        }
        entry.all |= access.all;
        if !fresh && (entry.members.len(), entry.all) == before {
            return;
        }
        let reexports = &index.modules[id.index()];
        for (target, source) in reexports.named.get(name).into_iter().flatten() {
            queue.push_back(Task::Name(*target, source, access.clone(), via));
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
                        .map(|&member| Task::Name(*target, member, Access::everything(), Via::Namespace)),
                );
            }
        }
        if !reexports.defined.contains(name) && name != "default" {
            for target in &reexports.stars {
                if index.exposes(*target, name) {
                    queue.push_back(Task::Name(*target, name, access.clone(), via));
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

/// Whether an import of `kind` without names loads the module object itself (`import("x")`, `require("x")`), which
/// holds the values of the module and none of its types.
pub(crate) const fn loads_module_value(kind: ImportKind) -> bool {
    matches!(kind, ImportKind::Dynamic | ImportKind::Require)
}

/// Whether the module `info` exports the binding of the imported `name` itself (`export default ns`, `export = ns`),
/// so that its importers can read the types of the module the binding stands for through it.
pub(crate) fn exports_binding(info: &ModuleInfo, name: &ImportedName) -> bool {
    name.local.as_deref().is_some_and(|local| {
        info.exports
            .iter()
            .any(|export| export.local_name.as_deref() == Some(local))
    })
}

#[cfg(test)]
mod tests {
    use skif_core::HashSet;

    use super::forwarded_names;

    fn names(items: &[&str]) -> HashSet<String> {
        items.iter().map(|&item| item.to_owned()).collect()
    }

    #[test]
    fn export_star_forwards_names_transitively_without_default_or_own_names() {
        // 0: `export * from 1`, defines `a`; 1: `export * from 2`, defines `b` and `default`; 2: defines `a` and `c`.
        let defined = [names(&["a"]), names(&["b", "default"]), names(&["a", "c"])];
        let defined: Vec<&HashSet<String>> = defined.iter().collect();
        let forwarded = forwarded_names(&defined, &[vec![1], vec![2], Vec::new()]);
        assert_eq!(forwarded, [names(&["b", "c"]), names(&["a", "c"]), names(&[])]);
    }

    #[test]
    fn a_cycle_of_export_star_settles() {
        let defined = [names(&["a"]), names(&["b"])];
        let defined: Vec<&HashSet<String>> = defined.iter().collect();
        let forwarded = forwarded_names(&defined, &[vec![1], vec![0]]);
        assert_eq!(forwarded, [names(&["b"]), names(&["a"])]);
    }
}
