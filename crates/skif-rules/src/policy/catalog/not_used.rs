use skif_core::{Category, Fixability, RuleMeta, Severity};
use skif_lint::{Emitter, LintContext, Rule};

use super::analysis::{FindingKind, emit, enforce_severity, findings, use_catalog_fix};

/// Metadata of `catalog-not-used`.
pub static META: RuleMeta = RuleMeta::new(
    "catalog-not-used",
    Category::Catalog,
    Some(Severity::Warning),
    Fixability::Sometimes,
    "A dependency that has a catalog entry is declared with a version instead of `catalog:`.",
);

/// Reports dependencies declared with a version although a catalog defines them.
#[derive(Debug, Clone, Copy, Default)]
pub struct CatalogNotUsed;

impl Rule for CatalogNotUsed {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn check(&self, cx: &LintContext<'_>, out: &mut Emitter<'_>) {
        if !cx.enabled.contains(META.id) {
            return;
        }
        let severity = enforce_severity(cx);
        for finding in findings(cx) {
            let name = &finding.instance.entry.name;
            let specifier = &finding.instance.entry.specifier;
            match &finding.kind {
                FindingKind::NotUsingCatalog { catalog } => emit(
                    out,
                    &META,
                    severity,
                    &finding,
                    format!(
                        "`{name}` is declared as `{specifier}` but {} defines it",
                        catalog.display()
                    ),
                    Some(format!(
                        "use `{}` so every package shares one version",
                        catalog.reference()
                    )),
                    Some(use_catalog_fix(finding.instance, catalog, &[])),
                ),
                FindingKind::NotUsingCatalogAndCatalogUnknown => emit(
                    out,
                    &META,
                    severity,
                    &finding,
                    format!("`{name}` is declared as `{specifier}` and more than one catalog could hold it"),
                    Some("reference the intended catalog explicitly with `catalog:<name>`".to_owned()),
                    None,
                ),
                _ => {}
            }
        }
    }
}
