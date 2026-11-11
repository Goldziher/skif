use skif_core::{Category, Fixability, RuleMeta, Severity};
use skif_lint::{Emitter, LintContext, Rule};
use skif_workspace::PackageManager;

use super::analysis::{FindingKind, emit, enforce_severity, findings, use_catalog_fix};

/// Metadata of `catalog-missing-entry`.
pub static META: RuleMeta = RuleMeta::new(
    "catalog-missing-entry",
    Category::Catalog,
    Some(Severity::Warning),
    Fixability::Sometimes,
    "A dependency that should come from a catalog has no catalog entry.",
);

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
                    Some(use_catalog_fix(cx.workspace, finding.instance, catalog, catalog_ops)),
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
                FindingKind::MissingFromCatalogAndCatalogUnknown => emit(
                    out,
                    &META,
                    severity,
                    &finding,
                    format!("no catalog holds `{name}`; more than one could"),
                    Some("add the entry to the intended catalog and reference it with `catalog:<name>`".to_owned()),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::catalog::not_used;
    use crate::testing::{config, lint};

    const LODASH: &str = r#""dependencies":{"lodash":"^4.17.21"}"#;

    /// With a default and a named catalog and the dependency in neither, it is a missing entry, not an unused one.
    #[test]
    fn a_dependency_in_none_of_several_catalogs_is_a_missing_entry() {
        let app = format!(r#"{{"name":"app","version":"1.0.0","private":true,{LODASH}}}"#);
        let lib = format!(r#"{{"name":"lib","version":"1.0.0","private":true,{LODASH}}}"#);
        let pnpm_root = r#"{"name":"root","version":"0.0.0","private":true,"packageManager":"pnpm@10.0.0"}"#;
        let pnpm_yaml = "packages: [\"packages/*\"]\ncatalog: { react: ^19.0.0 }\ncatalogs: { legacy: { react: ^18.0.0 } }\n";
        let bun_root = r#"{"name":"root","version":"0.0.0","private":true,"packageManager":"bun@1.2.0","workspaces":{"packages":["packages/*"],"catalog":{"react":"^19.0.0"},"catalogs":{"legacy":{"react":"^18.0.0"}}}}"#;
        let members = [
            ("packages/app/package.json", app.as_str()),
            ("packages/lib/package.json", lib.as_str()),
        ];
        let pnpm: Vec<(&str, &str)> = [("package.json", pnpm_root), ("pnpm-workspace.yaml", pnpm_yaml)]
            .into_iter()
            .chain(members)
            .collect();
        let bun: Vec<(&str, &str)> = std::iter::once(("package.json", bun_root)).chain(members).collect();
        let enforce = config("[catalog]\nmode = \"enforce\"\n");
        for (manager, files) in [("pnpm", pnpm), ("bun", bun)] {
            let diagnostics = lint(META.id, &files, &enforce);
            let found: Vec<(Option<&str>, &str, Option<&str>, bool)> = diagnostics
                .iter()
                .map(|d| (d.package.as_deref(), d.message.as_str(), d.detail.as_deref(), d.is_fixable()))
                .collect();
            let expected = (
                "no catalog holds `lodash`; more than one could",
                Some("MissingFromCatalogAndCatalogUnknown"),
                false,
            );
            assert_eq!(
                found,
                [
                    (Some("app"), expected.0, expected.1, expected.2),
                    (Some("lib"), expected.0, expected.1, expected.2),
                ],
                "{manager}"
            );
            assert_eq!(diagnostics[0].severity, Severity::Error, "{manager}");
            assert_eq!(lint(not_used::META.id, &files, &enforce), [], "{manager}");
        }
    }
}
