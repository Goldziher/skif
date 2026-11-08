#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;

use skif_config::{Config, Selector, UpdateTarget};
use skif_core::FixOp;
use skif_edit::FileKind;

use super::*;
use crate::testing::{Files, config, workspace};

/// 2026-01-01T00:00:00Z.
const NOW: i64 = 1_767_225_600;
const DAY: i64 = 86_400;

/// A registry answering from memory that records every name it is asked about.
#[derive(Default)]
struct Fake {
    packages: BTreeMap<String, Packument>,
    asked: Mutex<Vec<String>>,
}

impl Fake {
    /// `versions` as `(version, days before NOW)`; a negative age marks the version deprecated.
    fn with(mut self, name: &str, versions: &[(&str, i64)]) -> Self {
        let versions = versions
            .iter()
            .map(|&(version, days)| {
                let published = Published::new(version, Some(NOW - days.abs() * DAY));
                if days < 0 { published.deprecated() } else { published }
            })
            .collect();
        self.packages.insert(name.to_owned(), Packument::new(versions));
        self
    }

    fn packages_insert(&mut self, name: &str, packument: Packument) {
        self.packages.insert(name.to_owned(), packument);
    }

    fn asked(&self) -> Vec<String> {
        self.asked.lock().unwrap().clone()
    }
}

impl Registry for Fake {
    fn fetch(&self, name: &str) -> Result<Packument, FetchError> {
        self.asked.lock().unwrap().push(name.to_owned());
        self.packages.get(name).cloned().ok_or(FetchError::NotFound)
    }
}

fn react() -> Fake {
    Fake::default().with(
        "react",
        &[
            ("18.2.0", 900),
            ("18.2.1", 800),
            ("18.3.1", 400),
            ("18.4.0", -300),
            ("19.0.0", 100),
            ("19.1.0-rc.1", 50),
        ],
    )
}

const ROOT: &str =
    r#"{"name":"root","version":"0.0.0","private":true,"packageManager":"pnpm@9.0.0","workspaces":["packages/*"]}"#;
const APP: &str = r#"{"name":"app","version":"1.0.0","dependencies":{"react":"^18.2.0","lib":"1.0.0"},"peerDependencies":{"react":">=18"}}"#;
const LIB: &str = r#"{"name":"lib","version":"1.0.0","devDependencies":{"react":"~18.2.0"}}"#;

fn files() -> Vec<(&'static str, &'static str)> {
    vec![
        ("package.json", ROOT),
        ("packages/app/package.json", APP),
        ("packages/lib/package.json", LIB),
    ]
}

fn plan(files: Files<'_>, toml: &str, registry: &dyn Registry, options: &UpdateOptions) -> UpdateReport {
    let config: Config = config(toml);
    let workspace = workspace(files, &config);
    plan_updates(&workspace, &config, registry, options)
}

fn summary(report: &UpdateReport) -> Vec<(String, String, String, String)> {
    report
        .updates
        .iter()
        .map(|update| {
            (
                update.location.path.to_string_lossy().replace('\\', "/"),
                update.dependency.clone(),
                update.current.clone(),
                update.target.clone(),
            )
        })
        .collect()
}

fn row(path: &str, dependency: &str, current: &str, target: &str) -> (String, String, String, String) {
    (
        path.to_owned(),
        dependency.to_owned(),
        current.to_owned(),
        target.to_owned(),
    )
}

#[test]
fn every_instance_moves_to_the_highest_eligible_version_in_its_own_range_style() {
    let registry = react();
    let report = plan(&files(), "", &registry, &UpdateOptions::new(NOW));
    assert_eq!(
        summary(&report),
        [
            row("packages/app/package.json", "react", "^18.2.0", "^19.0.0"),
            row("packages/lib/package.json", "react", "~18.2.0", "~19.0.0"),
        ],
        "the prerelease is another channel, the deprecated 18.4.0 is skipped, the peer range and the local `lib` stay"
    );
    assert_eq!(registry.asked(), ["react"], "one request per dependency name");
    assert_eq!(report.queried, 1);
    assert_eq!(report.failures, []);
    let update = &report.updates[0];
    assert_eq!(update.place, "dependencies");
    assert_eq!(update.package.as_deref(), Some("app"));
    assert_eq!(update.fix.safety, FixSafety::Unsafe);
}

#[test]
fn the_target_limits_how_far_a_version_moves() {
    let minor = plan(
        &files(),
        "",
        &react(),
        &UpdateOptions::new(NOW).with_target(UpdateTarget::Minor),
    );
    assert_eq!(
        summary(&minor),
        [
            row("packages/app/package.json", "react", "^18.2.0", "^18.3.1"),
            row("packages/lib/package.json", "react", "~18.2.0", "~18.3.1"),
        ]
    );
    let patch = plan(
        &files(),
        "",
        &react(),
        &UpdateOptions::new(NOW).with_target(UpdateTarget::Patch),
    );
    assert_eq!(
        summary(&patch),
        [
            row("packages/app/package.json", "react", "^18.2.0", "^18.2.1"),
            row("packages/lib/package.json", "react", "~18.2.0", "~18.2.1"),
        ]
    );
}

#[test]
fn update_groups_cap_or_skip_the_instances_they_match_first() {
    let toml = r#"
[[update-groups]]
packages = ["lib"]
ignored = true

[[update-groups]]
dependencies = ["react"]
target = "minor"

[[update-groups]]
target = "patch"
"#;
    let report = plan(&files(), toml, &react(), &UpdateOptions::new(NOW));
    assert_eq!(
        summary(&report),
        [row("packages/app/package.json", "react", "^18.2.0", "^18.3.1")]
    );
    let stricter_run = plan(
        &files(),
        toml,
        &react(),
        &UpdateOptions::new(NOW).with_target(UpdateTarget::Patch),
    );
    assert_eq!(
        summary(&stricter_run),
        [row("packages/app/package.json", "react", "^18.2.0", "^18.2.1")],
        "a group never widens --target"
    );
}

#[test]
fn versions_younger_than_the_minimum_release_age_are_not_proposed() {
    let registry = Fake::default().with("left-pad", &[("1.0.0", 900), ("1.1.0", 0)]);
    let files = [
        ("package.json", ROOT),
        (
            "packages/a/package.json",
            r#"{"name":"a","dependencies":{"left-pad":"1.0.0"}}"#,
        ),
    ];
    let default = plan(&files, "", &registry, &UpdateOptions::new(NOW));
    assert!(default.updates.is_empty(), "published today, inside the default day");
    assert_eq!(default.minimum_release_age, DEFAULT_MINIMUM_RELEASE_AGE);
    let off = plan(
        &files,
        "[update]\nminimum-release-age = 0\n",
        &registry,
        &UpdateOptions::new(NOW),
    );
    assert_eq!(
        summary(&off),
        [row("packages/a/package.json", "left-pad", "1.0.0", "1.1.0")]
    );

    let pnpm = [
        files[0],
        files[1],
        (
            "pnpm-workspace.yaml",
            "packages:\n  - packages/*\nminimumReleaseAge: 0\n",
        ),
    ];
    let from_pnpm = plan(&pnpm, "", &registry, &UpdateOptions::new(NOW));
    assert_eq!(from_pnpm.minimum_release_age, 0);
    assert_eq!(
        from_pnpm.updates.len(),
        1,
        "pnpm's minimumReleaseAge applies when skif.toml sets none"
    );
    let config_wins = plan(
        &pnpm,
        "[update]\nminimum-release-age = 1440\n",
        &registry,
        &UpdateOptions::new(NOW),
    );
    assert_eq!(config_wins.updates, []);
}

#[test]
fn only_highest_preferring_groups_move_and_catalog_siblings_leave_it_to_the_catalog() {
    let registry = react()
        .with("vue", &[("3.0.0", 900), ("3.5.0", 100)])
        .with("typescript", &[("5.0.0", 900), ("5.6.3", 100)]);
    let files = [
        ("package.json", ROOT),
        (
            "pnpm-workspace.yaml",
            "packages:\n  - packages/*\ncatalog:\n  typescript: ^5.0.0\ncatalogs:\n  legacy:\n    vue: 3.0.0\n",
        ),
        (
            "packages/a/package.json",
            r#"{"name":"a","dependencies":{"react":"18.2.0","vue":"catalog:legacy"},"devDependencies":{"typescript":"catalog:"}}"#,
        ),
        (
            "packages/b/package.json",
            r#"{"name":"b","dependencies":{"vue":"3.0.0"},"devDependencies":{"typescript":"5.0.0"}}"#,
        ),
    ];
    let toml = "[[version-groups]]\ndependencies = [\"react\"]\npolicy = \"pinned\"\npin = \"18.2.0\"\n";
    let report = plan(&files, toml, &registry, &UpdateOptions::new(NOW));
    assert_eq!(
        summary(&report),
        [
            row("pnpm-workspace.yaml", "typescript", "^5.0.0", "^5.6.3"),
            row("pnpm-workspace.yaml", "vue", "3.0.0", "3.5.0"),
        ],
        "the pinned react and the plain siblings of catalog references stay; the catalog entries move"
    );
    assert_eq!(report.updates[0].place, "catalog");
    assert_eq!(report.updates[1].place, "catalogs.legacy");
    assert_eq!(report.updates[0].package, None);
    let yaml = files[1].1;
    let ops: Vec<&FixOp> = report.updates.iter().flat_map(|update| &update.fix.ops).collect();
    let edited = skif_edit::apply(
        yaml,
        FileKind::from_path(Path::new("pnpm-workspace.yaml")).unwrap(),
        &ops,
    )
    .unwrap();
    assert_eq!(
        edited,
        "packages:\n  - packages/*\ncatalog:\n  typescript: ^5.6.3\ncatalogs:\n  legacy:\n    vue: 3.5.0\n"
    );
}

#[test]
fn a_semver_group_s_range_is_applied_to_the_new_version() {
    let report = plan(
        &files(),
        "[[semver-groups]]\npackages = [\"app\"]\nrange = \"\"\n",
        &react(),
        &UpdateOptions::new(NOW),
    );
    assert_eq!(
        summary(&report),
        [
            row("packages/app/package.json", "react", "^18.2.0", "19.0.0"),
            row("packages/lib/package.json", "react", "~18.2.0", "~19.0.0"),
        ]
    );
}

#[test]
fn a_failed_name_is_reported_and_the_others_still_move() {
    let files = [
        ("package.json", ROOT),
        (
            "packages/a/package.json",
            r#"{"name":"a","dependencies":{"react":"18.2.0","@private/thing":"1.0.0"}}"#,
        ),
    ];
    let registry = react();
    let report = plan(&files, "", &registry, &UpdateOptions::new(NOW));
    assert_eq!(registry.asked(), ["@private/thing", "react"]);
    assert_eq!(
        report.failures,
        [Failure {
            dependency: "@private/thing".to_owned(),
            error: FetchError::NotFound,
        }]
    );
    assert_eq!(
        summary(&report),
        [row("packages/a/package.json", "react", "18.2.0", "19.0.0")]
    );
}

#[test]
fn the_dependency_filter_limits_what_the_registry_is_asked() {
    let files = [
        ("package.json", ROOT),
        (
            "packages/a/package.json",
            r#"{"name":"a","dependencies":{"react":"18.2.0","left-pad":"1.0.0","git-dep":"github:a/b","npm-alias":"npm:other@1.0.0"}}"#,
        ),
    ];
    let registry = react();
    let options = UpdateOptions::new(NOW).with_dependencies(Selector::new(["re*"]).unwrap());
    let report = plan(&files, "", &registry, &options);
    assert_eq!(registry.asked(), ["react"]);
    assert_eq!(report.updates.len(), 1);
    let unfiltered = Fake::default();
    let _ = plan(&files, "", &unfiltered, &UpdateOptions::new(NOW));
    assert_eq!(
        unfiltered.asked(),
        ["left-pad", "react"],
        "git specifiers and aliases of other packages are never looked up"
    );
}

#[test]
fn the_fix_writes_the_target_over_the_manifest_entry() {
    let report = plan(&files(), "", &react(), &UpdateOptions::new(NOW));
    let ops: Vec<&FixOp> = report.updates[0].fix.ops.iter().collect();
    let edited = skif_edit::apply(APP, FileKind::Json, &ops).unwrap();
    assert_eq!(edited, APP.replace(r#""react":"^18.2.0""#, r#""react":"^19.0.0""#));
}

#[test]
fn a_registry_document_is_read_with_deprecations_and_publish_times() {
    let text = r#"{
        "name": "x",
        "versions": {
            "1.0.0": {"name": "x", "dist": {}},
            "1.1.0": {"deprecated": "use 2"},
            "1.2.0": {"deprecated": ""},
            "2.0.0": {}
        },
        "time": {"created": "2020-01-01T00:00:00.000Z", "1.0.0": "2020-01-02T03:04:05.678Z", "2.0.0": "bad"}
    }"#;
    let packument = Packument::from_json(text).unwrap();
    let view: Vec<(&str, Option<i64>, bool)> = packument
        .versions
        .iter()
        .map(|published| (published.version.as_str(), published.published_at, published.deprecated))
        .collect();
    assert_eq!(
        view,
        [
            ("1.0.0", Some(1_577_934_245), false),
            ("1.1.0", None, true),
            ("1.2.0", None, false),
            ("2.0.0", None, false),
        ]
    );
    for bad in ["<html>", "\"text\"", "{\"versions\": []}"] {
        assert!(
            matches!(Packument::from_json(bad), Err(FetchError::Invalid(_))),
            "{bad}"
        );
    }
}

#[test]
fn registry_times_parse_to_unix_seconds() {
    assert_eq!(unix_seconds("1970-01-01T00:00:00Z"), Some(0));
    assert_eq!(unix_seconds("2026-01-01T00:00:00.000Z"), Some(NOW));
    assert_eq!(unix_seconds("2000-02-29T12:00:00.000Z"), Some(951_825_600));
    assert_eq!(unix_seconds("1969-12-31T23:59:59.000Z"), Some(-1));
    assert_eq!(unix_seconds("2026-01-01T02:00:00+02:00"), Some(NOW));
    assert_eq!(unix_seconds("2025-12-31T18:30:00.5-05:30"), Some(NOW));
    assert_eq!(unix_seconds("2026-01-01T00:00:00+00:00"), Some(NOW));
    for bad in [
        "",
        "2020-01-01",
        "2020-13-01T00:00:00Z",
        "2020-01-0xT00:00:00Z",
        "2020-01-01T00:00:00",
        "2020-01-01T00:00:00.Z",
        "2020-01-01T00:00:00+0100",
        "2020-01-01T00:00:00+24:00",
        "2020-01-01T00:00:00+01:00Z",
    ] {
        assert_eq!(unix_seconds(bad), None, "{bad}");
    }
}

#[test]
fn a_version_without_a_publish_time_is_not_proposed_while_a_release_age_applies() {
    let mut registry = Fake::default();
    registry.packages_insert(
        "left-pad",
        Packument::new(vec![
            Published::new("1.0.0", Some(NOW - 900 * DAY)),
            Published::new("1.1.0", Some(NOW - 400 * DAY)),
            Published::new("1.2.0", None),
        ]),
    );
    let files = [
        ("package.json", ROOT),
        (
            "packages/a/package.json",
            r#"{"name":"a","dependencies":{"left-pad":"1.0.0"}}"#,
        ),
        (
            "packages/b/package.json",
            r#"{"name":"b","dependencies":{"left-pad":"^1.0.0"}}"#,
        ),
    ];
    let report = plan(&files, "", &registry, &UpdateOptions::new(NOW));
    assert_eq!(
        summary(&report),
        [
            row("packages/a/package.json", "left-pad", "1.0.0", "1.1.0"),
            row("packages/b/package.json", "left-pad", "^1.0.0", "^1.1.0"),
        ],
        "1.2.0 cannot be shown to be a day old"
    );
    assert_eq!(
        report.notes,
        [Note::NoPublishTime {
            dependency: "left-pad".to_owned(),
            version: "1.2.0".to_owned(),
        }],
        "one note for both instances"
    );
    assert!(report.notes[0].to_string().contains("no publish time"));

    let off = plan(
        &files,
        "[update]\nminimum-release-age = 0\n",
        &registry,
        &UpdateOptions::new(NOW),
    );
    assert_eq!(
        off.updates[0].target, "1.2.0",
        "without a release age the time does not matter"
    );
    assert_eq!(off.notes, []);

    let mut older = Fake::default();
    older.packages_insert(
        "left-pad",
        Packument::new(vec![
            Published::new("1.0.0", None),
            Published::new("1.1.0", Some(NOW - 400 * DAY)),
        ]),
    );
    let report = plan(&files, "", &older, &UpdateOptions::new(NOW));
    assert_eq!(report.updates.len(), 2);
    assert_eq!(
        report.notes,
        [],
        "a timeless version that would not be the target is not worth a note"
    );
}

#[test]
fn an_invalid_package_name_is_not_looked_up_and_gets_a_note() {
    let registry = Fake::default().with("left-pad", &[("1.0.0", 900), ("1.1.0", 400)]);
    let files = [
        ("package.json", ROOT),
        (
            "packages/a/package.json",
            r#"{"name":"a","dependencies":{"a?x":"1.0.0","a#b":"1.0.0","a/../b":"1.0.0","left-pad":"1.0.0"}}"#,
        ),
    ];
    let report = plan(&files, "", &registry, &UpdateOptions::new(NOW));
    assert_eq!(registry.asked(), ["left-pad"]);
    assert_eq!(report.queried, 1);
    assert_eq!(
        summary(&report),
        [row("packages/a/package.json", "left-pad", "1.0.0", "1.1.0")]
    );
    let noted: Vec<&str> = report.notes.iter().map(Note::dependency).collect();
    assert_eq!(noted, ["a#b", "a/../b", "a?x"]);
    assert!(report.notes[0].to_string().contains("not a valid npm package name"));
}

#[test]
fn package_names_follow_the_npm_allowlist() {
    for good in [
        "react",
        "@types/node",
        "JSONStream",
        "lodash.merge",
        "~tilde",
        "-dash",
        "a_b",
        "@a-b/c.d_e~f",
    ] {
        assert!(is_valid_package_name(good), "{good}");
    }
    for bad in [
        "",
        "a?x",
        "a#b",
        "a/../b",
        "a/b",
        "..",
        ".x",
        "_x",
        "@scope",
        "@/x",
        "@scope/",
        "@scope/..",
        "@a/b/c",
        "a b",
        "a%2fb",
        "a:b",
        "é",
    ] {
        assert!(!is_valid_package_name(bad), "{bad}");
    }
    assert!(is_valid_package_name(&"a".repeat(214)));
    assert!(!is_valid_package_name(&"a".repeat(215)));
    assert!(!is_valid_package_name(&format!("@s/{}", "a".repeat(212))));
}

/// With syncpack's override reading the key of a yarn resolution is the dependency name. `**/foo` and `parent/child`
/// are not package names: they are noted and not looked up, so they no longer fail the run as a registry 404 would.
#[test]
fn a_yarn_resolution_path_read_as_a_name_is_noted_not_failed() {
    let registry = Fake::default().with("left-pad", &[("1.0.0", 900), ("1.1.0", 400)]);
    let files = [
        (
            "package.json",
            r#"{"name":"root","version":"0.0.0","private":true,"packageManager":"yarn@1.22.0","workspaces":["packages/*"],"resolutions":{"**/foo":"1.0.0","parent/child":"1.0.0"}}"#,
        ),
        (
            "packages/a/package.json",
            r#"{"name":"a","dependencies":{"left-pad":"1.0.0"}}"#,
        ),
    ];
    let toml = "[policy]\ndependency-types = [\"prod\", \"resolutions\"]\noverride-reading = \"syncpack\"\n";
    let report = plan(&files, toml, &registry, &UpdateOptions::new(NOW));
    assert_eq!(registry.asked(), ["left-pad"]);
    assert!(report.failures.is_empty(), "{:?}", report.failures.len());
    let noted: Vec<&str> = report.notes.iter().map(Note::dependency).collect();
    assert_eq!(noted, ["**/foo", "parent/child"]);
}
