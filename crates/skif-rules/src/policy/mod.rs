//! Version and semver group policies (ADR 0009).
//!
//! The rules of this module share one analysis. [`PolicyProvider`] collects the dependency [`instances`] of the
//! workspace once, assigns each to its first matching version group and semver group (`groups`), derives the state
//! of every instance (`visit`) and reports the result under the few stable rule IDs of the ADR, with the syncpack
//! state name in `Diagnostic.detail`.
//!
//! Rules that build on the same instances (catalogs, the workspace protocol, dependency families) extend
//! [`PolicyProvider::rules`] and [`PolicyProvider::run`] and reuse [`instances::Instances`].

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
            report::emit(&analysis, out);
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
    use skif_core::Diagnostic;

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
        let config =
            config("[[version-groups]]\nlabel = \"ghosts\"\ndependencies = [\"ghost\"]\npolicy = \"banned\"\n");
        let diagnostics = lint("policy-conflict", &files, &config);
        assert_eq!(details(&diagnostics), [("policy-conflict", Some("UnusedGroup"))]);
        let diagnostic = &diagnostics[0];
        assert_eq!(diagnostic.location.path.to_str(), Some("skif.toml"));
        assert!(diagnostic.package.is_none() && diagnostic.dependency.is_none() && !diagnostic.is_fixable());
        assert!(diagnostic.message.contains("ghosts"), "{}", diagnostic.message);
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
