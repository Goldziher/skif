//! Rules over the toolchain files beside `package.json`: version files, Dockerfiles, CI workflows and shell scripts.
//! [`ToolchainPinDrift`] checks the Node and package-manager versions they pin against the root, and
//! [`ForeignPackageManagerCommand`] the package managers their commands run.
//!
//! The rules share one scan of those files per run (see `scan`), read through the [`FileSystem`] the provider is
//! built with, so they come in their own provider: [`toolchain_provider`]. Every file kind takes `#` comments, and
//! `# skif-ignore-next-line <rule> -- <reason>` or `# skif-ignore-file <rule> -- <reason>` suppresses a finding
//! there; the `ignore-files` option suppresses a whole file from the configuration.

mod commands;
mod foreign_command;
mod options;
mod parse;
mod pin_drift;
mod scan;
mod suppress;

use std::ops::Range;
use std::sync::Arc;

use skif_core::{FileSystem, Related, RuleMeta, Severity, diagnostic};
use skif_lint::{Emitter, LintContext, Rule, RulesProvider};

pub use foreign_command::ForeignPackageManagerCommand;
pub use options::{ToolchainKind, ToolchainOptions};
pub use pin_drift::ToolchainPinDrift;

use scan::ToolchainFile;

/// Identifies the toolchain provider in errors.
pub const PROVIDER_ID: &str = "toolchain";

/// The ids of the toolchain rules; the scan reads the `files` globs of each.
fn rule_ids() -> [&'static str; 2] {
    [pin_drift::META.id, foreign_command::META.id]
}

/// The toolchain rules reading through `fs`, in the order their diagnostics are merged.
#[must_use]
pub fn rules(fs: Arc<dyn FileSystem>) -> Vec<Box<dyn Rule>> {
    vec![
        Box::new(ToolchainPinDrift::new(Arc::clone(&fs))),
        Box::new(ForeignPackageManagerCommand::new(fs)),
    ]
}

/// The provider of the toolchain rules, with id `"toolchain"`, reading the files through `fs`.
#[must_use]
pub fn toolchain_provider(fs: Arc<dyn FileSystem>) -> RulesProvider {
    RulesProvider::new(PROVIDER_ID, rules(fs))
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
