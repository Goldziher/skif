//! The skif Node-API addon, loaded by the `skif` npm package.
//!
//! `run` is the whole CLI behind the `skif` bin; `lint` and `lintAsync` are a minimal embedding API that returns
//! the JSON report. The addon is built with unwinding panics (the `node` profile), and every export that can panic
//! turns a panic into a JavaScript exception instead of taking the host process down.

use std::ffi::OsString;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

use napi::bindgen_prelude::{AsyncTask, Result, panic_to_error};
use napi::{Env, Task};
use napi_derive::napi;
use skif::ConfigOverrides;
use skif_cli::CliError;

/// Runs the skif command line with `argv` (the arguments after the program name) and returns its exit code.
///
/// Output streams to the process's stdout and stderr exactly as the native binary does.
#[napi(catch_unwind)]
pub fn run(argv: Vec<String>) -> u32 {
    let command_line = std::iter::once(OsString::from("skif")).chain(argv.into_iter().map(OsString::from));
    u32::from(skif_cli::run(command_line))
}

/// Options for [`lint`] and [`lint_async`]; each mirrors the CLI flag of the same name.
#[napi(object)]
#[derive(Default)]
pub struct LintOptions {
    /// Skip JavaScript and TypeScript source analysis (`--no-js`).
    pub no_js: Option<bool>,
    /// Lint production dependencies only (`--production`).
    pub production: Option<bool>,
    /// Treat warnings as failures (`--fail-on-warnings`).
    pub fail_on_warnings: Option<bool>,
    /// Rule ids to turn off (`--ignore-rule`).
    pub ignore_rules: Option<Vec<String>>,
    /// Lint every workspace and standalone package at or below the path (`--recursive`).
    pub recursive: Option<bool>,
    /// Package names to skip (`--ignore-package`).
    pub ignore_packages: Option<Vec<String>>,
    /// Dependency names to skip (`--ignore-dependency`).
    pub ignore_dependencies: Option<Vec<String>>,
    /// Use this configuration file (and the files it extends) instead of discovering `skif.toml` files
    /// (`--config`). Ignored when `noConfig` is set.
    pub config: Option<String>,
    /// Read no configuration file: the defaults plus the other options (`--no-config`).
    pub no_config: Option<bool>,
}

impl LintOptions {
    fn overrides(self) -> ConfigOverrides {
        ConfigOverrides {
            ignore_rules: self.ignore_rules.unwrap_or_default(),
            ignore_packages: self.ignore_packages.unwrap_or_default(),
            ignore_dependencies: self.ignore_dependencies.unwrap_or_default(),
            fail_on_warnings: self.fail_on_warnings,
            production: self.production,
            no_js: self.no_js.unwrap_or(false),
            config_file: self.config.map(|file| {
                let file = PathBuf::from(file);
                std::path::absolute(&file).unwrap_or(file)
            }),
            no_config: self.no_config.unwrap_or(false),
            ..ConfigOverrides::default()
        }
    }
}

/// Why a lint could not run.
#[napi(object)]
pub struct LintFailure {
    /// `config`, `workspace`, `io`, `usage`, `ci-refusal` or `internal` (a bug in skif).
    pub kind: String,
    /// What went wrong.
    pub message: String,
    /// The file the failure is about, with `/` separators, when there is one.
    pub path: Option<String>,
}

impl From<CliError> for LintFailure {
    fn from(error: CliError) -> Self {
        Self {
            kind: error.kind.as_str().to_owned(),
            message: error.message,
            path: error.path,
        }
    }
}

/// The result of [`lint_async`]: exactly one of `report` and `error` is set.
#[napi(object)]
pub struct LintOutcome {
    /// The versioned JSON document the CLI prints for `--format json`.
    pub report: Option<String>,
    /// Why no report could be produced.
    pub error: Option<LintFailure>,
}

fn outcome(path: &str, overrides: &ConfigOverrides, recursive: bool) -> LintOutcome {
    match skif_cli::lint_to_json(Path::new(path), overrides, recursive) {
        Ok(report) => LintOutcome {
            report: Some(report),
            error: None,
        },
        Err(error) => LintOutcome {
            report: None,
            error: Some(error.into()),
        },
    }
}

/// Lints the workspace containing `path` (or, with `recursive`, every root at or below it) and returns the report as the versioned JSON document the CLI prints for
/// `--format json`. Nothing is printed and no file is written. The call blocks the event loop; use [`lint_async`]
/// in a server.
///
/// # Errors
/// Throws with the error message when no workspace is found, the configuration is invalid or discovery fails.
#[napi(catch_unwind)]
pub fn lint(path: String, options: Option<LintOptions>) -> Result<String> {
    let options = options.unwrap_or_default();
    let recursive = options.recursive.unwrap_or(false);
    let overrides = options.overrides();
    match outcome(&path, &overrides, recursive) {
        LintOutcome {
            report: Some(report), ..
        } => Ok(report),
        LintOutcome { error, .. } => Err(napi::Error::from_reason(
            error.map_or_else(|| "the lint produced no report".to_owned(), |failure| failure.message),
        )),
    }
}

/// The work of [`lint_async`], run on the libuv thread pool.
pub struct LintTask {
    path: String,
    overrides: ConfigOverrides,
    recursive: bool,
}

impl Task for LintTask {
    type Output = LintOutcome;
    type JsValue = LintOutcome;

    fn compute(&mut self) -> Result<LintOutcome> {
        let path = std::mem::take(&mut self.path);
        let overrides = std::mem::take(&mut self.overrides);
        let recursive = self.recursive;
        Ok(
            catch_unwind(AssertUnwindSafe(|| outcome(&path, &overrides, recursive))).unwrap_or_else(|payload| {
                LintOutcome {
                    report: None,
                    error: Some(LintFailure {
                        kind: "internal".to_owned(),
                        message: panic_to_error(payload).reason,
                        path: None,
                    }),
                }
            }),
        )
    }

    fn resolve(&mut self, _env: Env, output: LintOutcome) -> Result<LintOutcome> {
        Ok(output)
    }
}

/// Like [`lint`], without blocking the event loop. Resolves with a [`LintOutcome`]: `report` on success, or a
/// structured `error` (kind, message, path) instead of a rejection when the lint cannot run.
#[napi(ts_return_type = "Promise<LintOutcome>")]
#[must_use]
pub fn lint_async(path: String, options: Option<LintOptions>) -> AsyncTask<LintTask> {
    let options = options.unwrap_or_default();
    let recursive = options.recursive.unwrap_or(false);
    AsyncTask::new(LintTask {
        path,
        overrides: options.overrides(),
        recursive,
    })
}

/// Panics on purpose, so the test suite can prove a panic reaches JavaScript as an exception. Compiled only with
/// the `test-hooks` feature, which release builds never enable.
///
/// # Panics
///
/// Always.
#[cfg(feature = "test-hooks")]
#[napi(catch_unwind)]
#[must_use]
pub fn panic_for_test() -> u32 {
    panic!("skif test hook: deliberate panic");
}
