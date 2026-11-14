//! The `skif` binary: [`skif_cli::run_to_exit`] with the process arguments, which catches a panic and exits 2 after
//! the default panic hook has printed it.

use std::ffi::OsString;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().collect();
    ExitCode::from(skif_cli::run_to_exit(args))
}
