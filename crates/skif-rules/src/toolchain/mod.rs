//! Rules over the toolchain files beside `package.json`: version files, Dockerfiles, CI workflows and shell scripts.
//! [`ToolchainPinDrift`] checks the Node and package-manager versions they pin against the root, and
//! [`ForeignPackageManagerCommand`] the package managers their commands run.
//!
//! The rules share one scan of those files per run (see `scan`), read through the run's
//! [`FileSystem`](skif_core::FileSystem) ([`LintContext::fs`]), and come in their own provider, [`toolchain_provider`],
//! which [`providers`](crate::providers) includes. Every file kind takes `#` comments, and
//! `# skif-ignore-next-line <rule> -- <reason>` or `# skif-ignore-file <rule> -- <reason>` suppresses a finding
//! there; the `ignore-files` option suppresses a whole file from the configuration.

pub(crate) mod commands;
mod docker_builds;
mod foreign_command;
mod options;
mod parse;
mod pin_drift;
mod scan;
mod suppress;

use std::collections::BTreeSet;
use std::ops::Range;

use skif_config::PackageManager;
use skif_core::{Related, RuleMeta, Severity, diagnostic};
use skif_lint::{Emitter, LintContext, Rule, RulesProvider};

pub use foreign_command::ForeignPackageManagerCommand;
pub use options::{ToolchainKind, ToolchainOptions};
pub use pin_drift::ToolchainPinDrift;

use crate::paths::{file_name, join};
use parse::Parsed;
use scan::{Scan, ToolchainFile};

/// Identifies the toolchain provider in errors.
pub const PROVIDER_ID: &str = "toolchain";

/// The ids of the toolchain rules; the scan reads the `files` globs of each.
fn rule_ids() -> [&'static str; 2] {
    [pin_drift::META.id, foreign_command::META.id]
}

/// The toolchain rules, in the order their diagnostics are merged.
#[must_use]
pub fn rules() -> Vec<Box<dyn Rule>> {
    vec![Box::new(ToolchainPinDrift), Box::new(ForeignPackageManagerCommand)]
}

/// The provider of the toolchain rules, with id `"toolchain"`.
#[must_use]
pub fn toolchain_provider() -> RulesProvider {
    RulesProvider::new(PROVIDER_ID, rules())
}

/// What a Dockerfile copies from its build context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DockerfileCopies {
    /// Workspace-relative path with `/` separators.
    pub path: String,
    /// The sources of its `COPY` and `ADD` instructions that copy from the build context, as written.
    pub sources: Vec<String>,
    /// The package managers a `RUN` installs the project with after a `COPY . .` of the whole build context.
    pub installs_after_context_copy: Vec<PackageManager>,
    /// The build contexts (workspace-relative, `""` for the root) a build command of a script or workflow, or a
    /// Compose file at the root, builds the Dockerfile from.
    pub contexts: Vec<String>,
}

impl DockerfileCopies {
    fn of(path: String, parsed: &Parsed) -> Self {
        let installs_after_context_copy = parsed.context_copies.iter().min().map_or_else(Vec::new, |&copy| {
            parsed
                .commands
                .iter()
                .filter(|command| command.span().start > copy)
                .filter_map(|command| commands::installs(&command.words))
                .collect()
        });
        Self {
            path,
            sources: parsed.copied.clone(),
            installs_after_context_copy,
            contexts: Vec::new(),
        }
    }
}

/// The builds the scripts and workflows of the scan and the Compose files at the root run, as (Dockerfile, build
/// context) pairs relative to the workspace root; a path that leaves the root is left out.
fn builds(cx: &LintContext<'_>, scan: &Scan) -> BTreeSet<(String, String)> {
    let commands = scan
        .files
        .iter()
        .filter(|file| matches!(file.kind, ToolchainKind::ShellScripts | ToolchainKind::Workflows))
        .flat_map(|file| &file.parsed.commands)
        .filter_map(|command| docker_builds::command_build(&command.words));
    let compose = docker_builds::COMPOSE_FILES
        .iter()
        .filter_map(|name| cx.fs().read_to_string(&cx.workspace.root.join(name)).ok())
        .flat_map(|source| docker_builds::compose_builds(&source));
    commands
        .chain(compose)
        .filter_map(|build| {
            let context = docker_builds::normalized(&build.context)?;
            let dockerfile = build.dockerfile.unwrap_or_else(|| join(&context, "Dockerfile"));
            Some((docker_builds::normalized(&dockerfile)?, context))
        })
        .collect()
}

/// The Dockerfiles that copy from their build context: those of the scan the toolchain rules share, those directly in
/// the workspace-relative `dirs` that the scan does not read, and those a build command or Compose file names
/// (see [`builds`]), with the contexts those build them from.
pub(crate) fn dockerfile_copies(cx: &LintContext<'_>, dirs: &BTreeSet<&str>) -> Vec<DockerfileCopies> {
    let scan = Scan::of(cx, &rule_ids());
    let scanned = scan
        .files
        .iter()
        .filter(|file| file.kind == ToolchainKind::Dockerfiles)
        .map(|file| DockerfileCopies::of(file.path.clone(), &file.parsed));
    let beside = dirs
        .iter()
        .flat_map(|dir| scan.unscanned_dockerfiles(cx.fs(), &cx.workspace.root, dir))
        .map(|(path, parsed)| DockerfileCopies::of(path, &parsed));
    let mut found: Vec<DockerfileCopies> = scanned.chain(beside).collect();
    for (dockerfile, context) in builds(cx, &scan) {
        if !found.iter().any(|copies| copies.path == dockerfile)
            && let Ok(source) = cx.fs().read_to_string(&cx.workspace.root.join(&dockerfile))
        {
            let parsed = parse::parse(ToolchainKind::Dockerfiles, file_name(&dockerfile), &source);
            found.push(DockerfileCopies::of(dockerfile.clone(), &parsed));
        }
        if let Some(copies) = found.iter_mut().find(|copies| copies.path == dockerfile) {
            copies.contexts.push(context);
        }
    }
    found.retain(|copies| !copies.sources.is_empty());
    found
}

/// One finding in a toolchain file, before suppression comments are applied.
struct Finding {
    span: Range<usize>,
    message: String,
    help: String,
    detail: &'static str,
    /// The severity proposed when the rule's level is not configured.
    severity: Option<Severity>,
    related: Option<Related>,
}

/// Reports `finding` in `file` unless a suppression comment with a reason covers it; a covering comment without a
/// reason is named in the help instead.
fn emit(out: &mut Emitter<'_>, cx: &LintContext<'_>, file: &ToolchainFile, meta: &'static RuleMeta, finding: Finding) {
    let line = file.line(finding.span.start);
    let help = match suppress::find(&file.parsed.directives, meta.id, line) {
        Some(directive) if directive.reason.is_some() => return,
        Some(directive) => format!(
            "{}; the `skif-ignore` comment on line {} does not apply because it gives no reason after `--`",
            finding.help, directive.line
        ),
        None => finding.help,
    };
    let mut builder = diagnostic(meta, finding.message, file.location(&finding.span))
        .package_id(file.package)
        .help(help)
        .detail(finding.detail);
    if let Some(package) = cx.workspace.package(file.package) {
        builder = builder.package(package.display_name());
    }
    if let Some(severity) = finding.severity {
        builder = builder.default_severity(severity);
    }
    if let Some(related) = finding.related {
        builder = builder.related(related);
    }
    out.emit(builder.build());
}
