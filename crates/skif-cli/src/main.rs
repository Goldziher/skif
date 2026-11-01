//! The `skif` binary: [`skif_cli::run`] with the process arguments.

use std::process::ExitCode;

fn main() -> ExitCode {
    ExitCode::from(skif_cli::run(std::env::args_os()))
}
