//! One analysis: plan the programs, build a few at a time, walk their judged files on their checkers, copy the facts
//! out and free every program before returning (ADR 0044, ADR 0045).

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroUsize;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
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
use crate::{ENGINE, TypeError, TypeFacts, TypeInput, TypeTimings};

/// The stack of the threads that build and check programs: checking recurses deeply, and tsrs-lint gives its program
/// threads the same.
const PROGRAM_STACK_SIZE: usize = 512 << 20;
/// The first chunk of the scratch region configs and program bookkeeping are allocated in.
const SCRATCH_CHUNK: usize = 1 << 20;
/// Cores per program built at once: each program checks on a pool of its own, so a quarter of the cores at a time
/// fills the machine without holding every program in memory (tsrs-lint's measurement).
const CORES_PER_PROGRAM: usize = 4;

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
    let session = Session {
        plan: &plan,
        fs: &fs,
        host: &host,
        candidates: &candidates,
        boms: &boms,
        cancel: input.cancel,
        parallelism: Parallelism::new(input.threads, plan.programs.len()),
        next: AtomicUsize::new(0),
        outcomes: plan.programs.iter().map(|_| Mutex::new(None)).collect(),
        regions: Mutex::new(Vec::new()),
    };
    session.run()?;
    let outcomes = session.into_outcomes()?;
    let mut facts = assemble(input, &plan, &candidates, outcomes)?;
    facts.timings.programs += planning;
    drop(region);
    Ok(facts)
}

/// How many programs are built at once and how many checkers each gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Parallelism {
    programs: usize,
    checkers: usize,
    single_threaded: bool,
}

impl Parallelism {
    fn new(threads: Option<NonZeroUsize>, programs: usize) -> Self {
        let threads = threads
            .or_else(|| std::thread::available_parallelism().ok())
            .map_or(1, NonZeroUsize::get);
        let at_once = (threads / CORES_PER_PROGRAM).clamp(1, programs.max(1));
        Self {
            programs: at_once,
            checkers: (threads / at_once).max(1),
            single_threaded: threads == 1,
        }
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
    parallelism: Parallelism,
    next: AtomicUsize,
    outcomes: Vec<Mutex<Option<Outcome>>>,
    /// The scratch regions of the program threads, freed when the analysis ends.
    regions: Mutex<Vec<Region>>,
}

impl Session<'_> {
    fn cancelled(&self) -> bool {
        self.cancel.is_some_and(|cancel| cancel.load(Ordering::Relaxed))
    }

    /// Builds and walks every program on `parallelism.programs` threads.
    fn run(&self) -> Result<(), TypeError> {
        if self.plan.programs.is_empty() {
            return Ok(());
        }
        let mut spawn_error = None;
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..self.parallelism.programs)
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
            let index = self.next.fetch_add(1, Ordering::Relaxed);
            let Some(program) = self.plan.programs.get(index) else {
                break;
            };
            let outcome = if self.cancelled() {
                Outcome::Cancelled
            } else {
                panic::catch_unwind(AssertUnwindSafe(|| self.run_program(program))).unwrap_or(Outcome::Failed)
            };
            *lock(&self.outcomes[index]) = Some(outcome);
        }
        drop(entered);
        self.regions.lock().unwrap_or_else(PoisonError::into_inner).push(region);
    }

    /// Builds `program`, walks its judged files and frees it.
    fn run_program(&self, program: &PlannedProgram) -> Outcome {
        let start = Instant::now();
        let Some(mut config) = programs::parse_config(&program.config, self.host) else {
            return Outcome::ConfigError;
        };
        let Some(base) = config.compiler_options() else {
            return Outcome::ConfigError;
        };
        let Parallelism {
            checkers,
            single_threaded,
            ..
        } = self.parallelism;
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
        let built = self.walk(owner.program(), program, start);
        drop(owner);
        drop(loader_regions);
        built
    }

    /// Walks the judged files of `program` once it is built, copying out what the walks find.
    fn walk(&self, tsrs: &tsrs_compiler::Program, program: &PlannedProgram, start: Instant) -> Outcome {
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
            let facts = query::query_file(checker, source, &context);
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
                        ancestry,
                    } = walked.facts;
                    found.reads.extend(reads);
                    found.whole.extend(whole);
                    for (class, ancestors) in ancestry {
                        found.ancestry.entry(class).or_default().extend(ancestors);
                    }
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
mod tests {
    use super::*;

    fn parallelism(threads: usize, programs: usize) -> Parallelism {
        Parallelism::new(NonZeroUsize::new(threads), programs)
    }

    #[test]
    fn a_quarter_of_the_threads_build_programs_and_share_the_rest_as_checkers() {
        assert_eq!(
            parallelism(16, 10),
            Parallelism {
                programs: 4,
                checkers: 4,
                single_threaded: false
            }
        );
        assert_eq!(
            parallelism(8, 10),
            Parallelism {
                programs: 2,
                checkers: 4,
                single_threaded: false
            }
        );
        assert_eq!(
            parallelism(3, 10),
            Parallelism {
                programs: 1,
                checkers: 3,
                single_threaded: false
            }
        );
    }

    #[test]
    fn fewer_programs_than_slots_get_more_checkers_each() {
        assert_eq!(
            parallelism(16, 1),
            Parallelism {
                programs: 1,
                checkers: 16,
                single_threaded: false
            }
        );
        assert_eq!(parallelism(16, 0).programs, 1);
    }

    #[test]
    fn one_thread_runs_tsrs_single_threaded() {
        assert_eq!(
            parallelism(1, 5),
            Parallelism {
                programs: 1,
                checkers: 1,
                single_threaded: true
            }
        );
    }

    #[test]
    fn panic_payloads_become_internal_errors() {
        let message = internal("planning the programs", &"boom");
        assert!(matches!(message, TypeError::Internal { message } if message == "planning the programs: boom"));
        let owned = internal("x", &String::from("bad"));
        assert!(matches!(owned, TypeError::Internal { message } if message == "x: bad"));
    }
}
