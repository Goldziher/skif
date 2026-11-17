//! The `skif` command line as a library: a thin consumer of the `skif` library.
//!
//! It is the only code that reads the environment, prints, prompts, writes files, spawns a process and opens a network
//! connection (`skif update` only); [`run`] returns an exit code and no function below calls `std::process::exit`.
//! The npm addon calls [`run`]; the binary calls [`run_to_exit`], which also changes the current directory under
//! `bazel run` and leaves the memory of a run to the operating system.

use std::cell::Cell;
use std::ffi::OsString;
use std::io::Write;
use std::sync::Once;

use clap::Parser;
use serde_json::Map;

mod args;
mod baseline;
mod cache;
mod cache_command;
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
pub use lint::{LintCache, lint_to_json, lint_to_json_cached};
use output::{emit_error, emit_error_with};

/// Runs the CLI with `args` (the first item is the program name, as in `std::env::args_os`) and returns the exit
/// code. The npm addon calls this; the `skif` binary calls [`run_to_exit`], which differs only in what happens to
/// memory and in entering the `bazel run` directory; everything else is shared, so their behaviour cannot drift.
///
/// It never changes the process's current directory, even under `bazel run`: in a host such as Node.js, which caches
/// its own current directory, a change made here would leave the host out of step. A host that runs skif under
/// `bazel run` enters that directory itself first, as the npm package's `skif` bin does (ADR 0011).
///
/// A panic in skif that reaches this call is caught and exits 2, as in the binary; the panic hook prints the message
/// and its location to stderr, and with `--format json` or `toon` stdout also receives the `error` document (kind
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

/// The stack of each worker of rayon's global pool. A worker that waits for a parallel job takes other queued jobs
/// on its own stack, so the analyses of the roots of a recursive run nest on one worker, and with few cores that
/// outgrew the 2 MiB default. Only the pages a thread touches are committed.
const WORKER_STACK_SIZE: usize = 64 << 20;

/// Builds rayon's global pool, once per process, with 64 MiB stacks. The thread count still follows
/// `RAYON_NUM_THREADS`. A host that built the global pool first keeps its own.
pub fn configure_global_pool() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = rayon::ThreadPoolBuilder::new()
            .stack_size(WORKER_STACK_SIZE)
            .thread_name(|index| format!("skif-worker-{index}"))
            .build_global();
    });
}

/// `body`'s exit code, or 2 when it panics, after the `internal` error document on `stdout` when `raw` asks for JSON
/// or TOON.
/// The panic hook has already printed the message and location to stderr. The document names the location the hook
/// recorded on this thread, or only the payload when the hook did not run for this panic: a host replaced it, or the
/// panic was raised on another thread and resumed here, after a panic `body` caught itself was recorded.
fn exit_two_on_panic(raw: &[OsString], stdout: &mut dyn Write, body: impl FnOnce() -> u8) -> u8 {
    record_panics();
    LAST_PANIC.with(|last| last.set(None));
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body));
    skif_core::timing::flush_stderr();
    outcome.unwrap_or_else(|payload| {
        let recorded = LAST_PANIC.with(Cell::take);
        let format = error_format(raw);
        if matches!(format, Format::Json | Format::Toon) {
            let text = payload
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| payload.downcast_ref::<String>().map(String::as_str));
            let report = match (recorded, text) {
                (Some(recorded), Some(text)) if !recorded.ends_with(text) => text.to_owned(),
                (Some(recorded), _) => recorded,
                (None, text) => text.unwrap_or("panic").to_owned(),
            };
            emit_error_with(&internal_error(&report), format, Map::new(), stdout);
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
///
/// Under `bazel run` (`BUILD_WORKSPACE_DIRECTORY` is set) it first changes the process's current directory to
/// `BUILD_WORKING_DIRECTORY`, else to the workspace root, so paths on the command line resolve as the user typed them
/// rather than in the runfiles tree (ADR 0011); a directory it cannot enter exits 2.
///
/// A panic exits 2 as in [`run`]. The release binary unwinds (`panic = "unwind"`) instead of aborting, so the panic
/// reaches this call.
pub fn run_to_exit(args: impl IntoIterator<Item = OsString>) -> u8 {
    let raw: Vec<OsString> = args.into_iter().collect();
    if let Err(error) = env::enter_bazel_run_directory() {
        emit_error(&error, error_format(&raw));
        return 2;
    }
    exit_two_on_panic(&raw, &mut std::io::stdout(), || run_with(raw.clone(), Teardown::Leak))
}

fn run_with(args: impl IntoIterator<Item = OsString>, teardown: Teardown) -> u8 {
    let raw: Vec<OsString> = args.into_iter().collect();
    configure_global_pool();
    env::enable_timings();
    if let Some(error) = removed_flag(&raw) {
        emit_error(&error, error_format(&raw));
        return 2;
    }
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
        Some(Command::Cache(args)) => cache_command::run(&args),
        Some(Command::Mcp) => mcp::run(),
        Some(Command::Lsp) => lsp::run(),
    }
}

/// The flags of the type-aware analysis, which skif 0.9.0 removed.
const REMOVED_FLAGS: [&str; 2] = ["--type-aware", "--no-type-aware"];

/// The usage error for a removed flag among the options of `raw` (before a `--`), so that it says the flag was removed
/// where clap would only call it unexpected.
fn removed_flag(raw: &[OsString]) -> Option<CliError> {
    raw.iter()
        .skip(1)
        .take_while(|arg| arg.to_str() != Some("--"))
        .filter_map(|arg| arg.to_str()?.split('=').next())
        .find(|flag| REMOVED_FLAGS.contains(flag))
        .map(|flag| {
            CliError::new(
                Kind::Usage,
                format!(
                    "`{flag}` is no longer a flag: type-aware analysis was removed in skif 0.9.0 and every member is \
                     judged by name, as it was without types; drop the flag"
                ),
            )
        })
}

/// Help and version go to stdout with exit 0; usage errors exit 2, as the `error` document when `--format json` or
/// `--format toon` is on the command line.
fn usage(raw: &[OsString], error: &clap::Error) -> u8 {
    if !error.use_stderr() {
        let _ = error.print();
        return 0;
    }
    let format = error_format(raw);
    if matches!(format, Format::Json | Format::Toon) {
        let message = error.to_string();
        emit_error(&CliError::new(Kind::Usage, message.trim_end()), format);
    } else {
        let _ = error.print();
    }
    2
}

/// The format of the errors the command line asks for: `json` or `toon` when that is the format that goes to stdout,
/// or, when every `--format` names a file, the first one (ADR 0012); else human. Read from the raw arguments, before
/// clap has parsed them.
fn error_format(raw: &[OsString]) -> Format {
    let args: Vec<&str> = raw.iter().filter_map(|arg| arg.to_str()).collect();
    let mut values = Vec::new();
    for (index, arg) in args.iter().enumerate() {
        if let Some(value) = arg.strip_prefix("--format=") {
            values.push(value);
        } else if *arg == "--format" {
            values.extend(args.get(index + 1));
        }
    }
    let chosen = values
        .iter()
        .find(|value| !value.contains('='))
        .or_else(|| values.first())
        .and_then(|value| value.split('=').next());
    match chosen {
        Some("json") => Format::Json,
        Some("toon") => Format::Toon,
        _ => Format::Human,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn a_removed_type_aware_flag_is_named_before_clap_parses() {
        for flag in REMOVED_FLAGS {
            for raw in [
                args(&["skif", flag]),
                args(&["skif", "explain-usage", "a.ts", flag]),
                args(&["skif", "baseline", "write", &format!("{flag}=true")]),
            ] {
                let error = removed_flag(&raw).unwrap_or_else(|| panic!("{raw:?}"));
                assert_eq!(error.kind, Kind::Usage);
                let message = error.to_string();
                assert!(
                    message.starts_with(&format!("`{flag}` is no longer a flag")),
                    "{message}"
                );
                assert!(
                    message.contains("type-aware analysis was removed in skif 0.9.0"),
                    "{message}"
                );
            }
        }
        assert!(removed_flag(&args(&["skif", "--no-js", "."])).is_none());
        assert!(
            removed_flag(&args(&["skif", "--", "--type-aware"])).is_none(),
            "a path after `--`"
        );
    }

    #[test]
    fn a_panic_exits_two_in_every_format() {
        for (raw, format) in [
            (args(&["skif"]), Format::Human),
            (args(&["skif", "--format", "json"]), Format::Json),
            (args(&["skif", "--format", "toon"]), Format::Toon),
        ] {
            let mut stdout = Vec::new();
            assert_eq!(exit_two_on_panic(&raw, &mut stdout, || panic!("boom")), 2);
            let text = String::from_utf8_lossy(&stdout);
            let document: serde_json::Value = match format {
                Format::Json => serde_json::from_str(&text).unwrap(),
                Format::Toon => serde_toon::decode_to_value(&text).unwrap_or_else(|error| panic!("{error}: {text:?}")),
                _ => {
                    assert!(stdout.is_empty(), "{text}");
                    continue;
                }
            };
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
        assert_eq!(
            exit_two_on_panic(&args(&["skif", "--format", "json"]), &mut stdout, body),
            2
        );
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
    fn a_panic_caught_inside_the_run_does_not_end_it() {
        let mut stdout = Vec::new();
        let code = exit_two_on_panic(&args(&["skif", "--format", "json"]), &mut stdout, || {
            let fallback = std::panic::catch_unwind(|| std::panic::panic_any("the checker's panic"));
            assert!(fallback.is_err(), "the inner body panicked");
            0
        });
        assert_eq!(code, 0);
        assert_eq!(stdout, b"");
    }

    #[test]
    fn a_panic_caught_inside_the_run_is_not_reported_as_the_one_that_ended_it() {
        let message = panic_message(|| {
            let fallback = std::panic::catch_unwind(|| std::panic::panic_any("the checker's panic"));
            assert!(fallback.is_err(), "the inner body panicked");
            std::panic::resume_unwind(Box::new("a worker's panic"))
        });
        assert_eq!(message, "internal error: a worker's panic");
    }

    #[test]
    fn a_panic_of_a_rayon_worker_exits_two() {
        let message = panic_message(|| {
            rayon::scope(|scope| scope.spawn(|_| panic!("a worker's panic")));
            0
        });
        assert!(message.ends_with("a worker's panic"), "{message}");
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
