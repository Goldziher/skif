//! Helpers shared by the JS rules.

use std::path::PathBuf;

use skif_core::{
    DependencyKind, Diagnostic, Fix, FixOp, FixSafety, JsonAction, Location, PathSeg, RuleMeta, Span, TextEdit,
    diagnostic,
};
use skif_js_syntax::{Import, ImportKind};
use skif_lint::Emitter;
use skif_workspace::{DependencyEntry, Package};

use crate::analysis::JsAnalysis;
use crate::fixture::in_fixture_directory;
use crate::graph::{Module, Target};

/// A position in a module.
pub(super) fn module_location(module: &Module, span: Option<Span>) -> Location {
    Location::new(module.path.clone(), span)
}

/// Whether the module sits in a fixture directory, whose imports are not held against the project (see
/// [`crate::fixture::in_fixture_directory`]).
pub(super) fn is_fixture(module: &Module) -> bool {
    in_fixture_directory(&module.path)
}

/// The imports that resolve to nothing in the modules whose issues are reported, each with its module, leaving out
/// the specifiers an `ignore-unresolved` or `ignore-dependencies` entry matches. With `mocks`, only the test-runner
/// mocks (`vi.mock`, `jest.requireActual`); without, every other import.
pub(super) fn unresolved_imports<'g>(
    analysis: &'g JsAnalysis<'_>,
    mocks: bool,
) -> impl Iterator<Item = (&'g Module, &'g Import)> {
    analysis
        .graph
        .modules()
        .iter()
        .filter(move |module| !module.ignored && !is_fixture(module) && !analysis.is_skipped(module.owner))
        .flat_map(move |module| {
            module
                .imports
                .iter()
                .filter(|resolved| resolved.target == Target::Unresolved)
                .filter_map(move |resolved| module.info.imports.get(resolved.import))
                .filter(move |import| (import.kind == ImportKind::Mock) == mocks)
                .filter(move |import| !analysis.ignores_unresolved(module.owner, &import.specifier))
                .map(move |import| (module, import))
        })
}

/// A dependency entry of a manifest, from its key through its value.
pub(super) fn entry_location(package: &Package, entry: &DependencyEntry) -> Location {
    Location::new(
        package.manifest.path.clone(),
        Some(
            package
                .manifest
                .line_index()
                .span(entry.key_span.start, entry.value_span.end),
        ),
    )
}

/// Removes a dependency entry from its manifest section.
pub(super) fn remove_dependency_fix(package: &Package, entry: &DependencyEntry) -> Fix {
    Fix::new(
        format!("Remove `{}` from `{}`", entry.name, entry.kind.field()),
        FixSafety::Unsafe,
        vec![FixOp::json(
            package.manifest.path.clone(),
            vec![
                PathSeg::Key(entry.kind.field().to_owned()),
                PathSeg::Key(entry.name.clone()),
            ],
            JsonAction::Remove,
        )],
    )
}

/// Deletes a byte range of a module.
pub(super) fn delete_range(module: &Module, start: u32, end: u32) -> FixOp {
    FixOp::text(module.path.clone(), TextEdit::new(start, end, String::new()))
}

/// Reports every dependency of the given sections that nothing uses, except one on a member that only orders tasks
/// ([`JsAnalysis::only_orders_tasks`]).
pub(super) fn report_unused_dependencies(
    meta: &'static RuleMeta,
    kinds: &[DependencyKind],
    analysis: &JsAnalysis<'_>,
    out: &mut Emitter<'_>,
) {
    for (id, package) in analysis.workspace.iter() {
        if analysis.is_skipped(id) || analysis.entries_incomplete(id) {
            continue;
        }
        for entry in package.dependencies.iter().filter(|entry| kinds.contains(&entry.kind)) {
            if analysis.dependencies.is_referenced(id, &entry.name)
                || analysis.unused_dependency_exemption(id, package, entry).is_some()
            {
                continue;
            }
            out.emit(unused_dependency(meta, package, entry));
        }
    }
}

fn unused_dependency(meta: &'static RuleMeta, package: &Package, entry: &DependencyEntry) -> Diagnostic {
    diagnostic(
        meta,
        format!("`{}` is listed in `{}` but never used", entry.name, entry.kind.field()),
        entry_location(package, entry),
    )
    .package(package.display_name())
    .dependency(entry.name.clone())
    .help("remove it, or add it to `ignore-dependencies` when a tool uses it without importing it")
    .fix(remove_dependency_fix(package, entry))
    .build()
}

/// The path of a package-relative file for diagnostics that have no module.
pub(super) fn file_location(path: PathBuf) -> Location {
    Location::new(path, None)
}
