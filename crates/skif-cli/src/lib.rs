//! The `skif` command line as a library: a thin consumer of the `skif` library.
//!
//! It is the only code that reads the environment, prints, prompts, writes files, spawns a process and opens a network
//! connection (`skif update` only); [`run`] returns an exit code and no function below calls `std::process::exit`,
//! except the panic hook that [`install_panic_hook`] puts in place for the `skif` binary. The npm addon calls [`run`];
//! the binary calls [`run_to_exit`], which behaves the same but leaves the memory of a run to the operating system.

use std::ffi::OsString;

use clap::Parser;

mod args;
mod baseline;
mod commands;
mod config;
mod config_toml;
mod env;
mod error;
mod explain;
mod filter;
mod fix;
mod format;
mod install;
mod instances;
mod lint;
mod lsp;
mod mcp;
mod migrate;
mod module_cache;
mod npmrc;
mod output;
mod registry;
mod roots;
mod trace;
mod update;
mod watch;
mod write;

use args::{Cli, Command, Format};
use env::Env;
pub use error::{CliError, Kind};
use lint::Teardown;
pub use lint::lint_to_json;
use output::emit_error;

/// Runs the CLI with `args` (the first item is the program name, as in `std::env::args_os`) and returns the exit
/// code. The npm addon calls this; the `skif` binary calls [`run_to_exit`], which differs only in what happens to
/// memory, so their behaviour cannot drift.
pub fn run(args: impl IntoIterator<Item = OsString>) -> u8 {
    run_with(args, Teardown::Free)
}

/// Like [`run`], for a process that exits as soon as this returns: the lint command leaves its diagnostics, workspace
/// model and file caches to the operating system instead of freeing them one allocation at a time, a measurable part
/// of a large run. Never call it from a long-lived host such as the npm addon: that memory would leak on every call.
pub fn run_to_exit(args: impl IntoIterator<Item = OsString>) -> u8 {
    run_with(args, Teardown::Leak)
}

fn run_with(args: impl IntoIterator<Item = OsString>, teardown: Teardown) -> u8 {
    let raw: Vec<OsString> = args.into_iter().collect();
    env::enable_timings();
    let cli = match Cli::try_parse_from(&raw) {
        Ok(cli) => cli,
        Err(error) => return usage(&raw, &error),
    };
    match cli.command {
        None if cli.lint.watch => watch::run(&cli.lint, Env::detect()),
        None => lint::run(&cli.lint, Env::detect(), teardown),
        Some(Command::Rules(args)) => commands::rules(&args),
        Some(Command::Schema) => commands::schema(),
        Some(Command::Init(args)) => commands::init(&args),
        Some(Command::Config(args)) => config::run(&args),
        Some(Command::ExplainUsage(args)) => explain::run(&args),
        Some(Command::Migrate(args)) => migrate::run(&args),
        Some(Command::Baseline(args)) => baseline::run(&args),
        Some(Command::Update(args)) => update::run(&args, Env::detect()),
        Some(Command::List(args)) => instances::list(&args),
        Some(Command::Json(args)) => instances::json(&args),
        Some(Command::Format(args)) => format::run(&args, Env::detect(), teardown),
        Some(Command::Mcp) => mcp::run(),
        Some(Command::Lsp) => lsp::run(),
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

/// Whether the command line asks for JSON-mode errors: the format that goes to stdout is `json`, or, when every
/// `--format` names a file, the first one is (ADR 0054). Read from the raw arguments, before clap has parsed them.
fn wants_json(raw: &[OsString]) -> bool {
    let args: Vec<&str> = raw.iter().filter_map(|arg| arg.to_str()).collect();
    let mut values = Vec::new();
    for (index, arg) in args.iter().enumerate() {
        if let Some(value) = arg.strip_prefix("--format=") {
            values.push(value);
        } else if *arg == "--format" {
            values.extend(args.get(index + 1));
        }
    }
    values
        .iter()
        .find(|value| !value.contains('='))
        .or_else(|| values.first())
        .is_some_and(|value| value.split('=').next() == Some("json"))
}
