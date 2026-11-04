//! Helpers shared by the JS rules.

use std::path::PathBuf;

use skif_core::{
    DependencyKind, Diagnostic, Fix, FixOp, FixSafety, JsonAction, Location, PathSeg, RuleMeta, Span, TextEdit,
    diagnostic,
};
use skif_lint::Emitter;
use skif_workspace::{DependencyEntry, Package};

use crate::analysis::JsAnalysis;
use crate::graph::Module;

/// A position in a module.
pub(super) fn module_location(module: &Module, span: Option<Span>) -> Location {
    Location::new(module.path.clone(), span)
}

/// Whether a directory name marks test data: `fixtures`, `__fixtures__`, `__testfixtures__` or a name ending in
/// `-fixtures` or `_fixtures`.
fn is_fixture_directory(name: &str) -> bool {
    matches!(name, "fixtures" | "__fixtures__" | "__testfixtures__")
        || name.ends_with("-fixtures")
        || name.ends_with("_fixtures")
}

/// Whether the module sits in a fixture directory (see [`is_fixture_directory`]). Fixtures are test data (often
/// deliberately broken sources), so their imports are not held against the project. Other directories of test data
/// go on `[js] ignore`.
pub(super) fn is_fixture(module: &Module) -> bool {
    module
        .path
        .components()
        .any(|part| part.as_os_str().to_str().is_some_and(is_fixture_directory))
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

/// Reports every dependency of the given sections that nothing uses.
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
            if analysis.dependencies.is_referenced(id, &entry.name) || analysis.ignores_dependency(id, &entry.name) {
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

#[cfg(test)]
mod tests {
    use super::is_fixture_directory;

    #[test]
    fn fixture_directories_are_recognised_by_name() {
        for name in [
            "fixtures",
            "__fixtures__",
            "__testfixtures__",
            "colorize-fixtures",
            "e2e_fixtures",
        ] {
            assert!(is_fixture_directory(name), "{name}");
        }
        for name in ["fixture", "src", "fixtures-api", "test_root", "my-fixtures-lib"] {
            assert!(!is_fixture_directory(name), "{name}");
        }
    }
}
