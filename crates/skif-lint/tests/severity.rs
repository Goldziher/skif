#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::Path;

use common::{config, leak_meta, location, workspace};
use skif_core::{RuleMeta, Severity};
use skif_lint::{Emitter, LintContext, Registry, Rule, RulesProvider, RunOptions, diagnostic, run};

/// Emits one finding that proposes `Error` and one that proposes nothing.
struct Proposing {
    meta: &'static RuleMeta,
}

impl Rule for Proposing {
    fn meta(&self) -> &'static RuleMeta {
        self.meta
    }

    fn check(&self, _cx: &LintContext<'_>, out: &mut Emitter<'_>) {
        out.emit(
            diagnostic(self.meta, "proposed", location())
                .default_severity(Severity::Error)
                .build(),
        );
        out.emit(diagnostic(self.meta, "plain", location()).build());
    }
}

/// Reports the configuration file the context names.
struct ConfigFile {
    meta: &'static RuleMeta,
}

impl Rule for ConfigFile {
    fn meta(&self) -> &'static RuleMeta {
        self.meta
    }

    fn check(&self, cx: &LintContext<'_>, out: &mut Emitter<'_>) {
        out.emit(diagnostic(self.meta, cx.config_file().display().to_string(), location()).build());
    }
}

fn severities(toml: &str) -> Vec<(String, Severity)> {
    let meta = leak_meta("demo", Some(Severity::Warning));
    let registry = Registry::new(vec![Box::new(RulesProvider::new(
        "p",
        vec![Box::new(Proposing { meta }) as Box<dyn Rule>],
    ))])
    .unwrap();
    let cfg = config(toml);
    let ws = workspace(&cfg);
    let mut found: Vec<_> = run(&registry, &ws, &cfg, &RunOptions::default())
        .unwrap()
        .diagnostics
        .into_iter()
        .map(|d| (d.message, d.severity))
        .collect();
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found
}

#[test]
fn a_proposed_severity_replaces_the_rule_default() {
    assert_eq!(
        severities(""),
        [
            ("plain".to_owned(), Severity::Warning),
            ("proposed".to_owned(), Severity::Error)
        ]
    );
}

#[test]
fn a_configured_level_beats_the_proposed_severity() {
    for (level, expected) in [
        ("warn", Severity::Warning),
        ("info", Severity::Info),
        ("error", Severity::Error),
    ] {
        let found = severities(&format!("[rules]\ndemo = \"{level}\"\n"));
        assert_eq!(
            found,
            [("plain".to_owned(), expected), ("proposed".to_owned(), expected)],
            "{level}"
        );
    }
    assert_eq!(
        severities("[rules]\ndemo = \"off\"\n"),
        [] as [(std::string::String, skif_core::Severity); 0]
    );
}

#[test]
fn the_proposal_is_cleared_once_emitted() {
    let meta = leak_meta("demo", Some(Severity::Warning));
    let registry = Registry::new(vec![Box::new(RulesProvider::new(
        "p",
        vec![Box::new(Proposing { meta }) as Box<dyn Rule>],
    ))])
    .unwrap();
    let cfg = config("");
    let ws = workspace(&cfg);
    let report = run(&registry, &ws, &cfg, &RunOptions::default()).unwrap();
    assert!(report.diagnostics.iter().all(|d| d.default_severity.is_none()));
}

fn reported_config_file(path: Option<&Path>) -> String {
    let meta = leak_meta("cfg", Some(Severity::Warning));
    let registry = Registry::new(vec![Box::new(RulesProvider::new(
        "p",
        vec![Box::new(ConfigFile { meta }) as Box<dyn Rule>],
    ))])
    .unwrap();
    let cfg = config("");
    let ws = workspace(&cfg);
    let options = path.map_or_else(RunOptions::default, |path| RunOptions::default().with_config_path(path));
    run(&registry, &ws, &cfg, &options)
        .unwrap()
        .diagnostics
        .remove(0)
        .message
}

#[test]
fn the_context_names_the_configuration_file_or_falls_back_to_the_default() {
    assert_eq!(reported_config_file(None), "skif.toml");
    assert_eq!(
        reported_config_file(Some(Path::new("tools/skif.toml"))),
        "tools/skif.toml"
    );
}
