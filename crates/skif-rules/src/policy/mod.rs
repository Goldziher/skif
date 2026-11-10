//! Version and semver group policies (ADR 0009).
//!
//! The rules of this module share one analysis. [`PolicyProvider`] collects the dependency instances (`instances`) of the
//! workspace once, assigns each to its first matching version group and semver group (`groups`), derives the state
//! of every instance (`visit`) and reports the result under the few stable rule IDs of the ADR, with the syncpack
//! state name in `Diagnostic.detail`.
//!
//! The catalog, workspace-protocol and dependency-family rules run as a separate provider, [`catalog_provider`].

mod analysis;
pub(crate) mod catalog;
mod families;
mod fields;
pub(crate) mod groups;
mod inspect;
pub(crate) mod instances;
mod lockfile_bans;
mod lockfile_families;
mod report;
mod specifiers;
mod state;
mod visit;
mod workspace_protocol;

use std::collections::BTreeSet;

use skif_core::RuleMeta;
use skif_lint::{
    Emitter, EnabledRules, LintContext, OptionsError, ProviderError, RuleProvider, RulesProvider, reject_options,
};

pub use inspect::{GroupRef, InstanceStatus, StatusType, inspect};
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

    /// The policy rules take no options: their behaviour is configured through `[policy]` and the groups.
    fn validate_options(&self, _rule_id: &str, options: &toml::Table) -> Result<(), OptionsError> {
        reject_options(options)
    }

    fn run(&self, cx: &LintContext<'_>, enabled: &EnabledRules, out: &mut Emitter<'_>) -> Result<(), ProviderError> {
        if rules().iter().any(|meta| enabled.contains(meta.id)) {
            // The catalog rules own a dependency only while the rule that reports it is on; otherwise the version
            // groups keep it. Either way exactly one rule reports an instance.
            let catalog_owns = enabled.contains(catalog::owning_rule(cx.config.catalog.mode));
            // The usage index (shared with the catalog rules) and the instances do not depend on each other, so they
            // are built side by side; the dependencies the catalog rules own are flagged once both are done.
            let (usages, mut instances) = skif_workspace::join(
                cx.sequential,
                || catalog_owns.then(|| catalog::usages(cx)),
                || {
                    instances::Instances::collect_with(
                        cx.workspace,
                        &BTreeSet::new(),
                        &cx.config.custom_types,
                        cx.config.policy.override_reading,
                        &cx.config.filter,
                        cx.sequential,
                    )
                },
            );
            let owned = usages.as_deref().map_or_else(BTreeSet::new, |usages| {
                catalog::owned_dependencies(usages, cx.workspace, &cx.config.catalog)
            });
            instances.mark_catalog_owned(&owned);
            let analysis = analysis::Analysis::from_instances(cx.workspace, cx.config, instances, cx.sequential);
            report::emit(&analysis, cx.config_file(), out, cx.sequential);
        }
        if enabled.contains(BANNED_DEPENDENCY.id) {
            lockfile_bans::emit(cx, out);
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
    use skif_lint::Rule;

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

    fn catalog_files(workspace_yaml: &str) -> [(&str, &str); 4] {
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
            (
                "catalog-not-used",
                CATALOGUED,
                "[catalog]\nmode = \"enforce\"\n",
                Severity::Error,
            ),
            (
                "catalog-not-used",
                CATALOGUED,
                "[catalog]\nmode = \"prefer\"\n",
                Severity::Warning,
            ),
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
            (
                "catalog-missing-entry",
                UNCATALOGUED,
                "[catalog]\nmode = \"enforce\"\n",
                Severity::Error,
            ),
            (
                "catalog-missing-entry",
                UNCATALOGUED,
                "[catalog]\nmode = \"prefer\"\n",
                Severity::Warning,
            ),
        ];
        for (rule, yaml, toml, expected) in cases {
            let found = severities(rule, yaml, toml);
            assert_eq!(found, [expected, expected], "{rule}: {toml}");
        }
    }

    #[test]
    fn catalogued_dependencies_are_left_to_the_catalog_rules_unless_the_catalog_is_off_or_excludes_them() {
        const ROOT: &str = r#"{"name":"monorepo","version":"0.0.0","private":true,"packageManager":"pnpm@9.15.0"}"#;
        const APP: &str =
            r#"{"name":"app","version":"1.0.0","private":true,"dependencies":{"react":"^18.2.0","zod":"^3.0.0"}}"#;
        const LIB: &str =
            r#"{"name":"lib","version":"1.0.0","private":true,"dependencies":{"react":"^18.3.1","zod":"^3.1.0"}}"#;
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
    fn with_the_catalog_off_a_sibling_using_it_leaves_the_plain_instances_to_catalog_mismatch() {
        const ROOT: &str = r#"{"name":"monorepo","version":"0.0.0","private":true,"packageManager":"pnpm@9.15.0"}"#;
        const YAML: &str = "packages:\n  - \"packages/*\"\ncatalog:\n  \"react\": \"^18.3.1\"\n";
        let package = |name: &str, react: &str| {
            format!(r#"{{"name":"{name}","version":"1.0.0","private":true,"dependencies":{{"react":"{react}"}}}}"#)
        };
        let (app, lib, web) = (
            package("app", "catalog:"),
            package("lib", "^18.2.0"),
            package("web", "^18.3.1"),
        );
        let files = [
            ("package.json", ROOT),
            ("pnpm-workspace.yaml", YAML),
            ("packages/app/package.json", app.as_str()),
            ("packages/lib/package.json", lib.as_str()),
            ("packages/web/package.json", web.as_str()),
        ];
        assert_eq!(lint("version-mismatch", &files, &config("")).len(), 0);
        let mismatches = lint("catalog-mismatch", &files, &config(""));
        assert_eq!(mismatches.len(), 2);
        let fixed = apply_fixes(&files, &mismatches);
        let fixed_react = |path: &str| {
            fixed
                .iter()
                .find(|(candidate, _)| candidate == path)
                .is_some_and(|(_, text)| {
                    text.contains(r#""react":"catalog:""#) || text.contains(r#""react": "catalog:""#)
                })
        };
        assert!(fixed_react("packages/lib/package.json") && fixed_react("packages/web/package.json"));
        let mismatch_off = "[rules]\ncatalog-mismatch = \"off\"\n";
        assert_eq!(
            lint("version-mismatch", &files, &config(mismatch_off)).len(),
            1,
            "with the rule that reports them off, the version groups keep the instances"
        );
    }

    #[test]
    fn the_catalog_owns_a_dependency_only_while_its_rule_is_on() {
        let files = react_files();
        let reported = |toml: &str| lint("version-mismatch", &files, &config(toml)).len();
        let enforce = "[catalog]\nmode = \"enforce\"\n";
        assert_eq!(reported(enforce), 0, "the catalog rules report it");
        assert_eq!(
            reported(&format!("{enforce}\n[rules]\ncatalog-not-used = \"off\"\n")),
            1,
            "with the rule that reports it off, the version groups keep the dependency"
        );
    }

    const REACT_ROOT: &str = r#"{"name":"monorepo","version":"0.0.0","private":true,"packageManager":"pnpm@9.15.0"}"#;
    const REACT_APP: &str = r#"{"name":"app","version":"1.0.0","private":true,"dependencies":{"react":"^18.2.0"}}"#;
    const REACT_LIB: &str = r#"{"name":"lib","version":"1.0.0","private":true,"dependencies":{"react":"^18.3.1"}}"#;
    const REACT_YAML: &str = "packages:\n  - \"packages/*\"\ncatalog:\n  \"react\": \"^18.3.1\"\n";

    fn react_files() -> [(&'static str, &'static str); 4] {
        [
            ("package.json", REACT_ROOT),
            ("pnpm-workspace.yaml", REACT_YAML),
            ("packages/app/package.json", REACT_APP),
            ("packages/lib/package.json", REACT_LIB),
        ]
    }

    /// `(rule, package)` of everything the policy and catalog rules report, so a test sees every report on an
    /// instance at once.
    fn reports(files: &[(&str, &str)], toml: &str) -> Vec<(String, String)> {
        let config = config(toml);
        let mut found: Vec<(String, String)> = [
            "version-mismatch",
            "banned-dependency",
            "semver-range-mismatch",
            "catalog-not-used",
            "catalog-mismatch",
            "catalog-missing-entry",
            "unresolved-catalog-references",
        ]
        .into_iter()
        .flat_map(|rule| lint(rule, files, &config))
        .map(|d| (d.rule.as_str().to_owned(), d.package.unwrap_or_default()))
        .collect();
        found.sort();
        found
    }

    fn report(rule: &str, package: &str) -> (String, String) {
        (rule.to_owned(), package.to_owned())
    }

    #[test]
    fn a_version_group_that_claims_an_instance_owns_it_not_the_catalog_rules() {
        let files = react_files();
        let group = |policy: &str| {
            format!("[catalog]\nmode = \"enforce\"\n\n[[version-groups]]\ndependencies = [\"react\"]\n{policy}\n")
        };
        assert_eq!(
            reports(&files, &group("policy = \"pinned\"\npin = \"18.3.1\"")),
            [report("version-mismatch", "app"), report("version-mismatch", "lib")],
            "a pinned group governs; catalog-not-used does not also propose catalog:"
        );
        assert_eq!(
            reports(&files, &group("policy = \"ignored\"")),
            [] as [(String, String); 0],
            "an ignored group exempts the instances from the catalog rules"
        );
        assert_eq!(
            reports(&files, &group("policy = \"banned\"")),
            [report("banned-dependency", "app"), report("banned-dependency", "lib")]
        );
        assert_eq!(
            reports(&files, &group("prefer = \"lowest\"")),
            [report("version-mismatch", "lib")],
            "an aligned policy with an explicit selector governs the instances too"
        );
    }

    #[test]
    fn a_group_for_one_package_leaves_the_rest_to_the_catalog() {
        let [root, yaml, app, lib] = react_files();
        let web = r#"{"name":"web","version":"1.0.0","private":true,"dependencies":{"react":"^18.3.1"}}"#;
        let files = [root, yaml, app, lib, ("packages/web/package.json", web)];
        let toml = "[catalog]\nmode = \"enforce\"\n\n[[version-groups]]\npackages = [\"app\"]\npolicy = \"pinned\"\npin = \"18.2.0\"\n";
        assert_eq!(
            reports(&files, toml),
            [
                report("catalog-not-used", "lib"),
                report("catalog-not-used", "web"),
                report("version-mismatch", "app")
            ]
        );
    }

    #[test]
    fn a_group_naming_another_dependency_does_not_claim_the_instance() {
        let files = react_files();
        let toml = "[catalog]\nmode = \"enforce\"\n\n[[version-groups]]\ndependencies = [\"left-pad\"]\npolicy = \"ignored\"\n";
        assert_eq!(
            reports(&files, toml),
            [report("catalog-not-used", "app"), report("catalog-not-used", "lib")]
        );
    }

    #[test]
    fn a_group_with_a_negated_dependency_type_claims_the_instance_for_every_catalog_mode() {
        let files = react_files();
        for mode in ["off", "enforce"] {
            let toml = format!(
                "[[custom-types]]\nname = \"node\"\npath = \"engines.node\"\nstrategy = \"version\"\n\n[catalog]\nmode = \"{mode}\"\n\n[[version-groups]]\npackages = [\"app\"]\ndependency-types = [\"!node\"]\npolicy = \"ignored\"\n"
            );
            assert_eq!(reports(&files, &toml), [] as [(String, String); 0], "mode {mode}");
        }
    }

    #[test]
    fn a_claimed_instance_does_not_count_toward_the_catalog_thresholds() {
        let files = react_files();
        let toml = "[catalog]\nmode = \"enforce\"\n\n[[version-groups]]\npackages = [\"app\"]\npolicy = \"ignored\"\n";
        assert_eq!(
            reports(&files, toml),
            [] as [(String, String); 0],
            "one package left of the two min-packages asks for"
        );
    }

    #[test]
    fn with_the_catalog_off_an_ignored_package_is_not_a_mismatch() {
        let files = [
            ("package.json", REACT_ROOT),
            ("pnpm-workspace.yaml", REACT_YAML),
            (
                "packages/app/package.json",
                r#"{"name":"app","version":"1.0.0","private":true,"dependencies":{"react":"catalog:"}}"#,
            ),
            ("packages/lib/package.json", REACT_LIB),
        ];
        assert_eq!(reports(&files, ""), [report("catalog-mismatch", "lib")]);
        assert_eq!(
            reports(
                &files,
                "[[version-groups]]\npackages = [\"lib\"]\npolicy = \"ignored\"\n"
            ),
            [] as [(String, String); 0]
        );
    }

    #[test]
    fn an_ignored_instance_is_not_an_unresolved_reference() {
        let files = [
            ("package.json", REACT_ROOT),
            ("pnpm-workspace.yaml", REACT_YAML),
            (
                "packages/app/package.json",
                r#"{"name":"app","version":"1.0.0","private":true,"dependencies":{"react":"catalog:legacy"}}"#,
            ),
        ];
        assert_eq!(reports(&files, ""), [report("unresolved-catalog-references", "app")]);
        assert_eq!(
            reports(
                &files,
                "[[version-groups]]\npackages = [\"app\"]\npolicy = \"ignored\"\n"
            ),
            [] as [(String, String); 0]
        );
    }

    #[test]
    fn override_references_to_a_missing_catalog_entry_are_unresolved() {
        let root = r#"{"name":"monorepo","version":"0.0.0","private":true,"packageManager":"pnpm@9.15.0","overrides":{"react@>=18":"catalog:legacy","left-pad":"catalog:"}}"#;
        let files = [("package.json", root), ("pnpm-workspace.yaml", REACT_YAML)];
        let found = lint("unresolved-catalog-references", &files, &config(""));
        let mut names: Vec<_> = found.iter().filter_map(|d| d.dependency.as_deref()).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            ["left-pad", "react"],
            "the range operator in `react@>=18` is not a separator"
        );
        let react = found.iter().find(|d| d.dependency.as_deref() == Some("react")).unwrap();
        assert!(
            located(&files, react).contains("catalog:legacy"),
            "{}",
            located(&files, react)
        );
    }

    #[test]
    fn override_values_that_reference_something_else_are_not_versions() {
        let root = r#"{"name":"monorepo","version":"0.0.0","private":true,"packageManager":"pnpm@9.15.0","dependencies":{"react":"^18.3.1"},"overrides":{"react":"^18.3.1","react-dom":"$react"},"pnpm":{"overrides":{"react":"-"}}}"#;
        let files = [("package.json", root)];
        let toml = "[policy]\ndependency-types = [\"prod\", \"overrides\", \"pnpm-overrides\"]\n";
        assert_eq!(reports(&files, toml), [] as [(String, String); 0]);
    }

    #[test]
    fn the_overrides_of_a_stale_pnpm_workspace_file_are_read_only_for_pnpm() {
        let manifest = |manager: &str| {
            format!(
                r#"{{"name":"monorepo","version":"0.0.0","private":true,"packageManager":"{manager}","overrides":{{"react":"^18.3.1"}}}}"#
            )
        };
        let yaml = "packages:\n  - \"packages/*\"\noverrides:\n  react: 17.0.0\n";
        let toml = "[policy]\ndependency-types = [\"overrides\", \"pnpm-overrides\"]\n";
        let yarn = manifest("yarn@4.1.0");
        let files = [("package.json", yarn.as_str()), ("pnpm-workspace.yaml", yaml)];
        assert_eq!(reports(&files, toml), [] as [(String, String); 0]);
        let pnpm = manifest("pnpm@9.15.0");
        let files = [("package.json", pnpm.as_str()), ("pnpm-workspace.yaml", yaml)];
        assert_eq!(reports(&files, toml), [report("version-mismatch", "monorepo")]);
    }

    #[test]
    fn a_group_that_names_no_type_does_not_reach_peer_dependencies() {
        let app = r#"{"name":"app","version":"1.0.0","private":true,"dependencies":{"left-pad":"^1.3.0"},"peerDependencies":{"left-pad":"^1.3.0"}}"#;
        let root_json = root("");
        let files = [("package.json", root_json.as_str()), ("packages/app/package.json", app)];
        let banned = |types: &str| {
            let toml = format!("[[version-groups]]\ndependencies = [\"left-pad\"]\n{types}policy = \"banned\"\n");
            lint("banned-dependency", &files, &config(&toml)).len()
        };
        assert_eq!(
            banned(""),
            1,
            "the policy scope is production and development dependencies"
        );
        assert_eq!(banned("dependency-types = [\"prod\", \"peer\"]\n"), 2);
    }

    #[test]
    fn a_missing_entry_joins_the_default_catalog_where_it_is_declared() {
        let yaml = "packages:\n  - \"packages/*\"\ncatalogs:\n  default:\n    react: ^18.3.1\n";
        let app = r#"{"name":"app","version":"1.0.0","private":true,"dependencies":{"zod":"^3.23.8"}}"#;
        let lib = r#"{"name":"lib","version":"1.0.0","private":true,"dependencies":{"zod":"^3.23.8"}}"#;
        let files = [
            ("package.json", REACT_ROOT),
            ("pnpm-workspace.yaml", yaml),
            ("packages/app/package.json", app),
            ("packages/lib/package.json", lib),
        ];
        let diagnostics = lint(
            "catalog-missing-entry",
            &files,
            &config("[catalog]\nmode = \"prefer\"\n"),
        );
        assert_eq!(diagnostics.len(), 2);
        let fixed = apply_fixes(&files, &diagnostics);
        let (_, text) = fixed.iter().find(|(path, _)| path == "pnpm-workspace.yaml").unwrap();
        let document: serde_json::Value = serde_saphyr::from_str(text).unwrap();
        assert_eq!(document.get("catalog"), None, "{text}");
        assert_eq!(document["catalogs"]["default"]["zod"], "^3.23.8", "{text}");
    }

    #[test]
    fn catalog_mismatch_defaults_to_a_warning() {
        assert_eq!(catalog::CatalogMismatch.meta().default, Some(Severity::Warning));
    }

    #[test]
    fn catalog_mismatch_is_a_warning_unless_a_level_is_configured() {
        const YAML: &str = "packages:\n  - \"packages/*\"\ncatalog:\n  \"react\": \"^18.3.1\"\n";
        const USES: &str = r#"{"name":"app","version":"1.0.0","private":true,"dependencies":{"react":"catalog:"}}"#;
        const DIFFERS: &str = r#"{"name":"lib","version":"1.0.0","private":true,"dependencies":{"react":"^18.2.0"}}"#;
        const ROOT: &str = r#"{"name":"monorepo","version":"0.0.0","private":true,"packageManager":"pnpm@9.15.0"}"#;
        let files = [
            ("package.json", ROOT),
            ("pnpm-workspace.yaml", YAML),
            ("packages/app/package.json", USES),
            ("packages/lib/package.json", DIFFERS),
        ];
        let severities = |toml: &str| -> Vec<Severity> {
            lint("catalog-mismatch", &files, &config(toml))
                .iter()
                .map(|diagnostic| diagnostic.severity)
                .collect()
        };
        assert_eq!(severities(""), [Severity::Warning]);
        assert_eq!(severities("[rules]\ncatalog-mismatch = \"error\"\n"), [Severity::Error]);
    }

    #[test]
    fn the_policy_rules_reject_options() {
        let registry = skif_lint::Registry::new(vec![Box::new(policy_provider())]).unwrap();
        let bad = config("[rules]\nversion-mismatch = { severity = \"warn\", moed = \"x\" }\n");
        let error = registry.validate_config(&bad).unwrap_err().to_string();
        assert!(error.contains("version-mismatch") && error.contains("moed"), "{error}");
        registry
            .validate_config(&config("[rules]\nversion-mismatch = \"warn\"\n"))
            .unwrap();
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
        let options = skif_lint::RunOptions::default().with_config_path(std::path::Path::new("config/skif.toml"));
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
