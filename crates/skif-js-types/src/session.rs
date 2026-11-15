//! One analysis: plan the programs, build a few at a time (the largest first, each with checkers in proportion to its
//! share of the judged files), walk their judged files on their checkers, copy the facts out and free each program as
//! soon as its walk ends (ADR 0044, ADR 0045).

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroUsize;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use tsrs_compiler::{ProgramOptions, ProgramRegions};
use tsrs_core::arena::Region;
use tsrs_core::{CompilerOptions, P, Tristate, tspath};
use tsrs_vfs::FS;

use crate::coverage::{self, FileState};
use crate::members::{self, Candidates, Found};
use crate::owner::ProgramOwner;
use crate::programs::{self, ConfigHost, Placement, PlannedProgram, ProgramPlan};
use crate::query::{self, FileFacts, QueryContext};
use crate::vfs::{self, BomFiles};
use crate::{ENGINE, ProgramTiming, TypeError, TypeFacts, TypeInput, TypeTimings};

/// The stack of the threads that build and check programs: checking recurses deeply, and tsrs-lint gives its program
/// threads the same.
const PROGRAM_STACK_SIZE: usize = 512 << 20;
/// The first chunk of the scratch region configs and program bookkeeping are allocated in.
const SCRATCH_CHUNK: usize = 1 << 20;
/// Cores per program built at once: each program checks on a pool of its own, so a quarter of the cores at a time
/// fills the machine without holding every program in memory (tsrs-lint's measurement).
const CORES_PER_PROGRAM: usize = 4;
/// The most checkers a program gets while other programs are built beside it. Each checker holds its own type
/// caches: on the largest repository of the corpus, 16 checkers on its main program raised peak memory past the
/// budget of ADR 0047 without a shorter run than 8.
const MOST_SHARED_CHECKERS: usize = 8;

/// Runs the analysis of `input`.
pub(crate) fn analyze(input: &TypeInput<'_>) -> Result<TypeFacts, TypeError> {
    let cancelled = || input.cancel.is_some_and(|cancel| cancel.load(Ordering::Relaxed));
    if cancelled() {
        return Err(TypeError::Cancelled);
    }
    let (fs, boms) = vfs::tsrs_fs(Arc::clone(&input.fs));
    let host = ConfigHost::new(Arc::clone(&fs), vfs::tsrs_path(input.root));
    // What planning allocates (the parsed configs) is freed when the analysis ends.
    let region = Region::new_scratch(SCRATCH_CHUNK);
    let start = Instant::now();
    let plan = {
        let _scope = region.enter();
        panic::catch_unwind(AssertUnwindSafe(|| programs::plan_programs(input, &host)))
            .map_err(|payload| internal("planning the programs", payload.as_ref()))??
    };
    let planning = start.elapsed();
    if cancelled() {
        return Err(TypeError::Cancelled);
    }
    let candidates = Candidates::new(input.query);
    let parallelism = Parallelism::new(input.threads, plan.programs.len());
    let sizes: Vec<usize> = plan.programs.iter().map(|program| program.files.len()).collect();
    let session = Session {
        plan: &plan,
        fs: &fs,
        host: &host,
        candidates: &candidates,
        boms: &boms,
        cancel: input.cancel,
        single_threaded: parallelism.single_threaded,
        schedule: Schedule::new(&parallelism, &sizes),
        cores: Cores::new(parallelism.threads),
        next: AtomicUsize::new(0),
        outcomes: plan.programs.iter().map(|_| Mutex::new(None)).collect(),
        regions: Mutex::new(Vec::new()),
    };
    let run_start = Instant::now();
    session.run(parallelism.programs)?;
    let wall = run_start.elapsed();
    let outcomes = session.into_outcomes()?;
    let mut facts = assemble(input, &plan, &candidates, outcomes)?;
    facts.timings.planning = planning;
    facts.timings.wall = wall;
    facts.timings.threads = parallelism.threads;
    drop(region);
    Ok(facts)
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
        let threads = threads
            .or_else(|| std::thread::available_parallelism().ok())
            .map_or(1, NonZeroUsize::get);
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
    /// The schedule of programs that hold `sizes` judged files each.
    fn new(parallelism: &Parallelism, sizes: &[usize]) -> Self {
        let total = sizes.iter().sum();
        let mut order: Vec<usize> = (0..sizes.len()).collect();
        order.sort_by_key(|&index| std::cmp::Reverse(sizes[index]));
        Self {
            order,
            checkers: sizes.iter().map(|&files| parallelism.checkers(files, total)).collect(),
        }
    }
}

/// The threads not handed to a program's checkers yet. A program starts once its checkers fit, so the programs that
/// run at once never hold more checkers than the analysis has threads, and a program that takes most of them runs
/// with few others beside it, which bounds the memory they hold together.
struct Cores {
    threads: usize,
    free: Mutex<usize>,
    returned: Condvar,
}

impl Cores {
    fn new(threads: usize) -> Self {
        Self {
            threads,
            free: Mutex::new(threads),
            returned: Condvar::new(),
        }
    }

    /// Waits until `count` threads are free (all of them, when `count` is more) and holds them until the guard drops.
    fn take(&self, count: usize) -> Taken<'_> {
        let count = count.min(self.threads);
        let mut free = lock(&self.free);
        while *free < count {
            free = self.returned.wait(free).unwrap_or_else(PoisonError::into_inner);
        }
        *free -= count;
        Taken { cores: self, count }
    }
}

/// Threads taken from [`Cores`], given back on drop.
struct Taken<'a> {
    cores: &'a Cores,
    count: usize,
}

impl Drop for Taken<'_> {
    fn drop(&mut self) {
        *lock(&self.cores.free) += self.count;
        self.cores.returned.notify_all();
    }
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
}

struct WalkedFile {
    /// The index into [`ProgramPlan::files`].
    file: usize,
    facts: FileFacts,
    unresolved_modules: u32,
}

struct Session<'a> {
    plan: &'a ProgramPlan,
    fs: &'a Arc<dyn FS>,
    host: &'a ConfigHost,
    candidates: &'a Candidates,
    boms: &'a BomFiles,
    cancel: Option<&'a AtomicBool>,
    single_threaded: bool,
    schedule: Schedule,
    cores: Cores,
    /// The next position in [`Schedule::order`].
    next: AtomicUsize,
    outcomes: Vec<Mutex<Option<Outcome>>>,
    /// The scratch regions of the program threads, freed when the analysis ends.
    regions: Mutex<Vec<Region>>,
}

impl Session<'_> {
    fn cancelled(&self) -> bool {
        self.cancel.is_some_and(|cancel| cancel.load(Ordering::Relaxed))
    }

    /// Builds and walks every program on `program_threads` threads.
    fn run(&self, program_threads: usize) -> Result<(), TypeError> {
        if self.plan.programs.is_empty() {
            return Ok(());
        }
        let mut spawn_error = None;
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..program_threads)
                .map(|index| {
                    std::thread::Builder::new()
                        .name(format!("skif-types-{index}"))
                        .stack_size(PROGRAM_STACK_SIZE)
                        .spawn_scoped(scope, || self.work())
                })
                .collect();
            for handle in handles {
                match handle {
                    // A panic outside a program's work leaves its slot empty, which `into_outcomes` reports.
                    Ok(handle) => drop(handle.join()),
                    Err(error) => spawn_error = Some(error),
                }
            }
        });
        match spawn_error {
            Some(error) if self.outcomes.iter().any(|slot| lock(slot).is_none()) => Err(TypeError::Internal {
                message: format!("cannot start a type-checker thread: {error}"),
            }),
            _ => Ok(()),
        }
    }

    fn work(&self) {
        let region = Region::new_scratch(SCRATCH_CHUNK);
        let entered = region.enter();
        loop {
            let position = self.next.fetch_add(1, Ordering::Relaxed);
            let Some(&index) = self.schedule.order.get(position) else {
                break;
            };
            let program = &self.plan.programs[index];
            let outcome = self.start(program, self.schedule.checkers[index]);
            *lock(&self.outcomes[index]) = Some(outcome);
        }
        drop(entered);
        self.regions.lock().unwrap_or_else(PoisonError::into_inner).push(region);
    }

    /// Runs `program` on `checkers` checkers once that many threads are free.
    fn start(&self, program: &PlannedProgram, checkers: usize) -> Outcome {
        if self.cancelled() {
            return Outcome::Cancelled;
        }
        let _taken = self.cores.take(checkers);
        if self.cancelled() {
            return Outcome::Cancelled;
        }
        panic::catch_unwind(AssertUnwindSafe(|| self.run_program(program, checkers))).unwrap_or(Outcome::Failed)
    }

    /// Builds `program`, walks its judged files and frees it.
    fn run_program(&self, program: &PlannedProgram, checkers: usize) -> Outcome {
        let start = Instant::now();
        let Some(mut config) = programs::parse_config(&program.config, self.host) else {
            return Outcome::ConfigError;
        };
        let Some(base) = config.compiler_options() else {
            return Outcome::ConfigError;
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
            &tspath::get_directory_path(&program.config),
            Arc::clone(self.fs),
            &vfs::lib_path(),
            None,
            None,
            None,
        );
        let mut options = ProgramOptions::new(P::new(config), compiler_host);
        // Referenced projects contribute their sources, as editors and tsrs-lint read them.
        options.use_source_of_project_reference = true;
        options.regions = Some(Arc::clone(&loader_regions));
        // The checkers' memory goes back with the program.
        options.checker_regions = true;
        if single_threaded {
            options.single_threaded = Tristate::True;
        }
        let owner = ProgramOwner::new(options);
        let built = self.walk(owner.program(), program, start, checkers);
        drop(owner);
        drop(loader_regions);
        built
    }

    /// Walks the judged files of `program` once it is built, copying out what the walks find.
    fn walk(
        &self,
        tsrs: &tsrs_compiler::Program,
        program: &PlannedProgram,
        start: Instant,
        checkers: usize,
    ) -> Outcome {
        tsrs.bind_source_files();
        let build = start.elapsed();
        if self.cancelled() {
            return Outcome::Cancelled;
        }
        let mut sources = Vec::new();
        let mut indices = Vec::new();
        let mut missing = Vec::new();
        for &index in &program.files {
            match tsrs.get_source_file(&self.plan.files[index].name) {
                Some(source) => {
                    sources.push(source);
                    indices.push(index);
                }
                None => missing.push(index),
            }
        }
        let context = QueryContext {
            candidates: self.candidates,
            boms: self.boms,
        };
        let slots: Vec<Mutex<Option<(FileFacts, u32)>>> = sources.iter().map(|_| Mutex::new(None)).collect();
        let query_start = Instant::now();
        let pooled = tsrs.for_each_checker_group(&sources, |checker, index, source| {
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
        });
        if !pooled {
            return Outcome::Failed;
        }
        let query = query_start.elapsed();
        if self.cancelled() {
            return Outcome::Cancelled;
        }
        let mut walked = Vec::with_capacity(indices.len());
        for (slot, index) in slots.into_iter().zip(indices) {
            let Some((facts, unresolved_modules)) = slot.into_inner().unwrap_or_else(PoisonError::into_inner) else {
                return Outcome::Failed;
            };
            walked.push(WalkedFile {
                file: index,
                facts,
                unresolved_modules,
            });
        }
        Outcome::Built(BuiltProgram {
            walked,
            missing,
            files: tsrs.get_source_files().len(),
            checkers,
            build,
            query,
        })
    }

    /// The outcome of every program, once the threads are done.
    fn into_outcomes(self) -> Result<Vec<Outcome>, TypeError> {
        let outcomes: Vec<Outcome> = self
            .outcomes
            .into_iter()
            .map(|slot| {
                slot.into_inner()
                    .unwrap_or_else(PoisonError::into_inner)
                    .unwrap_or(Outcome::Failed)
            })
            .collect();
        drop(self.regions);
        if outcomes.iter().any(|outcome| matches!(outcome, Outcome::Cancelled)) {
            return Err(TypeError::Cancelled);
        }
        Ok(outcomes)
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
                timings.per_program.push(ProgramTiming {
                    config: PathBuf::from(&program.config),
                    judged_files: program.files.len(),
                    files: built.files,
                    checkers: built.checkers,
                    build: built.build,
                    query: built.query,
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
        assert_eq!(*lock(&cores.free), 4);
    }

    #[test]
    fn a_program_that_asks_for_more_threads_than_there_are_takes_them_all() {
        let cores = Cores::new(4);
        let all = cores.take(9);
        assert_eq!(*lock(&cores.free), 0);
        drop(all);
        assert_eq!(*lock(&cores.free), 4);
    }

    #[test]
    fn panic_payloads_become_internal_errors() {
        let message = internal("planning the programs", &"boom");
        assert!(matches!(message, TypeError::Internal { message } if message == "planning the programs: boom"));
        let owned = internal("x", &String::from("bad"));
        assert!(matches!(owned, TypeError::Internal { message } if message == "x: bad"));
    }
}
