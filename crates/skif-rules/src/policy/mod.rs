//! Version and semver group policies (ADR 0009).
//!
//! The rules of this module share one analysis. [`PolicyProvider`] collects the dependency instances (`instances`) of the
//! workspace once, assigns each to its first matching version group and semver group (`groups`), derives the state
//! of every instance (`visit`) and reports the result under the few stable rule IDs of the ADR, with the syncpack
//! state name in `Diagnostic.detail`.
//!
//! The catalog, workspace-protocol and dependency-family rules run as a separate provider, [`catalog_provider`].

mod analysis;
mod catalog;
mod families;
mod groups;
pub(crate) mod instances;
mod report;
mod state;
mod visit;
mod workspace_protocol;

use skif_core::RuleMeta;
use skif_lint::{Emitter, EnabledRules, LintContext, ProviderError, RuleProvider, RulesProvider};

pub use state::{BANNED_DEPENDENCY, INVALID_LOCAL_VERSION, POLICY_CONFLICT, SEMVER_RANGE_MISMATCH, VERSION_MISMATCH};

/// Identifies the policy provider in errors.
pub const PROVIDER_ID: &str = "policy";

/// The rules of the version-group and semver-group policies.
#[must_use]
pub fn rules() -> Vec<&'static RuleMeta> {
    vec![
        &VERSION_MISMATCH,
        &BANNED_DEPENDENCY,
        &SEMVER_RANGE_MISMATCH,
        &POLICY_CONFLICT,
        &INVALID_LOCAL_VERSION,
    ]
}

/// Runs the policy rules over one shared analysis of the workspace.
#[derive(Debug, Clone, Copy, Default)]
pub struct PolicyProvider;

impl RuleProvider for PolicyProvider {
    fn id(&self) -> &'static str {
        PROVIDER_ID
    }

    fn rules(&self) -> Vec<&'static RuleMeta> {
        rules()
    }

    fn run(&self, cx: &LintContext<'_>, enabled: &EnabledRules, out: &mut Emitter<'_>) -> Result<(), ProviderError> {
        if rules().iter().any(|meta| enabled.contains(meta.id)) {
            let analysis = analysis::Analysis::run(cx.workspace, cx.config);
            report::emit(&analysis, cx.config_file(), out);
        }
        Ok(())
    }
}

/// The provider of the policy rules, with id `"policy"`.
#[must_use]
pub const fn policy_provider() -> PolicyProvider {
    PolicyProvider
}

/// Identifies the catalog, workspace-protocol and dependency-family rules in errors.
pub const CATALOG_PROVIDER_ID: &str = "catalog";

/// The catalog, workspace-protocol and dependency-family rules, with id `"catalog"`.
#[must_use]
pub fn catalog_provider() -> RulesProvider {
    let mut rules = catalog::rules();
    rules.extend(workspace_protocol::rules());
    rules.extend(families::rules());
    RulesProvider::new(CATALOG_PROVIDER_ID, rules)
}

#[cfg(test)]
mod tests {
    use skif_core::{Diagnostic, Severity};

    use super::*;
    use crate::testing::{apply_fixes, borrowed, config, lint, located, root};

    const APP: &str = r#"{"name":"app","version":"1.0.0","dependencies":{"react":"^18.2.0","left-pad":"^1.3.0"}}"#;
    const LIB: &str = r#"{"name":"lib","version":"1.0.0","dependencies":{"react":"^18.3.1"}}"#;

    fn files(root_json: &str) -> [(&str, &str); 3] {
        [
            ("package.json", root_json),
            ("packages/app/package.json", APP),
            ("packages/lib/package.json", LIB),
        ]
    }

    fn details(diagnostics: &[Diagnostic]) -> Vec<(&str, Option<&str>)> {
        diagnostics
            .iter()
            .map(|d| (d.rule.as_str(), d.detail.as_deref()))
            .collect()
    }

    fn catalog_files<'a>(workspace_yaml: &'a str) -> [(&'a str, &'a str); 4] {
        const ROOT: &str = r#"{"name":"monorepo","version":"0.0.0","private":true,"packageManager":"pnpm@9.15.0"}"#;
        const USER: &str = r#"{"name":"app","version":"1.0.0","private":true,"dependencies":{"react":"^18.3.1"}}"#;
        const OTHER: &str = r#"{"name":"lib","version":"1.0.0","private":true,"dependencies":{"react":"^18.3.1"}}"#;
        [
            ("package.json", ROOT),
            ("pnpm-workspace.yaml", workspace_yaml),
            ("packages/app/package.json", USER),
            ("packages/lib/package.json", OTHER),
        ]
    }

    fn severities(rule: &str, workspace_yaml: &str, toml: &str) -> Vec<Severity> {
        lint(rule, &catalog_files(workspace_yaml), &config(toml))
            .iter()
            .map(|diagnostic| diagnostic.severity)
            .collect()
    }

    #[test]
    fn catalog_usage_rules_are_errors_under_enforce_unless_a_level_is_configured() {
        const CATALOGUED: &str = "packages:\n  - \"packages/*\"\ncatalog:\n  \"react\": \"^18.3.1\"\n";
        const UNCATALOGUED: &str = "packages:\n  - \"packages/*\"\n";
        let cases = [
            ("catalog-not-used", CATALOGUED, "[catalog]\nmode = \"enforce\"\n", Severity::Error),
            ("catalog-not-used", CATALOGUED, "[catalog]\nmode = \"prefer\"\n", Severity::Warning),
            (
                "catalog-not-used",
                CATALOGUED,
                "[catalog]\nmode = \"enforce\"\n[rules]\ncatalog-not-used = \"warn\"\n",
                Severity::Warning,
            ),
            (
                "catalog-not-used",
                CATALOGUED,
                "[catalog]\nmode = \"enforce\"\n[rules]\ncatalog-not-used = \"info\"\n",
                Severity::Info,
            ),
            ("catalog-missing-entry", UNCATALOGUED, "[catalog]\nmode = \"enforce\"\n", Severity::Error),
            ("catalog-missing-entry", UNCATALOGUED, "[catalog]\nmode = \"prefer\"\n", Severity::Warning),
        ];
        for (rule, yaml, toml, expected) in cases {
            let found = severities(rule, yaml, toml);
            assert_eq!(found, [expected, expected], "{rule}: {toml}");
        }
    }

    #[test]
    fn catalogued_dependencies_are_left_to_the_catalog_rules_unless_the_catalog_is_off_or_excludes_them() {
        const ROOT: &str = r#"{"name":"monorepo","version":"0.0.0","private":true,"packageManager":"pnpm@9.15.0"}"#;
        const APP: &str = r#"{"name":"app","version":"1.0.0","private":true,"dependencies":{"react":"^18.2.0","zod":"^3.0.0"}}"#;
        const LIB: &str = r#"{"name":"lib","version":"1.0.0","private":true,"dependencies":{"react":"^18.3.1","zod":"^3.1.0"}}"#;
        const YAML: &str = "packages:\n  - \"packages/*\"\ncatalog:\n  \"react\": \"^18.3.1\"\n";
        let files = [
            ("package.json", ROOT),
            ("pnpm-workspace.yaml", YAML),
            ("packages/app/package.json", APP),
            ("packages/lib/package.json", LIB),
        ];
        let mismatched = |toml: &str| -> Vec<Option<String>> {
            lint("version-mismatch", &files, &config(toml))
                .into_iter()
                .map(|d| d.dependency)
                .collect()
        };
        let both = || vec![Some("react".to_owned()), Some("zod".to_owned())];
        let zod = || vec![Some("zod".to_owned())];
        assert_eq!(mismatched(""), both(), "catalog mode off: the policy aligns everything");
        assert_eq!(mismatched("[catalog]\nmode = \"prefer\"\n"), zod());
        assert_eq!(mismatched("[catalog]\nmode = \"enforce\"\n"), zod());
        assert_eq!(
            mismatched("[catalog]\nmode = \"prefer\"\nexclude = [\"react\"]\n"),
            both(),
            "an excluded dependency is not owned by the catalog"
        );
        assert_eq!(
            mismatched("[catalog]\nmode = \"prefer\"\nmin-packages = 3\n"),
            both(),
            "below min-packages the catalog rules do not report the dependency"
        );
    }

    #[test]
    fn the_rule_ids_are_the_adr_set() {
        let ids: Vec<&str> = rules().iter().map(|meta| meta.id).collect();
        assert_eq!(
            ids,
            [
                "version-mismatch",
                "banned-dependency",
                "semver-range-mismatch",
                "policy-conflict",
                "invalid-local-version",
            ]
        );
        assert_eq!(policy_provider().id(), "policy");
    }

    #[test]
    fn diagnostics_name_the_state_and_locate_the_entry() {
        let root_json = root("");
        let files = files(&root_json);
        let diagnostics = lint("version-mismatch", &files, &config(""));
        assert_eq!(
            details(&diagnostics),
            [("version-mismatch", Some("DiffersToHighestOrLowestSemver"))]
        );
        let diagnostic = &diagnostics[0];
        assert_eq!(diagnostic.package.as_deref(), Some("app"));
        assert_eq!(diagnostic.dependency.as_deref(), Some("react"));
        assert_eq!(located(&files, diagnostic), r#""react":"^18.2.0""#);
        assert!(diagnostic.message.contains("react@^18.2.0"), "{}", diagnostic.message);
    }

    #[test]
    fn every_diagnostic_names_the_package_id_of_its_package() {
        let root_json = root("");
        let app = r#"{"name":"app","version":"1.0.0","dependencies":{"lib":"^1.0.0","react":"^18.0.0","react-dom":"^17.0.0","zod":"catalog:"}}"#;
        let lib = r#"{"name":"lib","version":"1.0.0","dependencies":{"react":"^18.3.1"}}"#;
        let files = [
            ("package.json", root_json.as_str()),
            ("packages/app/package.json", app),
            ("packages/lib/package.json", lib),
        ];
        let config = config("[workspace-protocol]\ninternal = true\n");
        for rule in [
            "version-mismatch",
            "workspace-protocol",
            "unsynced-dependency-family",
            "unresolved-catalog-references",
        ] {
            let diagnostics = lint(rule, &files, &config);
            assert!(!diagnostics.is_empty(), "{rule} reported nothing");
            assert!(
                diagnostics.iter().all(|d| d.package_id.is_some()),
                "{rule} has a diagnostic without a package id"
            );
        }
    }

    #[test]
    fn fixes_write_the_chosen_specifier_and_converge() {
        let root_json = root("");
        let files = files(&root_json);
        let diagnostics = lint("version-mismatch", &files, &config(""));
        assert!(diagnostics.iter().all(Diagnostic::is_fixable));
        let fixed = apply_fixes(&files, &diagnostics);
        let fixed = borrowed(&fixed);
        assert_eq!(
            lint("version-mismatch", &fixed, &config("")),
            [] as [skif_core::Diagnostic; 0]
        );
        let app = fixed
            .iter()
            .find(|(path, _)| path.ends_with("app/package.json"))
            .unwrap()
            .1;
        assert!(app.contains(r#""react":"^18.3.1""#), "{app}");
    }

    #[test]
    fn banned_dependencies_are_removed_by_the_fix() {
        let root_json = root("");
        let files = files(&root_json);
        let config = config("[[version-groups]]\ndependencies = [\"left-pad\"]\npolicy = \"banned\"\n");
        let diagnostics = lint("banned-dependency", &files, &config);
        assert_eq!(details(&diagnostics), [("banned-dependency", Some("IsBanned"))]);
        let fixed = apply_fixes(&files, &diagnostics);
        let app = fixed
            .iter()
            .find(|(path, _)| path.ends_with("app/package.json"))
            .unwrap();
        assert!(!app.1.contains("left-pad"), "{}", app.1);
    }

    #[test]
    fn unused_groups_are_reported_on_the_configuration_file() {
        let root_json = root("");
        let files = files(&root_json);
        let config = config(
            "[[version-groups]]\nlabel = \"ghosts\"\ndependencies = [\"ghost\"]\npolicy = \"pinned\"\npin = \"1.0.0\"\n",
        );
        let diagnostics = lint("policy-conflict", &files, &config);
        assert_eq!(details(&diagnostics), [("policy-conflict", Some("UnusedGroup"))]);
        let diagnostic = &diagnostics[0];
        assert_eq!(diagnostic.location.path.to_str(), Some("skif.toml"));
        assert!(diagnostic.package.is_none() && diagnostic.dependency.is_none() && !diagnostic.is_fixable());
        assert!(diagnostic.message.contains("ghosts"), "{}", diagnostic.message);
    }

    #[test]
    fn group_diagnostics_point_at_the_configuration_file_the_host_names() {
        let root_json = root("");
        let files = files(&root_json);
        let config = config(
            "[[version-groups]]\ndependencies = [\"ghost\"]\npolicy = \"pinned\"\npin = \"1.0.0\"\n\n[[version-groups]]\ndependencies = [\"react\"]\npolicy = \"pinned\"\npin = \"\"\n",
        );
        let options = skif_lint::RunOptions {
            config_path: Some(std::path::Path::new("config/skif.toml")),
            ..skif_lint::RunOptions::default()
        };
        let named = crate::testing::lint_with("policy-conflict", &files, &config, &options);
        assert!(named.iter().any(|d| d.detail.as_deref() == Some("UnusedGroup")));
        assert!(named.iter().any(|d| d.detail.as_deref() == Some("InvalidPin")));
        assert!(
            named
                .iter()
                .all(|diagnostic| diagnostic.location.path.to_str() == Some("config/skif.toml")),
            "{named:#?}"
        );
        let unnamed = lint("policy-conflict", &files, &config);
        assert!(
            unnamed
                .iter()
                .all(|diagnostic| diagnostic.location.path.to_str() == Some("skif.toml")),
            "{unnamed:#?}"
        );
    }

    #[test]
    fn a_banned_group_that_matches_nothing_is_not_reported() {
        let root_json = root("");
        let files = files(&root_json);
        let config = config("[[version-groups]]\ndependencies = [\"ghost\"]\npolicy = \"banned\"\n");
        assert_eq!(details(&lint("policy-conflict", &files, &config)), []);
    }

    #[test]
    fn a_mismatch_points_at_the_instance_it_should_align_with() {
        let root_json = root("");
        let files = files(&root_json);
        let diagnostics = lint("version-mismatch", &files, &config(""));
        let related = &diagnostics[0].related;
        assert_eq!(related.len(), 1);
        assert_eq!(related[0].location.path.to_str(), Some("packages/lib/package.json"));
        assert_eq!(
            crate::testing::slice(&files, "packages/lib/package.json", related[0].location.span.unwrap()),
            r#""react":"^18.3.1""#
        );
    }

    #[test]
    fn an_unusable_pin_is_a_policy_conflict_and_changes_nothing() {
        let root_json = root("");
        let files = files(&root_json);
        for pin in ["", "   "] {
            let config = config(&format!(
                "[[version-groups]]\ndependencies = [\"react\"]\npolicy = \"pinned\"\npin = \"{pin}\"\n"
            ));
            let diagnostics = lint("policy-conflict", &files, &config);
            assert_eq!(
                details(&diagnostics),
                [("policy-conflict", Some("InvalidPin"))],
                "{pin:?}"
            );
            assert!(!diagnostics[0].is_fixable());
            assert_eq!(
                lint("version-mismatch", &files, &config),
                [] as [Diagnostic; 0],
                "{pin:?}"
            );
        }
    }

    #[test]
    fn rules_can_be_turned_off_individually() {
        let root_json = root("");
        let files = files(&root_json);
        let off = config("[rules]\nversion-mismatch = \"off\"\n");
        assert_eq!(lint("version-mismatch", &files, &off), [] as [skif_core::Diagnostic; 0]);
    }

    #[test]
    fn severities_are_per_rule() {
        let root_json = root("");
        let files = files(&root_json);
        let raised = config("[rules]\nversion-mismatch = \"warn\"\n");
        let diagnostics = lint("version-mismatch", &files, &raised);
        assert_eq!(diagnostics[0].severity, skif_core::Severity::Warning);
    }
}
