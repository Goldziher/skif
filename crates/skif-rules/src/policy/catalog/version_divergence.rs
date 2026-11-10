use std::collections::{BTreeMap, BTreeSet};

use skif_core::{Category, DependencyKind, Fixability, RuleMeta, Severity, diagnostic};
use skif_lint::{Emitter, LintContext, Rule};
use skif_workspace::{DependencyEntry, Package};

use super::analysis::Claims;
use super::support::{active_packages, value_location};

/// Metadata of `catalog-version-divergence`.
pub static META: RuleMeta = RuleMeta::new(
    "catalog-version-divergence",
    Category::Catalog,
    Some(Severity::Warning),
    Fixability::None,
    "Packages take one dependency from catalogs that resolve to different versions.",
)
.version_scoped();

/// Reports `catalog:` references to one dependency that resolve to different versions, whatever the catalog mode. A
/// reference hides its version from `version-mismatch`, so this is what compares them. Running two majors side by
/// side is a common reason to have named catalogs, so the rule warns, and `[[ignore]]` with a `reason` records a
/// deliberate split. Peer dependencies state a compatible range, not an installed version, and are left out; an
/// instance a configured version group claims is that group's to judge.
#[derive(Debug, Clone, Copy, Default)]
pub struct CatalogVersionDivergence;

struct Reference<'a> {
    package: &'a Package,
    entry: &'a DependencyEntry,
    version: &'a str,
}

impl Rule for CatalogVersionDivergence {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn check(&self, cx: &LintContext<'_>, out: &mut Emitter<'_>) {
        if !cx.enabled.contains(META.id) {
            return;
        }
        let catalogs = &cx.workspace.catalogs;
        let claims = Claims::new(cx.workspace, cx.config);
        let mut by_name: BTreeMap<&str, Vec<Reference<'_>>> = BTreeMap::new();
        for package in active_packages(cx.workspace) {
            for entry in package
                .dependencies
                .iter()
                .filter(|entry| entry.parsed.is_catalog() && entry.kind != DependencyKind::PeerDependencies)
                .filter(|entry| !claims.claims(package, entry))
            {
                if let Some(version) = catalogs.resolve(&entry.name, &entry.specifier) {
                    by_name.entry(&entry.name).or_default().push(Reference {
                        package,
                        entry,
                        version,
                    });
                }
            }
        }
        for (name, references) in by_name {
            let versions: BTreeSet<&str> = references.iter().map(|reference| reference.version).collect();
            if versions.len() < 2 {
                continue;
            }
            for Reference {
                package,
                entry,
                version,
            } in references
            {
                let others = others(&versions, version);
                out.emit(
                    diagnostic(
                        &META,
                        format!(
                            "`{name}` resolves to `{version}` through `{}`; other packages get {others}",
                            entry.specifier
                        ),
                        value_location(package, entry),
                    )
                    .package(package.display_name())
                    .package_id(package.id)
                    .dependency(name.to_owned())
                    .specifier(entry.specifier.clone())
                    .detail(DETAIL)
                    .help(HELP)
                    .build(),
                );
            }
        }
    }
}

const DETAIL: &str = "DiffersToOtherCatalog";
const HELP: &str = "align the catalogs, or add an `[[ignore]]` entry with a `reason` for a deliberate split";

fn others(versions: &BTreeSet<&str>, version: &str) -> String {
    versions
        .iter()
        .filter(|other| **other != version)
        .map(|other| format!("`{other}`"))
        .collect::<Vec<_>>()
        .join(", ")
}
