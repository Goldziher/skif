//! Which members of exported classes are used, for `unused-class-members` and `explain-usage` (ADR 0042).
//!
//! The rule keeps two questions apart. Which members are left alone whatever reads them (decorated, type-only,
//! inherited from a base class outside the project, implementing an interface, public API, tagged or listed in
//! `ignore-members`) is decided here from the declarations, by [`exemption`]. Which members are read is asked of a
//! [`MemberUsage`]: [`NameReads`] credits a member when any module of the graph reads a property of its name, the
//! approximation that needs no type information, and [`TypedMemberUsage`] takes the uses from the type checker where
//! it covered the declaring file (ADR 0044, ADR 0046).

use std::collections::VecDeque;

use serde::Serialize;
use skif_core::{HashMap, HashSet};
use skif_js_syntax::{
    ClassMember, ClassMemberKind, ClassShape, Export, ExportKind, Heritage, ImportKind, ImportedKeyRead,
};

use crate::analysis::JsAnalysis;
use crate::graph::{Module, ModuleGraph, ModuleId, Target};
use crate::types_bridge::TypeState;

/// Superclass hops followed across modules before the base counts as unknown.
const MAX_DEPTH: usize = 8;

/// A source of class member use: which members of exported classes some module uses.
pub(crate) trait MemberUsage {
    /// The modules that use `member` of the class behind export `export` of `module`, sorted.
    fn users(&self, module: &Module, export: &Export, member: &ClassMember) -> Vec<ModuleId>;

    /// Whether any module uses `member` of the class behind `export` of `module`.
    fn is_used(&self, module: &Module, export: &Export, member: &ClassMember) -> bool {
        !self.users(module, export, member).is_empty()
    }
}

/// Member use by name: a member is used when a module of the graph reads a property of its name on any object
/// ([`skif_js_syntax::ModuleInfo::property_reads`]), also through an element access whose key is a constant imported
/// from another module ([`skif_js_syntax::ModuleInfo::imported_key_reads`]), and a setter also when one assigns it.
/// An ECMAScript private name (`#count`) can only be read inside its class, so only its own module counts.
pub(crate) struct NameReads {
    reads: HashMap<String, Vec<ModuleId>>,
    writes: HashMap<String, Vec<ModuleId>>,
}

impl NameReads {
    /// The reads and writes of every module of `graph`.
    pub(crate) fn of(graph: &ModuleGraph) -> Self {
        let mut reads: HashMap<String, Vec<ModuleId>> = HashMap::default();
        let mut writes: HashMap<String, Vec<ModuleId>> = HashMap::default();
        for module in graph.modules() {
            for name in module.info.property_reads.iter().filter(|name| !name.starts_with('#')) {
                reads.entry(name.clone()).or_default().push(module.id);
            }
            for name in module.info.property_writes.iter().filter(|name| !name.starts_with('#')) {
                writes.entry(name.clone()).or_default().push(module.id);
            }
            for read in &module.info.imported_key_reads {
                if let Some(name) = imported_key(graph, module, read) {
                    reads.entry(name.to_owned()).or_default().push(module.id);
                }
            }
        }
        Self { reads, writes }
    }
}

/// The property name the imported constant key `read` of `module` holds: the string behind the export it names in
/// the module that declares it, followed through re-exports. `None` when the import leaves the graph or the export
/// holds no string.
fn imported_key<'g>(graph: &'g ModuleGraph, module: &Module, read: &ImportedKeyRead) -> Option<&'g str> {
    let target = module_of(module, &read.specifier)?;
    let (declaring, export) = declared_export(graph, target, &read.export, 0, &|_: &Export| true)?;
    declaring
        .info
        .string_constants
        .iter()
        .find(|constant| constant.export == export.name && constant.member == read.member)
        .map(|constant| constant.value.as_str())
}

impl MemberUsage for NameReads {
    fn users(&self, module: &Module, _export: &Export, member: &ClassMember) -> Vec<ModuleId> {
        let setter = member.kind == ClassMemberKind::Setter;
        if member.name.starts_with('#') {
            let info = &module.info;
            let used = info.property_reads.binary_search(&member.name).is_ok()
                || (setter && info.property_writes.binary_search(&member.name).is_ok());
            return if used { vec![module.id] } else { Vec::new() };
        }
        let mut users: Vec<ModuleId> = self.reads.get(&member.name).cloned().unwrap_or_default();
        if setter {
            users.extend(self.writes.get(&member.name).into_iter().flatten());
        }
        users.sort_by_key(|id| id.index());
        users.dedup();
        users
    }
}

/// Member use with types (ADR 0046). In a declaring file the checker covered, a member is used when a typed read
/// reaches it, its class is used as a whole, a covered file reads its name through a receiver the checker could not
/// type, or a file the checker did not cover (a Vue, Svelte or Astro template, a file in no program) reads its name.
/// A declaring file the checker did not cover, a private name and every member without typed facts keep
/// [`NameReads`] entirely.
pub(crate) struct TypedMemberUsage<'t> {
    names: NameReads,
    types: &'t TypeState,
}

impl<'t> TypedMemberUsage<'t> {
    /// The member uses of `analysis`: typed where it has typed facts, by name elsewhere.
    pub(crate) fn of(analysis: &'t JsAnalysis<'_>) -> Self {
        Self {
            names: NameReads::of(&analysis.graph),
            types: &analysis.types,
        }
    }

    /// Whether the uses of `member` of the class behind `export` of `module` come from the type checker.
    pub(crate) fn is_typed(&self, module: &Module, export: &Export, member: &ClassMember) -> bool {
        !member.name.starts_with('#') && self.types.judges(module, export)
    }

    /// Whether a typed read reaches `member`, or its class is used as a whole.
    pub(crate) fn typed_use(&self, module: &Module, export: &Export, member: &ClassMember) -> bool {
        self.is_typed(module, export, member)
            && (self.types.reads(module, export, &member.name) || self.types.whole(module, export))
    }

    /// The covered modules whose reads of the member's name the checker could not settle, which credit it by name.
    pub(crate) fn unresolved_users(&self, module: &Module, export: &Export, member: &ClassMember) -> Vec<ModuleId> {
        if !self.is_typed(module, export, member) {
            return Vec::new();
        }
        self.types.unresolved_readers(&member.name).to_vec()
    }
}

impl MemberUsage for TypedMemberUsage<'_> {
    /// The modules whose reads by name count: every reader without types, else the readers the checker did not
    /// cover and those whose reads it could not settle.
    fn users(&self, module: &Module, export: &Export, member: &ClassMember) -> Vec<ModuleId> {
        let by_name = self.names.users(module, export, member);
        if !self.is_typed(module, export, member) {
            return by_name;
        }
        let mut users: Vec<ModuleId> = by_name.into_iter().filter(|id| !self.types.covers(*id)).collect();
        users.extend(self.unresolved_users(module, export, member));
        users.sort_by_key(|id| id.index());
        users.dedup();
        users
    }

    fn is_used(&self, module: &Module, export: &Export, member: &ClassMember) -> bool {
        self.typed_use(module, export, member) || !self.users(module, export, member).is_empty()
    }
}

/// Why a member of a used exported class is not judged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum MemberExemption {
    /// The member is tagged `@public` (or another tag that exempts), or `include-tags` names none of its tags.
    Tagged,
    /// The member is on an `ignore-members` list.
    IgnoredName,
    /// The member has a decorator, which may hand it to a framework (`@Input()`, `@HostListener()`).
    Decorated,
    /// The class has a decorator, which may hand every member to a framework (`@Component()`, `@Injectable()`).
    DecoratedClass,
    /// The superclass chain leaves the project for a class whose members are not known (an installed package, a
    /// global such as `HTMLElement`, a mixin call), so the member may override one of them.
    UnknownBase,
    /// The superclass chain reaches a known library base class that declares a member of this name (a React
    /// lifecycle method), which the library calls.
    BaseMember,
    /// The class implements an interface that declares this name, or one whose members are not known.
    Implements,
    /// The class is exported from an entry file (also through re-exports), so it is public API, unless
    /// `include-entry-exports` is set.
    PublicApi,
    /// The field exists only for the type checker and no code reads it at runtime: a `declare` field, or a nominal
    /// brand or phantom field typed `void`, `never`, `undefined` or `unique symbol` with no value but `undefined`.
    TypeOnlyField,
}

impl MemberExemption {
    /// The exemption as a phrase for the human report.
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::Tagged => "tagged `@public` (or another ignored tag)",
            Self::IgnoredName => "listed in `ignore-members`",
            Self::Decorated => "decorated, so a framework may use it",
            Self::DecoratedClass => "its class is decorated, so a framework may use any member",
            Self::UnknownBase => "its class extends a class outside the project whose members are not known",
            Self::BaseMember => "it overrides a member of a known library base class, which the library calls",
            Self::Implements => "it implements an interface member",
            Self::PublicApi => "its class is exported from an entry file, so it is public API",
            Self::TypeOnlyField => "it is a field that exists only for the type checker, never read at runtime",
        }
    }
}

/// The base members of library classes that a subclass overrides for the library to call, by the specifier and
/// export the base is imported as. A base not listed here counts as unknown ([`MemberExemption::UnknownBase`]).
const KNOWN_BASES: &[(&str, &[&str], &[&str])] = &[("react", &["Component", "PureComponent"], REACT_COMPONENT)];

/// The instance and static members of React's `Component` that React reads or that a subclass declares for it.
const REACT_COMPONENT: &[&str] = &[
    "UNSAFE_componentWillMount",
    "UNSAFE_componentWillReceiveProps",
    "UNSAFE_componentWillUpdate",
    "childContextTypes",
    "componentDidCatch",
    "componentDidMount",
    "componentDidUpdate",
    "componentWillMount",
    "componentWillReceiveProps",
    "componentWillUnmount",
    "componentWillUpdate",
    "context",
    "contextType",
    "contextTypes",
    "defaultProps",
    "displayName",
    "forceUpdate",
    "getChildContext",
    "getDerivedStateFromError",
    "getDerivedStateFromProps",
    "getSnapshotBeforeUpdate",
    "props",
    "propTypes",
    "refs",
    "render",
    "setState",
    "shouldComponentUpdate",
    "state",
];

/// What a class inherits from outside the project.
enum Inherited {
    /// Nothing: the chain ends in a class of the project.
    Nothing,
    /// The members of a known library base.
    Names(&'static [&'static str]),
    /// Members that are not known.
    Unknown,
}

/// The members of library bases known by `(specifier, export)`.
fn known_base(specifier: &str, export: &str) -> Option<&'static [&'static str]> {
    KNOWN_BASES
        .iter()
        .find(|(module, exports, _)| *module == specifier && exports.contains(&export))
        .map(|(_, _, members)| *members)
}

/// The module of the graph that `specifier`, as imported by `module`, resolves to.
fn module_of(module: &Module, specifier: &str) -> Option<ModuleId> {
    module.imports.iter().find_map(|resolved| {
        let import = module.info.imports.get(resolved.import)?;
        let imports_module = !matches!(
            import.kind,
            ImportKind::SideEffect
                | ImportKind::Mock
                | ImportKind::Resolve
                | ImportKind::ImportMetaGlob
                | ImportKind::SourceGlob
        );
        match resolved.target {
            Target::Module(id) if imports_module && import.specifier == specifier => Some(id),
            _ => None,
        }
    })
}

/// The export `name` of module `id` that `declares` holds for, following re-exports: the module that declares it
/// and the export.
fn declared_export<'g>(
    graph: &'g ModuleGraph,
    id: ModuleId,
    name: &str,
    depth: usize,
    declares: &dyn Fn(&Export) -> bool,
) -> Option<(&'g Module, &'g Export)> {
    if depth > MAX_DEPTH {
        return None;
    }
    let module = graph.module(id);
    if let Some(export) = module.info.exports.iter().find(|export| export.name == name) {
        let Some(from) = export.from.as_deref() else {
            return declares(export).then_some((module, export));
        };
        let source = export.imported_name.as_deref().unwrap_or(name);
        if export.kind != ExportKind::ReExport || source == "*" {
            return None;
        }
        return declared_export(graph, module_of(module, from)?, source, depth + 1, declares);
    }
    module
        .info
        .exports
        .iter()
        .filter(|export| export.kind == ExportKind::StarReExport && export.name == "*")
        .filter_map(|export| module_of(module, export.from.as_deref()?))
        .find_map(|target| declared_export(graph, target, name, depth + 1, declares))
}

/// The class export `name` of module `id`, following re-exports: the module that declares it and the export.
fn class_export<'g>(graph: &'g ModuleGraph, id: ModuleId, name: &str) -> Option<(&'g Module, &'g Export)> {
    declared_export(graph, id, name, 0, &|export: &Export| export.class.is_some())
}

/// What the class `shape` of `module` inherits from outside the project.
fn inherited(graph: &ModuleGraph, module: &Module, shape: &ClassShape, depth: usize) -> Inherited {
    match &shape.extends {
        None => Inherited::Nothing,
        Some(Heritage::Global(_) | Heritage::Opaque) => Inherited::Unknown,
        Some(Heritage::Imported { specifier, name }) => {
            if let Some(names) = known_base(specifier, name) {
                return Inherited::Names(names);
            }
            let base = (depth < MAX_DEPTH)
                .then(|| module_of(module, specifier))
                .flatten()
                .and_then(|id| class_export(graph, id, name));
            match base {
                Some((base_module, export)) => export.class.as_ref().map_or(Inherited::Unknown, |base| {
                    inherited(graph, base_module, base, depth + 1)
                }),
                None => Inherited::Unknown,
            }
        }
    }
}

/// The exports that entry files expose, as `(module, export name)`, following re-exports to the modules that declare
/// them; entry files whose package sets `include-entry-exports` expose nothing.
pub(crate) fn entry_exposed(analysis: &JsAnalysis<'_>) -> HashSet<(ModuleId, String)> {
    let graph = &analysis.graph;
    let mut queue: VecDeque<(ModuleId, String)> = graph
        .modules()
        .iter()
        .filter(|module| module.is_entry && !analysis.setup.includes_entry_exports(module.owner))
        .flat_map(|module| {
            module
                .info
                .exports
                .iter()
                .map(move |export| (module.id, export.name.clone()))
        })
        .collect();
    let mut exposed = HashSet::default();
    while let Some((id, name)) = queue.pop_front() {
        if !exposed.insert((id, name.clone())) {
            continue;
        }
        let module = graph.module(id);
        let named: Vec<&Export> = module
            .info
            .exports
            .iter()
            .filter(|export| export.name == name)
            .collect();
        for export in &named {
            let Some(target) = export.from.as_deref().and_then(|from| module_of(module, from)) else {
                continue;
            };
            match (export.kind, export.imported_name.as_deref()) {
                (ExportKind::ReExport, Some(source)) => queue.push_back((target, source.to_owned())),
                (ExportKind::StarReExport, _) => {
                    let star = export.name == "*";
                    queue.extend(
                        graph
                            .module(target)
                            .info
                            .exports
                            .iter()
                            .filter(|inner| !(star && inner.name == "default"))
                            .map(|inner| (target, inner.name.clone())),
                    );
                }
                _ => {}
            }
        }
        if named.is_empty() && name != "default" {
            for export in &module.info.exports {
                if export.kind == ExportKind::StarReExport
                    && export.name == "*"
                    && let Some(target) = export.from.as_deref().and_then(|from| module_of(module, from))
                {
                    queue.push_back((target, name.clone()));
                }
            }
        }
    }
    exposed
}

/// The facts about one exported class that every member's judgement shares.
pub(crate) struct ClassFacts {
    inherited: Inherited,
    public_api: bool,
}

impl ClassFacts {
    /// The facts of the class `shape` behind `export` of `module`; `exposed` is [`entry_exposed`].
    pub(crate) fn of(
        analysis: &JsAnalysis<'_>,
        module: &Module,
        export: &Export,
        shape: &ClassShape,
        exposed: &HashSet<(ModuleId, String)>,
    ) -> Self {
        Self {
            inherited: inherited(&analysis.graph, module, shape, 0),
            public_api: exposed.contains(&(module.id, export.name.clone())),
        }
    }
}

/// Why `member` of the class `shape` of `module` is not judged, or `None` when it is.
pub(crate) fn exemption(
    analysis: &JsAnalysis<'_>,
    module: &Module,
    shape: &ClassShape,
    facts: &ClassFacts,
    member: &ClassMember,
) -> Option<MemberExemption> {
    if facts.public_api {
        return Some(MemberExemption::PublicApi);
    }
    if shape.decorated {
        return Some(MemberExemption::DecoratedClass);
    }
    if member.decorated {
        return Some(MemberExemption::Decorated);
    }
    if member.type_only {
        return Some(MemberExemption::TypeOnlyField);
    }
    if analysis.exempts_tags(&member.tags) {
        return Some(MemberExemption::Tagged);
    }
    if analysis.ignores_member(module.owner, &member.name) {
        return Some(MemberExemption::IgnoredName);
    }
    match facts.inherited {
        Inherited::Unknown => return Some(MemberExemption::UnknownBase),
        Inherited::Names(names) if names.contains(&member.name.as_str()) => return Some(MemberExemption::BaseMember),
        Inherited::Names(_) | Inherited::Nothing => {}
    }
    let implements =
        !member.is_static && (shape.implements_unknown || shape.implemented.binary_search(&member.name).is_ok());
    implements.then_some(MemberExemption::Implements)
}

/// Whether the class exports of `module` are judged at all: its exports are reported and it is no declaration file.
pub(crate) fn judges_classes_of(analysis: &JsAnalysis<'_>, module: &Module) -> bool {
    analysis.reports_exports_of(module)
        && !skif_js_syntax::is_declaration_file(&module.path.to_string_lossy().replace('\\', "/"))
}

/// The exports of `module` whose class members are judged: classes behind an export that another module uses and
/// that is not tagged to exempt it.
pub(crate) fn judged_classes<'m>(
    analysis: &JsAnalysis<'_>,
    module: &'m Module,
) -> impl Iterator<Item = (&'m Export, &'m ClassShape)> {
    module.info.exports.iter().filter_map(move |export| {
        let shape = export.class.as_ref()?;
        let judged = !analysis.is_tagged_public(export) && analysis.exports.is_used(module.id, &export.name);
        judged.then_some((export, shape))
    })
}
