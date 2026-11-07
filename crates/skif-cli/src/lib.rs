//! The `skif` command line as a library: a thin consumer of the `skif` library.
//!
//! It is the only code that reads the environment, prints, prompts, writes files, spawns a process and opens a network
//! connection (`skif update` only); [`run`] returns an exit code and no function below calls `std::process::exit`,
//! except the panic hook that [`install_panic_hook`] puts in place for the `skif` binary. The binary and the npm addon both call [`run`].

use std::ffi::OsString;

use clap::Parser;

mod args;
mod commands;
mod config;
mod config_toml;
mod env;
mod error;
mod explain;
mod fix;
mod install;
mod lint;
mod migrate;
mod npmrc;
mod output;
mod registry;
mod roots;
mod update;
mod write;

use args::{Cli, Command, Format};
use env::Env;
pub use error::{CliError, Kind};
pub use lint::lint_to_json;
use output::emit_error;

/// Runs the CLI with `args` (the first item is the program name, as in `std::env::args_os`) and returns the exit
/// code. Both the `skif` binary and the npm addon call this, so their behaviour cannot drift.
pub fn run(args: impl IntoIterator<Item = OsString>) -> u8 {
    let raw: Vec<OsString> = args.into_iter().collect();
    env::enable_timings();
    let cli = match Cli::try_parse_from(&raw) {
        Ok(cli) => cli,
        Err(error) => return usage(&raw, &error),
    };
    match cli.command {
        None => lint::run(&cli.lint, Env::detect()),
        Some(Command::Rules(args)) => commands::rules(&args),
        Some(Command::Schema) => commands::schema(),
        Some(Command::Init(args)) => commands::init(&args),
        Some(Command::Config(args)) => config::run(&args),
        Some(Command::ExplainUsage(args)) => explain::run(&args),
        Some(Command::Migrate(args)) => migrate::run(&args),
        Some(Command::Update(args)) => update::run(&args, Env::detect()),
    }
}

/// Keeps the JSON contract when skif itself panics: in `--format json` mode stdout still receives exactly one
/// `error` document (kind `internal`) and the process exits 2, instead of aborting with a bare message on stderr.
///
/// For the `skif` binary only; an embedding host owns its own panic handling. Without `--format json` the default
/// hook is left alone.
pub fn install_panic_hook(raw: &[OsString]) {
    if !wants_json(raw) {
        return;
    }
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default(info);
        emit_error(
            &CliError::new(Kind::Internal, format!("internal error: {info}")),
            Format::Json,
        );
        std::process::exit(2);
    }));
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
