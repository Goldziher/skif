use skif_core::{Category, Fixability, RuleMeta, Severity};
use skif_lint::{Emitter, LintContext, Rule};

use super::analysis::{FindingKind, emit, findings, use_catalog_fix};

/// Metadata of `catalog-mismatch`.
pub static META: RuleMeta = RuleMeta::new(
    "catalog-mismatch",
    Category::Catalog,
    Some(Severity::Warning),
    Fixability::Sometimes,
    "A dependency is declared with a version while other packages take it from a catalog.",
);

/// Reports instances that differ from the catalog entry their siblings use. It reports only with `[catalog] mode = "off"`,
/// where the mismatch is a hint about a catalog some packages happen to use, so it is a warning; a level set for the
/// rule always wins. Under any other mode `catalog-not-used` reports the instance.
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
        for finding in findings(cx).iter(cx.workspace) {
            let FindingKind::DiffersToCatalog { catalog } = &finding.kind else {
                continue;
            };
            let name = &finding.instance.entry.name;
            let specifier = &finding.instance.entry.specifier;
            let version = catalog
                .as_ref()
                .and_then(|catalog| cx.workspace.catalogs.resolve(name, &catalog.reference()))
                .map_or_else(String::new, |version| format!(" (`{version}`)"));
            emit(
                out,
                &META,
                None,
                &finding,
                format!("`{name}` is declared as `{specifier}` while other packages use the catalog{version}"),
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
