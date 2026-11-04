//! `[[overrides]]` and `max-level`: levels, caps and suppressions that apply only where an override selects.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::{Path, PathBuf};

use common::leak_meta;
use skif_config::{Ceiling, Config, ConfigCache, ConfigLoader, ConfigOverrides, Level};
use skif_core::{Location, RuleMeta, Severity};
use skif_lint::{Emitter, LintContext, Registry, Rule, RulesProvider, RunOptions, diagnostic, run};
use skif_workspace::{MemoryFs, Workspace};

/// Where a rule reports: the package it names and the file it points at.
const SPOTS: [(Option<&str>, &str); 4] = [
    (Some("a"), "packages/a/package.json"),
    (Some("b"), "packages/b/package.json"),
    (Some("a"), "packages/a/src/generated/api.ts"),
    (None, "package.json"),
];

struct Spots {
    meta: &'static RuleMeta,
}

impl Rule for Spots {
    fn meta(&self) -> &'static RuleMeta {
        self.meta
    }

    fn check(&self, _cx: &LintContext<'_>, out: &mut Emitter<'_>) {
        for (package, path) in SPOTS {
            let mut builder = diagnostic(
                self.meta,
                "finding",
                Location {
                    path: PathBuf::from(path),
                    span: None,
                },
            )
            .dependency("left-pad");
            if let Some(package) = package {
                builder = builder.package(package);
            }
            out.emit(builder.build());
        }
    }
}

fn registry() -> Registry {
    let rules = vec![
        Box::new(Spots {
            meta: leak_meta("demo", Some(Severity::Warning)),
        }) as Box<dyn Rule>,
        Box::new(Spots {
            meta: leak_meta("quiet", None),
        }) as Box<dyn Rule>,
    ];
    Registry::new(vec![Box::new(RulesProvider::new("p", rules))]).unwrap()
}

fn fs(toml: &str) -> MemoryFs {
    MemoryFs::new()
        .with_file("/ws/skif.toml", toml)
        .with_file(
            "/ws/package.json",
            r#"{"name":"root","private":true,"workspaces":["packages/*"]}"#,
        )
        .with_file("/ws/packages/a/package.json", r#"{"name":"a","version":"1.0.0"}"#)
        .with_file("/ws/packages/b/package.json", r#"{"name":"b","version":"1.0.0"}"#)
}

fn resolved(toml: &str) -> Config {
    ConfigLoader::new(&fs(toml), &ConfigCache::new())
        .resolve(Path::new("/ws"), &Ceiling::RootOnly)
        .unwrap()
        .config
}

/// `(rule, package, path, severity)` of every diagnostic, in a fixed order.
fn findings(config: &Config) -> Vec<(String, String, String, Severity)> {
    let ws = Workspace::discover_with_fs(Path::new("/ws"), config, &fs("")).unwrap();
    let mut found: Vec<_> = run(&registry(), &ws, config, &RunOptions::default())
        .unwrap()
        .diagnostics
        .into_iter()
        .map(|d| {
            (
                d.rule.to_string(),
                d.package.unwrap_or_default(),
                d.location.path.display().to_string(),
                d.severity,
            )
        })
        .collect();
    found.sort();
    found
}

/// The severities of `rule` by package, `-` standing for no package and `+gen` marking the generated file.
fn by_package(found: &[(String, String, String, Severity)], rule: &str) -> Vec<(String, Severity)> {
    found
        .iter()
        .filter(|(id, ..)| id == rule)
        .map(|(_, package, path, severity)| {
            let package = if package.is_empty() { "-" } else { package };
            let generated = if path.contains("generated") { "+gen" } else { "" };
            (format!("{package}{generated}"), *severity)
        })
        .collect()
}

#[test]
fn without_overrides_every_diagnostic_has_the_base_level() {
    let found = findings(&resolved("[rules]\ndemo = \"error\"\n"));
    assert_eq!(found.len(), 4);
    assert!(found.iter().all(|(_, _, _, severity)| *severity == Severity::Error));
    let default = findings(&resolved(""));
    assert_eq!(default.len(), 4);
    assert!(default.iter().all(|(_, _, _, severity)| *severity == Severity::Warning));
}

#[test]
fn an_override_raises_the_level_in_its_scope_only() {
    let config = resolved("[[overrides]]\npackages = [\"a\"]\n[overrides.rules]\ndemo = \"error\"\n");
    assert_eq!(
        by_package(&findings(&config), "demo"),
        [
            ("-".to_owned(), Severity::Warning),
            ("a".to_owned(), Severity::Error),
            ("a+gen".to_owned(), Severity::Error),
            ("b".to_owned(), Severity::Warning),
        ]
    );
}

#[test]
fn an_override_lowers_the_level_in_its_scope_only() {
    let config =
        resolved("[rules]\ndemo = \"error\"\n[[overrides]]\npackages = [\"b\"]\n[overrides.rules]\ndemo = \"info\"\n");
    assert_eq!(
        by_package(&findings(&config), "demo"),
        [
            ("-".to_owned(), Severity::Error),
            ("a".to_owned(), Severity::Error),
            ("a+gen".to_owned(), Severity::Error),
            ("b".to_owned(), Severity::Info),
        ]
    );
}

#[test]
fn a_directory_pattern_selects_by_package_location() {
    let config = resolved("[[overrides]]\npackages = [\"./packages/b\"]\n[overrides.rules]\ndemo = \"error\"\n");
    let found = by_package(&findings(&config), "demo");
    assert!(found.contains(&("b".to_owned(), Severity::Error)), "{found:?}");
    assert!(found.contains(&("a".to_owned(), Severity::Warning)), "{found:?}");
}

#[test]
fn later_overrides_win_over_earlier_ones() {
    let config = resolved(
        "[[overrides]]\npackages = [\"a\"]\n[overrides.rules]\ndemo = \"error\"\n\
         [[overrides]]\npackages = [\"a\"]\n[overrides.rules]\ndemo = \"info\"\n",
    );
    let found = by_package(&findings(&config), "demo");
    assert!(found.contains(&("a".to_owned(), Severity::Info)), "{found:?}");
}

#[test]
fn a_file_glob_override_selects_by_file_path() {
    let config = resolved("[[overrides]]\nfiles = [\"**/generated/**\"]\n[overrides.rules]\ndemo = \"off\"\n");
    assert_eq!(
        by_package(&findings(&config), "demo"),
        [
            ("-".to_owned(), Severity::Warning),
            ("a".to_owned(), Severity::Warning),
            ("b".to_owned(), Severity::Warning),
        ]
    );
}

#[test]
fn packages_and_files_must_both_match() {
    let config = resolved(
        "[[overrides]]\npackages = [\"b\"]\nfiles = [\"**/generated/**\"]\n[overrides.rules]\ndemo = \"off\"\n",
    );
    assert_eq!(findings(&config).iter().filter(|f| f.0 == "demo").count(), 4);
}

#[test]
fn the_global_max_level_caps_every_diagnostic() {
    let config = resolved("[lint]\nmax-level = \"info\"\n[rules]\ndemo = \"error\"\n");
    let found = findings(&config);
    assert_eq!(found.iter().filter(|f| f.0 == "demo").count(), 4);
    assert!(found.iter().all(|(_, _, _, severity)| *severity == Severity::Info));
}

#[test]
fn a_cap_never_raises_a_level() {
    let config = resolved("[lint]\nmax-level = \"error\"\n[rules]\ndemo = \"info\"\n");
    assert!(
        findings(&config)
            .iter()
            .all(|(_, _, _, severity)| *severity == Severity::Info)
    );
}

#[test]
fn an_override_max_level_caps_its_scope_only() {
    let config = resolved("[rules]\ndemo = \"error\"\n[[overrides]]\npackages = [\"a\"]\nmax-level = \"warn\"\n");
    assert_eq!(
        by_package(&findings(&config), "demo"),
        [
            ("-".to_owned(), Severity::Error),
            ("a".to_owned(), Severity::Warning),
            ("a+gen".to_owned(), Severity::Warning),
            ("b".to_owned(), Severity::Error),
        ]
    );
}

#[test]
fn a_cap_applies_after_the_level_of_an_override() {
    let config = resolved(
        "[lint]\nmax-level = \"warn\"\n[[overrides]]\npackages = [\"a\"]\n[overrides.rules]\ndemo = \"error\"\n",
    );
    let found = by_package(&findings(&config), "demo");
    assert!(
        found.iter().all(|(_, severity)| *severity == Severity::Warning),
        "{found:?}"
    );
}

#[test]
fn a_rule_off_at_the_base_runs_inside_an_enabling_override_and_stays_scoped() {
    let config = resolved("[[overrides]]\npackages = [\"a\"]\n[overrides.rules]\nquiet = \"error\"\n");
    let registry = registry();
    assert!(registry.enabled(&config).contains("quiet"));
    assert_eq!(registry.enabled(&config).severity("quiet"), None);
    assert_eq!(
        by_package(&findings(&config), "quiet"),
        [("a".to_owned(), Severity::Error), ("a+gen".to_owned(), Severity::Error)]
    );
}

#[test]
fn a_rule_off_everywhere_stays_disabled() {
    let config = resolved("[[overrides]]\npackages = [\"a\"]\n[overrides.rules]\nquiet = \"off\"\n");
    assert!(!registry().enabled(&config).contains("quiet"));
    assert!(findings(&config).iter().all(|f| f.0 != "quiet"));
}

#[test]
fn an_override_can_turn_a_rule_off_inside_its_scope() {
    let config = resolved("[[overrides]]\npackages = [\"a\"]\n[overrides.rules]\ndemo = \"off\"\n");
    let found = by_package(&findings(&config), "demo");
    assert!(found.iter().all(|(package, _)| !package.starts_with('a')), "{found:?}");
    assert_eq!(found.len(), 2);
}

#[test]
fn an_override_ignore_suppresses_within_its_scope() {
    let config = resolved("[[overrides]]\npackages = [\"a\"]\n[[overrides.ignore]]\nrules = [\"demo\"]\n");
    let found = by_package(&findings(&config), "demo");
    assert_eq!(
        found.iter().map(|(package, _)| package.as_str()).collect::<Vec<_>>(),
        ["-", "b"]
    );
}

#[test]
fn an_override_ignore_may_omit_every_filter_and_follows_its_dependency_filter() {
    let everything = resolved("[[overrides]]\npackages = [\"b\"]\n[[overrides.ignore]]\n");
    let found = findings(&everything);
    assert!(found.iter().all(|(_, package, ..)| package != "b"), "{found:?}");

    let other = resolved("[[overrides]]\npackages = [\"b\"]\n[[overrides.ignore]]\ndependencies = [\"lodash\"]\n");
    assert!(findings(&other).iter().any(|(_, package, ..)| package == "b"));
}

#[test]
fn the_command_line_level_beats_an_override() {
    let mut config = resolved("[[overrides]]\npackages = [\"a\"]\n[overrides.rules]\ndemo = \"error\"\n");
    config
        .apply(&ConfigOverrides {
            rules: [("demo".to_owned(), Level::Info)].into_iter().collect(),
            ..ConfigOverrides::default()
        })
        .unwrap();
    assert!(
        findings(&config)
            .iter()
            .filter(|f| f.0 == "demo")
            .all(|(_, _, _, severity)| *severity == Severity::Info)
    );
}

#[test]
fn the_command_line_cannot_be_undone_by_an_enabling_override() {
    let mut config = resolved("[[overrides]]\npackages = [\"a\"]\n[overrides.rules]\nquiet = \"error\"\n");
    config
        .apply(&ConfigOverrides {
            ignore_rules: vec!["quiet".to_owned()],
            ..ConfigOverrides::default()
        })
        .unwrap();
    assert!(!registry().enabled(&config).contains("quiet"));
}

#[test]
fn an_override_that_selects_nothing_changes_nothing() {
    let plain = findings(&resolved("[rules]\ndemo = \"error\"\n"));
    let config = resolved(
        "[rules]\ndemo = \"error\"\n[[overrides]]\npackages = [\"nobody\"]\nmax-level = \"info\"\n[overrides.rules]\ndemo = \"off\"\n",
    );
    assert_eq!(findings(&config), plain);
}
