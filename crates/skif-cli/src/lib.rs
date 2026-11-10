//! The `skif` command line as a library: a thin consumer of the `skif` library.
//!
//! It is the only code that reads the environment, prints, prompts, writes files, spawns a process and opens a network
//! connection (`skif update` only); [`run`] returns an exit code and no function below calls `std::process::exit`,
//! except the panic hook that [`install_panic_hook`] puts in place for the `skif` binary. The npm addon calls [`run`];
//! the binary calls [`run_to_exit`], which behaves the same but leaves the memory of a run to the operating system.

use std::ffi::OsString;
use std::io::Write;
use std::cell::Cell;
use std::sync::Once;

use clap::Parser;
use serde_json::Map;

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
use output::{emit_error, emit_error_to};

/// Runs the CLI with `args` (the first item is the program name, as in `std::env::args_os`) and returns the exit
/// code. The npm addon calls this; the `skif` binary calls [`run_to_exit`], which differs only in what happens to
/// memory, so their behaviour cannot drift.
///
/// A panic in skif is caught and exits 2, as the binary's [`install_panic_hook`] makes it do; the panic hook prints
/// the message and its location to stderr, and with `--format json` stdout also receives the `error` document (kind
/// `internal`) naming both.
///
/// To learn the location, the first call chains a hook in front of the process's panic hook (`std::panic::set_hook`),
/// once per process; it records the panic for the thread that raised it and then calls the hook it replaced, so the
/// host's own handling still runs. A host that sets its own hook after that call drops the recording hook: a panic
/// still exits 2, but the `error` document then names only the panic's payload, without the location.
pub fn run(args: impl IntoIterator<Item = OsString>) -> u8 {
    let raw: Vec<OsString> = args.into_iter().collect();
    exit_two_on_panic(&raw, &mut std::io::stdout(), || run_with(raw.clone(), Teardown::Free))
}

thread_local! {
    /// The last panic of this thread seen by the hook [`record_panics`] chains in front of the host's: its message
    /// and location, which the payload that `catch_unwind` returns does not carry. Per thread, since `catch_unwind`
    /// runs on the thread that panicked and a panic elsewhere in the process must not be reported as this one.
    static LAST_PANIC: Cell<Option<String>> = const { Cell::new(None) };
}

/// Chains a hook in front of the current panic hook, once per process, that records each panic in [`LAST_PANIC`]
/// and then calls the hook it replaced, so the host's panic handling is unchanged.
fn record_panics() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            // A panic while the thread's locals are torn down goes unrecorded; the payload then describes it.
            let _ = LAST_PANIC.try_with(|last| last.set(Some(info.to_string())));
            previous(info);
        }));
    });
}

/// `body`'s exit code, or 2 when it panics, after the `internal` error document on `stdout` when `raw` asks for JSON.
/// The panic hook has already printed the message and location to stderr. The document names the location the hook
/// recorded on this thread, or only the payload when the hook did not run (a host replaced it).
fn exit_two_on_panic(raw: &[OsString], stdout: &mut dyn Write, body: impl FnOnce() -> u8) -> u8 {
    record_panics();
    LAST_PANIC.with(|last| last.set(None));
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)).unwrap_or_else(|payload| {
        let recorded = LAST_PANIC.with(Cell::take);
        if wants_json(raw) {
            let report = recorded.unwrap_or_else(|| {
                payload
                    .downcast_ref::<&str>()
                    .copied()
                    .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
                    .unwrap_or("panic")
                    .to_owned()
            });
            emit_error_to(&internal_error(&report), Format::Json, Map::new(), stdout);
        }
        2
    })
}

/// The error a panic becomes; `report` describes it: the panic hook's `panicked at <location>:` and the message, or
/// the payload alone.
fn internal_error(report: &str) -> CliError {
    CliError::new(Kind::Internal, format!("internal error: {report}"))
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

/// Makes a panic in skif exit 2 (could not run) in every format, after the default hook has printed its message and
/// location to stderr; in `--format json` mode stdout also receives exactly one `error` document (kind `internal`).
/// The release binary is built with `panic = "abort"`, and the hook runs before the abort, so the process exits 2
/// instead of being killed by `SIGABRT`.
///
/// For the `skif` binary only; an embedding host owns its own panic handling.
pub fn install_panic_hook(raw: &[OsString]) {
    let json = wants_json(raw);
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default(info);
        if json {
            emit_error(&internal_error(&info.to_string()), Format::Json);
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn a_panic_exits_two_in_every_format() {
        for (raw, json) in [(args(&["skif"]), false), (args(&["skif", "--format", "json"]), true)] {
            let mut stdout = Vec::new();
            assert_eq!(exit_two_on_panic(&raw, &mut stdout, || panic!("boom")), 2);
            if !json {
                assert!(stdout.is_empty(), "{}", String::from_utf8_lossy(&stdout));
                continue;
            }
            let document: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
            assert_eq!(document["error"]["kind"], "internal", "{document:#}");
            let message = document["error"]["message"].as_str().unwrap();
            assert!(
                message.starts_with("internal error: panicked at ")
                    && message.contains("lib.rs:")
                    && message.ends_with(":\nboom"),
                "the message names the location and the panic: {message}"
            );
        }
    }

    /// The JSON `error` message of a run of `body` that exits 2.
    fn panic_message(body: impl FnOnce() -> u8) -> String {
        let mut stdout = Vec::new();
        assert_eq!(exit_two_on_panic(&args(&["skif", "--format", "json"]), &mut stdout, body), 2);
        let document: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
        document["error"]["message"].as_str().unwrap().to_owned()
    }

    #[test]
    fn a_panic_on_another_thread_is_not_reported_as_this_one() {
        // `resume_unwind` skips the panic hook, so only the payload describes this panic; the other thread's panic
        // goes through the hook while this run is in progress.
        let message = panic_message(|| {
            let other = std::thread::spawn(|| std::panic::panic_any("another thread's panic"));
            assert!(other.join().is_err(), "the other thread panicked");
            std::panic::resume_unwind(Box::new("boom"))
        });
        assert_eq!(message, "internal error: boom");
    }

    #[test]
    fn a_panic_caught_earlier_on_this_thread_is_not_reported_again() {
        record_panics();
        let earlier = std::panic::catch_unwind(|| std::panic::panic_any("an earlier panic"));
        assert!(earlier.is_err(), "the earlier body panicked");
        let message = panic_message(|| std::panic::resume_unwind(Box::new(String::from("boom"))));
        assert_eq!(message, "internal error: boom");
    }

    #[test]
    fn without_a_panic_the_exit_code_is_the_body_s() {
        let mut stdout = Vec::new();
        assert_eq!(
            exit_two_on_panic(&args(&["skif", "--format", "json"]), &mut stdout, || 1),
            1
        );
        assert_eq!(stdout, b"");
    }
}
