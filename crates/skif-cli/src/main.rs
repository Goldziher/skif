//! The `skif` binary: a thin consumer of the `skif` library.
//!
//! It is the only code that reads the environment, prints, prompts, writes files and spawns a process; it
//! returns an exit code from `main` and no function below calls `std::process::exit`.

use std::ffi::OsString;
use std::process::ExitCode;

use clap::Parser;

mod args;
mod commands;
mod env;
mod error;
mod fix;
mod install;
mod lint;
mod output;
mod write;

use args::{Cli, Command, Format};
use env::Env;
use error::{CliError, Kind};
use output::emit_error;

fn main() -> ExitCode {
    ExitCode::from(run())
}

fn run() -> u8 {
    let raw: Vec<OsString> = std::env::args_os().collect();
    let cli = match Cli::try_parse_from(&raw) {
        Ok(cli) => cli,
        Err(error) => return usage(&raw, &error),
    };
    match cli.command {
        None => lint::run(&cli.lint, &Env::detect()),
        Some(Command::Rules(args)) => commands::rules(&args),
        Some(Command::Schema) => commands::schema(),
        Some(Command::Init(args)) => commands::init(&args),
    }
}

/// Help and version go to stdout with exit 0; usage errors exit 2, as a JSON `error` document when `--format json`
/// is on the command line.
fn usage(raw: &[OsString], error: &clap::Error) -> u8 {
    if !error.use_stderr() {
        let _ = error.print();
        return 0;
    }
    if wants_json(raw) {
        let message = error.to_string();
        emit_error(&CliError::new(Kind::Usage, message.trim_end()), Format::Json);
    } else {
        let _ = error.print();
    }
    2
}

fn wants_json(raw: &[OsString]) -> bool {
    let args: Vec<&str> = raw.iter().filter_map(|arg| arg.to_str()).collect();
    args.contains(&"--format=json") || args.windows(2).any(|pair| pair[0] == "--format" && pair[1] == "json")
}
