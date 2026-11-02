//! The skif Node-API addon, loaded by the `skif` npm package.
//!
//! `run` is the whole CLI behind the `skif` bin; `lint` is a minimal embedding API that returns the JSON report.

use std::error::Error;
use std::ffi::OsString;

use napi::bindgen_prelude::Result;
use napi_derive::napi;
use skif::render::{RenderOptions, render_json};
use skif::{ConfigOverrides, Linter, OsFs, Workspace};

/// Runs the skif command line with `argv` (the arguments after the program name) and returns its exit code.
///
/// Output streams to the process's stdout and stderr exactly as the native binary does.
#[napi]
pub fn run(argv: Vec<String>) -> u32 {
    let command_line = std::iter::once(OsString::from("skif")).chain(argv.into_iter().map(OsString::from));
    u32::from(skif_cli::run(command_line))
}

/// Options for [`lint`]; each mirrors the CLI flag of the same name.
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
    /// Package names to skip (`--ignore-package`).
    pub ignore_packages: Option<Vec<String>>,
    /// Dependency names to skip (`--ignore-dependency`).
    pub ignore_dependencies: Option<Vec<String>>,
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
            ..ConfigOverrides::default()
        }
    }
}

/// Lints the workspace containing `path` and returns the report as the versioned JSON document the CLI prints for
/// `--format json`. Nothing is printed and no file is written.
///
/// # Errors
/// Rejects with the error message when no workspace is found, the configuration is invalid or discovery fails.
#[napi]
pub fn lint(path: String, options: Option<LintOptions>) -> Result<String> {
    lint_json(&path, options.unwrap_or_default().overrides())
        .map_err(|error| napi::Error::from_reason(error.to_string()))
}

fn lint_json(path: &str, overrides: ConfigOverrides) -> std::result::Result<String, Box<dyn Error>> {
    let fs = OsFs;
    let start = std::fs::canonicalize(path)?;
    let root = skif::find_workspace_root(&start, &fs)?;
    let config = skif::load_config(&root, &fs, &overrides)?;
    let linter = Linter::new(&config)?;
    let workspace = Workspace::discover_with_fs(&root, &config, &fs)?;
    let report = linter.run(&workspace)?;
    let options = RenderOptions::default()
        .with_root(&workspace.root)
        .with_tool_version(env!("CARGO_PKG_VERSION"));
    let mut buffer = Vec::new();
    render_json(&report, &options, &mut buffer)?;
    Ok(String::from_utf8(buffer)?)
}
