#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{FixedRule, config, item, leak_meta, workspace};
use skif_core::Severity;
use skif_lint::{
    EnabledRules, LintError, ProviderError, Registry, RegistryError, Rule, RuleProvider, RulesProvider, RunOptions, run,
};

fn provider(id: &'static str, rule_ids: &[&str]) -> Box<dyn RuleProvider> {
    let rules = rule_ids
        .iter()
        .map(|rule| {
            Box::new(FixedRule {
                meta: leak_meta(rule, Some(Severity::Warning)),
                items: Vec::new(),
            }) as Box<dyn Rule>
        })
        .collect();
    Box::new(RulesProvider::new(id, rules))
}

#[test]
fn duplicate_rule_ids_are_rejected() {
    type Case = (Vec<Box<dyn RuleProvider>>, Option<&'static str>);
    let cases: [Case; 4] = [
        (vec![provider("p", &["a", "b"])], None),
        (vec![provider("p", &["a"]), provider("q", &["b"])], None),
        (vec![provider("p", &["a", "a"])], Some("a")),
        (vec![provider("p", &["a", "b"]), provider("q", &["c", "b"])], Some("b")),
    ];
    for (providers, duplicate) in cases {
        match (Registry::new(providers), duplicate) {
            (Ok(registry), None) => assert!(registry.rules().count() >= 2),
            (Err(err), Some(id)) => assert_eq!(err, RegistryError::DuplicateRule { id: id.to_owned() }),
            (Ok(_), Some(id)) => panic!("expected duplicate {id}"),
            (Err(err), None) => panic!("unexpected {err}"),
        }
    }
}

#[test]
fn registry_lists_rules_in_provider_order() {
    let registry = Registry::new(vec![provider("p", &["c", "a"]), provider("q", &["b"])]).unwrap();
    let ids: Vec<&str> = registry.rules().map(|meta| meta.id).collect();
    assert_eq!(ids, ["c", "a", "b"]);
    assert_eq!(registry.get("a").map(|meta| meta.id), Some("a"));
    assert!(registry.get("zzz").is_none());
}

#[test]
fn severity_resolution_table() {
    let none = leak_meta("sev-none", None);
    let warning = leak_meta("sev-warning", Some(Severity::Warning));
    let error = leak_meta("sev-error", Some(Severity::Error));
    let cases = [
        (none, None, None),
        (none, Some("off"), None),
        (none, Some("warn"), Some(Severity::Warning)),
        (none, Some("error"), Some(Severity::Error)),
        (warning, None, Some(Severity::Warning)),
        (warning, Some("off"), None),
        (warning, Some("warn"), Some(Severity::Warning)),
        (warning, Some("error"), Some(Severity::Error)),
        (error, None, Some(Severity::Error)),
        (error, Some("off"), None),
        (error, Some("warn"), Some(Severity::Warning)),
        (error, Some("error"), Some(Severity::Error)),
    ];
    for (meta, level, expected) in cases {
        let toml = level.map_or_else(String::new, |level| format!("[rules]\n{} = \"{level}\"\n", meta.id));
        let enabled = EnabledRules::resolve([meta], &config(&toml));
        assert_eq!(enabled.severity(meta.id), expected, "{} / {level:?}", meta.id);
        assert_eq!(enabled.contains(meta.id), expected.is_some(), "{} / {level:?}", meta.id);
    }
}

#[test]
fn unknown_rule_in_config_is_a_config_error() {
    let registry = Registry::new(vec![provider("p", &["known"])]).unwrap();
    let cfg = config("[rules]\nunknwon = \"off\"\n");
    let ws = workspace(&cfg);
    let err = run(&registry, &ws, &cfg, &RunOptions::default()).unwrap_err();
    assert!(matches!(err, LintError::Config(_)), "{err}");
    assert!(err.to_string().contains("unknwon"));
}

#[test]
fn disabled_rules_do_not_run_and_severity_is_applied() {
    let on = leak_meta("on-rule", Some(Severity::Warning));
    let off = leak_meta("off-rule", Some(Severity::Warning));
    let none = leak_meta("opt-in", None);
    let rules: Vec<Box<dyn Rule>> = [on, off, none]
        .into_iter()
        .map(|meta| {
            Box::new(FixedRule {
                meta,
                items: vec![item(meta.id, None, None)],
            }) as Box<dyn Rule>
        })
        .collect();
    let registry = Registry::new(vec![Box::new(RulesProvider::new("p", rules))]).unwrap();
    let cfg = config("[rules]\noff-rule = \"off\"\non-rule = \"error\"\n");
    let ws = workspace(&cfg);
    let report = run(&registry, &ws, &cfg, &RunOptions::default()).unwrap();
    let got: Vec<(String, Severity)> = report
        .diagnostics
        .iter()
        .map(|d| (d.rule.to_string(), d.severity))
        .collect();
    assert_eq!(got, [("on-rule".to_owned(), Severity::Error)]);
    assert_eq!(report.summary.errors, 1);
    assert_eq!(report.summary.packages, 3);
}

struct Failing;

impl RuleProvider for Failing {
    fn id(&self) -> &'static str {
        "failing"
    }

    fn rules(&self) -> Vec<&'static skif_core::RuleMeta> {
        Vec::new()
    }

    fn run(
        &self,
        _cx: &skif_lint::LintContext<'_>,
        _enabled: &EnabledRules,
        _out: &mut skif_lint::Emitter<'_>,
    ) -> Result<(), ProviderError> {
        Err(ProviderError::new("boom"))
    }
}

#[test]
fn provider_error_propagates() {
    let registry = Registry::new(vec![provider("ok", &["a"]), Box::new(Failing)]).unwrap();
    let cfg = config("");
    let ws = workspace(&cfg);
    let err = run(&registry, &ws, &cfg, &RunOptions::default()).unwrap_err();
    match err {
        LintError::Provider { provider, source } => {
            assert_eq!(provider, "failing");
            assert_eq!(source.to_string(), "boom");
        }
        other => panic!("unexpected {other}"),
    }
}
