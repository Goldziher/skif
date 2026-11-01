#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{FixedRule, config, item, leak_meta, workspace};
use skif_core::Severity;
use skif_lint::{Registry, Rule, RulesProvider, RunOptions, run};

fn registry() -> Registry {
    let demo = leak_meta("demo", Some(Severity::Warning));
    let other = leak_meta("other", Some(Severity::Warning));
    let rules: Vec<Box<dyn Rule>> = vec![
        Box::new(FixedRule {
            meta: demo,
            items: vec![
                item("m1", Some("a"), Some("react")),
                item("m2", Some("b"), Some("react@18")),
                item("m3", Some("a"), Some("lodash")),
                item("m5", None, None),
                item("m6", Some("a"), Some("@scope/x@1")),
            ],
        }),
        Box::new(FixedRule {
            meta: other,
            items: vec![item("m4", Some("b"), None)],
        }),
    ];
    Registry::new(vec![Box::new(RulesProvider::new("p", rules))]).unwrap()
}

fn messages(toml: &str) -> Vec<String> {
    let cfg = config(toml);
    let ws = workspace(&cfg);
    let report = run(&registry(), &ws, &cfg, &RunOptions::default()).unwrap();
    let mut messages: Vec<String> = report.diagnostics.iter().map(|d| d.message.clone()).collect();
    messages.sort();
    messages
}

#[test]
fn ignore_matching_table() {
    let cases: [(&str, &[&str]); 14] = [
        ("", &["m1", "m2", "m3", "m4", "m5", "m6"]),
        ("[[ignore]]\nrules = [\"demo\"]\n", &["m4"]),
        ("[[ignore]]\nrules = [\"other\"]\n", &["m1", "m2", "m3", "m5", "m6"]),
        ("[[ignore]]\npackages = [\"a\"]\n", &["m2", "m4", "m5"]),
        (
            "[[ignore]]\nrules = [\"demo\"]\npackages = [\"a\"]\n",
            &["m2", "m4", "m5"],
        ),
        ("[[ignore]]\ndependencies = [\"react\"]\n", &["m3", "m4", "m5", "m6"]),
        ("[[ignore]]\ndependencies = [\"react@18\"]\n", &["m1", "m3", "m4", "m5", "m6"]),
        (
            "[[ignore]]\ndependencies = [\"react@17\"]\n",
            &["m1", "m2", "m3", "m4", "m5", "m6"],
        ),
        ("[[ignore]]\ndependencies = [\"lod*\"]\n", &["m1", "m2", "m4", "m5", "m6"]),
        ("[[ignore]]\ndependencies = [\"*act\"]\n", &["m3", "m4", "m5", "m6"]),
        ("[[ignore]]\ndependencies = [\"*od*\"]\n", &["m1", "m2", "m4", "m5", "m6"]),
        ("[[ignore]]\ndependencies = [\"@scope/*\"]\n", &["m1", "m2", "m3", "m4", "m5"]),
        (
            "[[ignore]]\npackages = [\"a\"]\ndependencies = [\"react\"]\n",
            &["m2", "m3", "m4", "m5", "m6"],
        ),
        ("[[ignore]]\npackages = [\"b\"]\n", &["m1", "m3", "m5", "m6"]),
    ];
    for (toml, expected) in cases {
        assert_eq!(messages(toml), expected, "{toml}");
    }
}

#[test]
fn multiple_ignore_entries_union() {
    let toml = "[[ignore]]\nrules = [\"other\"]\n\n[[ignore]]\ndependencies = [\"lodash\"]\n";
    assert_eq!(messages(toml), ["m1", "m2", "m5", "m6"]);
}

#[test]
fn workspace_ignore_packages_drops_their_diagnostics() {
    let cases: [(&str, &[&str]); 3] = [
        ("[workspace]\nignore-packages = [\"b\"]\n", &["m1", "m3", "m5", "m6"]),
        ("[workspace]\nignore-packages = [\"a\", \"b\"]\n", &["m5"]),
        ("[workspace]\nignore-packages = [\"packages/a\"]\n", &["m2", "m4", "m5"]),
    ];
    for (toml, expected) in cases {
        assert_eq!(messages(toml), expected, "{toml}");
    }
}
