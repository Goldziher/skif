use skif_core::{Category, Fixability, RuleMeta, Severity, diagnostic};
use skif_lint::{Emitter, LintContext, Rule};

use super::analysis::Claims;
use super::references::override_references;
use super::support::{active_packages, value_location};

/// Metadata of `unresolved-catalog-references`.
pub static META: RuleMeta = RuleMeta::new(
    "unresolved-catalog-references",
    Category::Catalog,
    Some(Severity::Error),
    Fixability::None,
    "A `catalog:` specifier names a catalog or an entry that does not exist.",
);

/// Reports `catalog:` specifiers, in dependency sections and in override sections, that no catalog entry answers,
/// whatever the catalog mode. An instance a configured version group claims is that group's to judge.
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
        let claims = Claims::new(cx.workspace, cx.config);
        for package in active_packages(cx.workspace) {
            for entry in package
                .dependencies
                .iter()
                .filter(|entry| entry.parsed.is_catalog())
                .filter(|entry| catalogs.resolve(&entry.name, &entry.specifier).is_none())
                .filter(|entry| !claims.claims(package, entry))
            {
                out.emit(
                    diagnostic(
                        &META,
                        message(&entry.name, &entry.specifier),
                        value_location(package, entry),
                    )
                    .package(package.display_name())
                    .package_id(package.id)
                    .dependency(entry.name.clone())
                    .specifier(entry.specifier.clone())
                    .detail(DETAIL)
                    .help(HELP)
                    .build(),
                );
            }
        }
        for instance in override_references(cx.workspace)
            .into_iter()
            .filter(|instance| catalogs.resolve(&instance.name, instance.raw()).is_none())
            .filter(|instance| !claims.claims_instance(instance))
        {
            out.emit(
                diagnostic(&META, message(&instance.name, instance.raw()), instance.location())
                    .package(instance.package.display_name())
                    .package_id(instance.package.id)
                    .dependency(instance.name.clone())
                    .specifier(instance.raw().to_owned())
                    .detail(DETAIL)
                    .help(HELP)
                    .build(),
            );
        }
    }
}

const DETAIL: &str = "DependsOnMissingCatalogDefinition";
const HELP: &str = "define the entry in the catalog or use a version";

fn message(name: &str, specifier: &str) -> String {
    format!("`{name}` is declared as `{specifier}` but no such catalog entry exists")
}
