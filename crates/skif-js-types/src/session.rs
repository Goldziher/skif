//! One analysis: plan the programs, build a few at a time (the largest first, each with checkers in proportion to its
//! share of the judged files), walk their judged files on their checkers, copy the facts out and free each program as
//! soon as its walk ends (ADR 0034).
//!
//! The programs can start before the host knows its query ([`start`]): they are planned from the files that may be
//! judged, and the first ones build while the host works the query out. Each waits, built, until
//! [`PendingTypes::finish`] plans the judged files, then walks them or, when no judged file needs it, is freed
//! unwalked. Which programs start early depends on timing, so nothing that shapes a fact does: a program gets the
//! checkers the early plan gives it whether it starts early or not, and the walks read the judged files alone.
//!
//! The analyses of a run over many roots can share their cores ([`TypeBudget`]), so that their programs together hold
//! no more cores than there are, and their programs ([`TypePrograms`](crate::TypePrograms)), so that a program the
//! judged files of several roots go to is built once for the run. Every program reads the bundled lib files and the
//! declaration files below `node_modules` from the files the run shares (`shared_files`), the run's or, without shared
//! programs, the analysis's: each is parsed and bound once instead of once per program. The plannings and the programs
//! read their configs, those the project references of a program reach and those they extend, from the configs the
//! run shares (`shared_configs`) in the same way: each is read and parsed once instead of once per planning and program.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroUsize;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::{Scope, ScopedJoinHandle};
use std::time::{Duration, Instant};

use tsrs_compiler::{ProgramOptions, ProgramRegions};
use tsrs_core::arena::Region;
use tsrs_core::{CompilerOptions, P, Tristate, tspath};
use tsrs_vfs::FS;

use crate::coverage::{self, FileState};
use crate::members::{self, Candidates, Found};
use crate::owner::ProgramOwner;
use crate::programs::{self, ConfigHost, Lookups, Placement, PlannedProgram, ProgramPlan};
use crate::query::{self, FileFacts, QueryContext};
use crate::resident::{Claim, ProgramKey, Resident, Ticket};
use crate::shared_configs::ConfigReader;
use crate::shared_files::{self, SharedFiles, SharingHost};
use crate::vfs::{self, BomFiles};
use crate::{ENGINE, ProgramTiming, TypeBuild, TypeError, TypeFacts, TypeInput, TypePackage, TypeQuery, TypeTimings};

/// The stack of the threads that build and check programs and of the types pool: checking recurses deeply, and
/// tsrs-lint gives its program threads the same.
const PROGRAM_STACK_SIZE: usize = 512 << 20;
/// The first chunk of the scratch region configs and program bookkeeping are allocated in.
const SCRATCH_CHUNK: usize = 1 << 20;
/// Cores per program built at once: each program checks on a pool of its own, so a quarter of the cores at a time
/// fills the machine without holding every program in memory (tsrs-lint's measurement).
const CORES_PER_PROGRAM: usize = 4;
/// The most checkers a program gets while other programs are built beside it. Each checker holds its own type
/// caches: on the largest repository of the corpus, 16 checkers on its main program raised peak memory past the
/// budget of ADR 0029 without a shorter run than 8.
const MOST_SHARED_CHECKERS: usize = 8;

/// Runs the analysis of `input`.
pub(crate) fn analyze(input: &TypeInput<'_>) -> Result<TypeFacts, TypeError> {
    if input.cancel.is_some_and(|cancel| cancel.load(Ordering::Relaxed)) {
        return Err(TypeError::Cancelled);
    }
    let build = TypeBuild {
        fs: Arc::clone(&input.fs),
        root: input.root.to_path_buf(),
        packages: input.packages.to_vec(),
        tsconfig_override: input.tsconfig_override.map(std::path::Path::to_path_buf),
        threads: input.threads,
        budget: input.budget.cloned(),
        programs: input.programs.cloned(),
    };
    std::thread::scope(|scope| start(scope, build, input.cancel).finish(input.packages, input.query))
}

/// Starts building the programs of `build` on threads of `scope`.
pub(crate) fn start<'scope, 'env>(
    scope: &'scope Scope<'scope, 'env>,
    build: TypeBuild,
    cancel: Option<&'env AtomicBool>,
) -> PendingTypes<'scope> {
    let (fs, boms) = vfs::tsrs_fs(Arc::clone(&build.fs));
    let cores = build.budget.as_ref().map_or_else(
        || Arc::new(Cores::new(cores_of(build.threads))),
        |budget| Arc::clone(&budget.cores),
    );
    let files = build
        .programs
        .as_ref()
        .map_or_else(Arc::default, crate::TypePrograms::files);
    let cwd = vfs::tsrs_path(&build.root);
    let configs = Arc::new(ConfigReader::new(
        build
            .programs
            .as_ref()
            .map_or_else(Arc::default, crate::TypePrograms::configs),
        Arc::default(),
        ConfigHost::new(Arc::clone(&fs), cwd.clone()),
    ));
    let shared = Arc::new(Shared {
        cwd,
        fs,
        boms,
        files,
        configs,
        file_counts: Arc::default(),
        pool: types_pool(build.threads),
        cores,
        build,
        planned: Slot::default(),
        judged: Slot::default(),
    });
    let spawned = {
        let shared = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("skif-types-plan".to_owned())
            .spawn_scoped(scope, move || {
                panic::catch_unwind(AssertUnwindSafe(|| Ok(coordinate(&shared, cancel)))).unwrap_or_else(|payload| {
                    // The host waits for the early plan; a panic before it was put must not leave it waiting.
                    shared
                        .planned
                        .put(Err(internal("planning the programs", payload.as_ref())));
                    Err(internal("building the programs", payload.as_ref()))
                })
            })
    };
    let (coordinator, spawn_error) = match spawned {
        Ok(handle) => (Some(handle), None),
        Err(error) => (None, Some(error.to_string())),
    };
    PendingTypes {
        shared,
        coordinator,
        spawn_error,
        cancel,
    }
}

/// Programs that are being built before the query is known ([`crate::start`]). [`PendingTypes::finish`] hands the
/// judged files and the query over and returns the facts; dropping it gives the analysis up and frees every program
/// built so far.
#[must_use = "dropping it gives the analysis up"]
pub struct PendingTypes<'scope> {
    shared: Arc<Shared>,
    coordinator: Option<ScopedJoinHandle<'scope, Result<Coordinated, TypeError>>>,
    spawn_error: Option<String>,
    cancel: Option<&'scope AtomicBool>,
}

impl std::fmt::Debug for PendingTypes<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PendingTypes")
            .field("root", &self.shared.build.root)
            .finish_non_exhaustive()
    }
}

impl PendingTypes<'_> {
    /// Plans the judged files of `packages`, walks them on the programs (those built already and the rest) for
    /// `query`, frees every program and returns the facts with the coverage of every package. The file system, root,
    /// `tsconfig` override and threads are the build's.
    ///
    /// # Errors
    /// As [`crate::analyze`].
    pub fn finish(mut self, packages: &[TypePackage], query: &TypeQuery) -> Result<TypeFacts, TypeError> {
        let asked = Instant::now();
        let shared = Arc::clone(&self.shared);
        let build = &shared.build;
        let input = TypeInput {
            fs: Arc::clone(&build.fs),
            root: &build.root,
            packages,
            tsconfig_override: build.tsconfig_override.as_deref(),
            query,
            threads: build.threads,
            budget: build.budget.as_ref(),
            programs: build.programs.as_ref(),
            cancel: self.cancel,
        };
        let handed = self.hand_over(&input);
        // Wakes the program threads when nothing was handed over, so that they free their programs and end.
        self.shared.judged.put(None);
        let coordinated = self.join();
        let Handed {
            judged,
            planning,
            region,
        } = handed?;
        let coordinated = coordinated?;
        let wall = coordinated.wall;
        let outcomes = into_outcomes(&judged, coordinated)?;
        let mut facts = assemble(&input, &judged.plan, &judged.candidates, outcomes)?;
        facts.timings.planning = planning;
        facts.timings.wall = wall;
        facts.timings.threads = judged.threads;
        facts.timings.waited = asked.elapsed();
        facts.timings.shared_files = shared.file_counts.snapshot();
        (facts.timings.shared_configs, facts.timings.shared_extended_configs) = shared.configs.counts();
        drop(region);
        Ok(facts)
    }

    /// Plans the judged files of `input` and hands the plan to the program threads.
    fn hand_over(&self, input: &TypeInput<'_>) -> Result<Handed, TypeError> {
        if let Some(error) = &self.spawn_error {
            return Err(TypeError::Internal {
                message: format!("cannot start a type-checker thread: {error}"),
            });
        }
        let planned = self.shared.planned.take()?;
        if self.cancel.is_some_and(|cancel| cancel.load(Ordering::Relaxed)) {
            return Err(TypeError::Cancelled);
        }
        // What this planning allocates is freed when the analysis ends.
        let region = Region::new_scratch(SCRATCH_CHUNK);
        let start = Instant::now();
        let (plan, _) = panic::catch_unwind(AssertUnwindSafe(|| {
            self.shared.on_pool(|| {
                let _scope = region.enter();
                programs::plan_programs(input, &self.shared.configs, &planned.lookups)
            })
        }))
        .unwrap_or_else(|payload| Err(internal("planning the programs", payload.as_ref())))?;
        let judged = Arc::new(Judged::new(
            self.shared.build.threads,
            &planned.early,
            plan,
            Candidates::new(input.query),
        ));
        self.shared.judged.put(Some(Arc::clone(&judged)));
        Ok(Handed {
            judged,
            planning: planned.planning.saturating_add(start.elapsed()),
            region,
        })
    }

    fn join(&mut self) -> Result<Coordinated, TypeError> {
        let Some(handle) = self.coordinator.take() else {
            return Ok(Coordinated::default());
        };
        handle
            .join()
            .unwrap_or_else(|payload| Err(internal("building the programs", payload.as_ref())))
    }
}

impl Drop for PendingTypes<'_> {
    fn drop(&mut self) {
        self.shared.judged.put(None);
        // The analysis asks no more: a program kept only for its root goes.
        if let Some(programs) = &self.shared.build.programs {
            programs.done(&self.shared.build.root);
        }
    }
}

/// What the host, the planning thread and the program threads share.
struct Shared {
    build: TypeBuild,
    fs: Arc<dyn FS>,
    boms: Arc<BomFiles>,
    /// The lib and `node_modules` declaration files the programs read: the run's, with shared programs, else the
    /// analysis's own.
    files: Arc<SharedFiles>,
    /// How the programs this analysis builds got those files.
    file_counts: Arc<shared_files::Counts>,
    /// The configs the plannings and programs read: the run's, with shared programs, else the analysis's own.
    configs: Arc<ConfigReader>,
    /// The workspace root as tsrs names files, the current directory configs are parsed in.
    cwd: String,
    /// The early plan, put by the planning thread.
    planned: Slot<Result<Planned, TypeError>>,
    /// The plan of the judged files, put by the host; `None` once the host gave the analysis up.
    judged: Slot<Option<Arc<Judged>>>,
    /// The threads tsrs parallelises on. tsrs runs its parallel work on the rayon pool of the calling thread; on the
    /// global pool, a host that runs many analyses at once on that pool's threads (one per root) blocks them all
    /// waiting for plans whose parallel work then finds no thread, and a waiting tsrs call there takes other jobs of
    /// the host that block. `None` when the pool cannot be started: tsrs then
    /// uses the global pool. Shared with the programs built on it, which are freed on it ([`ProgramOwner`]).
    pool: Option<Arc<rayon::ThreadPool>>,
    /// The cores the programs check on: the analysis's own, or those of the run's [`TypeBudget`].
    cores: Arc<Cores>,
}

impl Shared {
    /// Runs `work` on the analysis's own threads, so that the parallel work tsrs does in it stays there.
    fn on_pool<R: Send>(&self, work: impl FnOnce() -> R + Send) -> R {
        match &self.pool {
            Some(pool) => pool.install(work),
            None => work(),
        }
    }
}

/// The rayon pool of one analysis, a thread per core. Only tsrs's own work runs on it, never a wait for a plan or for
/// cores: a pool thread that waits inside tsrs takes other queued jobs, so a job that blocks would block it too.
fn types_pool(threads: Option<NonZeroUsize>) -> Option<Arc<rayon::ThreadPool>> {
    rayon::ThreadPoolBuilder::new()
        .num_threads(cores_of(threads))
        .stack_size(PROGRAM_STACK_SIZE)
        .thread_name(|index| format!("skif-types-{index}"))
        .build()
        .ok()
        .map(Arc::new)
}

/// `threads`, or every core of the machine.
fn cores_of(threads: Option<NonZeroUsize>) -> usize {
    threads
        .or_else(|| std::thread::available_parallelism().ok())
        .map_or(1, NonZeroUsize::get)
}

/// A value one thread puts once and others wait for.
struct Slot<T> {
    value: Mutex<Option<T>>,
    put: Condvar,
}

impl<T> Default for Slot<T> {
    fn default() -> Self {
        Self {
            value: Mutex::new(None),
            put: Condvar::new(),
        }
    }
}

impl<T> Slot<T> {
    /// Puts `value` unless the slot holds one, and wakes the threads that wait for it.
    fn put(&self, value: T) {
        let mut slot = lock(&self.value);
        if slot.is_none() {
            *slot = Some(value);
        }
        drop(slot);
        self.put.notify_all();
    }

    /// Waits for the value and takes it.
    fn take(&self) -> T {
        let mut slot = lock(&self.value);
        loop {
            if let Some(value) = slot.take() {
                return value;
            }
            slot = self.put.wait(slot).unwrap_or_else(PoisonError::into_inner);
        }
    }
}

impl<T: Clone> Slot<T> {
    /// The value, if it was put.
    fn get(&self) -> Option<T> {
        lock(&self.value).clone()
    }

    /// Waits for the value.
    fn wait(&self) -> T {
        let mut slot = lock(&self.value);
        loop {
            if let Some(value) = &*slot {
                return value.clone();
            }
            slot = self.put.wait(slot).unwrap_or_else(PoisonError::into_inner);
        }
    }
}

/// What the early planning hands the host.
struct Planned {
    /// Where every early file went, so the planning of the judged files searches only the files it lacks.
    lookups: Lookups,
    early: Early,
    planning: Duration,
}

/// The programs of the early plan in the order they start, with the checkers of each.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Early {
    configs: Vec<String>,
    checkers: Vec<usize>,
}

impl Early {
    fn new(plan: &ProgramPlan, schedule: &Schedule) -> Self {
        Self {
            configs: schedule
                .order
                .iter()
                .map(|&index| plan.programs[index].config.clone())
                .collect(),
            checkers: schedule.order.iter().map(|&index| schedule.checkers[index]).collect(),
        }
    }
}

/// The plan of the judged files with what its walks need.
struct Judged {
    plan: ProgramPlan,
    candidates: Candidates,
    schedule: Schedule,
    /// The configs of the programs in [`Schedule::order`].
    order: Vec<String>,
    /// The index of each program in [`ProgramPlan::programs`], by config.
    by_config: BTreeMap<String, usize>,
    /// The programs built at once.
    builders: usize,
    /// The threads the programs share.
    threads: usize,
}

impl Judged {
    fn new(threads: Option<NonZeroUsize>, early: &Early, plan: ProgramPlan, candidates: Candidates) -> Self {
        let parallelism = Parallelism::new(threads, plan.programs.len());
        let schedule = judged_schedule(&parallelism, early, &plan.programs);
        let order = schedule
            .order
            .iter()
            .map(|&index| plan.programs[index].config.clone())
            .collect();
        let by_config = plan
            .programs
            .iter()
            .enumerate()
            .map(|(index, program)| (program.config.clone(), index))
            .collect();
        Self {
            builders: parallelism.programs,
            threads: parallelism.threads,
            plan,
            candidates,
            schedule,
            order,
            by_config,
        }
    }
}

/// The schedule of the judged files' `programs`: those of the early plan first, in its order and with its checkers,
/// since they may be built with them already; then the others, the largest first, with the checkers of `parallelism`.
fn judged_schedule(parallelism: &Parallelism, early: &Early, programs: &[PlannedProgram]) -> Schedule {
    let sizes: Vec<usize> = programs.iter().map(|program| program.files.len()).collect();
    let own = Schedule::new(parallelism, &sizes);
    let by_config: BTreeMap<&str, usize> = programs
        .iter()
        .enumerate()
        .map(|(index, program)| (program.config.as_str(), index))
        .collect();
    let early_checkers: BTreeMap<&str, usize> = early
        .configs
        .iter()
        .map(String::as_str)
        .zip(early.checkers.iter().copied())
        .collect();
    let mut order: Vec<usize> = early
        .configs
        .iter()
        .filter_map(|config| by_config.get(config.as_str()).copied())
        .collect();
    let first: BTreeSet<usize> = order.iter().copied().collect();
    order.extend(own.order.iter().copied().filter(|index| !first.contains(index)));
    let checkers = programs
        .iter()
        .zip(own.checkers)
        .map(|(program, checkers)| early_checkers.get(program.config.as_str()).copied().unwrap_or(checkers))
        .collect();
    Schedule { order, checkers }
}

/// What the host takes from handing its plan over.
struct Handed {
    judged: Arc<Judged>,
    planning: Duration,
    /// The region of the judged files' planning, freed when the analysis ends.
    region: Region,
}

/// What the planning thread returns once every program thread has ended.
#[derive(Default)]
struct Coordinated {
    /// The outcome of each program of the judged files' plan, by its index there.
    outcomes: BTreeMap<usize, Outcome>,
    wall: Duration,
    spawn_error: Option<String>,
}

/// Plans the early files of `shared`, puts the plan for the host and builds and walks the programs until the host's
/// plan is done or given up.
fn coordinate(shared: &Shared, cancel: Option<&AtomicBool>) -> Coordinated {
    // What planning allocates is freed when the programs are; the configs it reads stay with the shared configs.
    let region = Region::new_scratch(SCRATCH_CHUNK);
    let start = Instant::now();
    let query = TypeQuery::default();
    let input = shared.build.input(&query);
    let planned = {
        panic::catch_unwind(AssertUnwindSafe(|| {
            shared.on_pool(|| {
                let _scope = region.enter();
                programs::plan_programs(&input, &shared.configs, &Lookups::new())
            })
        }))
        .unwrap_or_else(|payload| Err(internal("planning the programs", payload.as_ref())))
    };
    let (plan, lookups) = match planned {
        Ok(planned) => planned,
        Err(error) => {
            shared.planned.put(Err(error));
            return Coordinated::default();
        }
    };
    let parallelism = Parallelism::new(shared.build.threads, plan.programs.len());
    let sizes: Vec<usize> = plan.programs.iter().map(|program| program.files.len()).collect();
    let early = Early::new(&plan, &Schedule::new(&parallelism, &sizes));
    drop(plan);
    shared.planned.put(Ok(Planned {
        lookups,
        early: early.clone(),
        planning: start.elapsed(),
    }));
    let session = Session {
        shared,
        early: &early,
        cancel,
        single_threaded: parallelism.single_threaded,
        queue: Mutex::default(),
        outcomes: Mutex::default(),
        regions: Mutex::default(),
    };
    let run_start = Instant::now();
    let spawn_error = session.run(parallelism.programs);
    let wall = run_start.elapsed();
    let Session { outcomes, regions, .. } = session;
    drop(regions);
    drop(region);
    Coordinated {
        outcomes: outcomes.into_inner().unwrap_or_else(PoisonError::into_inner),
        wall,
        spawn_error: spawn_error.map(|error| error.to_string()),
    }
}

/// The outcome of every program of `judged`, once the threads are done.
fn into_outcomes(judged: &Judged, coordinated: Coordinated) -> Result<Vec<Outcome>, TypeError> {
    let Coordinated {
        mut outcomes,
        spawn_error,
        ..
    } = coordinated;
    let programs = judged.plan.programs.len();
    if let Some(error) = spawn_error
        && (0..programs).any(|index| !outcomes.contains_key(&index))
    {
        return Err(TypeError::Internal {
            message: format!("cannot start a type-checker thread: {error}"),
        });
    }
    // A panic outside a program's work leaves its outcome out.
    let outcomes: Vec<Outcome> = (0..programs)
        .map(|index| outcomes.remove(&index).unwrap_or(Outcome::Failed))
        .collect();
    if outcomes.iter().any(|outcome| matches!(outcome, Outcome::Cancelled)) {
        return Err(TypeError::Cancelled);
    }
    Ok(outcomes)
}

/// How many programs are built at once and how many checkers a program may get.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Parallelism {
    /// The threads the programs share.
    threads: usize,
    /// The programs built at once.
    programs: usize,
    /// The checkers of a program when the programs built at once share the threads evenly, and the fewest a program
    /// gets.
    even: usize,
    /// The most checkers a program gets: tsrs's own count for the machine, within the threads and
    /// [`MOST_SHARED_CHECKERS`], unless the even share is larger.
    most: usize,
    single_threaded: bool,
}

impl Parallelism {
    fn new(threads: Option<NonZeroUsize>, programs: usize) -> Self {
        let threads = cores_of(threads);
        let at_once = (threads / CORES_PER_PROGRAM).clamp(1, programs.max(1));
        let even = (threads / at_once).max(1);
        let machine =
            tsrs_compiler::checkerpool::default_checkers_for_parallelism(i64::try_from(threads).unwrap_or(i64::MAX));
        let machine = usize::try_from(machine).unwrap_or(threads);
        Self {
            threads,
            programs: at_once,
            even,
            most: machine.min(threads).min(MOST_SHARED_CHECKERS).max(even),
            single_threaded: threads == 1,
        }
    }

    /// The checkers of a program that holds `files` of the `total` judged files in programs: its share of the
    /// threads, at least the even share and at most [`Parallelism::most`]. A program with most of the judged files
    /// would otherwise check on a quarter of the cores while the rest wait for it.
    fn checkers(&self, files: usize, total: usize) -> usize {
        let share = if total == 0 {
            0
        } else {
            self.threads.saturating_mul(files).div_ceil(total)
        };
        share.clamp(self.even, self.most)
    }
}

/// The order the programs start in and the checkers of each.
#[derive(Debug, PartialEq, Eq)]
struct Schedule {
    /// Indices into [`ProgramPlan::programs`], the programs with the most judged files first: the longest program then
    /// does not start last and hold up the end of the analysis. Programs of one size keep their plan order.
    order: Vec<usize>,
    /// The checkers of each program of the plan.
    checkers: Vec<usize>,
}

impl Schedule {
    /// The schedule of programs that hold `sizes` judged files each. A program gets no more checkers than it has
    /// files to walk: tsrs creates every checker of a program before the walk, each with the program's global types,
    /// and one that owns no walked file only holds memory.
    fn new(parallelism: &Parallelism, sizes: &[usize]) -> Self {
        let total = sizes.iter().sum();
        let mut order: Vec<usize> = (0..sizes.len()).collect();
        order.sort_by_key(|&index| std::cmp::Reverse(sizes[index]));
        Self {
            order,
            checkers: sizes
                .iter()
                .map(|&files| parallelism.checkers(files, total).min(files.max(1)))
                .collect(),
        }
    }
}

/// The cores of a run that several analyses share: each analysis takes its programs' checkers from them instead of
/// cores of its own, so that the programs of a run over many roots together hold no more cores than the budget has,
/// which also bounds how many programs are in memory at once. Give the same budget (or a clone, which shares it) to
/// every analysis of the run with [`TypeBuild::with_budget`](crate::TypeBuild::with_budget) or
/// [`TypeInput::with_budget`](crate::TypeInput::with_budget). The facts do not depend on it.
#[derive(Clone)]
pub struct TypeBudget {
    cores: Arc<Cores>,
}

impl TypeBudget {
    /// A budget of `threads` cores; `None` for every core of the machine.
    #[must_use]
    pub fn new(threads: Option<NonZeroUsize>) -> Self {
        Self {
            cores: Arc::new(Cores::shared(cores_of(threads))),
        }
    }

    /// The cores of the budget.
    #[must_use]
    pub fn threads(&self) -> usize {
        self.cores.threads
    }

    /// The most cores the analyses held at once, for tests that check the budget holds.
    #[doc(hidden)]
    #[must_use]
    pub fn peak(&self) -> usize {
        self.cores.peak()
    }

    /// How many programs took cores from the budget, for tests that check every program does.
    #[doc(hidden)]
    #[must_use]
    pub fn takes(&self) -> usize {
        self.cores.takes()
    }
}

impl std::fmt::Debug for TypeBudget {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TypeBudget")
            .field("threads", &self.cores.threads)
            .finish_non_exhaustive()
    }
}

/// The cores not handed to a program's checkers yet. A program starts once its checkers fit, so the programs that
/// run at once never hold more checkers than there are cores, and a program that takes most of them runs with few
/// others beside it, which bounds the memory they hold together.
///
/// The cores of one analysis ([`Cores::new`]) are only its own programs'. Those a run's analyses share
/// ([`Cores::shared`]) must never be waited for by a program that waits for its host: a program built before its
/// analysis's judged files are planned waits for its host to plan them, and the host's thread can be running
/// another analysis meanwhile (a rayon thread that waits runs other jobs), whose host would then wait for cores held
/// by a program waiting for that thread. So a program waits for shared cores only once its judged files are planned,
/// when it no longer waits for its host; an early program takes cores only of a share kept for early programs, only
/// when they are free ([`Cores::admit_early`]), and nothing ever waits for those.
struct Cores {
    threads: usize,
    /// The cores of `threads` that only early programs take, without waiting; `None` for the cores of one analysis,
    /// whose early programs wait for cores like the others.
    early: Option<usize>,
    held: Mutex<Held>,
    returned: Condvar,
}

/// The cores held now, and the most held at once.
#[derive(Debug, Default)]
struct Held {
    /// By programs of a known plan or, for one analysis, by every program.
    planned: usize,
    /// By early programs of a shared budget.
    early: usize,
    peak: usize,
    /// The programs that took cores.
    takes: usize,
}

impl Held {
    fn took(&mut self) {
        self.peak = self.peak.max(self.planned + self.early);
        self.takes += 1;
    }
}

/// How an early program gets its cores.
enum Admitted<'a> {
    /// It waits for them before it builds, with the other programs of its analysis.
    Later,
    /// It holds them.
    Now(Taken<'a>),
}

impl Cores {
    /// The cores of one analysis.
    fn new(threads: usize) -> Self {
        Self::with_early(threads, None)
    }

    /// The cores the analyses of a run share, a quarter of them kept for early programs as for one program built at
    /// once ([`CORES_PER_PROGRAM`]); fewer than that keep none, and no program starts early.
    fn shared(threads: usize) -> Self {
        Self::with_early(threads, Some(threads / CORES_PER_PROGRAM))
    }

    fn with_early(threads: usize, early: Option<usize>) -> Self {
        Self {
            threads,
            early,
            held: Mutex::default(),
            returned: Condvar::new(),
        }
    }

    /// The cores programs of a known plan take.
    fn planned(&self) -> usize {
        self.threads - self.early.unwrap_or(0)
    }

    /// Waits until `count` cores are free (all those of programs of a known plan, when `count` is more) and holds
    /// them until the guard drops. Of shared cores, only a program whose judged files are planned waits.
    fn take(&self, count: usize) -> Taken<'_> {
        let planned = self.planned();
        let count = count.min(planned);
        let mut held = lock(&self.held);
        while held.planned + count > planned {
            held = self.returned.wait(held).unwrap_or_else(PoisonError::into_inner);
        }
        held.planned += count;
        held.took();
        Taken {
            cores: self,
            count,
            early: false,
        }
    }

    /// How a program started before its judged files are planned gets its `count` cores (all of the early share,
    /// when `count` is more): later with [`Cores::take`] for the cores of one analysis; at once of the early share
    /// of shared cores when that many are free, and not at all when they are not.
    fn admit_early(&self, count: usize) -> Option<Admitted<'_>> {
        let Some(share) = self.early else {
            return Some(Admitted::Later);
        };
        let count = count.min(share);
        let mut held = lock(&self.held);
        if count == 0 || held.early + count > share {
            return None;
        }
        held.early += count;
        held.took();
        Some(Admitted::Now(Taken {
            cores: self,
            count,
            early: true,
        }))
    }

    fn peak(&self) -> usize {
        lock(&self.held).peak
    }

    fn takes(&self) -> usize {
        lock(&self.held).takes
    }

    #[cfg(test)]
    fn in_use(&self) -> usize {
        let held = lock(&self.held);
        held.planned + held.early
    }
}

/// Cores taken from [`Cores`], given back on drop.
struct Taken<'a> {
    cores: &'a Cores,
    count: usize,
    early: bool,
}

impl Drop for Taken<'_> {
    fn drop(&mut self) {
        let mut held = lock(&self.cores.held);
        if self.early {
            held.early -= self.count;
        } else {
            held.planned -= self.count;
        }
        drop(held);
        self.cores.returned.notify_all();
    }
}

/// The cores a program on `checkers` checkers with `judged` judged files holds: a checker that owns none of its
/// judged files walks nothing, so one for each checker up to the judged files, and at least one.
fn charge(checkers: usize, judged: usize) -> usize {
    checkers.min(judged).max(1)
}

/// What became of one program.
enum Outcome {
    Built(BuiltProgram),
    ConfigError,
    Failed,
    Cancelled,
}

/// The owned facts of a built program.
struct BuiltProgram {
    walked: Vec<WalkedFile>,
    /// Planned files the program turned out not to hold.
    missing: Vec<usize>,
    /// Every file of the program.
    files: usize,
    checkers: usize,
    build: Duration,
    query: Duration,
    /// Waiting for its cores.
    cores_waited: Duration,
    /// Whether the program started from the early plan.
    early: bool,
    /// Whether another analysis of the run built the program ([`TypePrograms`](crate::TypePrograms)).
    shared: bool,
}

struct WalkedFile {
    /// The index into [`ProgramPlan::files`].
    file: usize,
    facts: FileFacts,
    unresolved_modules: u32,
}

/// The programs not started yet: those of the early plan until the judged files are planned, then those of the
/// judged files' plan that did not start early.
#[derive(Debug, Default)]
struct Queue {
    /// The next position in [`Early::configs`].
    early: usize,
    /// The next position in [`Judged::order`].
    judged: usize,
    /// The configs of the programs started.
    started: BTreeSet<String>,
}

/// Where the plan of the judged files stands, as [`Queue::next`] reads it.
#[derive(Debug, Clone, Copy)]
enum Gate<'a> {
    /// Not handed over yet.
    Closed,
    /// Handed over: the configs of its programs in the order they start.
    Open(&'a [String]),
    /// The host gave the analysis up.
    Abandoned,
}

/// What a program thread does next.
#[derive(Debug, PartialEq, Eq)]
enum Next {
    /// Build the program at this position of [`Early::configs`].
    Early(usize),
    /// Build the program at this position of [`Judged::order`].
    Judged(usize),
    /// Wait for the judged files' plan: every early program has started, or the next one has no cores.
    Wait,
    /// End: every program has started.
    Done,
}

impl Queue {
    /// The next program to start, of the `early` configs or, once the gate is open, of the judged files' plan. An
    /// early program starts only when `admit` lets the one at its position have its cores; otherwise it stays queued
    /// and the thread waits for the judged files' plan.
    fn next(&mut self, early: &[String], gate: Gate<'_>, admit: impl FnOnce(usize) -> bool) -> Next {
        match gate {
            Gate::Abandoned => Next::Done,
            Gate::Open(order) => {
                while let Some(config) = order.get(self.judged) {
                    let position = self.judged;
                    self.judged += 1;
                    if self.started.insert(config.clone()) {
                        return Next::Judged(position);
                    }
                }
                Next::Done
            }
            Gate::Closed => {
                let Some(config) = early.get(self.early) else {
                    return Next::Wait;
                };
                if !admit(self.early) {
                    return Next::Wait;
                }
                let position = self.early;
                self.early += 1;
                self.started.insert(config.clone());
                Next::Early(position)
            }
        }
    }
}

/// A program of the judged files' plan, by its index there.
type Target = (Arc<Judged>, usize);

struct Session<'a> {
    shared: &'a Shared,
    early: &'a Early,
    cancel: Option<&'a AtomicBool>,
    single_threaded: bool,
    queue: Mutex<Queue>,
    /// The outcome of each program of the judged files' plan, by its index there.
    outcomes: Mutex<BTreeMap<usize, Outcome>>,
    /// The scratch regions of the program threads, freed when the analysis ends.
    regions: Mutex<Vec<Region>>,
}

impl Session<'_> {
    fn cancelled(&self) -> bool {
        self.cancel.is_some_and(|cancel| cancel.load(Ordering::Relaxed))
    }

    /// Builds and walks every program, `builders` at once until the judged files are planned and then as many as
    /// their plan builds at once; returns why a thread could not start, if one could not.
    fn run(&self, builders: usize) -> Option<std::io::Error> {
        let mut spawn_error = None;
        std::thread::scope(|scope| {
            let spawn = |index: usize| {
                std::thread::Builder::new()
                    .name(format!("skif-types-{index}"))
                    .stack_size(PROGRAM_STACK_SIZE)
                    .spawn_scoped(scope, || self.work())
            };
            let mut handles: Vec<_> = (0..builders).map(&spawn).collect();
            if let Some(judged) = self.shared.judged.wait() {
                handles.extend((builders..judged.builders).map(&spawn));
            }
            for handle in handles {
                match handle {
                    // A panic outside a program's work leaves its outcome out, which `into_outcomes` reports.
                    Ok(handle) => drop(handle.join()),
                    Err(error) => spawn_error = Some(error),
                }
            }
        });
        spawn_error
    }

    fn work(&self) {
        let region = Region::new_scratch(SCRATCH_CHUNK);
        let entered = region.enter();
        loop {
            let judged = self.shared.judged.get();
            let gate = match &judged {
                None => Gate::Closed,
                Some(None) => Gate::Abandoned,
                Some(Some(judged)) => Gate::Open(&judged.order),
            };
            let mut admitted = None;
            let next = lock(&self.queue).next(&self.early.configs, gate, |position| {
                admitted = self.shared.cores.admit_early(self.early.checkers[position]);
                admitted.is_some()
            });
            match next {
                Next::Early(position) => {
                    let held = match admitted {
                        Some(Admitted::Now(taken)) => Some(taken),
                        Some(Admitted::Later) | None => None,
                    };
                    self.start(&self.early.configs[position], self.early.checkers[position], None, held);
                }
                Next::Judged(position) => {
                    let Some(Some(judged)) = judged else {
                        break;
                    };
                    let index = judged.schedule.order[position];
                    let checkers = judged.schedule.checkers[index];
                    let config = judged.plan.programs[index].config.clone();
                    self.start(&config, checkers, Some((judged, index)), None);
                }
                Next::Wait => {
                    self.shared.judged.wait();
                }
                Next::Done => break,
            }
        }
        drop(entered);
        lock(&self.regions).push(region);
    }

    /// Runs the program of `config` on `checkers` checkers with the cores it `held` already or once they are free,
    /// and keeps its outcome for its program of the judged files' plan: `target`, or the one with its config once
    /// that plan is known.
    fn start(&self, config: &str, checkers: usize, target: Option<Target>, held: Option<Taken<'_>>) {
        let early = target.is_none();
        let outcome = self.build_and_walk(config, checkers, target.clone(), held, early);
        let Some(outcome) = outcome else {
            return;
        };
        if let Some((_, index)) = target.or_else(|| self.target(config)) {
            lock(&self.outcomes).insert(index, outcome);
        }
    }

    fn build_and_walk(
        &self,
        config: &str,
        checkers: usize,
        target: Option<Target>,
        held: Option<Taken<'_>>,
        early: bool,
    ) -> Option<Outcome> {
        if self.cancelled() {
            return Some(Outcome::Cancelled);
        }
        let waiting = Instant::now();
        // Before taking cores: a build of the program by another analysis of the run holds its cores already and
        // waits for no host, so waiting for it waits for nothing this analysis holds.
        let programs = self.shared.build.programs.as_ref();
        let claim = programs.map(|programs| programs.claim(&ProgramKey::new(config, self.single_threaded)));
        let built_checkers = match &claim {
            Some(Claim::Ready(resident)) => resident.checkers(),
            _ => checkers,
        };
        // Without cores held, the program is of the judged files' plan, or an early one of an analysis's own cores
        // ([`Cores::admit_early`]): waiting for them cannot wait for a host.
        let _taken = held.unwrap_or_else(|| {
            let count = target.as_ref().map_or(built_checkers, |(judged, index)| {
                charge(built_checkers, judged.plan.programs[*index].files.len())
            });
            self.shared.cores.take(count)
        });
        let cores_waited = waiting.elapsed();
        if self.cancelled() {
            return Some(Outcome::Cancelled);
        }
        let mut outcome = panic::catch_unwind(AssertUnwindSafe(|| match claim {
            Some(Claim::Ready(resident)) => self.walk_shared(&resident, config, target, (early, false)),
            Some(Claim::Build(ticket)) => self.run_program(config, checkers, target, early, Some(ticket)),
            None => self.run_program(config, checkers, target, early, None),
        }))
        .unwrap_or(Some(Outcome::Failed));
        if let Some(Outcome::Built(built)) = &mut outcome {
            built.cores_waited = cores_waited;
        }
        outcome
    }

    /// The program of the judged files' plan with `config`, once the plan is known; `None` when no judged file needs
    /// it or the host gave the analysis up.
    fn target(&self, config: &str) -> Option<Target> {
        let judged = self.shared.judged.wait()?;
        let index = *judged.by_config.get(config)?;
        Some((judged, index))
    }

    /// Builds the program of `path` (in a run that shares its programs, under `ticket`, the claim to build it for the
    /// run), walks its judged files and frees it unless another root of the run may still need it; `None` when no
    /// judged file needs it.
    fn run_program(
        &self,
        path: &str,
        checkers: usize,
        target: Option<Target>,
        early: bool,
        ticket: Option<Ticket<'_>>,
    ) -> Option<Outcome> {
        let start = Instant::now();
        // What the config and the host allocate lives as long as the program, which points into it.
        let region = Region::new_scratch(SCRATCH_CHUNK);
        let built = {
            let _scope = region.enter();
            self.build(path, checkers, ticket.is_some())
        };
        let Some((owner, loader_regions, files)) = built else {
            return Some(Outcome::ConfigError);
        };
        let resident = Arc::new(Resident::new(
            (owner, loader_regions, region),
            (
                Arc::clone(&self.shared.boms),
                Arc::clone(&self.shared.files),
                self.shared.configs.configs(),
            ),
            ProgramKey::new(path, self.single_threaded),
            checkers,
            start.elapsed(),
        ));
        if let Some(ticket) = ticket {
            ticket.publish(&resident, &files, &self.shared.cwd);
        }
        drop(files);
        self.walk_shared(&resident, path, target, (early, true))
    }

    /// Walks `resident`, the program of `path`, as [`Session::walk`] does, and lets it go: it is freed once no other
    /// root of the run may need it.
    fn walk_shared(
        &self,
        resident: &Arc<Resident>,
        path: &str,
        target: Option<Target>,
        started: (bool, bool),
    ) -> Option<Outcome> {
        let outcome = self.walk(resident, path, target, started);
        if let Some(programs) = &self.shared.build.programs {
            programs.walked(resident, &self.shared.cwd);
        }
        outcome
    }

    /// Reads the config at `path` and builds and binds its program on `checkers` checkers, with the program's loader
    /// regions and, when `listed`, the files the config includes; `None` when the config cannot be read.
    fn build(
        &self,
        path: &str,
        checkers: usize,
        listed: bool,
    ) -> Option<(ProgramOwner, Arc<ProgramRegions>, Vec<String>)> {
        let mut config = self.shared.configs.parse_own(path)?;
        let base = config.compiler_options()?;
        let files = if listed {
            config.file_names().to_vec()
        } else {
            Vec::new()
        };
        let single_threaded = self.single_threaded;
        config.parsed_config.compiler_options = Some(P::new(CompilerOptions {
            checkers: Some(i64::try_from(checkers).unwrap_or(i64::MAX)),
            single_threaded: if single_threaded {
                Tristate::True
            } else {
                base.single_threaded
            },
            ..(*base).clone()
        }));
        // Dropped after the program: nothing that outlives the program points into its loader regions.
        let loader_regions: Arc<ProgramRegions> = Arc::default();
        let compiler_host = tsrs_compiler::new_compiler_host(
            &tspath::get_directory_path(path),
            Arc::clone(&self.shared.fs),
            &vfs::lib_path(),
            None,
            None,
            None,
        );
        // The lib and `node_modules` declaration files come parsed and bound from the files the run shares, which the
        // program's resident keeps until after the program is freed.
        let compiler_host: Arc<dyn tsrs_compiler::CompilerHost> = Arc::new(SharingHost::new(
            compiler_host,
            Arc::clone(&self.shared.files),
            Arc::clone(&self.shared.boms),
            Arc::clone(&self.shared.file_counts),
            Arc::clone(&self.shared.configs),
        ));
        let mut options = ProgramOptions::new(P::new(config), compiler_host);
        // Referenced projects contribute their sources, as editors and tsrs-lint read them.
        options.use_source_of_project_reference = true;
        options.regions = Some(Arc::clone(&loader_regions));
        // The checkers' memory goes back with the program, the diagnostics they make while answering included:
        // without a region of their own those go to the arena of the thread that made them, which is never freed,
        // so every program of a run would add its diagnostics. No diagnostic is read.
        options.checker_regions = true;
        options.checker_diagnostic_regions = true;
        if single_threaded {
            options.single_threaded = Tristate::True;
        }
        let pool = self.shared.pool.clone();
        let owner = self.shared.on_pool(|| ProgramOwner::new(options, pool));
        self.shared.on_pool(|| owner.program().bind_source_files());
        Some((owner, loader_regions, files))
    }

    /// Walks the judged files of `resident`, the program of `path`, copying out what the walks find; `started` says
    /// whether the program started from the early plan and whether this analysis built it. An early program waits
    /// here, built, for the judged files' plan, before it enters the program: another analysis of the run may walk it
    /// meanwhile.
    fn walk(&self, resident: &Resident, path: &str, target: Option<Target>, started: (bool, bool)) -> Option<Outcome> {
        let (early, built_here) = started;
        let (judged, index) = match target {
            Some(target) => target,
            None => self.target(path)?,
        };
        if self.cancelled() {
            return Some(Outcome::Cancelled);
        }
        let _walking = resident.walking();
        let tsrs = resident.program();
        let program = &judged.plan.programs[index];
        let mut sources = Vec::new();
        let mut indices = Vec::new();
        let mut missing = Vec::new();
        for &file in &program.files {
            match tsrs.get_source_file(&judged.plan.files[file].name) {
                Some(source) => {
                    sources.push(source);
                    indices.push(file);
                }
                None => missing.push(file),
            }
        }
        // The program's own record: it read the files through the file system of the analysis that built it.
        let context = QueryContext {
            candidates: &judged.candidates,
            boms: resident.boms(),
        };
        let slots: Vec<Mutex<Option<(FileFacts, u32)>>> = sources.iter().map(|_| Mutex::new(None)).collect();
        let query_start = Instant::now();
        let pooled = self.shared.on_pool(|| {
            tsrs.for_each_checker_group(&sources, |checker, index, source| {
                if self.cancelled() {
                    return;
                }
                let mut facts = query::query_file(checker, source, &context);
                // A file reads a member many times; one read of each is all the credit needs.
                facts.reads.sort_unstable();
                facts.reads.dedup();
                facts.whole.sort_unstable();
                facts.whole.dedup();
                let unresolved_modules = query::unresolved_modules(checker, tsrs, source);
                *lock(&slots[index]) = Some((facts, unresolved_modules));
            })
        });
        if !pooled {
            return Some(Outcome::Failed);
        }
        let query = query_start.elapsed();
        if self.cancelled() {
            return Some(Outcome::Cancelled);
        }
        let mut walked = Vec::with_capacity(indices.len());
        for (slot, file) in slots.into_iter().zip(indices) {
            let Some((facts, unresolved_modules)) = slot.into_inner().unwrap_or_else(PoisonError::into_inner) else {
                return Some(Outcome::Failed);
            };
            walked.push(WalkedFile {
                file,
                facts,
                unresolved_modules,
            });
        }
        Some(Outcome::Built(BuiltProgram {
            walked,
            missing,
            files: tsrs.get_source_files().len(),
            checkers: resident.checkers(),
            build: if built_here { resident.build() } else { Duration::ZERO },
            query,
            cores_waited: Duration::ZERO,
            early,
            shared: !built_here,
        }))
    }
}

/// Joins the outcomes of every program into the facts.
fn assemble(
    input: &TypeInput<'_>,
    plan: &ProgramPlan,
    candidates: &Candidates,
    outcomes: Vec<Outcome>,
) -> Result<TypeFacts, TypeError> {
    let mut states: Vec<FileState> = plan
        .files
        .iter()
        .map(|file| match file.placement {
            // Set from the program's outcome below.
            Placement::Program(_) | Placement::Outside => FileState::Outside,
            Placement::NoConfig => FileState::NoConfig,
            Placement::ConfigError => FileState::ConfigError,
        })
        .collect();
    let mut found = Found::default();
    let mut unresolved_names: BTreeMap<PathBuf, BTreeSet<String>> = BTreeMap::new();
    let mut timings = TypeTimings::default();
    for (program, outcome) in plan.programs.iter().zip(outcomes) {
        match outcome {
            Outcome::Built(built) => {
                timings.programs += built.build;
                timings.queries += built.query;
                timings.cores_waited += built.cores_waited;
                timings.early += usize::from(built.early);
                timings.per_program.push(ProgramTiming {
                    config: PathBuf::from(&program.config),
                    judged_files: program.files.len(),
                    files: built.files,
                    checkers: built.checkers,
                    build: built.build,
                    query: built.query,
                    shared: built.shared,
                });
                for index in built.missing {
                    states[index] = FileState::Outside;
                }
                for walked in built.walked {
                    states[walked.file] = FileState::Covered {
                        unresolved_modules: walked.unresolved_modules,
                    };
                    let FileFacts {
                        reads,
                        whole,
                        unresolved,
                        implementations,
                    } = walked.facts;
                    found.reads.extend(reads);
                    found.whole.extend(whole);
                    found.implementations.extend(implementations);
                    if !unresolved.is_empty() {
                        unresolved_names
                            .entry(plan.files[walked.file].path.clone())
                            .or_default()
                            .extend(unresolved);
                    }
                }
            }
            Outcome::ConfigError => mark(&mut states, program, FileState::ConfigError),
            Outcome::Failed => mark(&mut states, program, FileState::CheckerFailed),
            Outcome::Cancelled => return Err(TypeError::Cancelled),
        }
    }
    let credits = members::credit(candidates, &found);
    let covered = plan
        .files
        .iter()
        .zip(&states)
        .filter(|(_, state)| matches!(state, FileState::Covered { .. }))
        .map(|(file, _)| file.path.clone())
        .collect();
    let coverage = input
        .packages
        .iter()
        .enumerate()
        .filter(|(_, package)| !package.files.is_empty())
        .map(|(index, package)| {
            let package_states: Vec<FileState> = plan
                .files
                .iter()
                .zip(&states)
                .filter(|(file, _)| file.package == index)
                .map(|(_, &state)| state)
                .collect();
            coverage::package_coverage(package, &package_states)
        })
        .collect();
    Ok(TypeFacts {
        engine: ENGINE,
        reads: credits.reads,
        whole: credits.whole,
        unresolved_names,
        covered,
        coverage,
        timings,
    })
}

fn mark(states: &mut [FileState], program: &PlannedProgram, state: FileState) {
    for &index in &program.files {
        states[index] = state;
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A panic inside tsrs outside any program's work, as an error.
fn internal(stage: &str, payload: &(dyn std::any::Any + Send)) -> TypeError {
    let detail = payload
        .downcast_ref::<&str>()
        .map(|message| (*message).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "a panic without a message".to_owned());
    TypeError::Internal {
        message: format!("{stage}: {detail}"),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    fn parallelism(threads: usize, programs: usize) -> Parallelism {
        Parallelism::new(NonZeroUsize::new(threads), programs)
    }

    fn schedule(threads: usize, sizes: &[usize]) -> Schedule {
        Schedule::new(&parallelism(threads, sizes.len()), sizes)
    }

    #[test]
    fn a_quarter_of_the_threads_build_programs_and_share_the_rest_as_checkers() {
        let cases = [
            ((16, 10), (4, 4, 8)),
            ((8, 10), (2, 4, 8)),
            ((3, 10), (1, 3, 3)),
            ((64, 40), (16, 4, 8)),
        ];
        for ((threads, programs), (at_once, even, most)) in cases {
            assert_eq!(
                parallelism(threads, programs),
                Parallelism {
                    threads,
                    programs: at_once,
                    even,
                    most,
                    single_threaded: false
                },
                "{threads} threads, {programs} programs"
            );
        }
    }

    #[test]
    fn fewer_programs_than_slots_get_more_checkers_each() {
        assert_eq!(parallelism(16, 1).programs, 1);
        assert_eq!(parallelism(16, 1).even, 16);
        assert_eq!(parallelism(16, 0).programs, 1);
        assert_eq!(schedule(16, &[40]).checkers, [16]);
    }

    #[test]
    fn one_thread_runs_tsrs_single_threaded() {
        assert_eq!(
            parallelism(1, 5),
            Parallelism {
                threads: 1,
                programs: 1,
                even: 1,
                most: 1,
                single_threaded: true
            }
        );
        assert_eq!(schedule(1, &[100, 1]).checkers, [1, 1]);
    }

    #[test]
    fn programs_of_one_size_share_the_threads_evenly_in_plan_order() {
        assert_eq!(
            schedule(16, &[50; 10]),
            Schedule {
                order: (0..10).collect(),
                checkers: vec![4; 10],
            }
        );
    }

    #[test]
    fn a_program_with_most_of_the_judged_files_starts_first_with_most_of_the_threads() {
        assert_eq!(
            schedule(16, &[100, 10_000, 100, 100, 100]),
            Schedule {
                order: vec![1, 0, 2, 3, 4],
                checkers: vec![4, 8, 4, 4, 4],
            }
        );
    }

    #[test]
    fn checkers_follow_the_share_of_the_judged_files_between_the_even_share_and_the_most() {
        assert_eq!(schedule(16, &[500, 300, 100, 100]).checkers, [8, 5, 4, 4]);
        let mut sizes = vec![10; 17];
        sizes[0] = 10_000;
        let wide = schedule(64, &sizes);
        assert_eq!(
            wide.checkers[0], MOST_SHARED_CHECKERS,
            "a program beside others gets at most MOST_SHARED_CHECKERS"
        );
        assert!(wide.checkers[1..].iter().all(|&checkers| checkers == 4));
    }

    #[test]
    fn a_program_gets_no_more_checkers_than_judged_files() {
        assert_eq!(schedule(16, &[1, 2, 3, 400]).checkers, [1, 2, 3, 8]);
        assert_eq!(
            schedule(16, &[0]).checkers,
            [1],
            "a program without judged files still has a checker"
        );
        assert_eq!(
            schedule(16, &[3]).checkers,
            [3],
            "a lone program's even share is capped too"
        );
    }

    #[test]
    fn programs_of_one_size_keep_their_plan_order_behind_larger_ones() {
        assert_eq!(schedule(16, &[5, 9, 5, 9]).order, [1, 3, 0, 2]);
    }

    #[test]
    fn a_program_waits_until_its_checkers_fit_beside_the_running_ones() {
        let cores = Cores::new(4);
        let running = cores.take(3);
        let (started, waited) = mpsc::channel();
        std::thread::scope(|scope| {
            let cores = &cores;
            scope.spawn(move || {
                let _taken = cores.take(2);
                started.send(()).expect("the test waits for the program");
            });
            assert!(
                waited.recv_timeout(Duration::from_millis(50)).is_err(),
                "two threads do not fit beside three of four"
            );
            drop(running);
            assert!(
                waited.recv_timeout(Duration::from_secs(30)).is_ok(),
                "the program starts once the running one gives its threads back"
            );
        });
        assert_eq!(cores.in_use(), 0);
        assert_eq!(cores.peak(), 3, "the waiting program never ran beside the first");
    }

    #[test]
    fn a_program_that_asks_for_more_threads_than_there_are_takes_them_all() {
        let cores = Cores::new(4);
        let all = cores.take(9);
        assert_eq!(cores.in_use(), 4);
        drop(all);
        assert_eq!(cores.in_use(), 0);
    }

    #[test]
    fn an_analysis_of_its_own_lets_its_early_programs_wait_for_cores() {
        let cores = Cores::new(4);
        let _running = cores.take(4);
        assert!(
            matches!(cores.admit_early(9), Some(Admitted::Later)),
            "an early program of one analysis waits for cores like the others"
        );
        assert_eq!(cores.in_use(), 4);
    }

    #[test]
    fn a_shared_budget_gives_early_programs_only_free_cores_of_its_early_share() {
        // A quarter of 8 cores is the early share; the other 6 are for programs of a known plan.
        let cores = Cores::shared(8);
        let Some(Admitted::Now(first)) = cores.admit_early(1) else {
            panic!("a free core of the early share is taken at once");
        };
        assert!(
            cores.admit_early(5).is_none(),
            "a program asking for more than the early share asks for all of it, and one core of it is held"
        );
        let Some(Admitted::Now(second)) = cores.admit_early(1) else {
            panic!("the last free core of the early share is taken at once");
        };
        assert!(cores.admit_early(1).is_none(), "the early share is held");
        let planned = cores.take(9);
        assert_eq!(
            cores.in_use(),
            8,
            "a planned program takes the planned share, whatever the early programs hold"
        );
        assert_eq!(cores.peak(), 8);
        assert_eq!(cores.takes(), 3);
        drop((first, second, planned));
        assert_eq!(cores.in_use(), 0);
    }

    #[test]
    fn a_planned_program_never_waits_for_cores_an_early_program_holds() {
        let cores = Cores::shared(4);
        let Some(Admitted::Now(early)) = cores.admit_early(4) else {
            panic!("the early share of 4 cores is one core");
        };
        let (started, waited) = mpsc::channel();
        std::thread::scope(|scope| {
            let cores = &cores;
            scope.spawn(move || {
                let _taken = cores.take(3);
                started.send(()).expect("the test waits for the program");
            });
            assert!(
                waited.recv_timeout(Duration::from_secs(30)).is_ok(),
                "the planned share is free while the early program holds its core"
            );
        });
        drop(early);
        assert_eq!(cores.peak(), 4);
    }

    #[test]
    fn a_shared_budget_too_small_for_an_early_share_starts_no_program_early() {
        for threads in 1..CORES_PER_PROGRAM {
            let cores = Cores::shared(threads);
            assert!(cores.admit_early(1).is_none(), "{threads} cores");
            drop(cores.take(threads));
            assert_eq!(cores.peak(), threads, "{threads} cores are all for planned programs");
        }
    }

    #[test]
    fn a_program_holds_a_core_for_each_checker_that_has_a_judged_file() {
        let cases = [((8, 3), 3), ((4, 100), 4), ((4, 0), 1), ((1, 1), 1)];
        for ((checkers, judged), expected) in cases {
            assert_eq!(
                charge(checkers, judged),
                expected,
                "{checkers} checkers, {judged} judged files"
            );
        }
    }

    const fn admitted(_position: usize) -> bool {
        true
    }

    fn configs(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    fn planned(configs_and_sizes: &[(&str, usize)]) -> Vec<PlannedProgram> {
        let mut next = 0;
        configs_and_sizes
            .iter()
            .map(|&(config, size)| {
                let files = (next..next + size).collect();
                next += size;
                PlannedProgram {
                    config: config.to_owned(),
                    files,
                }
            })
            .collect()
    }

    #[test]
    fn early_programs_start_until_the_judged_files_are_planned_then_the_rest_of_their_plan() {
        let early = configs(&["a", "b"]);
        let mut queue = Queue::default();
        assert_eq!(queue.next(&early, Gate::Closed, admitted), Next::Early(0));
        assert_eq!(queue.next(&early, Gate::Closed, admitted), Next::Early(1));
        assert_eq!(
            queue.next(&early, Gate::Closed, admitted),
            Next::Wait,
            "every early program has started"
        );
        let judged = configs(&["b", "c", "a", "d"]);
        assert_eq!(
            queue.next(&early, Gate::Open(&judged), admitted),
            Next::Judged(1),
            "`b` started early"
        );
        assert_eq!(
            queue.next(&early, Gate::Open(&judged), admitted),
            Next::Judged(3),
            "`a` started early"
        );
        assert_eq!(queue.next(&early, Gate::Open(&judged), admitted), Next::Done);
    }

    #[test]
    fn an_early_program_not_started_when_the_judged_files_are_planned_starts_from_their_plan() {
        let early = configs(&["a", "b"]);
        let mut queue = Queue::default();
        assert_eq!(queue.next(&early, Gate::Closed, admitted), Next::Early(0));
        let judged = configs(&["a", "b"]);
        assert_eq!(queue.next(&early, Gate::Open(&judged), admitted), Next::Judged(1));
        assert_eq!(queue.next(&early, Gate::Open(&judged), admitted), Next::Done);
    }

    #[test]
    fn nothing_starts_once_the_analysis_is_given_up() {
        let early = configs(&["a"]);
        let mut queue = Queue::default();
        assert_eq!(queue.next(&early, Gate::Abandoned, admitted), Next::Done);
        assert_eq!(
            queue.next(&early, Gate::Closed, admitted),
            Next::Early(0),
            "the early program had not started"
        );
    }

    #[test]
    fn an_early_program_refused_cores_starts_from_the_judged_files_plan() {
        let early = configs(&["a", "b"]);
        let mut queue = Queue::default();
        assert_eq!(
            queue.next(&early, Gate::Closed, |_| false),
            Next::Wait,
            "a program refused cores waits for the judged files' plan"
        );
        assert_eq!(
            queue.next(&early, Gate::Closed, admitted),
            Next::Early(0),
            "and is still queued"
        );
        let judged = configs(&["a", "b"]);
        assert_eq!(queue.next(&early, Gate::Open(&judged), |_| false), Next::Judged(1));
        assert_eq!(queue.next(&early, Gate::Open(&judged), admitted), Next::Done);
    }

    #[test]
    fn the_judged_files_keep_the_schedule_of_an_early_plan_of_the_same_programs() {
        let programs = planned(&[("a", 100), ("b", 10_000), ("c", 100), ("d", 100), ("e", 100)]);
        let sizes: Vec<usize> = programs.iter().map(|program| program.files.len()).collect();
        let parallelism = parallelism(16, programs.len());
        let own = Schedule::new(&parallelism, &sizes);
        let plan = ProgramPlan {
            programs,
            files: Vec::new(),
        };
        let early = Early::new(&plan, &own);
        assert_eq!(judged_schedule(&parallelism, &early, &plan.programs), own);
    }

    #[test]
    fn a_judged_program_keeps_its_early_checkers_and_order_and_the_others_follow_by_size() {
        let early = Early {
            configs: configs(&["gone", "b", "a"]),
            checkers: vec![8, 6, 5],
        };
        // Fewer judged files than early ones in `a` and `b`, which may already be built with their early checkers.
        let programs = planned(&[("a", 1), ("b", 2), ("c", 30), ("d", 400)]);
        let schedule = judged_schedule(&parallelism(16, programs.len()), &early, &programs);
        assert_eq!(
            schedule,
            Schedule {
                order: vec![1, 0, 3, 2],
                checkers: vec![5, 6, 4, 8],
            }
        );
    }

    #[test]
    fn a_slot_hands_its_first_value_to_every_waiting_thread() {
        let slot: Slot<Option<u32>> = Slot::default();
        assert_eq!(slot.get(), None);
        std::thread::scope(|scope| {
            let waiter = scope.spawn(|| slot.wait());
            slot.put(Some(7));
            slot.put(None);
            assert_eq!(waiter.join().unwrap(), Some(7));
        });
        assert_eq!(slot.get(), Some(Some(7)), "a later value does not replace the first");
        assert_eq!(slot.take(), Some(7));
        assert_eq!(slot.get(), None);
    }

    #[test]
    fn panic_payloads_become_internal_errors() {
        let message = internal("planning the programs", &"boom");
        assert!(matches!(message, TypeError::Internal { message } if message == "planning the programs: boom"));
        let owned = internal("x", &String::from("bad"));
        assert!(matches!(owned, TypeError::Internal { message } if message == "x: bad"));
    }
}
