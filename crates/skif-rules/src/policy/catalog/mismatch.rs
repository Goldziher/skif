use skif_core::{Category, Fixability, RuleMeta, Severity};
use skif_lint::{Emitter, LintContext, Rule};

use super::analysis::{FindingKind, emit, findings, use_catalog_fix};

/// Metadata of `catalog-mismatch`.
pub static META: RuleMeta = RuleMeta {
    id: "catalog-mismatch",
    category: Category::Catalog,
    default: Some(Severity::Error),
    fixable: Fixability::Sometimes,
    summary: "A dependency is declared with a version while other packages take it from a catalog.",
};

/// Reports instances that differ from the catalog entry their siblings use.
#[derive(Debug, Clone, Copy, Default)]
pub struct CatalogMismatch;

impl Rule for CatalogMismatch {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn check(&self, cx: &LintContext<'_>, out: &mut Emitter<'_>) {
        if !cx.enabled.contains(META.id) {
            return;
        }
        for finding in findings(cx) {
            let FindingKind::DiffersToCatalog { catalog } = &finding.kind else {
                continue;
            };
            let name = &finding.instance.entry.name;
            let specifier = &finding.instance.entry.specifier;
            emit(
                out,
                &META,
                None,
                &finding,
                format!("`{name}` is declared as `{specifier}` while other packages use the catalog"),
                Some(match catalog {
                    Some(catalog) => format!("use `{}`", catalog.reference()),
                    None => "other packages use different catalogs; pick one".to_owned(),
                }),
                catalog
                    .as_ref()
                    .map(|catalog| use_catalog_fix(finding.instance, catalog, &[])),
            );
        }
    }
}
