//! The type-aware facts of one JS analysis (ADR 0034): the declarations whose member reads skif judges are
//! asked of the TypeScript checker of `skif-js-types`, and the answer is kept by module so the member rules can read
//! it without knowing the checker.
//!
//! Only a build with the `type-aware` feature compiles the checker. Without it every analysis is
//! [`TypeState::Off`] with [`OffReason::NotBuilt`], and the rules keep the name-based analysis.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::thread::Scope;

use skif_core::{FileSystem, HashMap, TypeAwarePackage, TypeAwareState, TypeAwareSummary};
use skif_js_syntax::Export;
use skif_workspace::{Package, PackageId, Workspace};

use crate::analysis::JsAnalysis;
use crate::discover::Candidate;
use crate::graph::{Module, ModuleId};
use crate::setup::Setup;

/// Why an analysis has no typed facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(
    not(feature = "type-aware"),
    allow(dead_code, reason = "the checker's failures only happen in a build with it")
)]
pub(crate) enum OffReason {
    /// No rule of the run reads typed facts, so the checker was not asked and the report says nothing about types.
    Unneeded,
    /// The configuration left type awareness off for every package (the default, `--no-type-aware`).
    Disabled,
    /// skif was built without the `type-aware` feature.
    #[cfg_attr(
        feature = "type-aware",
        expect(dead_code, reason = "only a build without the checker reports it")
    )]
    NotBuilt,
    /// The `tsconfig` given as `[js] tsconfig` cannot be read or parsed.
    TsconfigError,
    /// The checker failed in a way it does not recover from per program.
    CheckerFailed,
    /// The host cancelled the run.
    Cancelled,
}

impl OffReason {
    /// The reason as the report spells it (`TypeAwarePackage::reason`).
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Unneeded | Self::Disabled => "disabled",
            Self::NotBuilt => "not-built",
            Self::TsconfigError => "tsconfig-error",
            Self::CheckerFailed => "checker-failed",
            Self::Cancelled => "cancelled",
        }
    }
}

/// The typed facts of an analysis, or why it has none.
pub(crate) enum TypeState {
    /// The checker answered for the files it covered.
    #[cfg_attr(
        not(feature = "type-aware"),
        allow(dead_code, reason = "only a build with the checker types an analysis")
    )]
    Typed(Box<Typed>),
    /// No file is typed.
    Off(OffReason),
}

impl TypeState {
    fn typed(&self) -> Option<&Typed> {
        match self {
            Self::Typed(typed) => Some(typed.as_ref()),
            Self::Off(_) => None,
        }
    }

    /// Whether the checker covered `module`: its reads are typed, and a read by name in it credits nothing unless the
    /// checker could not settle it.
    pub(crate) fn covers(&self, module: ModuleId) -> bool {
        self.typed().is_some_and(|typed| typed.covers(module))
    }

    /// Whether the members of the class, enum or namespace behind `export` of `module` are judged with types: the
    /// declaration was asked about and the checker covered its file.
    pub(crate) fn judges(&self, module: &Module, export: &Export) -> bool {
        self.typed().is_some_and(|typed| typed.judges(module, export))
    }

    /// Whether a typed read reaches `member` of the declaration behind `export` of `module`, directly or through a
    /// base class or an implemented interface.
    pub(crate) fn reads(&self, module: &Module, export: &Export, member: &str) -> bool {
        self.typed().is_some_and(|typed| typed.reads(module, export, member))
    }

    /// Whether the declaration behind `export` of `module` is used as a whole, so every member counts as used.
    pub(crate) fn whole(&self, module: &Module, export: &Export) -> bool {
        self.typed().is_some_and(|typed| typed.whole(module, export))
    }

    /// The covered modules that read `name` through a receiver the checker could not type, sorted.
    pub(crate) fn unresolved_readers(&self, name: &str) -> &[ModuleId] {
        self.typed().map_or(&[], |typed| typed.unresolved_readers(name))
    }

    /// Why the members behind `export` of `module` are judged by name, or `None` when they are judged with types:
    /// the reason types are off, `file-untyped` when the checker did not cover the file, or `not-asked` for a
    /// declaration the checker was not asked about (one with no judged member).
    pub(crate) fn untyped_because(&self, module: &Module, export: &Export) -> Option<&'static str> {
        match self {
            Self::Off(reason) => Some(reason.label()),
            Self::Typed(typed) if typed.judges(module, export) => None,
            Self::Typed(typed) if !typed.covers(module.id) => Some("file-untyped"),
            Self::Typed(_) => Some("not-asked"),
        }
    }
}

/// The cores the type-aware analyses of a run over several roots share, so that their programs together hold no
/// more cores than the machine has, and the programs they share, so that a program the judged files of several roots
/// go to is built once for the run; empty in a build without the checker. Cloning it shares the same cores and
/// programs.
#[derive(Clone)]
pub(crate) struct SharedBudget {
    #[cfg(feature = "type-aware")]
    budget: skif_js_types::TypeBudget,
    #[cfg(feature = "type-aware")]
    programs: skif_js_types::TypePrograms,
}

impl SharedBudget {
    /// A budget of every core of the machine for the analyses of `roots`, the workspace roots of the run.
    #[cfg_attr(
        not(feature = "type-aware"),
        allow(unused_variables, reason = "only a build with the checker shares programs")
    )]
    pub(crate) fn for_roots(roots: &[std::path::PathBuf]) -> Self {
        Self {
            #[cfg(feature = "type-aware")]
            budget: skif_js_types::TypeBudget::new(None),
            #[cfg(feature = "type-aware")]
            programs: skif_js_types::TypePrograms::new(roots.iter().cloned()),
        }
    }

    /// Marks the analysis of `root` done without asking the checker, so that no program is kept for it.
    #[cfg_attr(
        not(feature = "type-aware"),
        allow(
            unused_variables,
            clippy::unused_self,
            reason = "only a build with the checker shares programs"
        )
    )]
    fn done(&self, root: &std::path::Path) {
        #[cfg(feature = "type-aware")]
        self.programs.done(root);
    }
}

/// What one analysis hands the checker: the file system to read through, the threads it may use, the cores it shares
/// with the other analyses of the run, if it shares any, and the host's cancellation flag; `wanted` is false when no
/// rule of the run reads typed facts.
#[derive(Clone, Copy)]
#[cfg_attr(
    not(feature = "type-aware"),
    allow(dead_code, reason = "only a build with the checker hands these on")
)]
pub(crate) struct TypeRun<'r> {
    pub fs: &'r Arc<dyn FileSystem>,
    pub sequential: bool,
    pub budget: Option<&'r SharedBudget>,
    pub cancel: Option<&'r AtomicBool>,
    pub wanted: bool,
}

/// The programs started before the module graph ([`start`]): `None` when the run asks no types, runs sequentially or
/// has no package that wants them, and always in a build without the checker.
pub(crate) struct EarlyTypes<'scope> {
    #[cfg(feature = "type-aware")]
    pending: Option<skif_js_types::PendingTypes<'scope>>,
    #[cfg(not(feature = "type-aware"))]
    _scope: std::marker::PhantomData<&'scope ()>,
}

impl EarlyTypes<'_> {
    const fn none() -> Self {
        Self {
            #[cfg(feature = "type-aware")]
            pending: None,
            #[cfg(not(feature = "type-aware"))]
            _scope: std::marker::PhantomData,
        }
    }
}

/// Starts building, on threads of `scope`, the programs of the discovered files `candidates` of the packages that want
/// types, so the programs build beside the module graph; [`run`] hands them the judged files and the query once the
/// graph is done. A sequential run builds them after the graph.
pub(crate) fn start<'scope, 'env>(
    scope: &'scope Scope<'scope, 'env>,
    workspace: &Workspace,
    setup: &Setup,
    candidates: &[Candidate],
    inputs: &TypeRun<'env>,
) -> EarlyTypes<'scope> {
    if !inputs.wanted || inputs.sequential {
        return EarlyTypes::none();
    }
    started(scope, workspace, setup, candidates, inputs)
}

#[cfg(not(feature = "type-aware"))]
fn started<'scope>(
    _scope: &'scope Scope<'scope, '_>,
    _workspace: &Workspace,
    _setup: &Setup,
    _candidates: &[Candidate],
    _inputs: &TypeRun<'_>,
) -> EarlyTypes<'scope> {
    EarlyTypes::none()
}

#[cfg(feature = "type-aware")]
fn started<'scope, 'env>(
    scope: &'scope Scope<'scope, 'env>,
    workspace: &Workspace,
    setup: &Setup,
    candidates: &[Candidate],
    inputs: &TypeRun<'env>,
) -> EarlyTypes<'scope> {
    use skif_js_types::{TypeBuild, TypePackage};

    let mut by_package: HashMap<PackageId, Vec<std::path::PathBuf>> = HashMap::default();
    for candidate in candidates {
        by_package
            .entry(candidate.owner)
            .or_default()
            .push(candidate.abs.clone());
    }
    let root = &workspace.root;
    let packages: Vec<TypePackage> = workspace
        .iter()
        .filter(|(id, _)| setup.package(*id).is_some() && setup.type_aware(*id))
        .filter_map(|(id, package)| {
            Some(TypePackage {
                dir: root.join(&package.dir),
                name: package.name.clone(),
                files: by_package.remove(&id)?,
            })
        })
        .collect();
    if packages.is_empty() {
        return EarlyTypes::none();
    }
    let mut build = TypeBuild::new(Arc::clone(inputs.fs), root.clone(), packages);
    if let Some(tsconfig) = &setup.root_tsconfig {
        build = build.with_tsconfig_override(tsconfig.clone());
    }
    if let Some(shared) = inputs.budget {
        build = build
            .with_budget(shared.budget.clone())
            .with_programs(shared.programs.clone());
    }
    EarlyTypes {
        pending: Some(skif_js_types::start(scope, build, inputs.cancel)),
    }
}

/// A package whose modules the analysis judges, with whether its configuration asks for types.
struct Analysed<'a> {
    package: &'a Package,
    modules: Vec<&'a Module>,
    type_aware: bool,
}

/// The packages that own a module of the graph (other projects' files excluded), in workspace order.
fn analysed<'a>(analysis: &'a JsAnalysis<'_>) -> Vec<Analysed<'a>> {
    let mut by_package: HashMap<PackageId, Vec<&Module>> = HashMap::default();
    for module in analysis.graph.modules().iter().filter(|module| !module.foreign) {
        by_package.entry(module.owner).or_default().push(module);
    }
    analysis
        .workspace
        .iter()
        .filter(|(id, _)| !analysis.is_skipped(*id))
        .filter_map(|(id, package)| {
            let modules = by_package.remove(&id)?;
            Some(Analysed {
                package,
                modules,
                type_aware: analysis.setup.type_aware(id),
            })
        })
        .collect()
}

/// The report entry of `analysed` when it stays untyped for `reason`.
fn untyped(analysed: &Analysed<'_>, reason: &str) -> TypeAwarePackage {
    TypeAwarePackage::new(
        &analysed.package.dir,
        analysed.package.display_name(),
        TypeAwareState::Untyped,
    )
    .with_reason(reason)
    .with_untyped_files(u32::try_from(analysed.modules.len()).unwrap_or(u32::MAX))
}

/// The summary of a run in which no package got types: those the configuration turned off are `disabled`, the
/// others untyped for `reason`.
fn summary_without_types(packages: &[Analysed<'_>], reason: OffReason) -> TypeAwareSummary {
    let enabled = packages.iter().any(|analysed| analysed.type_aware);
    TypeAwareSummary::new(
        enabled,
        packages
            .iter()
            .map(|analysed| {
                let label = if analysed.type_aware {
                    reason.label()
                } else {
                    OffReason::Disabled.label()
                };
                untyped(analysed, label)
            })
            .collect(),
    )
}

/// Asks the checker about `analysis`, on the programs `early` started when it started any, and returns the facts with
/// the run's coverage summary; the summary is `None` when no rule of the run reads typed facts.
pub(crate) fn run(
    analysis: &JsAnalysis<'_>,
    inputs: &TypeRun<'_>,
    early: EarlyTypes<'_>,
) -> (TypeState, Option<TypeAwareSummary>) {
    let root = &analysis.workspace.root;
    if !inputs.wanted {
        if let Some(shared) = inputs.budget {
            shared.done(root);
        }
        return (TypeState::Off(OffReason::Unneeded), None);
    }
    let packages = analysed(analysis);
    if !packages.iter().any(|analysed| analysed.type_aware) {
        if let Some(shared) = inputs.budget {
            shared.done(root);
        }
        return (
            TypeState::Off(OffReason::Disabled),
            Some(summary_without_types(&packages, OffReason::Disabled)),
        );
    }
    checked(analysis, inputs, &packages, early)
}

#[cfg(not(feature = "type-aware"))]
fn checked(
    _analysis: &JsAnalysis<'_>,
    _inputs: &TypeRun<'_>,
    packages: &[Analysed<'_>],
    _early: EarlyTypes<'_>,
) -> (TypeState, Option<TypeAwareSummary>) {
    (
        TypeState::Off(OffReason::NotBuilt),
        Some(summary_without_types(packages, OffReason::NotBuilt)),
    )
}

#[cfg(feature = "type-aware")]
fn checked(
    analysis: &JsAnalysis<'_>,
    inputs: &TypeRun<'_>,
    packages: &[Analysed<'_>],
    early: EarlyTypes<'_>,
) -> (TypeState, Option<TypeAwareSummary>) {
    use std::num::NonZeroUsize;

    use skif_js_types::{TypeError, TypeInput, TypePackage};

    let root = &analysis.workspace.root;
    let (query, decls) = skif_core::timing::phase(format_args!("js build query {}", root.display()), || {
        asked::build_query(analysis)
    });
    let typed_packages: Vec<TypePackage> = packages
        .iter()
        .filter(|analysed| analysed.type_aware)
        .map(|analysed| TypePackage {
            dir: root.join(&analysed.package.dir),
            name: analysed.package.name.clone(),
            files: analysed.modules.iter().map(|module| module.abs.clone()).collect(),
        })
        .collect();
    let mut input = TypeInput::new(Arc::clone(inputs.fs), root, &typed_packages, &query);
    if let Some(tsconfig) = analysis.setup.root_tsconfig.as_deref() {
        input = input.with_tsconfig_override(tsconfig);
    }
    if inputs.sequential {
        input = input.with_threads(NonZeroUsize::MIN);
    }
    if let Some(shared) = inputs.budget {
        input = input.with_budget(&shared.budget).with_programs(&shared.programs);
    }
    if let Some(cancel) = inputs.cancel {
        input = input.with_cancel(cancel);
    }
    let result = skif_core::timing::phase(format_args!("js types {}", root.display()), || match early.pending {
        Some(pending) => pending.finish(&typed_packages, &query),
        None => skif_js_types::analyze(&input),
    });
    let facts = match result {
        Ok(facts) => facts,
        Err(error) => {
            let reason = match error {
                TypeError::Config { .. } => OffReason::TsconfigError,
                TypeError::Cancelled => OffReason::Cancelled,
                _ => OffReason::CheckerFailed,
            };
            return (TypeState::Off(reason), Some(summary_without_types(packages, reason)));
        }
    };
    note_timings(root, &facts.timings, inputs.budget.map(|shared| &shared.programs));
    let summary = asked::summary(root, packages, &facts);
    let typed = Typed::new(analysis, decls, facts);
    (TypeState::Typed(Box::new(typed)), Some(summary))
}

/// The slowest programs `SKIF_TIMINGS` names.
#[cfg(feature = "type-aware")]
const SLOWEST_PROGRAMS: usize = 5;

/// The build split and project file counts of `timings`, as the line `SKIF_TIMINGS` prints.
#[cfg(feature = "type-aware")]
fn program_split(timings: &skif_js_types::TypeTimings) -> String {
    format!(
        "program build split: config parse {} ms, program creation {} ms, bind {} ms (summed); project files: {} \
         parsed across programs, {} distinct",
        timings.config_parse.as_millis(),
        timings.program_creation.as_millis(),
        timings.bind.as_millis(),
        timings.project_files_parsed,
        timings.project_files_distinct
    )
}

/// With timings on, the type-aware stages of the analysis at `root`: planning, the build and query times summed over
/// the programs beside the wall time and the time the analysis waited for them once the graph was done, how its
/// programs got the lib and `node_modules` declaration files they share with the run (and how the run's `programs`
/// got them so far), how its plannings and programs got their configs, and the slowest programs with their sizes and
/// checkers.
#[cfg(feature = "type-aware")]
fn note_timings(
    root: &std::path::Path,
    timings: &skif_js_types::TypeTimings,
    programs: Option<&skif_js_types::TypePrograms>,
) {
    if !skif_core::timing::enabled() {
        return;
    }
    skif_core::timing::note(format_args!(
        "js types {}: planning {} ms, programs {} ms, queries {} ms, waiting for cores {} ms (summed), wall {} ms, \
         waited {} ms, {} programs ({} started before the graph was done), {} threads",
        root.display(),
        timings.planning.as_millis(),
        timings.programs.as_millis(),
        timings.queries.as_millis(),
        timings.cores_waited.as_millis(),
        timings.wall.as_millis(),
        timings.waited.as_millis(),
        timings.per_program.len(),
        timings.early,
        timings.threads
    ));
    skif_core::timing::note(format_args!("js types {}: {}", root.display(), program_split(timings)));
    let files = timings.shared_files;
    skif_core::timing::note(format_args!(
        "js types {}: shared declaration files: {} parsed, {} reused, {} parsed unshared",
        root.display(),
        files.parsed,
        files.reused,
        files.unshared
    ));
    if let Some(run) = programs.map(skif_js_types::TypePrograms::shared_files) {
        skif_core::timing::note(format_args!(
            "js types: shared declaration files of the run so far: {} parsed, {} reused, {} parsed unshared",
            run.parsed, run.reused, run.unshared
        ));
    }
    let (configs, extended) = (timings.shared_configs, timings.shared_extended_configs);
    skif_core::timing::note(format_args!(
        "js types {}: shared configs: {} parsed, {} reused, {} parsed unshared; extended configs: {} parsed, {} \
         reused, {} parsed unshared",
        root.display(),
        configs.parsed,
        configs.reused,
        configs.unshared,
        extended.parsed,
        extended.reused,
        extended.unshared
    ));
    if let Some(peak) = skif_js_types::arena_peak_bytes() {
        skif_core::timing::note(format_args!(
            "js types {}: checker arenas {} MiB in use, {} MiB at most so far (process-wide)",
            root.display(),
            skif_js_types::arena_bytes_in_use() >> 20,
            peak >> 20
        ));
    }
    for program in timings.slowest(SLOWEST_PROGRAMS) {
        skif_core::timing::note(format_args!(
            "js types {}: program {}: {} judged of {} files, {} checkers, {}, queries {} ms",
            root.display(),
            program.config.display(),
            program.judged_files,
            program.files,
            program.checkers,
            if program.shared {
                "built by another root".to_owned()
            } else {
                format!("build {} ms", program.build.as_millis())
            },
            program.query.as_millis()
        ));
    }
}

#[cfg(feature = "type-aware")]
pub(crate) use asked::Typed;

/// The query skif asks and the facts it keeps, compiled only with the checker.
#[cfg(feature = "type-aware")]
mod asked {
    use std::path::Path;

    use skif_core::{HashMap, HashSet, TypeAwarePackage, TypeAwareState, TypeAwareSummary};
    use skif_js_syntax::{Export, ExportKind};
    use skif_js_types::{CoverageState, DeclId, DeclKind, DeclQuery, TypeFacts, TypeQuery};

    use super::{Analysed, untyped};
    use crate::analysis::JsAnalysis;
    use crate::class_members::{ClassFacts, entry_exposed, exemption, judged_classes, judges_classes_of};
    use crate::graph::{Module, ModuleId};

    /// A declaration by the module that declares it and the offset of its name.
    pub(super) type DeclKeys = HashMap<(ModuleId, u32), DeclId>;

    /// Adds the declaration at `name_start` of `module` with `members` to `query`, merging the members of a
    /// declaration two exports stand for (`export { E, E as F }` of one enum). The declaration is asked about even
    /// without members: the checker still finds its uses as a whole.
    fn add(
        query: &mut TypeQuery,
        keys: &mut DeclKeys,
        (module, name_start, kind): (&Module, u32, DeclKind),
        members: Vec<String>,
    ) {
        let next = DeclId(u32::try_from(query.decls.len()).unwrap_or(u32::MAX));
        let id = *keys.entry((module.id, name_start)).or_insert(next);
        if id == next {
            query.decls.push(DeclQuery {
                file: module.abs.clone(),
                name_start,
                kind,
                members: Vec::new(),
            });
        }
        let Some(decl) = usize::try_from(id.0).ok().and_then(|index| query.decls.get_mut(index)) else {
            return;
        };
        for member in members {
            if !decl.members.contains(&member) {
                decl.members.push(member);
            }
        }
    }

    /// The declarations whose members the rules judge: every used exported class with the members it does not
    /// exempt (ADR 0018), and every used enum and namespace export with its untagged members that are not
    /// used without types. Private names (`#count`) are left to their own module, as without types.
    pub(super) fn build_query(analysis: &JsAnalysis<'_>) -> (TypeQuery, DeclKeys) {
        let mut query = TypeQuery::default();
        let mut keys = DeclKeys::default();
        let exposed = entry_exposed(analysis);
        for module in analysis.graph.modules() {
            if module.info.exports.iter().any(|export| export.class.is_some()) && judges_classes_of(analysis, module) {
                for (export, shape) in judged_classes(analysis, module) {
                    let Some(name_start) = export.declaration_start else {
                        continue;
                    };
                    let facts = ClassFacts::of(analysis, module, export, shape, &exposed);
                    let members: Vec<String> = shape
                        .members
                        .iter()
                        .filter(|member| !member.name.starts_with('#'))
                        .filter(|member| exemption(analysis, module, shape, &facts, member).is_none())
                        .map(|member| member.name.clone())
                        .collect();
                    if !members.is_empty() {
                        add(&mut query, &mut keys, (module, name_start, DeclKind::Class), members);
                    }
                }
            }
            if !analysis.reports_exports_of(module) {
                continue;
            }
            for export in &module.info.exports {
                let kind = match export.kind {
                    ExportKind::Enum => DeclKind::Enum,
                    ExportKind::Namespace => DeclKind::Namespace,
                    _ => continue,
                };
                let Some(name_start) = export.declaration_start else {
                    continue;
                };
                if analysis.is_tagged_public(export) || !analysis.exports.is_used(module.id, &export.name) {
                    continue;
                }
                let mut judged = JsAnalysis::members_of(export)
                    .iter()
                    .filter(|member| !analysis.exempts_tags(&member.tags))
                    .peekable();
                if judged.peek().is_none() {
                    continue;
                }
                // Types only add uses (ADR 0034): a member used without types is used whatever the checker finds, so
                // only the others are asked about, which spares the checker the reads of their names.
                let members = judged
                    .filter(|member| !analysis.member_used_without_types(module, export, member))
                    .map(|member| member.name.clone())
                    .collect();
                add(&mut query, &mut keys, (module, name_start, kind), members);
            }
        }
        (query, keys)
    }

    /// The facts of the checker by module.
    pub(crate) struct Typed {
        decls: DeclKeys,
        facts: TypeFacts,
        covered: HashSet<ModuleId>,
        unresolved: HashMap<String, Vec<ModuleId>>,
    }

    impl Typed {
        pub(super) fn new(analysis: &JsAnalysis<'_>, decls: DeclKeys, facts: TypeFacts) -> Self {
            let graph = &analysis.graph;
            let covered = facts.covered.iter().filter_map(|path| graph.id_of(path)).collect();
            let mut unresolved: HashMap<String, Vec<ModuleId>> = HashMap::default();
            for (path, names) in &facts.unresolved_names {
                let Some(id) = graph.id_of(path) else {
                    continue;
                };
                for name in names {
                    unresolved.entry(name.clone()).or_default().push(id);
                }
            }
            for readers in unresolved.values_mut() {
                readers.sort_by_key(|id| id.index());
                readers.dedup();
            }
            Self {
                decls,
                facts,
                covered,
                unresolved,
            }
        }

        fn decl(&self, module: &Module, export: &Export) -> Option<DeclId> {
            let name_start = export.declaration_start?;
            self.decls.get(&(module.id, name_start)).copied()
        }

        pub(super) fn covers(&self, module: ModuleId) -> bool {
            self.covered.contains(&module)
        }

        pub(super) fn judges(&self, module: &Module, export: &Export) -> bool {
            self.covers(module.id) && self.decl(module, export).is_some()
        }

        pub(super) fn reads(&self, module: &Module, export: &Export, member: &str) -> bool {
            self.decl(module, export)
                .and_then(|id| self.facts.reads.get(&id))
                .is_some_and(|names| names.contains(member))
        }

        pub(super) fn whole(&self, module: &Module, export: &Export) -> bool {
            self.decl(module, export)
                .is_some_and(|id| self.facts.whole.contains(&id))
        }

        pub(super) fn unresolved_readers(&self, name: &str) -> &[ModuleId] {
            self.unresolved.get(name).map_or(&[], Vec::as_slice)
        }
    }

    const fn state(state: CoverageState) -> TypeAwareState {
        match state {
            CoverageState::Typed => TypeAwareState::Typed,
            CoverageState::Partial => TypeAwareState::Partial,
            _ => TypeAwareState::Untyped,
        }
    }

    /// The coverage of every analysed package: the checker's for those it was asked about, `disabled` for those the
    /// configuration turned off.
    pub(super) fn summary(root: &Path, packages: &[Analysed<'_>], facts: &TypeFacts) -> TypeAwareSummary {
        let entries = packages
            .iter()
            .map(|analysed| {
                if !analysed.type_aware {
                    return untyped(analysed, "disabled");
                }
                let dir = root.join(&analysed.package.dir);
                let Some(coverage) = facts.coverage.iter().find(|coverage| coverage.dir == dir) else {
                    let outside = analysed.modules.len();
                    return untyped(analysed, &format!("files-outside-programs: {outside}"));
                };
                let entry = TypeAwarePackage::new(
                    &analysed.package.dir,
                    analysed.package.display_name(),
                    state(coverage.state),
                )
                .with_untyped_files(coverage.untyped_files);
                match &coverage.reason {
                    Some(reason) => entry.with_reason(reason.label()),
                    None => entry,
                }
            })
            .collect();
        TypeAwareSummary::new(true, entries).with_engine(facts.engine)
    }
}

/// The facts of the checker; uninhabited in a build without it, so no analysis is ever typed.
#[cfg(not(feature = "type-aware"))]
#[allow(dead_code, reason = "only a build with the checker types an analysis")]
pub(crate) struct Typed {
    never: std::convert::Infallible,
}

#[cfg(not(feature = "type-aware"))]
impl Typed {
    fn covers(&self, _module: ModuleId) -> bool {
        match self.never {}
    }

    fn judges(&self, _module: &Module, _export: &Export) -> bool {
        match self.never {}
    }

    fn reads(&self, _module: &Module, _export: &Export, _member: &str) -> bool {
        match self.never {}
    }

    fn whole(&self, _module: &Module, _export: &Export) -> bool {
        match self.never {}
    }

    fn unresolved_readers(&self, _name: &str) -> &[ModuleId] {
        match self.never {}
    }
}

#[cfg(all(test, feature = "type-aware"))]
mod timing_tests {
    use std::time::Duration;

    #[test]
    #[allow(
        clippy::field_reassign_with_default,
        reason = "TypeTimings is non-exhaustive, so a struct literal is not available here"
    )]
    fn the_program_split_line_names_each_part_and_the_duplicate_parsing() {
        let mut timings = skif_js_types::TypeTimings::default();
        timings.config_parse = Duration::from_millis(3);
        timings.program_creation = Duration::from_millis(40);
        timings.bind = Duration::from_millis(7);
        timings.project_files_parsed = 90;
        timings.project_files_distinct = 30;
        assert_eq!(
            super::program_split(&timings),
            "program build split: config parse 3 ms, program creation 40 ms, bind 7 ms (summed); project files: 90 \
             parsed across programs, 30 distinct"
        );
    }
}
