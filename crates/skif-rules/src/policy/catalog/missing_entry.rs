use skif_core::{Category, Fixability, RuleMeta, Severity};
use skif_lint::{Emitter, LintContext, Rule};
use skif_workspace::PackageManager;

use super::analysis::{FindingKind, emit, enforce_severity, findings, use_catalog_fix};

/// Metadata of `catalog-missing-entry`.
pub static META: RuleMeta = RuleMeta {
    id: "catalog-missing-entry",
    category: Category::Catalog,
    default: Some(Severity::Warning),
    fixable: Fixability::Sometimes,
    summary: "A dependency that should come from a catalog has no catalog entry.",
};

/// Reports dependencies used in enough packages to belong in a catalog that has no entry for them.
#[derive(Debug, Clone, Copy, Default)]
pub struct CatalogMissingEntry;

impl Rule for CatalogMissingEntry {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn check(&self, cx: &LintContext<'_>, out: &mut Emitter<'_>) {
        if !cx.enabled.contains(META.id) {
            return;
        }
        let severity = enforce_severity(cx);
        for finding in findings(cx).iter(cx.workspace) {
            let name = &finding.instance.entry.name;
            match &finding.kind {
                FindingKind::MissingFromCatalog { catalog, catalog_ops } => emit(
                    out,
                    &META,
                    severity,
                    &finding,
                    format!("`{name}` has no entry in {}", catalog.display()),
                    Some(format!("add the entry and use `{}`", catalog.reference())),
                    Some(use_catalog_fix(finding.instance, catalog, catalog_ops)),
                ),
                FindingKind::MissingFromCatalogAndNonSemverMismatch => emit(
                    out,
                    &META,
                    severity,
                    &finding,
                    format!("`{name}` has no catalog entry and its specifiers differ and cannot be merged into one"),
                    Some("align the specifiers by hand, then add the entry".to_owned()),
                    None,
                ),
                FindingKind::CannotInferCatalogFile => emit(
                    out,
                    &META,
                    severity,
                    &finding,
                    format!("`{name}` has no catalog entry and skif cannot tell which file should hold the catalog"),
                    Some(match cx.workspace.package_manager {
                        Some(PackageManager::Npm) => "npm has no catalogs".to_owned(),
                        _ => "create the catalog in the file your package manager reads".to_owned(),
                    }),
                    None,
                ),
                _ => {}
            }
        }
    }
}
