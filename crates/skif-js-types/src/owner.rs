//! The owner of one tsrs program, which gives the program's memory back when it is dropped (ADR 0045).

use tsrs_compiler::{Program, ProgramOptions};

/// A program built for one analysis. Dropping it frees the program with its checkers (built with
/// `checker_regions`) and its processed-file data; every tsrs handle read from it dangles afterwards, so only owned
/// facts may outlive it.
pub(crate) struct ProgramOwner {
    program: &'static Program,
}

impl ProgramOwner {
    /// Builds the program `options` describe.
    pub(crate) fn new(options: ProgramOptions) -> Self {
        Self {
            program: tsrs_compiler::new_program(options),
        }
    }

    /// The program, for as long as the owner lives.
    pub(crate) fn program(&self) -> &Program {
        self.program
    }
}

impl Drop for ProgramOwner {
    #[expect(
        unsafe_code,
        reason = "tsrs leaks each program from a box; only `free_unshared_program` gives its arenas back (ADR 0045)"
    )]
    fn drop(&mut self) {
        // A program dropped while a panic inside tsrs unwinds may be half updated, and a second panic while freeing
        // it would abort the host: its memory is left to the process instead.
        if std::thread::panicking() {
            return;
        }
        // SAFETY: `new_program` made the program, which never went through `update_program`, so no other program
        // shares its processed files or mapper. Every checker handle was released when `for_each_checker_group`
        // returned, and the analysis copies its facts into owned data before the owner is dropped.
        unsafe { tsrs_compiler::free_unshared_program(self.program) };
    }
}
