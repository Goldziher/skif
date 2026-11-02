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
    Location {
        path: module.path.clone(),
        span,
    }
}

/// A dependency entry of a manifest, from its key through its value.
pub(super) fn entry_location(package: &Package, entry: &DependencyEntry) -> Location {
    Location {
        path: package.manifest.path.clone(),
        span: Some(
            package
                .manifest
                .line_index
                .span(entry.key_span.start, entry.value_span.end),
        ),
    }
}

/// Removes a dependency entry from its manifest section.
pub(super) fn remove_dependency_fix(package: &Package, entry: &DependencyEntry) -> Fix {
    Fix {
        title: format!("Remove `{}` from `{}`", entry.name, entry.kind.field()),
        safety: FixSafety::Unsafe,
        ops: vec![FixOp::Json {
            path: package.manifest.path.clone(),
            pointer: vec![
                PathSeg::Key(entry.kind.field().to_owned()),
                PathSeg::Key(entry.name.clone()),
            ],
            action: JsonAction::Remove,
        }],
    }
}

/// Deletes a byte range of a module.
pub(super) fn delete_range(module: &Module, start: u32, end: u32) -> FixOp {
    FixOp::Text {
        path: module.path.clone(),
        edit: TextEdit {
            start,
            end,
            replacement: String::new(),
        },
    }
}

/// Reports every dependency of the given sections that nothing uses.
pub(super) fn report_unused_dependencies(
    meta: &'static RuleMeta,
    kinds: &[DependencyKind],
    analysis: &JsAnalysis<'_>,
    out: &mut Emitter<'_>,
) {
    for (id, package) in analysis.workspace.iter() {
        if analysis.is_skipped(id) {
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
    Location { path, span: None }
}
