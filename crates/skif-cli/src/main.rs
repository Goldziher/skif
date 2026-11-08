//! The `skif` binary: [`skif_cli::run_to_exit`] with the process arguments.

use std::ffi::OsString;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().collect();
    skif_cli::install_panic_hook(&args);
    ExitCode::from(skif_cli::run_to_exit(args))
}
