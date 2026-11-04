use skif_core::{Category, Fixability, RuleMeta, Severity, diagnostic};
use skif_lint::{Emitter, LintContext, Rule};

use super::support::{active_packages, value_location};

/// Metadata of `unresolved-catalog-references`.
pub static META: RuleMeta = RuleMeta::new(
    "unresolved-catalog-references",
    Category::Catalog,
    Some(Severity::Error),
    Fixability::None,
    "A `catalog:` specifier names a catalog or an entry that does not exist.",
);

/// Reports `catalog:` specifiers that no catalog entry answers, whatever the catalog mode.
#[derive(Debug, Clone, Copy, Default)]
pub struct UnresolvedCatalogReferences;

impl Rule for UnresolvedCatalogReferences {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn check(&self, cx: &LintContext<'_>, out: &mut Emitter<'_>) {
        if !cx.enabled.contains(META.id) {
            return;
        }
        let catalogs = &cx.workspace.catalogs;
        for package in active_packages(cx.workspace) {
            for entry in package
                .dependencies
                .iter()
                .filter(|entry| entry.parsed.is_catalog())
                .filter(|entry| catalogs.resolve(&entry.name, &entry.specifier).is_none())
            {
                out.emit(
                    diagnostic(
                        &META,
                        format!(
                            "`{}` is declared as `{}` but no such catalog entry exists",
                            entry.name, entry.specifier
                        ),
                        value_location(package, entry),
                    )
                    .package(package.display_name())
                    .package_id(package.id)
                    .dependency(entry.name.clone())
                    .specifier(entry.specifier.clone())
                    .detail("DependsOnMissingCatalogDefinition")
                    .help("define the entry in the catalog or use a version")
                    .build(),
                );
            }
        }
    }
}
