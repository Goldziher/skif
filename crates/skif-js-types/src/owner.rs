//! The owner of one tsrs program, which gives the program's memory back when it is dropped (ADR 0010).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tsrs_compiler::{Program, ProgramOptions};

use crate::shared_files::ProgramFiles;

/// A program built for one analysis, or for the analyses of a run that share it ([`crate::TypePrograms`]). Dropping
/// it frees the program with its checkers (built with `checker_regions`) and its processed-file data; every tsrs
/// handle read from it dangles afterwards, so only owned facts may outlive it.
pub(crate) struct ProgramOwner {
    program: &'static Program,
    /// The project files the program read from the run (`shared_files`), let go once the program is freed.
    files: Arc<ProgramFiles>,
    /// The pool the program was built on, which frees it: the parallel work of freeing then never runs on a host's
    /// threads, whichever thread drops the last handle to a shared program.
    pool: Option<Arc<rayon::ThreadPool>>,
    /// Set when a walk of the program panicked inside tsrs.
    poisoned: AtomicBool,
}

impl ProgramOwner {
    /// Builds the program `options` describe, whose compiler host holds the project files it reads in `files`; it is
    /// freed on `pool`, or on the dropping thread without one.
    pub(crate) fn new(options: ProgramOptions, files: Arc<ProgramFiles>, pool: Option<Arc<rayon::ThreadPool>>) -> Self {
        Self {
            program: tsrs_compiler::new_program(options),
            files,
            pool,
            poisoned: AtomicBool::new(false),
        }
    }

    /// The program, for as long as the owner lives.
    pub(crate) fn program(&self) -> &Program {
        self.program
    }

    /// The project files the program read, for whatever must keep them until after the memory the program points
    /// into from beside it (its loader regions, its config) is gone.
    pub(crate) fn files(&self) -> Arc<ProgramFiles> {
        Arc::clone(&self.files)
    }

    /// Leaves the program's memory to the process when the owner drops: a walk that panicked inside tsrs may have
    /// left it half updated, and a panic while freeing it would abort the host.
    pub(crate) fn poison(&self) {
        self.poisoned.store(true, Ordering::Release);
    }

    /// Whether a walk of the program panicked.
    pub(crate) fn poisoned(&self) -> bool {
        self.poisoned.load(Ordering::Acquire)
    }
}

impl Drop for ProgramOwner {
    #[expect(
        unsafe_code,
        reason = "tsrs leaks each program from a box; only `free_unshared_program` gives its arenas back (ADR 0010)"
    )]
    fn drop(&mut self) {
        // A program dropped while a panic inside tsrs unwinds may be half updated, and a second panic while freeing
        // it would abort the host: its memory is left to the process instead.
        if std::thread::panicking() || self.poisoned() {
            return;
        }
        let program = self.program;
        // SAFETY: `new_program` made the program, which never went through `update_program`, so no other program
        // shares its processed files or mapper. Every checker handle was released when `for_each_checker_group`
        // returned, the analyses that share the program hold it through one `Arc` and drop it after their walks,
        // and each copies its facts into owned data before then. Freeing drops the program, its processed files and
        // its mapper, none of which owns a source file: every file it read through its compiler host lives in a
        // region of the run's shared files (`shared_files`), kept alive past this call by the run (the lib and
        // `node_modules` declaration files) or by this owner's and the resident's hold on the program's project files,
        // and the files it parsed for itself alone live in its loader regions, dropped after it.
        let free = move || unsafe { tsrs_compiler::free_unshared_program(program) };
        match &self.pool {
            Some(pool) => pool.install(free),
            None => free(),
        }
    }
}
