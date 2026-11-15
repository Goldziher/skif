use skif_core::{Category, DependencyKind, Fixability, RuleMeta, Severity, diagnostic};
use skif_lint::Emitter;

use super::JsRule;
use super::support::{entry_location, remove_dependency_fix, report_unused_dependencies};
use crate::analysis::JsAnalysis;

/// Metadata of `unused-dev-dependencies`. Warning until the rule reaches corpus parity with knip; `unused-dependencies`, `unlisted-dependencies` and `unresolved-imports` stay errors.
pub static META: RuleMeta = RuleMeta::new(
    "unused-dev-dependencies",
    Category::Js,
    Some(Severity::Warning),
    Fixability::Unsafe,
    "A package in `devDependencies` is never imported, required or run by a script, or is also in `dependencies`.",
);

/// Reports `devDependencies` that nothing uses, and those the same manifest also lists in `dependencies`. Production
/// mode does not look at them.
pub struct UnusedDevDependencies;

impl JsRule for UnusedDevDependencies {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn check(&self, analysis: &JsAnalysis<'_>, out: &mut Emitter<'_>) {
        if analysis.production() {
            return;
        }
        report_unused_dependencies(&META, &[DependencyKind::DevDependencies], analysis, out);
        report_production_duplicates(analysis, out);
    }
}

/// Reports a used `devDependencies` entry of a package the manifest also lists in `dependencies`: the production entry
/// installs it for development as well, so the development entry changes nothing. An unused one is already reported
/// as unused.
fn report_production_duplicates(analysis: &JsAnalysis<'_>, out: &mut Emitter<'_>) {
    for (id, package) in analysis.workspace.iter() {
        if analysis.is_skipped(id) || analysis.entries_incomplete(id) {
            continue;
        }
        for entry in package.dependencies.iter().filter(|entry| {
            entry.kind == DependencyKind::DevDependencies
                && package
                    .dependencies
                    .iter()
                    .any(|other| other.kind == DependencyKind::Dependencies && other.name == entry.name)
        }) {
            if !analysis.dependencies.is_referenced(id, &entry.name) || analysis.ignores_dependency(id, &entry.name) {
                continue;
            }
            out.emit(
                diagnostic(
                    &META,
                    format!("`{}` is listed in `devDependencies` and in `dependencies`", entry.name),
                    entry_location(package, entry),
                )
                .package(package.display_name())
                .dependency(&*entry.name)
                .help("remove it from `devDependencies`: `dependencies` installs it for development as well")
                .fix(remove_dependency_fix(package, entry))
                .build(),
            );
        }
    }
}
