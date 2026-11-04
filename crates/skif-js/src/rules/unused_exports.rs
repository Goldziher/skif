use std::collections::{HashMap, HashSet};

use skif_core::{Category, Fix, FixSafety, Fixability, RuleMeta, Severity, diagnostic};
use skif_js_syntax::Export;
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
        report(&META, Scope::Imports, false, analysis, out);
    }
}

/// The deletion that fixes each unused export, keyed by the export's address in the module.
///
/// The deletions come from [`skif_js_syntax::ModuleInfo::removal_edits`], so they never overlap. Each one is
/// attached to the first unused export it removes; the others it removes get no fix of their own. Exports whose
/// removal would delete an expression (`export default <expression>`) get no fix.
fn fixes(module: &Module, unused: &[&Export]) -> HashMap<*const Export, (u32, u32)> {
    let wanted: HashSet<*const Export> = unused
        .iter()
        .filter(|export| export.removal.is_some_and(|removal| !removal.default_expression))
        .map(|export| std::ptr::from_ref::<Export>(export))
        .collect();
    let edits = module
        .info
        .removal_edits(|export| wanted.contains(&std::ptr::from_ref::<Export>(export)));
    let mut claimed = vec![false; edits.len()];
    let mut fixes = HashMap::new();
    for export in unused {
        let Some(removal) = export.removal else { continue };
        if removal.default_expression {
            continue;
        }
        let overlapping = edits
            .iter()
            .position(|(start, end)| *start < removal.end && removal.start < *end);
        if let Some(position) = overlapping
            && !claimed[position]
        {
            claimed[position] = true;
            fixes.insert(std::ptr::from_ref::<Export>(export), edits[position]);
        }
    }
    fixes
}

fn removal_fix(module: &Module, export: &Export, range: (u32, u32)) -> Fix {
    let whole_statement = export.removal.is_some_and(|removal| removal.statement == range);
    let title = if whole_statement {
        "Remove the export statement".to_owned()
    } else {
        format!("Remove the unused export `{}`", export.name)
    };
    Fix {
        title,
        safety: FixSafety::Unsafe,
        ops: vec![delete_range(module, range.0, range.1)],
    }
}

/// Which modules a rule reports the unused exports of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Scope {
    /// Modules some other module imports by name, or uses as a whole, or nobody uses.
    Imports,
    /// Modules that are only read as a namespace (`ns.member`).
    Namespaces,
}

/// Reports the unused exports of every module in `scope` that are types (`want_type`) or values.
pub(super) fn report(
    meta: &'static RuleMeta,
    scope: Scope,
    want_type: bool,
    analysis: &JsAnalysis<'_>,
    out: &mut Emitter<'_>,
) {
    for module in analysis.graph.modules() {
        if analysis.exports.is_namespace_only(module.id) != (scope == Scope::Namespaces) {
            continue;
        }
        let unused = analysis.unused_exports(module);
        let fixes = fixes(module, &unused);
        let package = analysis.workspace.package(module.owner);
        for export in unused.iter().copied().filter(|export| export.is_type == want_type) {
            let noun = if want_type { "type" } else { "export" };
            let message = match scope {
                Scope::Imports => format!("{noun} `{}` is never imported by another module", export.name),
                Scope::Namespaces => format!("{noun} `{}` is never read through a namespace import", export.name),
            };
            let mut builder = diagnostic(meta, message, module_location(module, Some(export.span)));
            builder = if export.referenced_in_file {
                builder.help("it is only used in this file; remove the `export` keyword")
            } else {
                builder.help("remove it, or tag it `@public` when it is part of a public API")
            };
            if let Some(package) = package {
                builder = builder.package(package.display_name());
            }
            if let Some(range) = fixes.get(&std::ptr::from_ref::<Export>(export)) {
                builder = builder.fix(removal_fix(module, export, *range));
            }
            out.emit(builder.build());
        }
    }
}
