use skif_core::{Category, Fix, FixSafety, Fixability, RuleMeta, Severity, diagnostic};
use skif_js_syntax::{Export, RemovalKind};
use skif_lint::Emitter;

use super::JsRule;
use super::support::{delete_range, module_location};
use crate::analysis::JsAnalysis;
use crate::graph::Module;

/// Metadata of `unused-exports`. Warning until the rule reaches corpus parity with knip; `unused-dependencies`, `unlisted-dependencies` and `unresolved-imports` stay errors.
pub static META: RuleMeta = RuleMeta {
    id: "unused-exports",
    category: Category::Js,
    default: Some(Severity::Warning),
    fixable: Fixability::Unsafe,
    summary: "An exported value is not imported by any other module.",
};

/// Reports exported values (functions, classes, constants, enums, re-exports) that no module imports.
pub struct UnusedExports;

impl JsRule for UnusedExports {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn check(&self, analysis: &JsAnalysis<'_>, out: &mut Emitter<'_>) {
        report(&META, false, analysis, out);
    }
}

/// The edit that removes `export` from the code, or `None` when removing it needs the other exports of its
/// statement to go as well and `export` is not the first of them.
fn removal_fix(module: &Module, export: &Export, unused: &[&Export]) -> Option<Fix> {
    let removal = export.removal?;
    let siblings: Vec<&&Export> = unused
        .iter()
        .filter(|other| other.removal.is_some_and(|r| r.statement == removal.statement))
        .collect();
    let all_unused = u32::try_from(siblings.len()).is_ok_and(|count| count >= removal.siblings);
    let first = siblings
        .first()
        .is_some_and(|other| other.name == export.name && other.span == export.span);
    let (start, end, title) = match removal.kind {
        RemovalKind::ExportKeyword | RemovalKind::Statement => (removal.start, removal.end, None),
        RemovalKind::SharedExportKeyword => {
            if !(all_unused && first) {
                return None;
            }
            (removal.start, removal.end, Some("Remove the shared `export` keyword"))
        }
        RemovalKind::Specifier => {
            if !all_unused {
                (removal.start, removal.end, None)
            } else if first {
                (
                    removal.statement.0,
                    removal.statement.1,
                    Some("Remove the export statement"),
                )
            } else {
                return None;
            }
        }
    };
    let title = title.map_or_else(|| format!("Remove the unused export `{}`", export.name), str::to_owned);
    Some(Fix {
        title,
        safety: FixSafety::Unsafe,
        ops: vec![delete_range(module, start, end)],
    })
}

/// Reports the unused exports of every module that are types (`want_type`) or values.
pub(super) fn report(meta: &'static RuleMeta, want_type: bool, analysis: &JsAnalysis<'_>, out: &mut Emitter<'_>) {
    for module in analysis.graph.modules() {
        let unused = analysis.unused_exports(module);
        let package = analysis.workspace.package(module.owner);
        for export in unused.iter().copied().filter(|export| export.is_type == want_type) {
            let noun = if want_type { "type" } else { "export" };
            let mut builder = diagnostic(
                meta,
                format!("{noun} `{}` is never imported by another module", export.name),
                module_location(module, Some(export.span)),
            );
            builder = if export.referenced_in_file {
                builder.help("it is only used in this file; remove the `export` keyword")
            } else {
                builder.help("remove it, or tag it `@public` when it is part of a public API")
            };
            if let Some(package) = package {
                builder = builder.package(package.display_name());
            }
            if let Some(fix) = removal_fix(module, export, &unused) {
                builder = builder.fix(fix);
            }
            out.emit(builder.build());
        }
    }
}
