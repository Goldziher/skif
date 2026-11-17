//! The skif Node-API addon, loaded by the `skif` npm package.
//!
//! `run` is the whole CLI behind the `skif` bin; `lint` and `lintAsync` are a minimal embedding API that returns
//! the JSON report. The addon is built with unwinding panics (the `node` profile): `run` turns a panic into exit code 2
//! (with the JSON `error` document under `--format json`), as the native binary does, and every other export that can
//! panic turns it into a JavaScript exception instead of taking the host process down.
//!
//! `skif_cli::run` catches a panic of the command itself, so the `catch_unwind` on `run` does not fire in practice; it
//! stays as a guard for a panic outside that boundary (converting the arguments, or a payload whose drop panics),
//! which would otherwise unwind into Node.

use std::ffi::OsString;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

use napi::bindgen_prelude::{AsyncTask, Either, Result, panic_to_error};
use napi::{Env, Status, Task};
use napi_derive::napi;
use skif::ConfigOverrides;
use skif_cli::{CliError, Kind, LintCache};

/// Runs the skif command line with `argv` (the arguments after the program name) and returns its exit code.
///
/// Output streams to the process's stdout and stderr exactly as the native binary does. It is written by Rust, not
/// through `process.stdout`, so on a pipe it is not ordered with what Node writes in the same tick. A panic exits 2,
/// as it does in the binary.
#[napi(catch_unwind)]
pub fn run(argv: Vec<String>) -> u32 {
    let command_line = std::iter::once(OsString::from("skif")).chain(argv.into_iter().map(OsString::from));
    u32::from(skif_cli::run(command_line))
}

/// Where [`lint`] and [`lint_async`] keep their cache and how large it may grow.
#[napi(object)]
#[derive(Default)]
pub struct CacheOptions {
    /// The directory of the cache (`--cache-location`).
    pub location: Option<String>,
    /// The most the cache location may hold: bytes as a number, or a size such as `"256MiB"` (`--cache-max-size`).
    pub max_size: Option<Either<f64, String>>,
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
    /// The cache of what a lint computed, on by default as on the command line: `false` turns it off (`--no-cache`),
    /// `true` turns it on whatever `SKIF_CACHE` and `[cache]` say (`--cache`), and an object names its location and
    /// size. The cache never changes the report.
    pub cache: Option<Either<bool, CacheOptions>>,
    /// @deprecated Removed in skif 0.9.0 with the type-aware analysis: setting it, to either value, fails with a
    /// `usage` error.
    pub type_aware: Option<bool>,
    /// @deprecated Removed in skif 0.9.0 with the type-aware analysis: setting it, to either value, fails with a
    /// `usage` error.
    pub no_type_aware: Option<bool>,
}

/// The cache the `cache` option asks for.
///
/// # Errors
/// An empty `location`, or a `maxSize` that is not a size.
fn lint_cache(cache: Option<Either<bool, CacheOptions>>) -> std::result::Result<LintCache, String> {
    let options = match cache {
        None => return Ok(LintCache::default()),
        Some(Either::A(enabled)) => return Ok(LintCache::default().with_enabled(enabled)),
        Some(Either::B(options)) => options,
    };
    let mut cache = LintCache::default();
    if let Some(location) = options.location {
        if location.is_empty() {
            return Err("`cache.location` must name a directory; it is empty".to_owned());
        }
        let location = PathBuf::from(location);
        cache = cache.with_location(std::path::absolute(&location).unwrap_or(location));
    }
    match options.max_size {
        None => {}
        // The largest integer a JavaScript number holds exactly.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        Some(Either::A(bytes)) if bytes >= 0.0 && bytes.fract() == 0.0 && bytes <= 9_007_199_254_740_991.0 => {
            cache = cache.with_max_size(bytes as u64);
        }
        Some(Either::A(bytes)) => return Err(format!("`cache.maxSize` must be a whole number of bytes, not {bytes}")),
        Some(Either::B(text)) => {
            cache =
                cache.with_max_size(skif::parse_byte_size(&text).map_err(|error| format!("`cache.maxSize`: {error}"))?);
        }
    }
    Ok(cache)
}

impl LintOptions {
    /// The configuration overrides and the cache the options ask for.
    fn split(
        mut self,
    ) -> (
        std::result::Result<ConfigOverrides, CliError>,
        std::result::Result<LintCache, String>,
    ) {
        let cache = lint_cache(self.cache.take());
        (self.overrides(), cache)
    }

    /// The usage error for a removed option that is set, so that it fails as the removed flag does on the command
    /// line: a `#[napi(object)]` drops the properties it does not declare, which would accept the option silently.
    fn removed_option(&self) -> Option<CliError> {
        [("typeAware", self.type_aware), ("noTypeAware", self.no_type_aware)]
            .into_iter()
            .find(|(_, value)| value.is_some())
            .map(|(option, _)| {
                CliError::new(
                    Kind::Usage,
                    format!(
                        "`{option}` is no longer an option: type-aware analysis was removed in skif 0.9.0 and every \
                         member is judged by name, as it was without types; drop the option"
                    ),
                )
            })
    }

    /// The overrides the options ask for.
    ///
    /// # Errors
    /// The usage error of a removed option that is set.
    fn overrides(self) -> std::result::Result<ConfigOverrides, CliError> {
        if let Some(error) = self.removed_option() {
            return Err(error);
        }
        Ok(ConfigOverrides::default()
            .with_ignore_rules(self.ignore_rules.unwrap_or_default())
            .with_ignore_packages(self.ignore_packages.unwrap_or_default())
            .with_ignore_dependencies(self.ignore_dependencies.unwrap_or_default())
            .with_fail_on_warnings(self.fail_on_warnings)
            .with_production(self.production)
            .with_no_js(self.no_js.unwrap_or(false))
            .with_config_file(self.config.map(|file| {
                let file = PathBuf::from(file);
                std::path::absolute(&file).unwrap_or(file)
            }))
            .with_no_config(self.no_config.unwrap_or(false)))
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

fn outcome(
    path: &str,
    overrides: std::result::Result<ConfigOverrides, CliError>,
    recursive: bool,
    cache: &std::result::Result<LintCache, String>,
) -> LintOutcome {
    let linted = overrides.and_then(|overrides| match cache {
        Ok(cache) => skif_cli::lint_to_json_cached(Path::new(path), &overrides, recursive, cache),
        Err(message) => Err(CliError::new(Kind::Usage, message.clone())),
    });
    match linted {
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

/// Lints the workspace containing `path` (or, with `recursive`, every root at or below it) and returns the report as
/// the versioned JSON document the CLI prints for `--format json`, with the CLI's configuration checks. No file of
/// the project is written; unless the `cache` option or the environment turns it off, the lint reads and writes the
/// cache store as the CLI does. The warnings the CLI prints go to stderr. The call blocks the event loop; use
/// [`lint_async`] in a server.
///
/// # Errors
/// Throws an `Error` with the failure's message, and with its `kind` and, when there is one, `path` as properties
/// (the same fields [`lint_async`] returns in `error`) when no workspace is found, the configuration is invalid,
/// discovery fails or a removed option is set.
#[napi(catch_unwind)]
pub fn lint(env: Env, path: String, options: Option<LintOptions>) -> Result<String> {
    let options = options.unwrap_or_default();
    let recursive = options.recursive.unwrap_or(false);
    let (overrides, cache) = options.split();
    match outcome(&path, overrides, recursive, &cache) {
        LintOutcome {
            report: Some(report), ..
        } => Ok(report),
        LintOutcome { error, .. } => Err(throw(
            env,
            error.unwrap_or_else(|| LintFailure {
                kind: "internal".to_owned(),
                message: "the lint produced no report".to_owned(),
                path: None,
            }),
        )),
    }
}

/// Throws `failure` as a JavaScript `Error` carrying `kind` and `path`, and returns the marker that tells the
/// binding the exception is already pending. Falls back to a plain message when the error object cannot be built.
fn throw(env: Env, failure: LintFailure) -> napi::Error {
    let thrown = env
        .create_error(napi::Error::from_reason(failure.message.clone()))
        .and_then(|mut error| {
            error.set("kind", failure.kind.clone())?;
            if let Some(path) = &failure.path {
                error.set("path", path.clone())?;
            }
            env.throw(error)
        });
    match thrown {
        Ok(()) => napi::Error::new(Status::PendingException, failure.message),
        Err(_) => napi::Error::from_reason(failure.message),
    }
}

/// The work of [`lint_async`], run on the libuv thread pool.
pub struct LintTask {
    path: String,
    overrides: std::result::Result<ConfigOverrides, CliError>,
    recursive: bool,
    cache: std::result::Result<LintCache, String>,
}

impl Task for LintTask {
    type Output = LintOutcome;
    type JsValue = LintOutcome;

    fn compute(&mut self) -> Result<LintOutcome> {
        let path = std::mem::take(&mut self.path);
        let overrides = std::mem::replace(&mut self.overrides, Ok(ConfigOverrides::default()));
        let recursive = self.recursive;
        let cache = std::mem::replace(&mut self.cache, Ok(LintCache::default()));
        Ok(
            catch_unwind(AssertUnwindSafe(|| outcome(&path, overrides, recursive, &cache))).unwrap_or_else(|payload| {
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
    let (overrides, cache) = options.split();
    AsyncTask::new(LintTask {
        path,
        overrides,
        recursive,
        cache,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_cache_location_is_refused() {
        let options = |location: &str| {
            Some(Either::B(CacheOptions {
                location: Some(location.to_owned()),
                max_size: None,
            }))
        };
        let error = lint_cache(options("")).unwrap_err();
        assert!(error.contains("`cache.location`"), "{error}");
        let named = lint_cache(options("some/dir")).unwrap();
        assert!(
            named
                .location
                .is_some_and(|location| location.ends_with(Path::new("some").join("dir")))
        );
    }

    #[test]
    fn a_removed_type_aware_option_is_a_usage_error_whatever_its_value() {
        for (option, options) in [
            (
                "typeAware",
                LintOptions {
                    type_aware: Some(true),
                    ..LintOptions::default()
                },
            ),
            (
                "typeAware",
                LintOptions {
                    type_aware: Some(false),
                    ..LintOptions::default()
                },
            ),
            (
                "noTypeAware",
                LintOptions {
                    no_type_aware: Some(true),
                    ..LintOptions::default()
                },
            ),
        ] {
            let failure = LintFailure::from(options.overrides().expect_err(option));
            assert_eq!(failure.kind, "usage", "{option}");
            assert!(
                failure
                    .message
                    .starts_with(&format!("`{option}` is no longer an option")),
                "{}",
                failure.message
            );
            assert!(
                failure
                    .message
                    .contains("type-aware analysis was removed in skif 0.9.0"),
                "{}",
                failure.message
            );
        }
        assert!(LintOptions::default().overrides().is_ok());
    }
}
