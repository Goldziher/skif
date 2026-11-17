use skif_workspace::MemoryFs;

use super::*;

fn set(keys: &Keys, category: &str) -> Vec<String> {
    keys[category].iter().cloned().collect()
}

#[test]
fn normalizes_absolute_and_relative_paths() {
    let root = Path::new("/work/repo");
    assert_eq!(normalize("/work/repo/a/b.ts", root), "a/b.ts");
    assert_eq!(normalize("./a/b.ts", root), "a/b.ts");
    assert_eq!(normalize("a\\b.ts", root), "a/b.ts");
    assert_eq!(normalize("/work/repository/a.ts", root), "/work/repository/a.ts");
}

#[test]
fn maps_skif_diagnostics_to_knip_categories() {
    let doc = json!({ "diagnostics": [
        { "rule": "unused-dependencies", "message": "`left-pad` is never used", "location": { "path": "packages/a/package.json" } },
        { "rule": "unlisted-dependencies", "message": "`react` is imported", "dependency": "react", "location": { "path": "/r/src/a.ts" } },
        { "rule": "unused-files", "message": "`src/dead.ts` is not reachable", "location": { "path": "src/dead.ts" } },
        { "rule": "duplicate-exports", "message": "`local` is exported as `b` and `a`", "location": { "path": "src/d.ts" } },
        { "rule": "version-mismatch", "message": "x", "location": { "path": "package.json" } },
    ]});
    let keys = skif_keys(&doc, Path::new("/r"));
    assert_eq!(set(&keys, "dependencies"), ["packages/a/package.json|left-pad"]);
    assert_eq!(set(&keys, "unlisted"), ["src/a.ts|react"]);
    assert_eq!(set(&keys, "files"), ["src/dead.ts"]);
    assert_eq!(set(&keys, "duplicates"), ["src/d.ts|a|b"]);
    assert_eq!(keys.len(), 4);
}

#[test]
fn maps_knip_issues_of_every_shape() {
    let doc = json!({ "files": ["old.ts"], "issues": [
        { "file": "package.json", "dependencies": [{ "name": "zzz" }], "devDependencies": [] },
        { "file": "src/dead.ts", "files": [{ "name": "src/dead.ts" }] },
        { "file": "src/e.ts", "enumMembers": { "Color": [{ "name": "Red" }] }, "exports": [{ "name": "x", "line": 1 }],
          "duplicates": [[{ "name": "b" }, { "name": "a" }]], "nsTypes": [{ "name": "T" }], "classMembers": [{ "name": "m" }] },
    ]});
    let keys = knip_keys(&doc, Path::new("/r"));
    assert_eq!(set(&keys, "dependencies"), ["package.json|zzz"]);
    assert_eq!(set(&keys, "files"), ["old.ts", "src/dead.ts"]);
    assert_eq!(set(&keys, "enumMembers"), ["src/e.ts|Red"]);
    assert_eq!(set(&keys, "exports"), ["src/e.ts|x"]);
    assert_eq!(set(&keys, "duplicates"), ["src/e.ts|a|b"]);
    assert_eq!(set(&keys, "nsTypes"), ["src/e.ts|T"]);
    assert!(!keys.contains_key("namespaceMembers"));
    assert!(!keys.contains_key("devDependencies"));
    assert!(!keys.contains_key("classMembers"));
}

#[test]
fn equal_findings_agree_completely() {
    let skif = json!({ "diagnostics": [
        { "rule": "unused-exports", "message": "export `x` is never imported", "location": { "path": "src/e.ts" } },
    ]});
    let knip = json!({ "issues": [{ "file": "src/e.ts", "exports": [{ "name": "x" }] }] });
    let (counts, _) = diff::compare(&skif_keys(&skif, Path::new("/r")), &knip_keys(&knip, Path::new("/r")));
    assert_eq!(
        counts["exports"],
        Counts {
            skif_only: 0,
            tool_only: 0,
            both: 1
        }
    );
}

#[test]
fn install_commands_follow_the_package_manager() {
    let repo = |pm: &str, install: bool| Repo {
        name: "r".into(),
        url: String::new(),
        rev: "0".repeat(40),
        package_manager: pm.into(),
        suites: vec!["parity".into()],
        sparse: Vec::new(),
        install,
        notes: "why".into(),
        recursive: false,
        knip_unread: Vec::new(),
        install_timeout_secs: None,
        install_command: Vec::new(),
        entry: Vec::new(),
        bench_knip_args: Vec::new(),
        bench_knip_failure: None,
        bench_knip_config: None,
        bench_max_runs: None,
        bench_max_warmup: None,
        knip_invalid: None,
        knip_root: None,
        build_command: Vec::new(),
        build_timeout_secs: None,
    };
    let dir = Path::new("/nonexistent");
    assert_eq!(install_command(&repo("npm", true), dir).unwrap().1[0], "ci");
    assert!(
        install_command(&repo("pnpm", true), dir)
            .unwrap()
            .1
            .iter()
            .any(|a| a == "--ignore-scripts")
    );
    let mut overridden = repo("pnpm", true);
    overridden.install_command = vec!["npx".into(), "pnpm@9".into(), "install".into()];
    assert_eq!(
        install_command(&overridden, dir).unwrap(),
        ("npx".to_owned(), vec!["pnpm@9".to_owned(), "install".to_owned()])
    );
    assert!(install_command(&repo("rush", true), dir).is_err());
    assert!(install_command(&repo("pnpm", false), dir).unwrap_err().contains("why"));
}

#[test]
fn member_keys_drop_the_qualifier_on_both_sides() {
    let skif = json!({ "diagnostics": [
        { "rule": "unused-enum-members", "message": "enum member `Color.Red` is never used", "location": { "path": "src/e.ts" } },
        { "rule": "unused-namespace-members", "message": "namespace member `ns.Helper` is never used", "location": { "path": "src/n.ts" } },
    ]});
    let knip = json!({ "issues": [
        { "file": "src/e.ts", "enumMembers": { "Color": [{ "name": "Red" }] } },
        { "file": "src/n.ts", "namespaceMembers": [{ "name": "ns.Helper" }] },
    ]});
    let root = Path::new("/r");
    let (counts, _) = diff::compare(&skif_keys(&skif, root), &knip_keys(&knip, root));
    assert_eq!(counts["enumMembers"].both, 1);
    assert_eq!(counts["namespaceMembers"].both, 1);
    assert_eq!(counts["enumMembers"].skif_only + counts["enumMembers"].tool_only, 0);
}

/// The JSON that `knip --reporter json` prints: every reported issue type is present on every row, empty or not,
/// members are a flat list of `{ namespace, name }` (the parent first, the bare member name second), and a module
/// that is only read through a namespace import lists its unused exports and types with that namespace.
fn knip_report() -> Value {
    json!({ "issues": [
        { "file": "src/e.ts", "binaries": [], "dependencies": [], "duplicates": [],
          "enumMembers": [{ "namespace": "Color", "name": "Green", "line": 1, "col": 26, "pos": 25 }],
          "exports": [], "files": [], "namespaceMembers": [],
          "types": [{ "name": "Unused", "line": 2, "col": 13, "pos": 45 }], "unlisted": [], "unresolved": [] },
        { "file": "src/n.ts", "enumMembers": [],
          "exports": [{ "namespace": "ns", "name": "other", "line": 2, "col": 14, "pos": 36 }],
          "namespaceMembers": [{ "namespace": "Helpers", "name": "run", "line": 4, "col": 3, "pos": 80 }],
          "types": [{ "namespace": "ns", "name": "B", "line": 3, "col": 13, "pos": 60 }] },
    ]})
}

#[test]
fn maps_the_json_knip_prints() {
    let keys = knip_keys(&knip_report(), Path::new("/r"));
    assert_eq!(set(&keys, "enumMembers"), ["src/e.ts|Green"]);
    assert_eq!(set(&keys, "types"), ["src/e.ts|Unused"]);
    assert_eq!(set(&keys, "nsExports"), ["src/n.ts|other"]);
    assert_eq!(set(&keys, "nsTypes"), ["src/n.ts|B"]);
    assert_eq!(set(&keys, "namespaceMembers"), ["src/n.ts|run"]);
    assert!(!keys.contains_key("exports"));
    assert!(!keys.contains_key("duplicates"));
}

#[test]
fn every_member_and_namespace_finding_agrees_with_the_json_knip_prints() {
    let skif = json!({ "diagnostics": [
        { "rule": "unused-enum-members", "message": "enum member `Color.Green` is never used", "location": { "path": "src/e.ts" } },
        { "rule": "unused-types", "message": "type `Unused` is never imported by another module", "location": { "path": "src/e.ts" } },
        { "rule": "unused-namespace-exports", "message": "export `other` is never read through a namespace import", "location": { "path": "src/n.ts" } },
        { "rule": "unused-namespace-types", "message": "type `B` is never read through a namespace import", "location": { "path": "src/n.ts" } },
        { "rule": "unused-namespace-members", "message": "namespace member `Helpers.run` is never used", "location": { "path": "src/n.ts" } },
    ]});
    let root = Path::new("/r");
    let (counts, _) = diff::compare(&skif_keys(&skif, root), &knip_keys(&knip_report(), root));
    let both = Counts {
        skif_only: 0,
        tool_only: 0,
        both: 1,
    };
    for category in ["enumMembers", "types", "nsExports", "nsTypes", "namespaceMembers"] {
        assert_eq!(counts[category], both, "{category}");
    }
}

#[test]
fn corpus_entry_files_reach_both_tools_after_the_defaults() {
    assert_eq!(knip_config(&[]), "{}\n");
    let entry = ["src/vs/workbench/workbench.desktop.main.ts".to_owned()];
    let knip: Value = serde_json::from_str(&knip_config(&entry)).unwrap();
    let expected = json!([DEFAULT_ENTRY[0], DEFAULT_ENTRY[1], entry[0]]);
    assert_eq!(knip["workspaces"]["."]["entry"], expected);
    let skif: toml::Value = toml::from_str(&skif_entry_config(&entry)).unwrap();
    let listed: Vec<&str> = skif["js"]["workspaces"]["."]["entry"]
        .as_array()
        .unwrap()
        .iter()
        .map(|glob| glob.as_str().unwrap())
        .collect();
    assert_eq!(listed, [DEFAULT_ENTRY[0], DEFAULT_ENTRY[1], entry[0].as_str()]);
}

#[test]
fn finds_the_report_among_banners() {
    let noisy = b"Platform libraries usePool true\n[dotenv@17] injected env (0) from .env\n{\"issues\":[{\"file\":\"a.ts\",\"exports\":[{\"name\":\"x\"}]}]}\n";
    let doc = parse_knip_report(noisy).unwrap();
    assert_eq!(doc["issues"][0]["file"], "a.ts");
    let skipped_object = b"{\"log\":1}\n{\"files\":[\"b.ts\"],\"issues\":[]}\n";
    assert_eq!(parse_knip_report(skipped_object).unwrap()["files"][0], "b.ts");
    let pretty = b"banner\n{\n  \"issues\": []\n}\n";
    assert!(parse_knip_report(pretty).is_ok());
}

#[test]
fn reports_why_no_report_was_found() {
    assert_eq!(parse_knip_report(b"  \n").unwrap_err(), "stdout is empty");
    assert!(
        parse_knip_report(b"just text\n")
            .unwrap_err()
            .contains("no knip report")
    );
    assert!(
        parse_knip_report(b"{\"issues\": [oops\n")
            .unwrap_err()
            .contains("first parse error")
    );
    assert!(parse_knip_report(b"{\"other\":1}\n").is_err());
}

#[test]
fn classifies_unused_files_knip_alone_reports() {
    assert_eq!(classify_file("src/app.css", false), Some(NON_CODE_FILE));
    assert_eq!(classify_file("docs/index.mdx", true), Some(NON_CODE_FILE));
    assert_eq!(classify_file("pkg/test/a.ts", false), Some(TEST_FILE));
    assert_eq!(classify_file("pkg/src/a.spec.ts", false), Some(TEST_FILE));
    assert_eq!(classify_file("pkg/src/a_spec.ts", false), Some(TEST_FILE));
    assert_eq!(classify_file("pkg/__fixtures__/a.ts", false), Some(FIXTURE_FILE));
    assert_eq!(classify_file("pkg/fixtures/test/a.ts", false), Some(FIXTURE_FILE));
    assert_eq!(
        classify_file("pkg/x/colorize-fixtures/a.ts", false),
        None,
        "skif judges other test-data names"
    );
    assert_eq!(classify_file("pkg/testdata/a.ts", false), None);
    assert_eq!(classify_file("pkg/fixture/test/a.ts", false), Some(TEST_FILE));
    assert_eq!(classify_file("pkg/test/a.ts", true), None);
    assert_eq!(classify_file("pkg/fixtures/a.ts", true), None);
    assert_eq!(classify_file("pkg/src/a.ts", false), None);
    assert_eq!(classify_file("pkg/src/latest.tsx", false), None);
    assert_eq!(classify_file("typings/legacy.d.ts", false), Some(DECLARATION_FILE));
    assert_eq!(
        classify_file("pkg/test/globals.d.ts", true),
        Some(DECLARATION_FILE),
        "whatever `--include-test-files` says"
    );
    assert_eq!(classify_file("lib/watch.d.mts", false), None, "skif judges `.d.mts`");
    assert_eq!(classify_file("lib/types.d.cts", false), None);
    assert_eq!(classify_file("src/d.ts", false), None);
}

#[test]
fn unlisted_imports_only_foreign_tsconfig_paths_map_split_off() {
    let skif_doc = json!({ "diagnostics": [
        { "rule": "unlisted-dependencies", "message": "`shared/value` resolves only through the `paths` of `packages/app/tsconfig.json`; add the mapping to this package's tsconfig or import it by package name",
          "location": { "path": "/r/packages/lib/src/index.ts" }, "dependency": "shared", "detail": "ForeignTsconfigPaths" },
        { "rule": "unlisted-dependencies", "message": "`left-pad` is imported but not listed in `package.json`",
          "location": { "path": "/r/packages/lib/src/index.ts" }, "dependency": "left-pad" },
        { "rule": "unused-files", "message": "`x.ts` is unused", "location": { "path": "/r/packages/lib/src/x.ts" },
          "detail": "ForeignTsconfigPaths" },
    ]});
    let root = Path::new("/r");
    let foreign = foreign_tsconfig_paths_keys(&skif_doc, root);
    assert_eq!(
        foreign.iter().map(String::as_str).collect::<Vec<_>>(),
        ["packages/lib/src/index.ts|shared"]
    );
    let skif = skif_keys(&skif_doc, root);
    let knip = Keys::from([(
        "unlisted".to_owned(),
        ["packages/lib/src/index.ts|shared"].map(str::to_owned).into(),
    )]);
    let split = diff::split_known(&skif, &Keys::new(), &|side, category, key| {
        (side == Side::Skif)
            .then(|| skif_only_deviation(category, key, &foreign))
            .flatten()
    });
    let (counts, _) = diff::compare(&split.skif, &split.tool);
    assert_eq!(
        counts["unlisted"],
        Counts {
            skif_only: 1,
            tool_only: 0,
            both: 0
        },
        "the plain unlisted import stays a difference"
    );
    assert_eq!(split.known[FOREIGN_TSCONFIG_PATHS].skif_only, 1);
    assert_eq!(split.known.len(), 1, "{:?}", split.known);
    // A finding both tools report is agreement, never a deviation.
    let both = diff::split_known(&skif, &knip, &|side, category, key| {
        (side == Side::Skif)
            .then(|| skif_only_deviation(category, key, &foreign))
            .flatten()
    });
    assert!(both.known.is_empty(), "{:?}", both.known);
    assert_eq!(
        skif_only_deviation("files", "packages/lib/src/index.ts|shared", &foreign),
        None
    );
}

#[test]
fn unused_files_only_known_deviations_split_off() {
    let skif = Keys::from([("files".to_owned(), ["src/dead.ts".to_owned()].into())]);
    let knip = Keys::from([(
        "files".to_owned(),
        ["src/dead.ts", "src/a.css", "test/a.ts", "src/real.ts"]
            .map(str::to_owned)
            .into(),
    )]);
    let split = diff::split_known(&skif, &knip, &|side, category, key| {
        (side == Side::Tool && category == "files")
            .then(|| classify_file(key, false))
            .flatten()
    });
    let (counts, _) = diff::compare(&split.skif, &split.tool);
    assert_eq!(
        counts["files"],
        Counts {
            skif_only: 0,
            tool_only: 1,
            both: 1
        }
    );
    assert_eq!(split.known[NON_CODE_FILE].tool_only, 1);
    assert_eq!(split.known[TEST_FILE].tool_only, 1);
}

#[test]
fn the_same_export_under_another_label_is_agreement() {
    let mut skif = Keys::from([
        (
            "exports".to_owned(),
            ["a.ts|ns", "a.ts|plain", "a.ts|mine"].map(str::to_owned).into(),
        ),
        ("types".to_owned(), ["b.ts|Shared"].map(str::to_owned).into()),
    ]);
    let knip = Keys::from([
        ("exports".to_owned(), ["a.ts|plain"].map(str::to_owned).into()),
        ("nsExports".to_owned(), ["a.ts|ns"].map(str::to_owned).into()),
        ("types".to_owned(), ["b.ts|Shared"].map(str::to_owned).into()),
    ]);
    align_export_labels(&mut skif, &knip);
    let (counts, _) = diff::compare(&skif, &knip);
    assert_eq!(
        counts["exports"],
        Counts {
            skif_only: 1,
            tool_only: 0,
            both: 1
        }
    );
    assert_eq!(
        counts["nsExports"],
        Counts {
            skif_only: 0,
            tool_only: 0,
            both: 1
        }
    );
    assert_eq!(counts["types"].both, 1);
}

#[test]
fn an_import_one_tool_reports_unlisted_and_the_other_unresolved_is_agreement() {
    let mut skif = Keys::from([
        (
            "unlisted".to_owned(),
            strings(&[
                "packages/a/tsconfig.json|tslib",
                "packages/b/tsconfig.json|vite",
                "packages/c/src/a.ts|left-pad",
                "packages/d/tsconfig.json|vitest",
                "packages/f/src/a.ts|shared",
            ]),
        ),
        (
            "unresolved".to_owned(),
            strings(&["packages/e/src/a.ts|@scope/x/sub", "packages/e/src/b.ts|./gone"]),
        ),
    ]);
    let knip = Keys::from([
        (
            "unresolved".to_owned(),
            strings(&[
                "packages/a/tsconfig.json|tslib",
                "packages/b/tsconfig.json|vite/client",
                "packages/c/src/b.ts|left-pad",
                "packages/d/tsconfig.json|vitest/globals",
            ]),
        ),
        (
            "unlisted".to_owned(),
            strings(&[
                "packages/d/tsconfig.json|vitest",
                "packages/e/src/a.ts|@scope/x",
                "packages/e/src/b.ts|gone",
            ]),
        ),
    ]);
    align_unlisted_unresolved(&mut skif, &knip);
    assert_eq!(
        set(&skif, "unresolved"),
        [
            "packages/a/tsconfig.json|tslib",
            "packages/b/tsconfig.json|vite/client",
            "packages/e/src/b.ts|./gone",
        ]
    );
    assert_eq!(
        set(&skif, "unlisted"),
        [
            "packages/c/src/a.ts|left-pad",
            "packages/d/tsconfig.json|vitest",
            "packages/e/src/a.ts|@scope/x",
            "packages/f/src/a.ts|shared",
        ],
        "another file, a finding knip shares already and a relative specifier stay where they are"
    );
    let (counts, _) = diff::compare(&skif, &knip);
    assert_eq!(counts["unresolved"].both, 2);
    assert_eq!(counts["unlisted"].both, 2);
}

#[test]
fn an_unresolved_import_skif_reports_as_a_stale_mock_on_the_same_file_and_specifier_is_agreement() {
    let doc = json!({ "diagnostics": [
        { "rule": "stale-mocks", "message": "`jest.requireActual('./pathToModule')` names a module that does not exist", "location": { "path": "/r/packages/t/a.test.ts" } },
        { "rule": "stale-mocks", "message": "`vi.mock('./gone')` names a module that does not exist", "location": { "path": "/r/packages/t/b.test.ts" } },
        { "rule": "unresolved-imports", "message": "`./other` cannot be resolved", "location": { "path": "/r/packages/t/c.ts" } },
    ]});
    let stale = stale_mock_keys(&doc, Path::new("/r"));
    assert_eq!(
        stale,
        strings(&["packages/t/a.test.ts|./pathToModule", "packages/t/b.test.ts|./gone"])
    );
    let mut skif = Keys::new();
    let knip = Keys::from([(
        "unresolved".to_owned(),
        strings(&[
            "packages/t/a.test.ts|./pathToModule",
            "packages/t/b.test.ts|./elsewhere",
            "packages/t/c.ts|./pathToModule",
        ]),
    )]);
    align_stale_mocks(&mut skif, &knip, &stale);
    assert_eq!(
        set(&skif, "unresolved"),
        ["packages/t/a.test.ts|./pathToModule"],
        "another specifier in the mocking file, or the same specifier in another file, stays one-sided"
    );
    let (counts, _) = diff::compare(&skif, &knip);
    assert_eq!(counts["unresolved"].both, 1);
    assert_eq!(counts["unresolved"].tool_only, 2);
}

#[test]
fn knip_only_findings_in_packages_skif_did_not_judge_split_off() {
    let skif_doc = json!({ "diagnostics": [], "incomplete-packages": [
        { "name": "web", "dir": "packages/web", "reasons": [{ "cause": "unknown-spread" }] },
    ]});
    let dirs = not_judged_dirs(&skif_doc);
    assert_eq!(
        dirs,
        BTreeMap::from([("packages/web".to_owned(), NotJudgedScope::Package)])
    );
    assert_eq!(
        not_judged_dirs(&json!({ "incomplete-packages": [{ "dir": ".", "scope": "package" }] })),
        BTreeMap::from([(String::new(), NotJudgedScope::Package)])
    );
    assert!(not_judged_dirs(&json!({ "diagnostics": [] })).is_empty());

    let packages = ["packages/web", "packages/web/nested", "packages/api"];
    let is_package = |dir: &str| packages.contains(&dir);
    assert_eq!(owner_dir("packages/web/src/a.ts", &is_package), "packages/web");
    assert_eq!(owner_dir("packages/web/package.json|vite", &is_package), "packages/web");
    assert_eq!(
        owner_dir("packages/web/nested/a.ts", &is_package),
        "packages/web/nested"
    );
    assert_eq!(owner_dir("scripts/a.ts", &is_package), "");

    let knip = Keys::from([
        (
            "files".to_owned(),
            [
                "packages/web/src/orphan.ts",
                "packages/web/nested/orphan.ts",
                "packages/api/orphan.ts",
            ]
            .map(str::to_owned)
            .into(),
        ),
        (
            "devDependencies".to_owned(),
            ["packages/web/package.json|vite"].map(str::to_owned).into(),
        ),
        (
            "exports".to_owned(),
            ["packages/web/src/a.ts|unused"].map(str::to_owned).into(),
        ),
    ]);
    let split = diff::split_known(&Keys::new(), &knip, &|side, category, key| {
        (side == Side::Tool && not_judged(category, key, &dirs, &is_package)).then_some(NOT_JUDGED)
    });
    assert_eq!(
        split.known[NOT_JUDGED],
        diff::KnownCounts {
            skif_only: 0,
            tool_only: 3
        }
    );
    let (counts, _) = diff::compare(&split.skif, &split.tool);
    assert_eq!(
        counts["files"].tool_only, 2,
        "a nested complete package and another package stay one-sided"
    );
    assert!(
        !counts.contains_key("exports"),
        "skif does not judge the exports of an incomplete package (ADR 0014)"
    );
    assert!(!counts.contains_key("devDependencies"));
}

#[test]
fn not_judged_covers_exports_types_and_members_but_not_unlisted_or_duplicates() {
    let dirs = BTreeMap::from([("packages/web".to_owned(), NotJudgedScope::Package)]);
    let is_package = |dir: &str| dir == "packages/web";
    for category in [
        "exports",
        "types",
        "nsExports",
        "nsTypes",
        "enumMembers",
        "namespaceMembers",
    ] {
        assert!(
            not_judged(category, "packages/web/src/a.ts|x", &dirs, &is_package),
            "{category}"
        );
    }
    for category in [
        "unlisted",
        "unresolved",
        "binaries",
        "duplicates",
        "optionalPeerDependencies",
    ] {
        assert!(
            !not_judged(category, "packages/web/src/a.ts|x", &dirs, &is_package),
            "{category}"
        );
    }
    assert!(!not_judged("exports", "packages/api/src/a.ts|x", &dirs, &is_package));
}

#[test]
fn a_narrowed_scope_sets_apart_only_the_files_its_globs_match_and_every_dependency() {
    let skif_doc = json!({ "diagnostics": [], "incomplete-packages": [
        { "name": "web", "dir": "packages/web", "reasons": [{ "cause": "unknown-config", "plugin": "storybook" }],
          "scope": { "within": ["src/**/*.stories.tsx"] } },
        { "name": "web", "dir": "packages/web", "reasons": [{ "cause": "unknown-config", "plugin": "vitest" }],
          "scope": { "within": ["test/*.ts", "src/**/*.stories.tsx"] } },
        { "name": "root", "dir": ".", "reasons": [{ "cause": "unknown-config" }],
          "scope": { "within": ["scripts/*.ts"] } },
        { "name": "api", "dir": "packages/api", "reasons": [{ "cause": "unknown-spread" }], "scope": "package" },
    ]});
    let dirs = not_judged_dirs(&skif_doc);
    assert_eq!(
        dirs,
        BTreeMap::from([
            (String::new(), NotJudgedScope::within(vec!["scripts/*.ts".to_owned()])),
            ("packages/api".to_owned(), NotJudgedScope::Package),
            (
                "packages/web".to_owned(),
                NotJudgedScope::within(vec!["src/**/*.stories.tsx".to_owned(), "test/*.ts".to_owned()])
            ),
        ]),
        "the scopes of one directory are joined"
    );
    let packages = ["packages/web", "packages/api"];
    let is_package = |dir: &str| packages.contains(&dir);
    for (category, key) in [
        ("files", "packages/web/src/ui/Button.stories.tsx"),
        ("exports", "packages/web/src/ui/Button.stories.tsx|Primary"),
        ("types", "packages/web/test/setup.ts|Fixture"),
        ("enumMembers", "packages/web/test/setup.ts|Red"),
        ("dependencies", "packages/web/package.json|react"),
        ("devDependencies", "packages/web/package.json|vitest"),
        ("files", "scripts/release.ts"),
        ("devDependencies", "package.json|tsx"),
        ("files", "packages/api/src/orphan.ts"),
    ] {
        assert!(not_judged(category, key, &dirs, &is_package), "{category} {key}");
    }
    for (category, key) in [
        ("files", "packages/web/src/ui/Button.tsx"),
        ("exports", "packages/web/src/ui/Button.tsx|Button"),
        ("files", "packages/web/test/deep/setup.ts"),
        ("files", "scripts/deep/release.ts"),
        ("files", "tools/a.ts"),
        ("unlisted", "packages/web/src/ui/Button.stories.tsx|react"),
    ] {
        assert!(!not_judged(category, key, &dirs, &is_package), "{category} {key}");
    }
    assert_eq!(
        not_judged_dirs(&json!({ "incomplete-packages": [
            { "dir": "packages/web", "scope": { "within": ["src/*.ts"] } },
            { "dir": "packages/web", "scope": "package" },
        ]})),
        BTreeMap::from([("packages/web".to_owned(), NotJudgedScope::Package)]),
        "a whole-package entry wins over a narrowed one"
    );
}

#[test]
fn a_narrowed_scope_reaches_the_files_of_the_packages_below_its_package() {
    let dirs = BTreeMap::from([
        (
            String::new(),
            NotJudgedScope::within(vec!["packages/*/e2e/**".to_owned()]),
        ),
        (
            "packages/core".to_owned(),
            NotJudgedScope::within(vec!["**/__tests__/**".to_owned()]),
        ),
        ("apps".to_owned(), NotJudgedScope::Package),
    ]);
    let packages = [
        "packages/core",
        "packages/core/nested",
        "packages/web",
        "apps",
        "apps/site",
    ];
    let is_package = |dir: &str| packages.contains(&dir);
    for (category, key) in [
        ("files", "packages/core/nested/src/__tests__/helper.ts"),
        ("exports", "packages/core/nested/src/__tests__/helper.ts|setup"),
        ("types", "packages/web/e2e/run.ts|Options"),
        ("files", "packages/web/e2e/fixtures/app.ts"),
    ] {
        assert!(not_judged(category, key, &dirs, &is_package), "{category} {key}");
    }
    for (category, key) in [
        ("files", "packages/core/nested/src/helper.ts"),
        ("files", "packages/web/src/e2e/run.ts"),
        ("devDependencies", "packages/web/package.json|playwright"),
        ("devDependencies", "packages/core/nested/package.json|vitest"),
        ("unlisted", "packages/web/e2e/run.ts|react"),
        ("files", "apps/site/src/orphan.ts"),
    ] {
        assert!(!not_judged(category, key, &dirs, &is_package), "{category} {key}");
    }
}

#[test]
fn reads_the_config_files_knip_could_not_load() {
    let stderr = "❌ Invalid environment variables: [\n\
            ERROR: Error loading /r/apps/expo/metro.config.js (Cannot find module 'metro-cache')\n\
            ERROR: Please fix or visit https://knip.dev/reference/known-issues\n\
            ERROR: Error loading packages/db/drizzle.config.ts (Missing POSTGRES_URL)\n";
    let errors = knip_config_errors(stderr, Path::new("/r"));
    assert_eq!(
        errors,
        BTreeSet::from([
            "apps/expo/metro.config.js".to_owned(),
            "packages/db/drizzle.config.ts".to_owned()
        ])
    );
    assert!(knip_config_errors("ERROR: something else\n", Path::new("/r")).is_empty());
}

#[test]
fn the_summary_counts_not_judged_findings_apart_from_the_known_deviations() {
    let counts = BTreeMap::from([(
        "files".to_owned(),
        Counts {
            skif_only: 1,
            tool_only: 2,
            both: 3,
        },
    )]);
    let known = diff::Known::from([
        (
            NOT_JUDGED.to_owned(),
            diff::KnownCounts {
                skif_only: 0,
                tool_only: 4,
            },
        ),
        (
            NON_CODE_FILE.to_owned(),
            diff::KnownCounts {
                skif_only: 0,
                tool_only: 5,
            },
        ),
    ]);
    let entries = BTreeMap::from([
        ("r".to_owned(), Entry::compared(counts.clone()).with_known(known)),
        ("s".to_owned(), Entry::compared(counts.clone()).with_tool_errors(2)),
        (
            "t".to_owned(),
            Entry::compared(counts)
                .with_tool_errors(1)
                .with_invalid(Some("configs import unbuilt packages".to_owned())),
        ),
        (
            "u".to_owned(),
            Entry::skipped("knip failed (exit 2): no report; stderr: a | b\nc"),
        ),
    ]);
    let table = markdown("knip", &entries);
    assert!(table.contains("Dependencies installed: no."), "{table}");
    assert!(
            table.contains(
                "| t | compared; knip could not load 1 config files; not a valid comparison: configs import unbuilt packages | 1 | 2 | 3 | 0 | 0 |"
            ),
            "{table}"
        );
    assert!(table.contains("| r | compared | 1 | 2 | 3 | 4 | 5 |"), "{table}");
    assert!(
        table.contains("| u | skipped: knip failed (exit 2): no report; stderr: a \\| b c | - | - | - | - | - |\n"),
        "{table}"
    );
    assert!(
        table.contains("| s | compared; knip could not load 2 config files | 1 | 2 | 3 | 0 | 0 |"),
        "{table}"
    );
    assert!(table.contains("`not-judged-by-skif`"), "{table}");
}

#[cfg(unix)]
#[test]
fn a_build_runs_within_its_own_time_limit_and_a_failed_one_writes_no_marker() {
    let dir = std::env::temp_dir().join(format!("skif-bench-parity-build-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let scratch = dir.join("scratch");
    let mut repo: Repo = toml::from_str(&format!(
        "name = \"r\"\nurl = \"u\"\nrev = \"{}\"\npackage-manager = \"pnpm\"\n\
             install-command = [\"sh\", \"-c\", \"exit 0\"]\n\
             build-command = [\"sleep\", \"30\"]\nbuild-timeout-secs = 1\n",
        "a".repeat(40)
    ))
    .unwrap();
    assert_eq!(repo.build_timeout_secs, Some(1));
    let reason = install(&repo, &dir, &scratch, Duration::from_secs(600)).unwrap();
    assert_eq!(reason.as_deref(), Some("sleep build timed out after 1 s"));
    assert!(
        !is_installed(&repo, &dir),
        "a timed-out build leaves the checkout not installed"
    );

    // The builds below finish at once; a generous limit keeps a loaded machine from timing them out.
    repo.build_timeout_secs = Some(120);
    repo.build_command = vec!["sh".to_owned(), "-c".to_owned(), "exit 4".to_owned()];
    let reason = install(&repo, &dir, &scratch, Duration::from_secs(600))
        .unwrap()
        .unwrap();
    assert!(reason.starts_with("sh build failed"), "{reason}");
    assert!(!is_installed(&repo, &dir));

    repo.build_command = vec!["sh".to_owned(), "-c".to_owned(), "exit 0".to_owned()];
    assert_eq!(install(&repo, &dir, &scratch, Duration::from_secs(600)).unwrap(), None);
    assert!(is_installed(&repo, &dir));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_install_state_comes_from_the_checkout_markers() {
    let dir = std::env::temp_dir().join(format!("skif-bench-parity-installed-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let mut repo: Repo = toml::from_str(&format!(
        "name = \"r\"\nurl = \"u\"\nrev = \"{}\"\npackage-manager = \"pnpm\"\n",
        "a".repeat(40)
    ))
    .unwrap();
    assert!(!is_installed(&repo, &dir));
    fs::write(dir.join(INSTALLED_MARKER), format!("{}\n", "a".repeat(40))).unwrap();
    assert!(is_installed(&repo, &dir));
    repo.build_command = vec!["pnpm".to_owned(), "run".to_owned(), "build".to_owned()];
    assert!(
        !is_installed(&repo, &dir),
        "a marker written without the build is stale"
    );
    fs::write(dir.join(INSTALLED_MARKER), install_marker(&repo)).unwrap();
    assert!(is_installed(&repo, &dir));
    assert_eq!(
        install_marker(&repo),
        format!("{}\nbuild: pnpm run build", "a".repeat(40))
    );
    repo.install_command = vec!["pnpm".to_owned(), "install".to_owned()];
    assert!(
        !is_installed(&repo, &dir),
        "a marker written with another install command is stale"
    );
    assert_eq!(
        install_marker(&repo),
        format!("{}\ninstall: pnpm install\nbuild: pnpm run build", "a".repeat(40))
    );
    fs::write(dir.join(INSTALLED_MARKER), install_marker(&repo)).unwrap();
    assert!(is_installed(&repo, &dir));
    repo.rev = "b".repeat(40);
    assert!(!is_installed(&repo, &dir), "a marker of another commit is stale");
    fs::remove_dir_all(&dir).unwrap();

    let entry = |installed: bool| Entry::compared(BTreeMap::new()).with_installed(installed);
    let mut entries = BTreeMap::from([
        ("a".to_owned(), entry(true)),
        ("b".to_owned(), entry(true)),
        ("skipped".to_owned(), Entry::skipped("knip failed")),
    ]);
    assert_eq!(installed_summary(&entries), "yes");
    entries.insert("c".to_owned(), entry(false));
    assert_eq!(installed_summary(&entries), "partial (2 of 3)");
    assert_eq!(
        installed_summary(&BTreeMap::from([("c".to_owned(), entry(false))])),
        "no"
    );
}

#[test]
fn the_recorded_category_set_holds_every_compared_category_and_deviation() {
    let set = category_set();
    assert_eq!(set.compared.len(), KNIP_CATEGORIES.len());
    assert!(
        set.compared.windows(2).all(|pair| pair[0] < pair[1]),
        "sorted: {:?}",
        set.compared
    );
    for id in [
        IN_KNIP_UNUSED_FILE,
        IN_SKIF_UNUSED_FILE,
        USED_IN_KNIP_UNUSED_FILE,
        USED_IN_SKIF_UNUSED_FILE,
        IMPORTED_BY_KNIP_UNUSED_FILE,
        IMPORTED_BY_SKIF_UNUSED_FILE,
        ENTRY_MODULE_DUPLICATE,
        BAZEL_BUILD_FILE,
        DECLARATION_FILE,
        SIGNATURE_TYPE,
        KNIP_CONFIG_ERROR,
        NOT_JUDGED,
        TYPE_RESOLVED,
    ] {
        assert!(set.known_deviations.iter().any(|known| known == id), "{id}");
    }
    assert_eq!(set.known_deviations.len(), KNIP_DEVIATIONS.len());
}

#[test]
fn type_resolved_takes_the_one_sided_findings_only_one_mode_has_after_the_other_deviations() {
    let keys = |items: &[(&str, &[&str])]| -> Keys {
        items
            .iter()
            .map(|(category, names)| ((*category).to_owned(), names.iter().map(|n| (*n).to_owned()).collect()))
            .collect()
    };
    let typed = keys(&[(
        "enumMembers",
        &["a.ts|E.both", "a.ts|E.typed", "test/t.ts|E.typed", "a.ts|E.always"],
    )]);
    let untyped = keys(&[("enumMembers", &["a.ts|E.both", "a.ts|E.untyped", "a.ts|E.always"])]);
    let knip = keys(&[("enumMembers", &["a.ts|E.both", "a.ts|E.untyped", "a.ts|E.knip"])]);
    let classify = |side, category: &str, key: &str| {
        let known = key.starts_with("test/").then_some(TEST_FILE);
        with_type_resolved(known, (side, category, key), Some(&untyped))
    };
    let split = diff::split_known(&typed, &knip, &classify);
    assert_eq!(
        split.known[TYPE_RESOLVED],
        diff::KnownCounts {
            skif_only: 1,
            tool_only: 1
        }
    );
    assert_eq!(
        split.samples[TYPE_RESOLVED],
        ["enumMembers|a.ts|E.typed", "enumMembers|a.ts|E.untyped"]
    );
    assert_eq!(
        split.known[TEST_FILE],
        diff::KnownCounts {
            skif_only: 1,
            tool_only: 0
        },
        "another deviation explains a finding first"
    );
    assert_eq!(split.skif, keys(&[("enumMembers", &["a.ts|E.always", "a.ts|E.both"])]));
    assert_eq!(split.tool, keys(&[("enumMembers", &["a.ts|E.both", "a.ts|E.knip"])]));

    let without_pass = diff::split_known(&typed, &knip, &|side, category, key| {
        with_type_resolved(None, (side, category, key), None)
    });
    assert!(without_pass.known.is_empty(), "{:?}", without_pass.known);
}

#[test]
fn a_rush_repository_gets_a_root_manifest_from_rush_json() {
    let rush = r#"/**
 * Rush's main config, see https://rushjs.io // not a comment end
 */
{
  "$schema": "https://developer.microsoft.com/json-schemas/rush/v5/rush.schema.json",
  // "projects": [] in a comment is ignored
  "rushVersion": "5.0.0",
  "note": "a \"quoted\" /* string */ stays",
  "projects": [
    { "packageName": "@rushstack/heft", "projectFolder": "apps/heft" },
    /* a block comment */
    { "packageName": "@rushstack/node-core-library", "projectFolder": "libraries/node-core-library", },
  ],
}
"#;
    let plain: Value = serde_json::from_str(&strip_jsonc(rush)).unwrap();
    assert_eq!(plain["note"], r#"a "quoted" /* string */ stays"#);
    assert_eq!(plain["projects"].as_array().unwrap().len(), 2);
    let manifest: Value = serde_json::from_str(&rush_root_manifest(rush).unwrap()).unwrap();
    assert_eq!(
        manifest,
        json!({ "//": SCRATCH_ROOT_MARK, "private": true, "workspaces": ["apps/heft", "libraries/node-core-library"] })
    );
    assert!(
        rush_root_manifest(r#"{ "projects": [] }"#)
            .unwrap_err()
            .contains("no projectFolder")
    );
    assert!(
        rush_root_manifest("{ not json")
            .unwrap_err()
            .contains("parsing rush.json")
    );
}

#[test]
fn knip_gets_a_root_manifest_in_a_rush_checkout_only_while_it_runs() {
    let mut repo: Repo = toml::from_str(&format!(
        "name = \"r\"\nurl = \"u\"\nrev = \"{}\"\npackage-manager = \"rush\"\n",
        "a".repeat(40)
    ))
    .unwrap();
    let missing = Path::new("/nonexistent/checkout");
    assert!(knip_root(&repo, missing).unwrap().is_none());
    repo.knip_root = Some("lerna".to_owned());
    assert!(knip_root(&repo, missing).err().unwrap().contains("unknown knip-root"));
    repo.knip_root = Some(RUSH_KNIP_ROOT.to_owned());
    assert!(knip_root(&repo, missing).err().unwrap().contains("reading rush.json"));

    let root = std::env::temp_dir().join(format!("skif-bench-parity-rush-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("rush.json"),
        r#"{ "projects": [{ "packageName": "a", "projectFolder": "apps/a" }] }"#,
    )
    .unwrap();
    let manifest = root.join("package.json");
    let guard = knip_root(&repo, &root).unwrap().unwrap();
    let written: Value = serde_json::from_str(&fs::read_to_string(&manifest).unwrap()).unwrap();
    assert_eq!(written["workspaces"], json!(["apps/a"]));
    assert!(is_scratch_root(&manifest));
    drop(guard);
    assert!(
        !manifest.exists(),
        "the checkout has no root manifest after the knip run"
    );

    fs::write(&manifest, written.to_string()).unwrap();
    drop(knip_root(&repo, &root).unwrap().unwrap());
    assert!(
        !manifest.exists(),
        "a manifest an interrupted run left is replaced and removed"
    );

    fs::write(&manifest, r#"{ "name": "real" }"#).unwrap();
    assert!(knip_root(&repo, &root).err().unwrap().contains("needs no knip-root"));
    assert_eq!(fs::read_to_string(&manifest).unwrap(), r#"{ "name": "real" }"#);
    fs::remove_dir_all(&root).unwrap();
}

fn strings(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}

static NO_PATHS: [String; 0] = [];
static NO_KEYS: BTreeSet<String> = BTreeSet::new();
static NO_READS: KnipReadSets = KnipReadSets {
    opaque_imports: BTreeSet::new(),
    import_defaults: BTreeSet::new(),
    required_actuals: BTreeSet::new(),
    spied_namespaces: BTreeSet::new(),
    enumerated_namespaces: BTreeSet::new(),
    import_types: BTreeSet::new(),
    namespace_reexports: BTreeSet::new(),
    typescript_commonjs: BTreeSet::new(),
    declaration_reexports: BTreeSet::new(),
    node_test_files: BTreeSet::new(),
    ng_package_reexports: BTreeSet::new(),
};
static ALL_JUDGED: BTreeMap<String, NotJudgedScope> = BTreeMap::new();
static NO_DEPENDENCY_READS: DependencyReads = DependencyReads {
    tool: BTreeMap::new(),
    skif: BTreeMap::new(),
};

/// A classifier over `layout` that knows nothing of the two reports: no path knip does not read, no package skif
/// left unjudged or knip could not load a config of, no unused file and nothing one imports, no root of a
/// recursive run and no Rush root. A test sets the facts it needs with `..bare(&layout)`.
fn bare<'a>(layout: &'a Layout<'a>) -> Classifier<'a> {
    Classifier {
        knip_unread: &NO_PATHS,
        include_test_files: false,
        not_judged: &ALL_JUDGED,
        knip_failed: &NO_KEYS,
        knip_manifests: &NO_KEYS,
        foreign_paths: &NO_KEYS,
        knip_unused_files: &NO_KEYS,
        skif_unused_files: &NO_KEYS,
        knip_unused_imports: &NO_KEYS,
        skif_unused_imports: &NO_KEYS,
        skif_root_packages: &NO_KEYS,
        published_reach: &NO_KEYS,
        knip_unused_credits: &NO_KEYS,
        skif_unused_credits: &NO_KEYS,
        bazel_credits: &NO_KEYS,
        signature_types: &NO_KEYS,
        ambient_modules: &NO_KEYS,
        own_paths: &NO_KEYS,
        paths_before_packages: &NO_KEYS,
        whole_enum_members: &NO_KEYS,
        published_reexports: &NO_KEYS,
        root_entry_reexports: &NO_KEYS,
        bazel_sources: &NO_KEYS,
        non_member_credits: &NO_KEYS,
        skipped_credits: &NO_KEYS,
        non_member_members: &NO_KEYS,
        skipped_members: &NO_KEYS,
        knip_dot_ignores: &[],
        reexport_aliases: &NO_KEYS,
        tsconfig_declarations: &NO_KEYS,
        unimported_declarations: &NO_KEYS,
        declaration_reach: &NO_KEYS,
        module_object_types: &NO_KEYS,
        uninstalled_projects: &NO_KEYS,
        knip_unused_members: &NO_KEYS,
        knip_dot_ignore_imports: &NO_KEYS,
        knip_dot_ignore_credits: &NO_KEYS,
        heft_credits: &NO_KEYS,
        type_asserted_requires: &NO_KEYS,
        module_object_values: &NO_KEYS,
        commonjs_default_members: &NO_KEYS,
        template_import_reach: &NO_KEYS,
        installed_main_reach: &NO_KEYS,
        type_referenced_members: &NO_KEYS,
        raw_glob_reach: &NO_KEYS,
        typescript_twins: &NO_KEYS,
        knip_reads: &NO_READS,
        dependency_reads: &NO_DEPENDENCY_READS,
        rush_root: false,
        replaced_knip_config: false,
        layout,
    }
}

/// Runs `check` with a classifier for a checkout with these workspace members, manifests (by directory) and
/// `package.json` files knip reports on, and no other facts.
fn with_classifier(
    members: &[&str],
    manifests: &[(&str, Value)],
    judged_by_knip: &[&str],
    check: impl FnOnce(&Classifier<'_>),
) {
    let read: BTreeMap<String, Rc<Value>> = manifests
        .iter()
        .map(|(dir, manifest)| ((*dir).to_owned(), Rc::new(manifest.clone())))
        .collect();
    let manifest = move |dir: &str| read.get(dir).cloned();
    let every_file = |_: &str| true;
    let layout = Layout::new(strings(members), &manifest, &every_file);
    let judged = strings(judged_by_knip);
    check(&Classifier {
        knip_manifests: &judged,
        ..bare(&layout)
    });
}

#[test]
fn fixture_findings_cover_every_category_skif_is_silent_in_there() {
    with_classifier(&[], &[], &[], |classifier| {
        for category in [
            "files",
            "exports",
            "types",
            "nsExports",
            "nsTypes",
            "enumMembers",
            "namespaceMembers",
            "unresolved",
            "unlisted",
        ] {
            let key = if category == "files" {
                "pkg/__fixtures__/a.ts"
            } else {
                "pkg/__fixtures__/a.ts|x"
            };
            assert_eq!(
                classifier.classify(Side::Tool, category, key),
                Some(FIXTURE_FILE),
                "{category}"
            );
        }
        for category in ["duplicates", "binaries", "dependencies"] {
            assert_eq!(
                classifier.classify(Side::Tool, category, "pkg/fixtures/a.ts|x"),
                None,
                "{category}"
            );
        }
        assert_eq!(
            classifier.classify(Side::Tool, "unresolved", "pkg/fixture/a.ts|./b"),
            None
        );
        assert_eq!(classifier.classify(Side::Tool, "unresolved", "fixtures.ts|./b"), None);
        assert_eq!(
            classifier.classify(Side::Skif, "unresolved", "pkg/fixtures/a.ts|./b"),
            None
        );
    });
}

#[test]
fn binaries_rush_provides_shell_builtins_and_skif_s_global_binaries_split_off() {
    let manifest = |dir: &str| dir.is_empty().then(|| Rc::new(json!({ "name": "root" })));
    let rush = |file: &str| file == "rush.json";
    let plain = |_: &str| false;
    let checkouts: [(&FileProbe<'_>, Option<&str>); 2] = [(&rush, Some(RUSH_PROVIDED_COMMAND)), (&plain, None)];
    for (is_file, expected) in checkouts {
        let layout = Layout::new(BTreeSet::new(), &manifest, is_file);
        let classifier = bare(&layout);
        for binary in ["rush", "rushx", "rush-pnpm"] {
            assert_eq!(
                classifier.classify(Side::Tool, "binaries", &format!("apps/a/package.json|{binary}")),
                expected,
                "{binary} with a rush.json: {}",
                expected.is_some()
            );
        }
    }
    let layout = Layout::new(BTreeSet::new(), &manifest, &plain);
    let classifier = bare(&layout);
    for (category, key, expected) in [
        ("binaries", "package.json|read", Some(SHELL_BUILTIN)),
        ("binaries", "package.json|ulimit", Some(SHELL_BUILTIN)),
        ("binaries", "package.json|docker", Some(GLOBAL_BINARY)),
        ("binaries", "package.json|curl", Some(GLOBAL_BINARY)),
        ("binaries", "package.json|bazel", Some(GLOBAL_BINARY)),
        ("binaries", "package.json|eslint", None),
        ("unlisted", "src/a.ts|docker", None),
    ] {
        assert_eq!(
            classifier.classify(Side::Tool, category, key),
            expected,
            "{category} {key}"
        );
    }
    assert_eq!(
        classifier.classify(Side::Skif, "binaries", "package.json|docker"),
        None,
        "a global binary skif reports stays compared"
    );
}

#[test]
fn unlisted_data_uris_split_off() {
    with_classifier(&[], &[("", json!({ "name": "root" }))], &[], |classifier| {
        assert_eq!(
            classifier.classify(Side::Tool, "unlisted", "src/a.test.ts|data:text"),
            Some(DATA_URI)
        );
        assert_eq!(
            classifier.classify(Side::Tool, "unlisted", "src/a.test.ts|database"),
            None
        );
        assert_eq!(
            classifier.classify(Side::Tool, "unresolved", "src/a.test.ts|data:text"),
            None
        );
    });
}

#[test]
fn the_knip_config_the_harness_replaces_splits_off_at_a_workspace_root() {
    let manifests = [
        ("", json!({ "name": "root", "workspaces": ["packages/*"] })),
        ("packages/a", json!({ "name": "a" })),
    ];
    let read: BTreeMap<String, Rc<Value>> = manifests
        .iter()
        .map(|(dir, manifest)| ((*dir).to_owned(), Rc::new(manifest.clone())))
        .collect();
    let manifest = move |dir: &str| read.get(dir).cloned();
    let every_file = |_: &str| true;
    let layout = Layout::new(strings(&["packages/a"]), &manifest, &every_file);
    let replaced = Classifier {
        replaced_knip_config: true,
        ..bare(&layout)
    };
    for key in ["knip.ts", "packages/a/knip.config.js"] {
        assert_eq!(
            replaced.classify(Side::Tool, "files", key),
            Some(REPLACED_KNIP_CONFIG),
            "{key}"
        );
    }
    for key in ["packages/a/src/knip.ts", "knip-helpers.ts"] {
        assert_eq!(replaced.classify(Side::Tool, "files", key), None, "{key}");
    }
    assert_eq!(
        bare(&layout).classify(Side::Tool, "files", "knip.ts"),
        None,
        "knip read the repository's own configuration"
    );
}

#[test]
fn findings_only_skif_reports_on_manifests_knip_does_not_judge_split_off() {
    let manifests = [
        ("", json!({ "name": "root", "workspaces": ["packages/*"] })),
        ("packages/a", json!({ "name": "a" })),
        (
            "examples/app",
            json!({ "name": "app", "dependencies": { "left-pad": "1" } }),
        ),
        ("judged", json!({ "name": "judged" })),
    ];
    with_classifier(&["packages/a"], &manifests, &["judged/package.json"], |classifier| {
        for (category, key) in [
            ("dependencies", "examples/app/package.json|left-pad"),
            ("devDependencies", "examples/app/package.json|vitest"),
            ("optionalPeerDependencies", "examples/app/package.json|react"),
            ("binaries", "examples/app/package.json|turbo"),
        ] {
            assert_eq!(
                classifier.classify(Side::Skif, category, key),
                Some(KNIP_UNJUDGED_MANIFEST),
                "{key}"
            );
        }
        for (category, key) in [
            ("unlisted", "examples/app/src/a.ts|react"),
            ("unlisted", "examples/app/.eslintrc.json|@nrwl/eslint-plugin-nx"),
            ("binaries", "examples/app/.github/workflows/ci.yml|turbo"),
            ("files", "examples/app/src/orphan.ts"),
        ] {
            assert_eq!(
                classifier.classify(Side::Skif, category, key),
                Some(NON_MEMBER_PROJECT),
                "{category} {key}"
            );
        }
        for (category, key) in [
            ("dependencies", "packages/a/package.json|left-pad"),
            ("dependencies", "package.json|left-pad"),
            ("dependencies", "judged/package.json|left-pad"),
            ("binaries", "judged/package.json|turbo"),
            ("exports", "examples/app/src/a.ts|unused"),
            ("unresolved", "examples/app/src/a.ts|./gone"),
            ("unlisted", "packages/a/src/a.ts|react"),
            ("files", "packages/a/src/orphan.ts"),
        ] {
            assert_eq!(classifier.classify(Side::Skif, category, key), None, "{category} {key}");
        }
        assert_eq!(
            classifier.classify(Side::Tool, "dependencies", "examples/app/package.json|left-pad"),
            None,
            "knip does not report on a manifest it does not judge"
        );
    });
}

#[test]
fn knip_only_findings_in_files_below_non_member_projects_split_off() {
    let manifests = [
        ("", json!({ "name": "root", "workspaces": ["packages/*"] })),
        ("packages/a", json!({ "name": "a" })),
        ("packages/a/vendor/lib", json!({ "name": "vendored" })),
        (
            "examples/app",
            json!({ "name": "app", "devDependencies": { "@mastra/core": "1" } }),
        ),
        ("examples/app/esm", json!({ "type": "module" })),
    ];
    with_classifier(&["packages/a"], &manifests, &[], |classifier| {
        for key in [
            "examples/app/src/index.ts",
            "examples/app/esm/index.ts",
            "packages/a/vendor/lib/index.js",
        ] {
            assert_eq!(
                classifier.classify(Side::Tool, "files", key),
                Some(NON_MEMBER_PROJECT),
                "{key}"
            );
        }
        assert_eq!(
            classifier.classify(Side::Tool, "unlisted", "examples/app/esm/a.ts|@mastra/core/agent"),
            Some(NON_MEMBER_PROJECT),
            "the project's own manifest lists the package"
        );
        for (category, key) in [
            ("exports", "examples/app/src/a.ts|unused"),
            ("types", "examples/app/src/a.ts|Unused"),
            ("nsExports", "examples/app/src/a.ts|helper"),
            ("nsTypes", "examples/app/src/a.ts|Helper"),
            ("enumMembers", "examples/app/src/a.ts|Red"),
            ("namespaceMembers", "examples/app/src/a.ts|run"),
            ("duplicates", "examples/app/src/a.ts|a|b"),
            ("unresolved", "examples/app/src/a.ts|../../types/cache"),
            ("unresolved", "packages/a/vendor/lib/index.js|./gone"),
        ] {
            assert_eq!(
                classifier.classify(Side::Tool, category, key),
                Some(NON_MEMBER_PROJECT),
                "{category} {key}"
            );
        }
        for (category, key) in [
            ("unlisted", "examples/app/src/a.ts|zod"),
            ("dependencies", "examples/app/package.json|left-pad"),
            ("binaries", "examples/app/package.json|tsc"),
            ("exports", "packages/a/src/a.ts|unused"),
            ("unresolved", "scripts/a.ts|./gone"),
            ("files", "packages/a/src/orphan.ts"),
            ("files", "scripts/orphan.ts"),
        ] {
            assert_eq!(classifier.classify(Side::Tool, category, key), None, "{key}");
        }
        assert_eq!(
            classifier.classify(Side::Skif, "files", "examples/app/src/index.ts"),
            Some(NON_MEMBER_PROJECT),
            "a recursive run lints the project with its own entry files"
        );
        assert_eq!(
            classifier.classify(Side::Skif, "exports", "examples/app/src/a.ts|unused"),
            None,
            "knip judges the exports of the project from the enclosing workspace"
        );
    });
}

#[test]
fn in_file_findings_in_a_project_a_recursive_run_lints_as_a_root_stay_compared() {
    let skif_doc = json!({ "diagnostics": [], "roots": [
        { "path": ".", "packages": [".", "packages/a"] },
        { "path": "examples/app", "packages": ["examples/app", "examples/app/plugins/x/"] },
    ]});
    let roots = skif_root_packages(&skif_doc);
    assert_eq!(
        roots,
        strings(&["", "examples/app", "examples/app/plugins/x", "packages/a"])
    );
    assert!(skif_root_packages(&json!({ "diagnostics": [] })).is_empty());

    let read: BTreeMap<String, Rc<Value>> = [
        ("", json!({ "name": "root", "workspaces": ["packages/*"] })),
        ("packages/a", json!({ "name": "a" })),
        (
            "examples/app",
            json!({ "name": "app", "dependencies": { "react": "1" } }),
        ),
        ("examples/other", json!({ "name": "other" })),
    ]
    .into_iter()
    .map(|(dir, manifest)| (dir.to_owned(), Rc::new(manifest)))
    .collect();
    let manifest = move |dir: &str| read.get(dir).cloned();
    let every_file = |_: &str| true;
    let layout = Layout::new(strings(&["packages/a"]), &manifest, &every_file);
    let classifier = Classifier {
        skif_root_packages: &roots,
        ..bare(&layout)
    };
    for (category, key) in [
        ("exports", "examples/app/src/a.ts|unused"),
        ("types", "examples/app/src/a.ts|Unused"),
        ("nsExports", "examples/app/src/a.ts|helper"),
        ("nsTypes", "examples/app/src/a.ts|Helper"),
        ("enumMembers", "examples/app/src/a.ts|Red"),
        ("namespaceMembers", "examples/app/src/a.ts|run"),
        ("duplicates", "examples/app/src/a.ts|a|b"),
    ] {
        assert_eq!(classifier.classify(Side::Tool, category, key), None, "{category} {key}");
        assert_eq!(
            classifier.classify(Side::Tool, category, &key.replace("examples/app", "examples/other")),
            Some(NON_MEMBER_PROJECT),
            "{category}: a project that is no root of the run"
        );
    }
    for category in ["exports", "types", "nsExports", "nsTypes"] {
        assert_eq!(
            classifier.classify(Side::Tool, category, "examples/app/src/index.ts|getStaticProps"),
            Some(NON_MEMBER_PROJECT),
            "{category}: the exports of the project's entry are public in skif's run of it"
        );
    }
    assert_eq!(
        classifier.classify(Side::Tool, "enumMembers", "examples/app/index.ts|Red"),
        None
    );
    assert_eq!(
        classifier.classify(Side::Tool, "duplicates", "examples/app/index.ts|a|b"),
        Some(ENTRY_MODULE_DUPLICATE)
    );
    for (category, key) in [
        ("files", "examples/app/src/orphan.ts"),
        ("unresolved", "examples/app/src/a.ts|./gone"),
        ("unlisted", "examples/app/src/a.ts|react"),
    ] {
        assert_eq!(
            classifier.classify(Side::Tool, category, key),
            Some(NON_MEMBER_PROJECT),
            "{category} {key}"
        );
    }
}

#[test]
fn knip_only_findings_below_directories_skif_discovery_skips_split_off() {
    let manifests = [
        ("", json!({ "name": "root" })),
        ("packages/a", json!({ "name": "a" })),
        (".tools", json!({ "name": "tools" })),
        (".vscode/extensions/helper", json!({ "name": "helper" })),
    ];
    let members = ["packages/a", ".tools", ".vscode/extensions/helper"];
    with_classifier(&members, &manifests, &[], |classifier| {
        for (category, key) in [
            ("files", ".claude/skills/template/index.ts"),
            ("files", ".vscode/extensions/stray.ts"),
            ("files", ".vscode/extensions/helper/.cache/x.ts"),
            ("files", "app/.vercel/output/functions/index.func/index.js"),
            ("files", "out/main.js"),
            ("files", "packages/a/coverage/report.js"),
            ("files", "packages/a/src/dist/bundle.js"),
            ("unresolved", ".eslint-plugin-local/rule.ts|./missing"),
        ] {
            assert_eq!(
                classifier.classify(Side::Tool, category, key),
                Some(SKIPPED_DIRECTORY),
                "{key}"
            );
        }
        for key in [
            ".github/scripts/release.ts",
            ".storybook/main.ts",
            ".tools/index.ts",
            ".vscode/extensions/helper/src/extension.ts",
            "packages/a/src/out/emit.ts",
            "packages/a/src/target.ts",
        ] {
            assert_eq!(classifier.classify(Side::Tool, "files", key), None, "{key}");
        }
    });
}

#[test]
fn typescript_sources_of_published_paths_split_off() {
    let manifests = [
        ("", json!({ "name": "root", "bin": { "tool": "./cli.js" } })),
        (
            "packages/core",
            json!({ "name": "core", "exports": {
                    ".": { "types": "./index.d.ts", "default": "./bundles/index.js" },
                    "./schematics/*": { "default": "./schematics/*.js" },
                    "./*": "./*",
                } }),
        ),
        (
            "packages/common",
            json!({ "name": "common", "exports": { "./locales/*": { "types": "./locales/*.d.ts" } } }),
        ),
    ];
    with_classifier(&["packages/core", "packages/common"], &manifests, &[], |classifier| {
        for key in [
            "cli.ts",
            "packages/core/index.ts",
            "packages/core/bundles/index.ts",
            "packages/core/schematics/migrations/model-output/index.ts",
            "packages/common/locales/ff-CM.ts",
        ] {
            assert_eq!(
                classifier.classify(Side::Tool, "files", key),
                Some(PUBLISHED_SOURCE),
                "{key}"
            );
        }
        for key in [
            "packages/core/src/other.ts",
            "packages/core/schematics.ts",
            "packages/common/locales/ff-CM.mts",
            "packages/common/locale/ff-CM.ts",
        ] {
            assert_eq!(classifier.classify(Side::Tool, "files", key), None, "{key}");
        }
        // A JavaScript file `exports` publishes as it is is a published file, not a source of one.
        assert_eq!(
            classifier.classify(Side::Tool, "files", "packages/core/schematics/a.js"),
            Some(PUBLISHED_FILE)
        );
        assert!(
            !classifier
                .layout
                .publishes_source_of("packages/core/schematics/types.d.ts")
        );
        assert_eq!(
            classifier.classify(Side::Tool, "files", "packages/core/schematics/types.d.ts"),
            Some(DECLARATION_FILE)
        );
        for category in ["exports", "types", "nsExports", "nsTypes"] {
            assert_eq!(
                classifier.classify(Side::Tool, category, "packages/core/index.ts|unused"),
                Some(PUBLISHED_SOURCE),
                "the exports of an entry are public: {category}"
            );
            assert_eq!(
                classifier.classify(Side::Tool, category, "packages/core/src/other.ts|unused"),
                None,
                "{category}"
            );
            assert_eq!(
                classifier.classify(Side::Skif, category, "packages/core/index.ts|unused"),
                None,
                "{category}"
            );
        }
        for category in ["enumMembers", "namespaceMembers"] {
            assert_eq!(
                classifier.classify(Side::Tool, category, "packages/core/index.ts|Red"),
                None,
                "skif judges the members of a used export: {category}"
            );
        }
    });
}

#[test]
fn duplicates_only_knip_reports_in_default_entries_and_published_sources_split_off() {
    let manifests = [
        ("", json!({ "name": "root", "workspaces": ["packages/*"] })),
        ("packages/a", json!({ "name": "a", "main": "./dist/server.js" })),
    ];
    with_classifier(&["packages/a"], &manifests, &[], |classifier| {
        for key in [
            "packages/a/src/server.ts|default|format",
            "packages/a/index.mjs|a|b",
            "packages/a/src/index.ts|a|b",
            "packages/a/cli.cts|a|b",
            "src/main.tsx|a|b",
        ] {
            assert_eq!(
                classifier.classify(Side::Tool, "duplicates", key),
                Some(ENTRY_MODULE_DUPLICATE),
                "{key}"
            );
        }
        for (side, category, key) in [
            (Side::Tool, "duplicates", "packages/a/src/util.ts|a|b"),
            (Side::Tool, "duplicates", "packages/a/lib/index.ts|a|b"),
            (Side::Tool, "duplicates", "packages/a/src/main/index.ts|a|b"),
            (Side::Tool, "duplicates", "packages/a/index.json|a|b"),
            (Side::Tool, "exports", "packages/a/src/index.ts|unused"),
            (Side::Skif, "duplicates", "packages/a/src/index.ts|a|b"),
        ] {
            assert_eq!(
                classifier.classify(side, category, key),
                None,
                "{side:?} {category} {key}"
            );
        }
        assert_eq!(
            classifier.classify(Side::Tool, "exports", "packages/a/src/server.ts|unused"),
            Some(PUBLISHED_SOURCE)
        );
    });
}

#[test]
fn published_paths_map_to_their_sources_through_the_build_directories_of_the_package() {
    assert_eq!(
        build_source("dist", "src", "dist/a/b.js").as_deref(),
        Some("src/a/b.js")
    );
    assert_eq!(build_source("dist/esm", "", "dist/esm/a.js").as_deref(), Some("a.js"));
    assert_eq!(build_source("dist", "src", "dist").as_deref(), Some("src"));
    assert_eq!(build_source("dist", "src", "distant/a.js"), None);
    assert_eq!(build_source("", "src", "a.js"), None);

    for (config, found) in [
        (
            "{ // the ESM build\n \"compilerOptions\": { \"outDir\": \"./dist/esm\", \"rootDir\": \"./src\", }, }",
            Some(("dist/esm", "src")),
        ),
        (
            r#"{ "compilerOptions": { "outDir": "dist" }, "include": ["src/**/*.ts", "src/types.d.ts"] }"#,
            Some(("dist", "src")),
        ),
        (
            r#"{ "compilerOptions": { "outDir": "lib" }, "include": ["src/**/*", "scripts/build.ts"] }"#,
            Some(("lib", "")),
        ),
        (
            r#"{ "compilerOptions": { "outDir": "dist" }, "files": ["src/a/index.ts", "src/b.ts"] }"#,
            Some(("dist", "src")),
        ),
        (r#"{ "compilerOptions": { "outDir": "dist" } }"#, None),
        (r#"{ "compilerOptions": { "rootDir": "src" } }"#, None),
        (
            r#"{ "compilerOptions": { "outDir": "../out", "rootDir": "src" } }"#,
            None,
        ),
        ("{ not json", None),
    ] {
        assert_eq!(
            tsconfig_build_dir(config),
            found.map(|(out_dir, root_dir)| (out_dir.to_owned(), root_dir.to_owned())),
            "{config}"
        );
    }

    let read: BTreeMap<String, Rc<Value>> = [
        ("", json!({ "name": "root" })),
        (
            "packages/ai",
            json!({ "name": "ai", "exports": {
                    ".": { "import": "./dist/esm/index.js", "require": "./dist/cjs/index.js" },
                    "./tools": "./dist/esm/tools/index.js",
                } }),
        ),
    ]
    .into_iter()
    .map(|(dir, manifest)| (dir.to_owned(), Rc::new(manifest)))
    .collect();
    let manifest = move |dir: &str| read.get(dir).cloned();
    let files = strings(&["packages/ai/src/index.ts", "packages/ai/src/tools/index.ts"]);
    let is_file = |file: &str| files.contains(file);
    let build_dirs = |dir: &str| {
        if dir == "packages/ai" {
            vec![
                ("dist/esm".to_owned(), "src".to_owned()),
                ("dist/cjs".to_owned(), "src".to_owned()),
            ]
        } else {
            Vec::new()
        }
    };
    let without = Layout::new(strings(&["packages/ai"]), &manifest, &is_file);
    assert!(
        !without.publishes_source_of("packages/ai/src/index.ts"),
        "`dist/esm` maps to `src/esm` by convention alone"
    );
    let layout = Layout::new(strings(&["packages/ai"]), &manifest, &is_file).with_build_dirs(&build_dirs);
    assert!(layout.publishes_source_of("packages/ai/src/index.ts"));
    assert!(layout.publishes_source_of("packages/ai/src/tools/index.ts"));
    let classifier = bare(&layout);
    assert_eq!(
        classifier.classify(Side::Tool, "files", "packages/ai/src/index.ts"),
        Some(PUBLISHED_SOURCE)
    );
    assert_eq!(
        classifier.classify(Side::Tool, "types", "packages/ai/src/tools/index.ts|ToolOptions"),
        Some(PUBLISHED_SOURCE)
    );
}

#[test]
fn the_harness_reads_the_build_directories_of_a_package_from_its_tsconfig_files() {
    let root = std::env::temp_dir().join(format!("skif-bench-parity-build-dirs-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let package = root.join("packages/ai");
    fs::create_dir_all(&package).unwrap();
    fs::write(
        package.join("tsconfig.json"),
        r#"{ "compilerOptions": { "strict": true } }"#,
    )
    .unwrap();
    fs::write(
        package.join("tsconfig.build.esm.json"),
        r#"{ "extends": "./tsconfig.json", "compilerOptions": { "outDir": "dist/esm", "rootDir": "src" } }"#,
    )
    .unwrap();
    fs::write(
        package.join("tsconfig.build.cjs.json"),
        r#"{ "compilerOptions": { "outDir": "dist/cjs" }, "include": ["src"] }"#,
    )
    .unwrap();
    fs::write(
        package.join("base.json"),
        r#"{ "compilerOptions": { "outDir": "out", "rootDir": "." } }"#,
    )
    .unwrap();
    let read = build_dir_reader(&root);
    assert_eq!(
        read("packages/ai"),
        vec![
            ("dist/cjs".to_owned(), "src".to_owned()),
            ("dist/esm".to_owned(), "src".to_owned()),
        ],
        "in file name order; a file that is no tsconfig is not read"
    );
    assert_eq!(read("packages/missing"), Vec::<(String, String)>::new());
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn published_paths_below_build_directories_and_without_extensions_map_to_their_sources() {
    let manifests = [
        ("", json!({ "name": "root" })),
        (
            "libraries/heft",
            json!({ "name": "heft", "main": "./lib-commonjs/index.js", "exports": {
                    "./lib/*": { "types": "./lib/*.d.ts", "default": "./lib-commonjs/*.js" },
                    "./package.json": "./package.json",
                } }),
        ),
        (
            "extensions/git",
            json!({ "name": "git", "main": "./out/main", "browser": "./dist/browser/extension" }),
        ),
        (
            "packages/tool",
            json!({ "name": "tool", "bin": { "tool": "./bin/tool" } }),
        ),
        (
            "packages/styles",
            json!({ "name": "styles", "main": "./dist/index.css" }),
        ),
    ];
    let members = ["libraries/heft", "extensions/git", "packages/tool", "packages/styles"];
    with_classifier(&members, &manifests, &[], |classifier| {
        for key in [
            "libraries/heft/src/index.ts",
            "libraries/heft/src/utilities/fs.ts",
            "libraries/heft/src/plugins/copy/CopyFilesPlugin.tsx",
            "extensions/git/src/main.ts",
            "extensions/git/src/browser/extension.ts",
            "packages/tool/bin/tool.ts",
        ] {
            assert_eq!(
                classifier.classify(Side::Tool, "files", key),
                Some(PUBLISHED_SOURCE),
                "{key}"
            );
        }
        for key in [
            "libraries/heft/scripts/build.ts",
            "extensions/git/src/other.ts",
            "extensions/git/main.ts",
            "packages/tool/src/tool.ts",
            "packages/styles/src/index.ts",
        ] {
            assert_eq!(classifier.classify(Side::Tool, "files", key), None, "{key}");
        }
        assert!(
            !classifier.layout.publishes_source_of("libraries/heft/src/index.d.ts"),
            "a declaration file is no source of a published path"
        );
    });
    assert!(is_published_twin("./lib-commonjs/*.js", "src/a/b", "ts"));
    assert!(!is_published_twin("./lib-commonjs/*.js", "lib-commonjs-src/a", "ts"));
    assert!(
        !is_published_twin("./bundles/*.js", "src/index", "ts"),
        "`bundles` is not a build directory"
    );
    assert!(
        !is_published_twin("./out/*", "src/main", "js"),
        "only TypeScript sources"
    );
}

#[test]
fn a_published_path_without_a_star_names_only_the_first_source_that_exists() {
    let read: BTreeMap<String, Rc<Value>> = [
        ("", json!({ "name": "root" })),
        (
            "packages/x",
            json!({ "name": "x", "main": "./lib/x.js", "types": "./lib/y.d.ts" }),
        ),
    ]
    .into_iter()
    .map(|(dir, manifest)| (dir.to_owned(), Rc::new(manifest)))
    .collect();
    let manifest = move |dir: &str| read.get(dir).cloned();
    let both = strings(&["packages/x/src/x.ts", "packages/x/lib/x.ts", "packages/x/lib/y.ts"]);
    let is_file = |file: &str| both.contains(file);
    let layout = Layout::new(strings(&["packages/x"]), &manifest, &is_file);
    assert!(layout.publishes_source_of("packages/x/src/x.ts"));
    assert!(
        !layout.publishes_source_of("packages/x/lib/x.ts"),
        "skif maps `./lib/x.js` to `src/x.ts`, the first candidate that exists"
    );
    assert!(
        layout.publishes_source_of("packages/x/lib/y.ts"),
        "the path itself when no `src` twin exists"
    );
    assert!(
        !layout.publishes_source_of("packages/x/src/y.ts"),
        "a source that does not exist"
    );

    let only_lib = strings(&["packages/x/lib/x.ts", "packages/x/lib/x.tsx"]);
    let is_file = |file: &str| only_lib.contains(file);
    let layout = Layout::new(strings(&["packages/x"]), &manifest, &is_file);
    assert!(layout.publishes_source_of("packages/x/lib/x.ts"));
    assert!(
        !layout.publishes_source_of("packages/x/lib/x.tsx"),
        "`.ts` comes before `.tsx`"
    );
}

/// The manifests of a checkout with an n8n node package in `packages/nodes`, by directory.
fn n8n_manifests() -> impl Fn(&str) -> Option<Rc<Value>> {
    let read: BTreeMap<String, Rc<Value>> = [
        ("", json!({ "name": "root" })),
        (
            "packages/nodes",
            json!({ "name": "nodes", "main": "index.js", "n8n": {
                    "nodes": ["dist/nodes/A/A.node.js"],
                    "credentials": ["dist/credentials/AApi.credentials.js"],
                } }),
        ),
    ]
    .into_iter()
    .map(|(dir, manifest)| (dir.to_owned(), Rc::new(manifest)))
    .collect();
    move |dir: &str| read.get(dir).cloned()
}

#[test]
fn the_node_and_credential_files_of_an_n8n_node_package_are_published_sources() {
    let manifest = n8n_manifests();
    let files = strings(&[
        "packages/nodes/nodes/A/A.node.ts",
        "packages/nodes/credentials/AApi.credentials.ts",
        "packages/nodes/nodes/A/GenericFunctions.ts",
        "packages/nodes/src/nodes/B/B.node.ts",
    ]);
    let is_file = |file: &str| files.contains(file);
    let layout = Layout::new(strings(&["packages/nodes"]), &manifest, &is_file);
    assert!(
        layout.publishes_source_of("packages/nodes/nodes/A/A.node.ts"),
        "`dist/nodes/A/A.node.js` is built from `nodes/A/A.node.ts`"
    );
    assert!(layout.publishes_source_of("packages/nodes/credentials/AApi.credentials.ts"));
    assert!(
        !layout.publishes_source_of("packages/nodes/nodes/A/GenericFunctions.ts"),
        "a file the package does not list"
    );
    assert!(
        !layout.publishes_source_of("packages/nodes/src/nodes/B/B.node.ts"),
        "a node the package does not list"
    );
}

#[test]
fn files_a_published_source_reaches_through_files_only_knip_reports_unused_are_published_sources() {
    let manifest = n8n_manifests();
    let sources: BTreeMap<&str, &str> = [
        (
            "packages/nodes/nodes/A/A.node.ts",
            "import { request } from './GenericFunctions';\n\
                 import * as fields from './descriptions';\n\
                 import { both } from '../Both';\n\
                 import text from './Raw.ts?raw';\n\
                 import { outside } from '../../../../../outside';\n",
        ),
        (
            "packages/nodes/nodes/A/GenericFunctions.ts",
            "export const request = 1;\n",
        ),
        (
            "packages/nodes/nodes/A/descriptions/index.ts",
            "export * from './Fields.js';\n",
        ),
        ("packages/nodes/nodes/A/descriptions/Fields.ts", "export const a = 1;\n"),
        ("packages/nodes/nodes/A/Raw.ts", "export const raw = 1;\n"),
        ("packages/nodes/nodes/Both.ts", "import { other } from './Other';\n"),
        ("packages/nodes/nodes/Other.ts", "export const other = 1;\n"),
    ]
    .into_iter()
    .collect();
    let is_file = |file: &str| sources.contains_key(file);
    let layout = Layout::new(strings(&["packages/nodes"]), &manifest, &is_file);
    let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
    let knip_unused: BTreeSet<String> = sources.keys().map(|file| (*file).to_owned()).collect();
    let skif_unused = strings(&["packages/nodes/nodes/Both.ts"]);
    assert_eq!(
        published_reach(&knip_unused, &skif_unused, &layout, &read),
        strings(&[
            "packages/nodes/nodes/A/GenericFunctions.ts",
            "packages/nodes/nodes/A/descriptions/Fields.ts",
            "packages/nodes/nodes/A/descriptions/index.ts",
        ]),
        "a file skif reports unused too ends the chain, and a `?raw` import reads no module"
    );
    assert_eq!(
        published_reach(&knip_unused, &knip_unused, &layout, &read),
        BTreeSet::new(),
        "only files knip alone reports unused"
    );
}

#[test]
fn relative_specifiers_resolve_with_extensions_twins_and_index_files() {
    let files = strings(&["src/a.ts", "src/b/index.tsx", "src/c.mts", "index.ts"]);
    let is_file = |file: &str| files.contains(file);
    assert_eq!(resolve_relative("src", "./a", &is_file).as_deref(), Some("src/a.ts"));
    assert_eq!(
        resolve_relative("src/b", "../a.js", &is_file).as_deref(),
        Some("src/a.ts")
    );
    assert_eq!(
        resolve_relative("src", "./b", &is_file).as_deref(),
        Some("src/b/index.tsx")
    );
    assert_eq!(
        resolve_relative("src", "./c.mjs", &is_file).as_deref(),
        Some("src/c.mts")
    );
    assert_eq!(resolve_relative("src", "..", &is_file).as_deref(), Some("index.ts"));
    assert_eq!(resolve_relative("src", "./missing", &is_file), None);
    assert_eq!(
        resolve_relative("src", "../../a", &is_file),
        None,
        "outside the checkout"
    );
}

#[test]
fn a_star_matches_one_or_more_characters_across_directories() {
    assert!(wildcard_match("schematics/*", "schematics/a/b/index"));
    assert!(wildcard_match("locales/*-x", "locales/ff-x"));
    assert!(!wildcard_match("schematics/*", "schematics/"));
    assert!(!wildcard_match("locales/*-x", "locales/ff-y"));
    assert!(wildcard_match("index", "index"));
    assert_eq!(package_name("@mastra/core/agent"), "@mastra/core");
    assert_eq!(package_name("lodash/fp"), "lodash");
    assert_eq!(package_name("react"), "react");
}

#[test]
fn the_harness_reads_members_and_manifests_from_the_checkout() {
    let root = std::env::temp_dir().join(format!("skif-bench-parity-layout-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    for dir in ["packages/a", "packages/b", "examples/app"] {
        fs::create_dir_all(root.join(dir)).unwrap();
        fs::write(root.join(dir).join("package.json"), r#"{"name":"x"}"#).unwrap();
    }
    fs::create_dir_all(root.join("packages/no-manifest")).unwrap();
    fs::create_dir_all(root.join("examples/broken")).unwrap();
    fs::write(root.join("examples/broken/package.json"), "{ not json").unwrap();
    fs::write(root.join("package.json"), r#"{"name":"root"}"#).unwrap();
    fs::write(
        root.join("pnpm-workspace.yaml"),
        "packages:\n  - 'packages/*'\n  - '!packages/b'\n",
    )
    .unwrap();
    let root = fs::canonicalize(&root).unwrap();
    assert_eq!(workspace_members(&root), strings(&["packages/a"]));
    let read = manifest_reader(&root);
    assert_eq!(read("examples/app").as_deref(), Some(&json!({ "name": "x" })));
    assert_eq!(
        read("examples/broken").as_deref(),
        Some(&Value::Null),
        "an unparseable manifest is still a manifest"
    );
    assert!(read("packages/no-manifest").is_none());
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn findings_inside_a_file_the_other_tool_calls_unused_and_config_errors_split_off_on_both_sides() {
    let read: BTreeMap<String, Rc<Value>> = [
        ("", json!({ "name": "root" })),
        ("packages/cfg", json!({ "name": "cfg" })),
    ]
    .into_iter()
    .map(|(dir, manifest)| (dir.to_owned(), Rc::new(manifest)))
    .collect();
    let manifest = move |dir: &str| read.get(dir).cloned();
    let every_file = |_: &str| true;
    let layout = Layout::new(strings(&["packages/cfg"]), &manifest, &every_file);
    let knip_unused = strings(&["examples/a/src/dead.ts"]);
    let skif_unused = strings(&["packages/b/src/gone.ts"]);
    let failed = strings(&["packages/cfg"]);
    let classifier = Classifier {
        knip_failed: &failed,
        knip_unused_files: &knip_unused,
        skif_unused_files: &skif_unused,
        ..bare(&layout)
    };
    for (category, key) in [
        ("exports", "examples/a/src/dead.ts|x"),
        ("types", "examples/a/src/dead.ts|T"),
        ("unlisted", "examples/a/src/dead.ts|react"),
        ("duplicates", "examples/a/src/dead.ts|a|b"),
        ("unresolved", "examples/a/src/dead.ts|./gone"),
    ] {
        assert_eq!(
            classifier.classify(Side::Skif, category, key),
            Some(IN_KNIP_UNUSED_FILE),
            "{category} {key}"
        );
    }
    assert_eq!(
        classifier.classify(Side::Tool, "exports", "packages/b/src/gone.ts|x"),
        Some(IN_SKIF_UNUSED_FILE)
    );
    for (side, category, key) in [
        (Side::Skif, "files", "examples/a/src/dead.ts"),
        (Side::Skif, "binaries", "examples/a/src/dead.ts|tsc"),
        (Side::Skif, "exports", "examples/a/src/live.ts|x"),
        (Side::Skif, "exports", "packages/b/src/gone.ts|x"),
        (Side::Tool, "exports", "examples/a/src/dead.ts|x"),
    ] {
        assert_eq!(
            classifier.classify(side, category, key),
            None,
            "{side:?} {category} {key}"
        );
    }
    for side in [Side::Skif, Side::Tool] {
        assert_eq!(
            classifier.classify(side, "exports", "packages/cfg/src/a.ts|x"),
            Some(KNIP_CONFIG_ERROR),
            "{side:?}"
        );
    }
    assert_eq!(
        classifier.classify(Side::Skif, "exports", "packages/other/a.ts|x"),
        None
    );
}

#[test]
fn the_import_scan_names_the_bare_packages_a_file_imports() {
    let source = r##"import React from "react";
import type { Config } from "@scope/config/types";
import "side-effect/register";
import { join } from "node:path";
import local from "./local";
import internal from "#internal";
export { helper } from "re-exported";
const lazy = await import("lazy-package");
const required = require("required-package/sub");
const files = import.meta.glob("./pages/*.ts");
vi.mock("mocked-package");
"##;
    let packages = imported_packages("src/a.ts", source);
    assert_eq!(
        packages.iter().map(String::as_str).collect::<Vec<_>>(),
        [
            "@scope/config",
            "lazy-package",
            "re-exported",
            "react",
            "required-package",
            "side-effect"
        ]
    );
    assert!(imported_packages("styles/a.css", ".a { color: red }").is_empty());
    assert_eq!(bare_package("https://esm.sh/react"), None);
    assert_eq!(bare_package("C:\\deps\\a.js"), None);
    assert_eq!(bare_package("@scope/name/deep"), Some("@scope/name"));
    assert_eq!(typed_package("node"), "node");
    assert_eq!(typed_package("babel__core"), "@babel/core");
}

#[test]
fn dependencies_only_files_knip_lists_unused_import_split_off() {
    let sources = BTreeMap::from([
        (
            "libraries/a/src/dead.ts",
            "import { x } from 'left-pad';\nimport core from '@babel/core';\n",
        ),
        ("libraries/a/vendor/old.ts", "import 'from-nested';\n"),
        ("scripts/dead.ts", "import 'root-only';\n"),
    ]);
    let unused: BTreeSet<String> = sources
        .keys()
        .map(|file| (*file).to_owned())
        .chain(["libraries/a/src/gone.ts".to_owned()])
        .collect();
    let workspaces = ["libraries/a"];
    let read_source = |file: &str| sources.get(file).map(|source| (*source).to_owned());
    let workspace_of = |file: &str| owner_dir(file, &|dir: &str| workspaces.contains(&dir));
    let imports = unused_file_imports(&unused, &BTreeSet::new(), &read_source, &workspace_of);
    assert_eq!(
        imports,
        strings(&[
            "libraries/a/package.json|@babel/core",
            "libraries/a/package.json|from-nested",
            "libraries/a/package.json|left-pad",
            "package.json|root-only",
        ]),
        "a file counts to its workspace, an unreadable one imports nothing"
    );
    assert_eq!(
        unused_file_imports(
            &unused,
            &strings(&["libraries/a/src/dead.ts", "scripts/dead.ts"]),
            &read_source,
            &workspace_of
        ),
        strings(&["libraries/a/package.json|from-nested"]),
        "a file skif calls unused too credits nothing on either side"
    );

    let read: BTreeMap<String, Rc<Value>> = [("", json!({ "name": "root" })), ("libraries/a", json!({ "name": "a" }))]
        .into_iter()
        .map(|(dir, manifest)| (dir.to_owned(), Rc::new(manifest)))
        .collect();
    let manifest = move |dir: &str| read.get(dir).cloned();
    let every_file = |_: &str| true;
    let layout = Layout::new(strings(&workspaces), &manifest, &every_file);
    let classifier = Classifier {
        knip_unused_files: &unused,
        knip_unused_imports: &imports,
        ..bare(&layout)
    };
    for (category, key) in [
        ("dependencies", "libraries/a/package.json|left-pad"),
        ("devDependencies", "libraries/a/package.json|from-nested"),
        ("optionalPeerDependencies", "libraries/a/package.json|@babel/core"),
        ("devDependencies", "libraries/a/package.json|@types/babel__core"),
        ("devDependencies", "package.json|root-only"),
    ] {
        assert_eq!(
            classifier.classify(Side::Tool, category, key),
            Some(USED_IN_KNIP_UNUSED_FILE),
            "{category} {key}"
        );
    }
    for (side, category, key) in [
        (Side::Tool, "dependencies", "package.json|left-pad"),
        (Side::Tool, "dependencies", "libraries/a/package.json|root-only"),
        (Side::Tool, "devDependencies", "libraries/a/package.json|@types/node"),
        (Side::Tool, "binaries", "libraries/a/package.json|left-pad"),
        (Side::Tool, "unlisted", "libraries/a/src/live.ts|left-pad"),
        (Side::Skif, "dependencies", "libraries/a/package.json|left-pad"),
    ] {
        assert_eq!(
            classifier.classify(side, category, key),
            None,
            "{side:?} {category} {key}"
        );
    }

    let skif_unused = strings(&["libraries/a/src/dead.ts"]);
    let imports = unused_file_imports(&unused, &skif_unused, &read_source, &workspace_of);
    let both_unused = Classifier {
        skif_unused_files: &skif_unused,
        knip_unused_imports: &imports,
        ..classifier
    };
    assert_eq!(
        both_unused.classify(Side::Tool, "dependencies", "libraries/a/package.json|left-pad"),
        None,
        "a dependency only a file both tools call unused imports stays compared"
    );
    assert_eq!(
        both_unused.classify(Side::Tool, "devDependencies", "libraries/a/package.json|from-nested"),
        Some(USED_IN_KNIP_UNUSED_FILE)
    );
}

#[test]
fn dependencies_only_files_skif_reports_unused_import_split_off_on_skif_side() {
    let sources = BTreeMap::from([
        (
            "libraries/a/src/dead.ts",
            "import { x } from 'left-pad';\nimport core from '@babel/core';\n",
        ),
        ("libraries/a/src/both.ts", "import 'both-unused';\n"),
    ]);
    let skif_unused = strings(&["libraries/a/src/dead.ts", "libraries/a/src/both.ts"]);
    let knip_unused = strings(&["libraries/a/src/both.ts"]);
    let read_source = |file: &str| sources.get(file).map(|source| (*source).to_owned());
    let workspace_of = |file: &str| owner_dir(file, &|dir: &str| dir == "libraries/a");
    let imports = unused_file_imports(&skif_unused, &knip_unused, &read_source, &workspace_of);
    assert_eq!(
        imports,
        strings(&[
            "libraries/a/package.json|@babel/core",
            "libraries/a/package.json|left-pad"
        ]),
        "only the files skif alone reports unused are scanned"
    );

    let read: BTreeMap<String, Rc<Value>> = [("", json!({ "name": "root" })), ("libraries/a", json!({ "name": "a" }))]
        .into_iter()
        .map(|(dir, manifest)| (dir.to_owned(), Rc::new(manifest)))
        .collect();
    let manifest = move |dir: &str| read.get(dir).cloned();
    let every_file = |_: &str| true;
    let layout = Layout::new(strings(&["libraries/a"]), &manifest, &every_file);
    let classifier = Classifier {
        knip_unused_files: &knip_unused,
        skif_unused_files: &skif_unused,
        skif_unused_imports: &imports,
        ..bare(&layout)
    };
    for (category, key) in [
        ("dependencies", "libraries/a/package.json|left-pad"),
        ("optionalPeerDependencies", "libraries/a/package.json|@babel/core"),
        ("devDependencies", "libraries/a/package.json|@types/babel__core"),
    ] {
        assert_eq!(
            classifier.classify(Side::Skif, category, key),
            Some(USED_IN_SKIF_UNUSED_FILE),
            "{category} {key}"
        );
    }
    for (side, category, key) in [
        (Side::Skif, "dependencies", "libraries/a/package.json|both-unused"),
        (Side::Skif, "dependencies", "package.json|left-pad"),
        (Side::Skif, "binaries", "libraries/a/package.json|left-pad"),
        (Side::Tool, "dependencies", "libraries/a/package.json|left-pad"),
    ] {
        assert_eq!(
            classifier.classify(side, category, key),
            None,
            "{side:?} {category} {key}"
        );
    }
}

#[test]
fn exports_only_files_one_tool_lists_unused_import_split_off_through_re_exports() {
    let sources = BTreeMap::from([
        (
            "packages/a/src/dead.ts",
            "import { helper, type Shape } from './util';\n\
                 import * as ns from './ns';\n\
                 import value from './def';\n\
                 import './side';\n\
                 import { deep, alias, own } from './barrel';\n\
                 const lazy = import('./lazy');\n\
                 console.log(helper, ns.read, value, deep, alias, own, lazy);\n",
        ),
        (
            "packages/a/src/both.ts",
            "import { second } from './util';\nconsole.log(second);\n",
        ),
        (
            "packages/a/src/util.ts",
            "export const helper = 1;\nexport const second = 2;\nexport const unusedHelper = 3;\nexport type Shape = { a: number };\n",
        ),
        (
            "packages/a/src/ns.ts",
            "export const read = 1;\nexport const unread = 2;\n",
        ),
        ("packages/a/src/def.ts", "export default 1;\nexport const other = 2;\n"),
        ("packages/a/src/side.ts", "export const effect = 1;\n"),
        (
            "packages/a/src/barrel.ts",
            "export * from './impl';\nexport { original as alias } from './other';\nexport const own = 1;\n",
        ),
        (
            "packages/a/src/impl.ts",
            "export const deep = 1;\nexport const own = 2;\nexport const shallow = 3;\n",
        ),
        (
            "packages/a/src/other.ts",
            "export const original = 1;\nexport const rest = 2;\n",
        ),
        ("packages/a/src/lazy.ts", "export const anything = 1;\n"),
    ]);
    let is_file = |file: &str| sources.contains_key(file);
    let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
    let unused = strings(&["packages/a/src/dead.ts", "packages/a/src/both.ts"]);
    let both = strings(&["packages/a/src/both.ts"]);
    let credits = unused_file_credits(&unused, &both, &Resolver::relative(&is_file), &read);
    assert_eq!(
        credits,
        strings(&[
            "packages/a/src/barrel.ts|alias",
            "packages/a/src/barrel.ts|deep",
            "packages/a/src/barrel.ts|own",
            "packages/a/src/def.ts|default",
            "packages/a/src/impl.ts|deep",
            "packages/a/src/lazy.ts|*",
            "packages/a/src/ns.ts|read",
            "packages/a/src/other.ts|original",
            "packages/a/src/util.ts|Shape",
            "packages/a/src/util.ts|helper",
        ]),
        "a side-effect import uses nothing, an `export *` forwards only names the barrel does not export itself, \
             and a file both tools call unused credits nothing"
    );
    assert!(unused_file_credits(&unused, &unused, &Resolver::relative(&is_file), &read).is_empty());

    let manifest = |dir: &str| (dir.is_empty() || dir == "packages/a").then(|| Rc::new(json!({ "name": "a" })));
    let layout = Layout::new(strings(&["packages/a"]), &manifest, &is_file);
    let classifier = Classifier {
        knip_unused_files: &unused,
        skif_unused_files: &both,
        knip_unused_credits: &credits,
        skif_unused_credits: &credits,
        ..bare(&layout)
    };
    for (category, key) in [
        ("exports", "packages/a/src/util.ts|helper"),
        ("types", "packages/a/src/util.ts|Shape"),
        ("nsExports", "packages/a/src/ns.ts|read"),
        ("exports", "packages/a/src/impl.ts|deep"),
        ("exports", "packages/a/src/lazy.ts|anything"),
        ("duplicates", "packages/a/src/impl.ts|deep|shallow"),
    ] {
        assert_eq!(
            classifier.classify(Side::Tool, category, key),
            Some(IMPORTED_BY_KNIP_UNUSED_FILE),
            "{category} {key}"
        );
        assert_eq!(
            classifier.classify(Side::Skif, category, key),
            Some(IMPORTED_BY_SKIF_UNUSED_FILE),
            "{category} {key}"
        );
    }
    for (category, key) in [
        ("exports", "packages/a/src/util.ts|unusedHelper"),
        ("exports", "packages/a/src/util.ts|second"),
        ("nsExports", "packages/a/src/ns.ts|unread"),
        ("exports", "packages/a/src/impl.ts|own"),
        ("exports", "packages/a/src/impl.ts|shallow"),
        ("exports", "packages/a/src/other.ts|rest"),
        ("exports", "packages/a/src/side.ts|effect"),
        ("enumMembers", "packages/a/src/lazy.ts|Red"),
        ("unlisted", "packages/a/src/lazy.ts|react"),
    ] {
        assert_eq!(classifier.classify(Side::Tool, category, key), None, "{category} {key}");
    }
}

fn no_manifest(_: &str) -> Option<Rc<Value>> {
    None
}

#[test]
fn tsconfig_paths_map_a_specifier_through_the_pattern_typescript_picks() {
    let sources = BTreeMap::from([
        (
            "tsconfig.base.json",
            r#"{ "compilerOptions": { "baseUrl": ".", "paths": {
                    "@app/*": ["packages/app/src/*"],
                    "@app/special": ["packages/app/special.ts"],
                    "vendor/*": ["node_modules/vendor/*"] } } }"#,
        ),
        (
            "packages/app/tsconfig.json",
            "{ \"extends\": \"../../tsconfig.base.json\", // the shared paths\n \"include\": [\"src\"] }",
        ),
        (
            "packages/web/tsconfig.json",
            r#"{ "compilerOptions": { "paths": { "@/*": ["${configDir}/src/*", "./fallback/*"] } } }"#,
        ),
        (
            "packages/web/lib/tsconfig.json",
            r#"{ "compilerOptions": { "strict": true } }"#,
        ),
        ("packages/app/src/util.ts", ""),
        ("packages/app/src/deep/index.ts", ""),
        ("packages/app/special.ts", ""),
        ("node_modules/vendor/x.js", ""),
        ("packages/web/fallback/only.ts", ""),
        ("packages/web/src/lib/api.ts", ""),
    ]);
    let is_file = |file: &str| sources.contains_key(file);
    let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
    let layout = Layout::new(BTreeSet::new(), &no_manifest, &is_file);
    let resolver = Resolver::new(&layout, &read);
    for (importer, specifier, expected) in [
        ("packages/app/src/a.ts", "@app/util", Some("packages/app/src/util.ts")),
        (
            "packages/app/src/a.ts",
            "@app/deep",
            Some("packages/app/src/deep/index.ts"),
        ),
        ("packages/app/src/a.ts", "@app/special", Some("packages/app/special.ts")),
        ("packages/app/src/a.ts", "@app/missing", None),
        ("packages/app/src/a.ts", "vendor/x", None),
        (
            "packages/web/src/a.ts",
            "@/lib/api",
            Some("packages/web/src/lib/api.ts"),
        ),
        ("packages/web/src/a.ts", "@/only", Some("packages/web/fallback/only.ts")),
        ("packages/web/lib/a.ts", "@/lib/api", None),
        ("other/a.ts", "@app/util", None),
    ] {
        assert_eq!(
            resolver.paths_target(importer, specifier).as_deref(),
            expected,
            "{importer} {specifier}: an exact pattern wins, a target in node_modules or one the nearest config does \
                 not map is none"
        );
    }
    assert_eq!(
        Resolver::relative(&is_file).resolve("packages/app/src/a.ts", "@app/util"),
        None,
        "a resolver of relative specifiers reads no paths"
    );
}

#[test]
fn unlisted_imports_a_declare_module_or_the_importer_s_own_tsconfig_paths_explain_split_off() {
    let sources = BTreeMap::from([
        (
            "src/typings/vscode.d.ts",
            "declare module 'vscode' {\n  export const x: number;\n}\n",
        ),
        (
            "src/typings/icons.d.ts",
            "declare module 'icons/*' {\n  const value: string;\n  export default value;\n}\n",
        ),
        (
            "src/augment.ts",
            "import 'react';\ndeclare module 'react' {\n  interface Extra {}\n}\nexport {};\n",
        ),
        (
            "src/a.ts",
            "import * as vscode from 'vscode';\nimport home from 'icons/home';\nimport React from 'react';\n\
                 import { api } from '@/lib/api';\nimport z from 'zod';\nconsole.log(vscode, home, React, api, z);\n",
        ),
        (
            "tsconfig.json",
            r#"{ "compilerOptions": { "paths": { "@/*": ["./src/*"] } } }"#,
        ),
        ("src/lib/api.ts", "export const api = 1;\n"),
    ]);
    let is_file = |file: &str| sources.contains_key(file);
    let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
    let layout = Layout::new(BTreeSet::new(), &no_manifest, &is_file);
    let resolver = Resolver::new(&layout, &read);
    let files: Vec<String> = sources.keys().map(|file| (*file).to_owned()).collect();
    let unlisted = strings(&[
        "src/a.ts|vscode",
        "src/a.ts|icons",
        "src/a.ts|react",
        "src/a.ts|@/lib",
        "src/a.ts|zod",
    ]);
    let ambient = ambient_module_keys(&unlisted, &files, &read);
    assert_eq!(
        ambient,
        strings(&["src/a.ts|icons", "src/a.ts|vscode"]),
        "a pattern declares the subpaths it matches, and an augmentation in a module file declares nothing"
    );
    assert!(ambient_module_keys(&BTreeSet::new(), &files, &read).is_empty());
    let own = own_paths_keys(&unlisted, &resolver, &read, &|_: &str| false);
    assert_eq!(own, strings(&["src/a.ts|@/lib"]));

    let classifier = Classifier {
        ambient_modules: &ambient,
        own_paths: &own,
        ..bare(&layout)
    };
    for (side, key, expected) in [
        (Side::Tool, "src/a.ts|vscode", Some(AMBIENT_MODULE)),
        (Side::Tool, "src/a.ts|icons", Some(AMBIENT_MODULE)),
        (Side::Tool, "src/a.ts|@/lib", Some(OWN_TSCONFIG_PATHS)),
        (Side::Tool, "src/a.ts|react", None),
        (Side::Tool, "src/a.ts|zod", None),
        (Side::Skif, "src/a.ts|vscode", None),
    ] {
        assert_eq!(classifier.classify(side, "unlisted", key), expected, "{side:?} {key}");
    }
}

#[test]
fn dependencies_and_files_of_imports_tsconfig_paths_map_before_the_package_split_off() {
    let sources = BTreeMap::from([
        (
            "packages/app/tsconfig.json",
            r#"{ "compilerOptions": { "paths": { "@acme/ui": ["./src/ui/index.ts"], "@acme/ui/*": ["./src/ui/*"],
                    "@acme/theme": ["../theme/src/index.ts"] } } }"#,
        ),
        (
            "packages/app/src/main.ts",
            "import { Button } from '@acme/ui';\nimport { cn } from '@acme/ui/cn';\nimport z from 'zod';\n\
                 import { dark } from '@acme/theme';\nconsole.log(Button, cn, z, dark);\n",
        ),
        ("packages/app/src/ui/index.ts", "export const Button = 1;\n"),
        ("packages/app/src/ui/cn.ts", "export const cn = 1;\n"),
        ("packages/theme/src/index.ts", "export const dark = 1;\n"),
        (
            "packages/other/src/main.ts",
            "import { Button } from '@acme/ui';\nconsole.log(Button);\n",
        ),
    ]);
    let manifests: BTreeMap<&str, Rc<Value>> = [
            ("", json!({ "name": "root", "workspaces": ["packages/*"] })),
            (
                "packages/app",
                json!({ "name": "app", "dependencies": { "@acme/ui": "1.0.0", "@acme/theme": "workspace:*", "zod": "3.0.0" } }),
            ),
            ("packages/other", json!({ "name": "other", "dependencies": { "@acme/ui": "1.0.0" } })),
            ("packages/theme", json!({ "name": "@acme/theme" })),
        ]
        .into_iter()
        .map(|(dir, manifest)| (dir, Rc::new(manifest)))
        .collect();
    let manifest = |dir: &str| manifests.get(dir).cloned();
    let is_file = |file: &str| sources.contains_key(file);
    let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
    let layout = Layout::new(
        strings(&["packages/app", "packages/other", "packages/theme"]),
        &manifest,
        &is_file,
    );
    let resolver = Resolver::new(&layout, &read);
    let files: Vec<String> = sources.keys().map(|file| (*file).to_owned()).collect();
    let found = paths_before_packages(&files, &resolver, &layout, &read);
    assert_eq!(
        found,
        strings(&[
            "packages/app/package.json|@acme/ui",
            "packages/app/src/ui/cn.ts",
            "packages/app/src/ui/index.ts",
        ]),
        "a mapping into another workspace member is an import of it, a workspace without a configuration that \
             maps the package keeps it, and a mapping to no file is none"
    );
    let classifier = Classifier {
        paths_before_packages: &found,
        ..bare(&layout)
    };
    for (side, category, key, expected) in [
        (
            Side::Skif,
            "dependencies",
            "packages/app/package.json|@acme/ui",
            Some(PATHS_BEFORE_PACKAGES),
        ),
        (
            Side::Tool,
            "files",
            "packages/app/src/ui/cn.ts",
            Some(PATHS_BEFORE_PACKAGES),
        ),
        (Side::Skif, "dependencies", "packages/app/package.json|zod", None),
        (
            Side::Skif,
            "dependencies",
            "packages/app/package.json|@acme/theme",
            None,
        ),
        (Side::Skif, "dependencies", "packages/other/package.json|@acme/ui", None),
        (Side::Skif, "files", "packages/app/src/ui/cn.ts", None),
        (Side::Tool, "dependencies", "packages/app/package.json|@acme/ui", None),
    ] {
        assert_eq!(
            classifier.classify(side, category, key),
            expected,
            "{side:?} {category} {key}"
        );
    }
}

#[test]
fn exports_files_only_knip_lists_unused_import_through_tsconfig_paths_and_workspace_packages_split_off() {
    let sources = BTreeMap::from([
        (
            "apps/web/tsconfig.json",
            r#"{ "compilerOptions": { "paths": { "@/*": ["./src/*"] } } }"#,
        ),
        (
            "apps/web/src/components/login.tsx",
            "import { login } from '@/lib/api';\nimport { Button } from '@acme/ui';\nimport { cn } from '@acme/ui/cn';\n\
                 console.log(login, Button, cn);\n",
        ),
        (
            "apps/web/src/lib/api.ts",
            "export const login = 1;\nexport const logout = 2;\n",
        ),
        ("packages/ui/src/index.ts", "export * from './button';\n"),
        (
            "packages/ui/src/button.ts",
            "export const Button = 1;\nexport const Link = 2;\n",
        ),
        ("packages/ui/src/cn.ts", "export const cn = 1;\n"),
    ]);
    let manifests: BTreeMap<&str, Rc<Value>> = [
        ("", json!({ "name": "root", "workspaces": ["apps/*", "packages/*"] })),
        ("apps/web", json!({ "name": "web" })),
        (
            "packages/ui",
            json!({ "name": "@acme/ui", "exports": { ".": "./dist/index.js", "./cn": "./dist/cn.js" } }),
        ),
    ]
    .into_iter()
    .map(|(dir, manifest)| (dir, Rc::new(manifest)))
    .collect();
    let manifest = |dir: &str| manifests.get(dir).cloned();
    let is_file = |file: &str| sources.contains_key(file);
    let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
    let layout = Layout::new(strings(&["apps/web", "packages/ui"]), &manifest, &is_file);
    let resolver = Resolver::new(&layout, &read);
    let unused = strings(&["apps/web/src/components/login.tsx"]);
    let credits = unused_file_credits(&unused, &BTreeSet::new(), &resolver, &read);
    for credited in [
        "apps/web/src/lib/api.ts|login",
        "packages/ui/src/index.ts|Button",
        "packages/ui/src/button.ts|Button",
        "packages/ui/src/cn.ts|cn",
    ] {
        assert!(credits.contains(credited), "{credited} in {credits:?}");
    }
    for uncredited in ["apps/web/src/lib/api.ts|logout", "packages/ui/src/button.ts|Link"] {
        assert!(!credits.contains(uncredited), "{uncredited} in {credits:?}");
    }
    assert!(
        unused_file_credits(&unused, &BTreeSet::new(), &Resolver::relative(&is_file), &read).is_empty(),
        "without paths and workspace packages the file imports nothing by relative path"
    );
}

#[test]
fn members_of_an_enum_used_as_a_whole_split_off() {
    let sources = BTreeMap::from([
        (
            "src/enums.ts",
            "export enum KeyCode { A, B }\nexport enum Language { En, De }\nexport enum Read { P, Q }\n\
                 export enum Typed { M, N }\nexport enum Local { L1, L2 }\nexport enum Lazy { Z1, Z2 }\n\
                 export const names = Object.keys(Local);\n",
        ),
        ("src/barrel.ts", "export * from './enums';\n"),
        (
            "src/api.ts",
            "import * as enums from './enums';\nexport const api = { KeyCode: enums.KeyCode, p: enums.Read.P };\n",
        ),
        (
            "src/dto.ts",
            "import { Language } from './barrel';\nexport class Dto { @IsEnum(Language) language!: string; }\n",
        ),
        (
            "src/input.ts",
            "import { Typed } from './barrel';\nexport function take(value: Typed): void { console.log(value); }\n",
        ),
        (
            "src/lazy.ts",
            "export async function load() { return import('./other'); }\n",
        ),
    ]);
    let is_file = |file: &str| sources.contains_key(file);
    let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
    let layout = Layout::new(BTreeSet::new(), &no_manifest, &is_file);
    let resolver = Resolver::new(&layout, &read);
    let files: Vec<String> = sources.keys().map(|file| (*file).to_owned()).collect();
    let members = strings(&[
        "src/enums.ts|A",
        "src/enums.ts|En",
        "src/enums.ts|L1",
        "src/enums.ts|Q",
        "src/enums.ts|M",
        "src/enums.ts|Z1",
        "src/gone.ts|A",
    ]);
    let found = whole_enum_members(&members, &files, &resolver, &read);
    assert_eq!(
        found,
        strings(&["src/enums.ts|A", "src/enums.ts|En", "src/enums.ts|L1"]),
        "a namespace member in an object literal, a decorator argument through a barrel and `Object.keys` in the \
             own file use the whole enum; a member read, a type annotation and an enum no file names do not"
    );
    assert!(whole_enum_members(&BTreeSet::new(), &files, &resolver, &read).is_empty());

    let classifier = Classifier {
        whole_enum_members: &found,
        ..bare(&layout)
    };
    assert_eq!(
        classifier.classify(Side::Tool, "enumMembers", "src/enums.ts|A"),
        Some(WHOLE_ENUM_USE)
    );
    for (side, category, key) in [
        (Side::Tool, "enumMembers", "src/enums.ts|Q"),
        (Side::Skif, "enumMembers", "src/enums.ts|A"),
        (Side::Tool, "exports", "src/enums.ts|A"),
    ] {
        assert_eq!(
            classifier.classify(side, category, key),
            None,
            "{side:?} {category} {key}"
        );
    }
}

/// A manifest reader over these manifests, by directory.
fn manifests_of(manifests: &[(&str, Value)]) -> impl Fn(&str) -> Option<Rc<Value>> + use<> {
    let read: BTreeMap<String, Rc<Value>> = manifests
        .iter()
        .map(|(dir, manifest)| ((*dir).to_owned(), Rc::new(manifest.clone())))
        .collect();
    move |dir: &str| read.get(dir).cloned()
}

#[test]
fn exports_a_published_source_or_a_nested_root_s_default_entry_re_exports_split_off() {
    let manifest = manifests_of(&[
        ("", json!({ "name": "root", "workspaces": ["packages/*"] })),
        (
            "packages/ai",
            json!({ "name": "@n8n/ai", "exports": { ".": "./dist/index.js" } }),
        ),
        ("e2e/registry", json!({ "name": "registry-setup", "private": true })),
    ]);
    let sources = BTreeMap::from([
        (
            "packages/ai/src/index.ts",
            "export * from './gen-ai/attributes';\nexport { get } from './node-catalog/get';\n",
        ),
        (
            "packages/ai/src/gen-ai/attributes.ts",
            "export const a = 1;\nexport const b = 2;\n",
        ),
        (
            "packages/ai/src/node-catalog/get.ts",
            "export function get() {}\nexport function other() {}\n",
        ),
        ("packages/ai/src/unused.ts", "export const lonely = 1;\n"),
        ("e2e/registry/index.js", "export * from './registry.js';\n"),
        ("e2e/registry/registry.js", "export const start = 1;\n"),
        ("e2e/registry/other.js", "export const stop = 1;\n"),
    ]);
    let is_file = |file: &str| sources.contains_key(file);
    let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
    let layout = Layout::new(strings(&["packages/ai"]), &manifest, &is_file);
    let resolver = Resolver::new(&layout, &read);
    let files: Vec<String> = sources.keys().map(|file| (*file).to_owned()).collect();
    let roots = strings(&["e2e/registry"]);
    let keys = strings(&[
        "packages/ai/src/gen-ai/attributes.ts|a",
        "packages/ai/src/node-catalog/get.ts|get",
        "packages/ai/src/unused.ts|lonely",
        "e2e/registry/registry.js|start",
    ]);
    let no_plugins: &dyn Fn(&str) -> PluginEntries = &|_: &str| PluginEntries::none();
    let (published, defaults) = entry_modules(&keys, &files, &layout, (&roots, no_plugins));
    assert_eq!(published, ["packages/ai/src/index.ts"]);
    assert_eq!(
        defaults,
        ["e2e/registry/index.js"],
        "a workspace's default entry is knip's entry as well"
    );
    assert_eq!(
        entry_modules(&BTreeSet::new(), &files, &layout, (&roots, no_plugins)),
        (Vec::new(), Vec::new())
    );
    let published_reexports = entry_reexports(published, &resolver, &read);
    let root_entry_reexports = entry_reexports(defaults, &resolver, &read);
    let classifier = Classifier {
        published_reexports: &published_reexports,
        root_entry_reexports: &root_entry_reexports,
        skif_root_packages: &roots,
        ..bare(&layout)
    };
    for (category, key, expected) in [
        (
            "exports",
            "packages/ai/src/gen-ai/attributes.ts|a",
            Some(PUBLISHED_SOURCE),
        ),
        (
            "types",
            "packages/ai/src/node-catalog/get.ts|get",
            Some(PUBLISHED_SOURCE),
        ),
        ("exports", "packages/ai/src/node-catalog/get.ts|other", None),
        ("exports", "packages/ai/src/unused.ts|lonely", None),
        ("enumMembers", "packages/ai/src/gen-ai/attributes.ts|a", None),
        ("exports", "e2e/registry/registry.js|start", Some(NON_MEMBER_PROJECT)),
        ("exports", "e2e/registry/other.js|stop", None),
    ] {
        assert_eq!(
            classifier.classify(Side::Tool, category, key),
            expected,
            "{category} {key}"
        );
    }
}

#[test]
fn exports_of_the_files_skif_s_plugins_make_entries_of_a_nested_root_split_off() {
    let kyc = json!({ "name": "kyc", "dependencies": { "@mastra/core": "1.0.0" } });
    let gql = json!({ "name": "gql", "devDependencies": { "@graphql-codegen/cli": "2.13.7" } });
    let manifest = manifests_of(&[
        ("", json!({ "name": "root", "workspaces": ["packages/*"] })),
        ("templates/kyc", kyc.clone()),
        ("examples/gql", gql.clone()),
    ]);
    let sources = BTreeMap::from([
        ("templates/kyc/package.json", kyc.to_string()),
        (
            "templates/kyc/src/mastra/tools/start.ts",
            "export type StartInput = { id: string };\nexport const startTool = 1;\n".to_owned(),
        ),
        (
            "templates/kyc/src/mastra/workflows/flow.ts",
            "export { collect } from './steps/collect';\nexport const flow = 1;\n".to_owned(),
        ),
        (
            "templates/kyc/src/mastra/workflows/steps/collect.ts",
            "export const collect = 1;\nexport const resumeSchema = 1;\n".to_owned(),
        ),
        ("examples/gql/package.json", gql.to_string()),
        (
            "examples/gql/codegen.ts",
            "export default { schema: 'schema.graphql', generates: { './lib/gql/': { preset: 'client' } } };\n"
                .to_owned(),
        ),
        (
            "examples/gql/lib/gql/graphql.ts",
            "export type Maybe<T> = T | null;\n".to_owned(),
        ),
        ("examples/gql/lib/other.ts", "export type Other = 1;\n".to_owned()),
    ]);
    let mut memory = MemoryFs::new();
    for (path, source) in &sources {
        memory = memory.with_file(format!("/repo/{path}"), source);
    }
    let root = Path::new("/repo");
    let kyc_entries = plugin_entries(&memory, root, ("templates/kyc", true), &kyc, &[]);
    assert!(kyc_entries.contains("src/mastra/tools/start.ts"));
    assert!(kyc_entries.contains("src/mastra/workflows/flow.ts"));
    assert!(
        !kyc_entries.contains("src/mastra/workflows/steps/collect.ts"),
        "Mastra's workflow glob takes the files directly in `workflows`"
    );
    let gql_entries = plugin_entries(&memory, root, ("examples/gql", true), &gql, &[]);
    assert!(gql_entries.contains("lib/gql/graphql.ts"), "a `generates` output");
    assert!(gql_entries.contains("codegen.ts"), "a code configuration file");
    assert!(!gql_entries.contains("lib/other.ts"));
    assert!(!PluginEntries::none().contains("lib/gql/graphql.ts"));
    assert_eq!(
        packages_below(
            &strings(&["", "templates/kyc", "templates/kyc/portal", "templates/kycx"]),
            "templates/kyc"
        ),
        ["portal"]
    );

    let is_file = |file: &str| sources.contains_key(file);
    let read = |file: &str| sources.get(file).cloned();
    let layout = Layout::new(strings(&["packages/app"]), &manifest, &is_file);
    let resolver = Resolver::new(&layout, &read);
    let files: Vec<String> = sources.keys().map(|file| (*file).to_owned()).collect();
    let roots = strings(&["templates/kyc", "examples/gql"]);
    let manifests = [("templates/kyc", &kyc), ("examples/gql", &gql)];
    let entries_of: &dyn Fn(&str) -> PluginEntries = &|dir: &str| {
        manifests
            .iter()
            .find(|(project, _)| *project == dir)
            .map_or_else(PluginEntries::none, |(_, manifest)| {
                plugin_entries(&memory, root, (dir, true), manifest, &[])
            })
    };
    let keys = strings(&[
        "templates/kyc/src/mastra/tools/start.ts|StartInput",
        "examples/gql/lib/gql/graphql.ts|Maybe",
    ]);
    let (published, defaults) = entry_modules(&keys, &files, &layout, (&roots, entries_of));
    assert_eq!(published, Vec::<String>::new());
    assert_eq!(
        defaults,
        [
            "examples/gql/codegen.ts",
            "examples/gql/lib/gql/graphql.ts",
            "templates/kyc/src/mastra/tools/start.ts",
            "templates/kyc/src/mastra/workflows/flow.ts",
        ]
    );
    let (_, not_root) = entry_modules(&keys, &files, &layout, (&BTreeSet::new(), entries_of));
    assert!(
        not_root.is_empty(),
        "a project skif does not lint as a root package has no entries here"
    );
    let root_entry_reexports = entry_reexports(defaults, &resolver, &read);
    let classifier = Classifier {
        root_entry_reexports: &root_entry_reexports,
        skif_root_packages: &roots,
        ..bare(&layout)
    };
    for (category, key, expected) in [
        (
            "types",
            "templates/kyc/src/mastra/tools/start.ts|StartInput",
            Some(NON_MEMBER_PROJECT),
        ),
        (
            "exports",
            "templates/kyc/src/mastra/workflows/steps/collect.ts|collect",
            Some(NON_MEMBER_PROJECT),
        ),
        (
            "exports",
            "templates/kyc/src/mastra/workflows/steps/collect.ts|resumeSchema",
            None,
        ),
        (
            "nsTypes",
            "examples/gql/lib/gql/graphql.ts|Maybe",
            Some(NON_MEMBER_PROJECT),
        ),
        ("types", "examples/gql/lib/other.ts|Other", None),
        ("enumMembers", "templates/kyc/src/mastra/tools/start.ts|A", None),
    ] {
        assert_eq!(
            classifier.classify(Side::Tool, category, key),
            expected,
            "{category} {key}"
        );
    }
}

#[test]
fn the_sources_a_published_file_loads_from_build_output_are_published_sources() {
    let manifest = manifests_of(&[
        ("", json!({ "name": "root", "workspaces": ["packages/*"] })),
        (
            "packages/next",
            json!({ "name": "next", "main": "./dist/server/next.js",
                        "files": ["dist", "client.js", "client.d.ts", "app.js", "*.d.mts"] }),
        ),
    ]);
    let sources = BTreeMap::from([
        (
            "packages/next/client.js",
            "module.exports = require('./dist/client/index')\n",
        ),
        ("packages/next/client.d.ts", "export * from './dist/client/index'\n"),
        ("packages/next/app.js", "module.exports = require('./compat/app')\n"),
        ("packages/next/dist/server/next.js", "require('../client/other');\n"),
        (
            "packages/next/src/client/index.tsx",
            "import { help } from './helper';\nhelp();\n",
        ),
        ("packages/next/src/client/helper.ts", "export function help() {}\n"),
        ("packages/next/src/client/other.ts", "export const other = 1;\n"),
        ("packages/next/compat/app.ts", "export const app = 1;\n"),
        ("packages/next/src/lonely.ts", "export const lonely = 1;\n"),
    ]);
    let is_file = |file: &str| sources.contains_key(file);
    let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
    let layout = Layout::new(strings(&["packages/next"]), &manifest, &is_file);
    assert_eq!(
        published_file_sources(&layout, "packages/next", &read),
        ["packages/next/src/client/index.tsx"],
        "a specifier outside build output and one from a file inside it map to nothing"
    );
    let knip_unused = strings(&[
        "packages/next/src/client/index.tsx",
        "packages/next/src/client/helper.ts",
        "packages/next/src/client/other.ts",
        "packages/next/compat/app.ts",
        "packages/next/src/lonely.ts",
    ]);
    let reach = published_reach(&knip_unused, &BTreeSet::new(), &layout, &read);
    assert_eq!(
        reach,
        strings(&[
            "packages/next/src/client/helper.ts",
            "packages/next/src/client/index.tsx"
        ])
    );
    let classifier = Classifier {
        published_reach: &reach,
        ..bare(&layout)
    };
    for (key, expected) in [
        ("packages/next/src/client/index.tsx", Some(PUBLISHED_SOURCE)),
        ("packages/next/src/client/helper.ts", Some(PUBLISHED_SOURCE)),
        ("packages/next/src/client/other.ts", None),
        ("packages/next/src/lonely.ts", None),
    ] {
        assert_eq!(classifier.classify(Side::Tool, "files", key), expected, "{key}");
    }
}

#[test]
fn findings_in_files_a_dot_slash_gitignore_pattern_hides_from_knip_split_off_on_skif_side() {
    let gitignores = [
        (
            "packages/core/.gitignore".to_owned(),
            "dist\n./storage/\n./a/b\n!./keep\n./*.log\n# ./c\n./\n./x/../y\n".to_owned(),
        ),
        (".gitignore".to_owned(), "./root-only\n".to_owned()),
    ];
    let ignores = dot_slash_ignores(&gitignores);
    assert_eq!(
        ignores,
        [
            ("packages/core".to_owned(), vec!["storage".to_owned()]),
            ("packages/core".to_owned(), vec!["a".to_owned(), "b".to_owned()]),
        ]
    );
    for (path, expected) in [
        ("packages/core/src/storage/index.ts", true),
        ("packages/core/storage", true),
        ("packages/core/src/a/b/c.ts", true),
        ("packages/core/src/a/c.ts", false),
        ("packages/core/src/storage-utils.ts", false),
        ("packages/core-x/src/storage/x.ts", false),
        ("packages/other/src/storage/x.ts", false),
        ("root-only/x.ts", false),
    ] {
        assert_eq!(in_dot_slash_ignore(&ignores, path), expected, "{path}");
    }
    let every_file = |_: &str| true;
    let layout = Layout::new(BTreeSet::new(), &no_manifest, &every_file);
    let classifier = Classifier {
        knip_dot_ignores: &ignores,
        ..bare(&layout)
    };
    for (side, category, key, expected) in [
        (
            Side::Skif,
            "duplicates",
            "packages/core/src/storage/logs.ts|createLog|log",
            Some(KNIP_DOT_IGNORE),
        ),
        (
            Side::Skif,
            "files",
            "packages/core/src/storage/old.ts",
            Some(KNIP_DOT_IGNORE),
        ),
        (Side::Skif, "exports", "packages/core/src/other.ts|x", None),
        (Side::Tool, "exports", "packages/core/src/storage/logs.ts|x", None),
    ] {
        assert_eq!(
            classifier.classify(side, category, key),
            expected,
            "{side:?} {category} {key}"
        );
    }
}

#[test]
fn duplicates_whose_aliases_name_an_export_of_an_import_split_off_on_skif_side() {
    let sources = BTreeMap::from([
        (
            "locations.ts",
            "import { Daily as importedDaily } from './constants';\n\
                 export const DailyLocationType = importedDaily;\n\
                 export const CalVideoLocationType = DailyLocationType;\n",
        ),
        (
            "menu.ts",
            "export const inset = 'pl-1';\nexport const labelInset = inset;\n",
        ),
        (
            "with-default.ts",
            "import { a } from './a';\nexport const b = a;\nexport const c = b;\nexport default c;\n",
        ),
        ("broken.ts", "export const = ;\n"),
    ]);
    let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
    let duplicates = strings(&[
        "locations.ts|CalVideoLocationType|DailyLocationType",
        "menu.ts|inset|labelInset",
        "with-default.ts|b|c|default",
        "broken.ts|a|b",
        "missing.ts|a|b",
    ]);
    let found = reexport_alias_duplicates(&duplicates, &read);
    assert_eq!(found, strings(&["locations.ts|CalVideoLocationType|DailyLocationType"]));
    let every_file = |_: &str| true;
    let layout = Layout::new(BTreeSet::new(), &no_manifest, &every_file);
    let classifier = Classifier {
        reexport_aliases: &found,
        ..bare(&layout)
    };
    for (side, key, expected) in [
        (
            Side::Skif,
            "locations.ts|CalVideoLocationType|DailyLocationType",
            Some(REEXPORT_ALIAS),
        ),
        (Side::Skif, "menu.ts|inset|labelInset", None),
        (Side::Tool, "locations.ts|CalVideoLocationType|DailyLocationType", None),
    ] {
        assert_eq!(classifier.classify(side, "duplicates", key), expected, "{side:?} {key}");
    }
}

#[test]
fn declaration_and_javascript_files_a_manifest_publishes_as_they_are_split_off() {
    let manifest = manifests_of(&[
        ("", json!({ "name": "root", "workspaces": ["packages/*"] })),
        (
            "packages/next",
            json!({ "name": "next", "files": ["dist", "cache.js", "cache.d.ts", "*.d.mts"],
                        "exports": { "./navigation": "./navigation.js" } }),
        ),
    ]);
    let every_file = |_: &str| true;
    let layout = Layout::new(strings(&["packages/next"]), &manifest, &every_file);
    for (path, expected) in [
        ("packages/next/cache.d.ts", true),
        ("packages/next/cache.js", true),
        ("packages/next/navigation.js", true),
        ("packages/next/types.d.mts", true),
        ("packages/next/lib/server.js", false),
        ("packages/next/other.js", false),
        ("packages/next/cache.ts", false),
    ] {
        assert_eq!(layout.publishes_file(path), expected, "{path}");
    }
    let classifier = bare(&layout);
    for (side, category, key, expected) in [
        (
            Side::Tool,
            "types",
            "packages/next/cache.d.ts|Cache",
            Some(PUBLISHED_FILE),
        ),
        (
            Side::Tool,
            "exports",
            "packages/next/navigation.js|redirect",
            Some(PUBLISHED_FILE),
        ),
        (Side::Tool, "files", "packages/next/cache.js", Some(PUBLISHED_FILE)),
        (Side::Tool, "files", "packages/next/cache.d.ts", Some(DECLARATION_FILE)),
        (Side::Tool, "enumMembers", "packages/next/cache.js|X", None),
        (Side::Tool, "exports", "packages/next/other.js|x", None),
        (Side::Skif, "exports", "packages/next/cache.js|x", None),
    ] {
        assert_eq!(
            classifier.classify(side, category, key),
            expected,
            "{side:?} {category} {key}"
        );
    }
}

#[test]
fn files_bazel_rules_build_from_and_what_they_import_split_off() {
    let build = "load(\"@aspect_rules_ts//ts:defs.bzl\", \"ts_project\")\n\
                     ts_project(\n    name = \"pipeline\",\n    srcs = [\"index.mts\", \":helpers/format.mts\", \
                     \"//other:x.mts\", \"missing.mts\"] + glob([\"globbed/*.mts\"]),\n    \
                     tsconfig = \"tsconfig.json\",\n    data = select({\"//:on\": [\"on.mts\"]}),\n)\n\
                     js_binary(name = \"run\", entry_point = \"run.mts\", data = [\"data.json\"])\n\
                     js_unknown(name = \"x\", srcs = [\"unknown.mts\"])\n\
                     \"tail.mts\".format(x)\n";
    let literals: Vec<(String, String, String)> = rule_calls(build)
        .into_iter()
        .flat_map(|call| {
            let rule = call.rule;
            call.literals
                .into_iter()
                .map(move |(attribute, literal)| (rule.clone(), attribute, literal))
        })
        .collect();
    let named = |rule: &str, attribute: &str| -> Vec<&str> {
        literals
            .iter()
            .filter(|(called, key, _)| called == rule && key == attribute)
            .map(|(_, _, literal)| literal.as_str())
            .collect()
    };
    assert_eq!(
        named("ts_project", "srcs"),
        ["index.mts", ":helpers/format.mts", "//other:x.mts", "missing.mts"],
        "a glob is a nested call"
    );
    assert!(named("ts_project", "data").is_empty(), "a select is a nested call");
    assert_eq!(named("js_binary", "entry_point"), ["run.mts"]);
    assert!(
        literals
            .iter()
            .all(|(called, _, _)| called != "load" && called != "format")
    );

    let rules_js = "bazel_dep(name = \"aspect_rules_js\", version = \"2.0.0\")\n";
    let sources = BTreeMap::from([
        ("MODULE.bazel", rules_js),
        ("adev/pipeline/BUILD.bazel", build),
        (
            "adev/pipeline/index.mts",
            "import { util } from './lib/util.mts';\nconsole.log(util);\n",
        ),
        ("adev/pipeline/helpers/format.mts", "export const format = 1;\n"),
        (
            "adev/pipeline/lib/util.mts",
            "export { deep } from './deep.mts';\nexport const util = 1;\n",
        ),
        ("adev/pipeline/lib/deep.mts", "export const deep = 1;\n"),
        ("adev/pipeline/run.mts", "console.log(1);\n"),
        ("adev/pipeline/data.json", "{}"),
        ("adev/pipeline/unknown.mts", "export const unknown = 1;\n"),
        ("adev/pipeline/globbed/a.mts", "export const a = 1;\n"),
        ("adev/pipeline/on.mts", "export const on = 1;\n"),
        ("plain/MODULE.bazel", "module(name = \"plain\")\n"),
        ("plain/BUILD.bazel", "ts_project(name = \"a\", srcs = [\"a.mts\"])\n"),
        ("plain/a.mts", "export const a = 1;\n"),
    ]);
    let is_file = |file: &str| sources.contains_key(file);
    let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
    let build_files: Vec<(String, String)> = sources
        .iter()
        .filter(|(path, _)| is_build_file(path))
        .map(|(path, source)| ((*path).to_owned(), (*source).to_owned()))
        .collect();
    let files: Vec<String> = sources.keys().map(|path| (*path).to_owned()).collect();
    let sources = bazel_sources(&build_files, &files, &is_file, &read);
    assert_eq!(
        sources,
        strings(&[
            "adev/pipeline/data.json",
            "adev/pipeline/helpers/format.mts",
            "adev/pipeline/index.mts",
            "adev/pipeline/run.mts",
        ]),
        "a label of another package, a missing file, a rule skif does not read, the glob of a target that is no \
             test and a repository without rules_js name none"
    );
    let knip_unused = strings(&[
        "adev/pipeline/index.mts",
        "adev/pipeline/lib/util.mts",
        "adev/pipeline/lib/deep.mts",
        "adev/pipeline/unknown.mts",
        "adev/pipeline/globbed/a.mts",
        "plain/a.mts",
    ]);
    let reached = reach_from_sources(&sources, (&knip_unused, &BTreeSet::new()), &is_file, &read);
    assert_eq!(
        reached,
        strings(&[
            "adev/pipeline/index.mts",
            "adev/pipeline/lib/deep.mts",
            "adev/pipeline/lib/util.mts",
        ])
    );
    assert!(reach_from_sources(&sources, (&knip_unused, &knip_unused), &is_file, &read).is_empty());

    let layout = Layout::new(BTreeSet::new(), &no_manifest, &is_file);
    let classifier = Classifier {
        bazel_sources: &reached,
        ..bare(&layout)
    };
    for (category, key, expected) in [
        ("files", "adev/pipeline/index.mts", Some(BAZEL_BUILD_FILE)),
        ("files", "adev/pipeline/lib/deep.mts", Some(BAZEL_BUILD_FILE)),
        ("files", "adev/pipeline/unknown.mts", None),
        ("files", "adev/pipeline/globbed/a.mts", None),
        ("exports", "adev/pipeline/index.mts|x", None),
    ] {
        assert_eq!(
            classifier.classify(Side::Tool, category, key),
            expected,
            "{category} {key}"
        );
    }
}

#[test]
fn the_glob_patterns_of_a_bazel_test_target_name_build_sources() {
    let build = "ts_project(\n    name = \"test_lib\",\n    testonly = True,\n    srcs = glob(\n        \
                     [\"*_spec.ts\", \"testing/**/*.ts\"],\n        exclude = [\"skipped_spec.ts\"],\n    ) + \
                     [\"extra.ts\"],\n)\n\
                     ng_project(name = \"specs\", testonly = True, srcs = native.glob(include = [\"ng/*.ts\"]))\n\
                     js_test(name = \"run\", entry_point = \"run.mjs\", data = glob([\"fixtures/*.mjs\"]))\n\
                     ts_project(name = \"lib\", srcs = glob([\"src/*.ts\"]))\n\
                     ts_project(name = \"maybe\", testonly = False, srcs = glob([\"maybe/*.ts\"]))\n\
                     ts_project(name = \"nested\", testonly = True, srcs = select({\"//:on\": glob([\"on/*.ts\"])}))\n\
                     ts_project(name = \"climbs\", testonly = True, srcs = glob([\"../up/*.ts\"]))\n";
    let sources = BTreeMap::from([
        (
            "MODULE.bazel",
            "bazel_dep(name = \"aspect_rules_js\", version = \"2.0.0\")\n",
        ),
        ("pkg/BUILD.bazel", build),
        ("pkg/a_spec.ts", "export {};\n"),
        ("pkg/skipped_spec.ts", "export {};\n"),
        ("pkg/extra.ts", "export {};\n"),
        ("pkg/testing/deep/helper.ts", "export {};\n"),
        ("pkg/ng/b.spec.ts", "export {};\n"),
        ("pkg/fixtures/c.mjs", "export {};\n"),
        ("pkg/run.mjs", "export {};\n"),
        ("pkg/src/d.ts", "export {};\n"),
        ("pkg/maybe/e.ts", "export {};\n"),
        ("pkg/on/f.ts", "export {};\n"),
        ("up/g.ts", "export {};\n"),
        ("other/a_spec.ts", "export {};\n"),
    ]);
    let is_file = |file: &str| sources.contains_key(file);
    let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
    let build_files = vec![("pkg/BUILD.bazel".to_owned(), build.to_owned())];
    let files: Vec<String> = sources.keys().map(|path| (*path).to_owned()).collect();
    assert_eq!(
        bazel_sources(&build_files, &files, &is_file, &read),
        strings(&[
            "pkg/a_spec.ts",
            "pkg/extra.ts",
            "pkg/fixtures/c.mjs",
            "pkg/ng/b.spec.ts",
            "pkg/run.mjs",
            "pkg/skipped_spec.ts",
            "pkg/testing/deep/helper.ts",
        ]),
        "the include patterns of a test target's glob match below its BUILD file, its exclude patterns are not \
             applied, and the glob of a production target, one inside select and one that climbs out name none"
    );
}

#[test]
fn the_sources_a_published_node_bin_stub_loads_are_published_sources() {
    assert_eq!(
        stub_specifiers(
            "#!/usr/bin/env node\nrequire('../lib-commonjs/start.js');\nimport(\"./x\");\nrequire('fs');\n"
        ),
        ["../lib-commonjs/start.js", "./x"]
    );
    let manifest = manifests_of(&[
        ("", json!({ "name": "root" })),
        (
            "apps/client",
            json!({ "name": "client", "bin": { "client": "./bin/client" } }),
        ),
        ("apps/plain", json!({ "name": "plain", "bin": "./bin/plain" })),
    ]);
    let sources = BTreeMap::from([
        (
            "apps/client/bin/client",
            "#!/usr/bin/env node\nrequire('../lib-commonjs/start.js');\n",
        ),
        ("apps/client/src/start.ts", "import { run } from './run';\nrun();\n"),
        ("apps/client/src/run.ts", "export function run() {}\n"),
        ("apps/client/src/lonely.ts", "export const lonely = 1;\n"),
        ("apps/plain/bin/plain", "#!/bin/sh\nexec ../src/x.js\n"),
        ("apps/plain/src/x.ts", "export const x = 1;\n"),
    ]);
    let is_file = |file: &str| sources.contains_key(file);
    let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
    let layout = Layout::new(strings(&["apps/client", "apps/plain"]), &manifest, &is_file);
    assert_eq!(
        bin_stub_sources(&layout, "apps/client", &read),
        ["apps/client/src/start.ts"]
    );
    assert!(
        bin_stub_sources(&layout, "apps/plain", &read).is_empty(),
        "a shell stub is no node stub"
    );
    let knip_unused = strings(&[
        "apps/client/src/start.ts",
        "apps/client/src/run.ts",
        "apps/client/src/lonely.ts",
        "apps/plain/src/x.ts",
    ]);
    assert_eq!(
        published_reach(&knip_unused, &BTreeSet::new(), &layout, &read),
        strings(&["apps/client/src/run.ts", "apps/client/src/start.ts"])
    );
}

#[test]
fn exports_only_skif_reports_whose_users_skif_does_not_read_split_off_on_skif_side() {
    let manifest = manifests_of(&[
        ("", json!({ "name": "root" })),
        (
            ".github/scripts",
            json!({ "name": "scripts", "type": "module", "private": true }),
        ),
    ]);
    let sources = BTreeMap::from([
        (
            "scripts/preview/labels.mjs",
            "export const PREFIX = 'x';\nexport const other = 1;\n",
        ),
        (
            ".github/scripts/preview.mjs",
            "import { PREFIX } from '../../scripts/preview/labels.mjs';\nconsole.log(PREFIX);\n",
        ),
        (".github/scripts/own.mjs", "export const inner = 1;\n"),
        (
            ".github/scripts/user.mjs",
            "import { inner } from './own.mjs';\nconsole.log(inner);\n",
        ),
        (
            ".devcontainer/ssh.test.mjs",
            "import { alias } from '../scripts/ssh.mjs';\nconsole.log(alias);\n",
        ),
        ("scripts/ssh.mjs", "export const alias = 1;\nexport const kept = 2;\n"),
    ]);
    let is_file = |file: &str| sources.contains_key(file);
    let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
    let layout = Layout::new(BTreeSet::new(), &manifest, &is_file);
    let resolver = Resolver::new(&layout, &read);
    let files: Vec<String> = sources.keys().map(|file| (*file).to_owned()).collect();
    let (non_member, skipped) = outside_credits(&files, &layout, &resolver, &read);
    assert!(
        non_member.contains("scripts/preview/labels.mjs|PREFIX"),
        "{non_member:?}"
    );
    assert!(skipped.contains("scripts/ssh.mjs|alias"), "{skipped:?}");
    assert!(!skipped.contains("scripts/preview/labels.mjs|PREFIX"), "{skipped:?}");
    let classifier = Classifier {
        non_member_credits: &non_member,
        skipped_credits: &skipped,
        ..bare(&layout)
    };
    for (side, category, key, expected) in [
        (
            Side::Skif,
            "exports",
            "scripts/preview/labels.mjs|PREFIX",
            Some(NON_MEMBER_PROJECT),
        ),
        (Side::Skif, "exports", "scripts/ssh.mjs|alias", Some(SKIPPED_DIRECTORY)),
        (Side::Skif, "exports", "scripts/preview/labels.mjs|other", None),
        (Side::Skif, "exports", "scripts/ssh.mjs|kept", None),
        (Side::Skif, "exports", ".github/scripts/own.mjs|inner", None),
        (Side::Skif, "enumMembers", "scripts/ssh.mjs|alias", None),
        (Side::Tool, "exports", "scripts/ssh.mjs|alias", None),
    ] {
        assert_eq!(
            classifier.classify(side, category, key),
            expected,
            "{side:?} {category} {key}"
        );
    }
}

#[test]
fn members_only_skif_reports_whose_users_skif_does_not_read_split_off_on_skif_side() {
    let manifest = manifests_of(&[
        ("", json!({ "name": "root" })),
        (".github/scripts", json!({ "name": "scripts", "private": true })),
    ]);
    let sources = BTreeMap::from([
        (
            "src/phases.ts",
            "export enum Phase { Start = 'start', Stop = 'stop', Idle = 'idle' }\nexport enum Kept { Only = 1 }\n",
        ),
        ("src/mode.ts", "export enum Mode { On, Off }\n"),
        (
            "src/tools.ts",
            "export namespace Tools {\nexport const run = 1;\nexport const idle = 2;\nexport const spare = 3;\n}\n",
        ),
        (
            "src/index.ts",
            "export * from './phases';\nexport { Tools as T } from './tools';\n",
        ),
        (
            ".github/scripts/preview.ts",
            "import { Phase } from '../../src/phases';\nconsole.log(Phase.Start);\n",
        ),
        (
            ".github/scripts/all.ts",
            "import * as api from '../../src/index';\nconsole.log(api.Phase.Stop);\n",
        ),
        (
            ".github/scripts/values.ts",
            "import { Mode } from '../../src/mode';\nconsole.log(Object.values(Mode));\n",
        ),
        (
            ".devcontainer/check.ts",
            "import { Tools } from '../src/tools';\nconsole.log(Tools.run);\n",
        ),
        (
            ".devcontainer/alias.ts",
            "import { T } from '../src/index';\nconsole.log(T.idle);\n",
        ),
    ]);
    let is_file = |file: &str| sources.contains_key(file);
    let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
    let layout = Layout::new(BTreeSet::new(), &manifest, &is_file);
    let resolver = Resolver::new(&layout, &read);
    let files: Vec<String> = sources.keys().map(|file| (*file).to_owned()).collect();
    let members = strings(&[
        "src/phases.ts|Start",
        "src/phases.ts|Stop",
        "src/phases.ts|Idle",
        "src/phases.ts|Only",
        "src/mode.ts|On",
        "src/mode.ts|Off",
        "src/tools.ts|run",
        "src/tools.ts|idle",
        "src/tools.ts|spare",
    ]);
    let (non_member, skipped) = outside_members(&members, &files, &layout, &resolver, &read);
    assert_eq!(
        non_member,
        strings(&[
            "src/mode.ts|Off",
            "src/mode.ts|On",
            "src/phases.ts|Start",
            "src/phases.ts|Stop",
        ])
    );
    assert_eq!(skipped, strings(&["src/tools.ts|idle", "src/tools.ts|run"]));
    let classifier = Classifier {
        non_member_members: &non_member,
        skipped_members: &skipped,
        ..bare(&layout)
    };
    for (side, category, key, expected) in [
        (
            Side::Skif,
            "enumMembers",
            "src/phases.ts|Start",
            Some(NON_MEMBER_PROJECT),
        ),
        (
            Side::Skif,
            "enumMembers",
            "src/phases.ts|Stop",
            Some(NON_MEMBER_PROJECT),
        ),
        (Side::Skif, "enumMembers", "src/mode.ts|On", Some(NON_MEMBER_PROJECT)),
        (Side::Skif, "enumMembers", "src/phases.ts|Idle", None),
        (Side::Skif, "enumMembers", "src/phases.ts|Only", None),
        (
            Side::Skif,
            "namespaceMembers",
            "src/tools.ts|run",
            Some(SKIPPED_DIRECTORY),
        ),
        (
            Side::Skif,
            "namespaceMembers",
            "src/tools.ts|idle",
            Some(SKIPPED_DIRECTORY),
        ),
        (Side::Skif, "namespaceMembers", "src/tools.ts|spare", None),
        (Side::Skif, "exports", "src/tools.ts|run", None),
        (Side::Tool, "enumMembers", "src/phases.ts|Start", None),
    ] {
        assert_eq!(
            classifier.classify(side, category, key),
            expected,
            "{side:?} {category} {key}"
        );
    }
}

#[test]
fn install_dependent_findings_of_a_project_without_installed_packages_split_off_on_both_sides() {
    let skif = Keys::from([("unlisted".to_owned(), strings(&["package.json|ultracite"]))]);
    let knip = Keys::from([
        ("binaries".to_owned(), strings(&["package.json|husky"])),
        (
            "devDependencies".to_owned(),
            strings(&["package.json|oxfmt", "examples/basic/package.json|turbo"]),
        ),
        ("exports".to_owned(), strings(&["src/a.ts|x"])),
    ]);
    let manifest = manifests_of(&[
        ("", json!({ "name": "root" })),
        ("examples/basic", json!({ "name": "basic" })),
    ]);
    let every_file = |_: &str| true;
    let layout = Layout::new(BTreeSet::new(), &manifest, &every_file);
    let is_project = |dir: &str| layout.is_project(dir);
    let has_packages = |dir: &str| dir == "examples/basic";
    let uninstalled = uninstalled_projects((&skif, &knip), &is_project, &has_packages);
    assert_eq!(uninstalled, strings(&[""]));
    let installed_root = |dir: &str| dir.is_empty();
    assert!(uninstalled_projects((&skif, &knip), &is_project, &installed_root).is_empty());
    let classifier = Classifier {
        uninstalled_projects: &uninstalled,
        ..bare(&layout)
    };
    for (side, category, key, expected) in [
        (Side::Tool, "binaries", "package.json|husky", Some(UNINSTALLED_PROJECT)),
        (
            Side::Tool,
            "devDependencies",
            "package.json|oxfmt",
            Some(UNINSTALLED_PROJECT),
        ),
        (
            Side::Skif,
            "unlisted",
            "package.json|ultracite",
            Some(UNINSTALLED_PROJECT),
        ),
        (Side::Tool, "devDependencies", "examples/basic/package.json|turbo", None),
        (Side::Tool, "exports", "src/a.ts|x", None),
    ] {
        assert_eq!(
            classifier.classify(side, category, key),
            expected,
            "{side:?} {category} {key}"
        );
    }
}

#[test]
fn exports_only_knip_reports_that_a_used_export_s_signature_names_split_off() {
    let source = "export interface Props { a: string }\n\
                      export interface Hidden { a: string }\n\
                      export interface Entry { a: string }\n\
                      export interface Lonely { a: string }\n\
                      export function Widget(p: Props): void {}\n\
                      export function unused(h: Hidden): void {}\n\
                      export class Store { get(): Entry { return { a: '' }; } }\n\
                      export function open(): Store { return new Store(); }\n";
    let read = |file: &str| (file == "src/ops.ts").then(|| source.to_owned());
    let skif = Keys::from([
        (
            "exports".to_owned(),
            strings(&["src/ops.ts|unused", "src/ops.ts|Store"]),
        ),
        ("types".to_owned(), strings(&["src/ops.ts|Lonely"])),
    ]);
    let knip = Keys::from([
        (
            "types".to_owned(),
            strings(&[
                "src/ops.ts|Props",
                "src/ops.ts|Hidden",
                "src/ops.ts|Entry",
                "src/ops.ts|Lonely",
                "src/gone.ts|Props",
            ]),
        ),
        ("exports".to_owned(), strings(&["src/ops.ts|unused"])),
    ]);
    let found = signature_types(&skif, &knip, &read);
    assert_eq!(
        found,
        strings(&["src/ops.ts|Entry", "src/ops.ts|Props"]),
        "the signature of an export skif reports does not count, one of an export in a used signature does, and \
             a file that cannot be read yields nothing"
    );

    let manifest = |dir: &str| dir.is_empty().then(|| Rc::new(json!({ "name": "root" })));
    let every_file = |_: &str| true;
    let layout = Layout::new(BTreeSet::new(), &manifest, &every_file);
    let classifier = Classifier {
        signature_types: &found,
        ..bare(&layout)
    };
    for category in ["types", "nsTypes", "exports", "nsExports"] {
        assert_eq!(
            classifier.classify(Side::Tool, category, "src/ops.ts|Props"),
            Some(SIGNATURE_TYPE),
            "{category}"
        );
    }
    for (side, category, key) in [
        (Side::Tool, "types", "src/ops.ts|Hidden"),
        (Side::Tool, "enumMembers", "src/ops.ts|Props"),
        (Side::Skif, "types", "src/ops.ts|Props"),
    ] {
        assert_eq!(
            classifier.classify(side, category, key),
            None,
            "{side:?} {category} {key}"
        );
    }
}

#[test]
fn starlark_strings_and_linked_dependency_labels_read_as_skif_reads_them() {
    assert_eq!(
        starlark_strings("x = ['a', \"b\"] # \"comment\"\ny = \"\"\"c \"quoted\" d\"\"\" + 'e\\'f' + \"\""),
        ["a", "b", "c \"quoted\" d", "e'f", ""]
    );
    for (label, found) in [
        (":node_modules/esbuild", Some(("packages/app", "esbuild"))),
        ("node_modules/esbuild", Some(("packages/app", "esbuild"))),
        (":node_modules/@types/node", Some(("packages/app", "@types/node"))),
        (":node_modules/typescript/bin/tsc", Some(("packages/app", "typescript"))),
        ("//:node_modules/typescript", Some(("", "typescript"))),
        ("//packages/lib:node_modules/zod", Some(("packages/lib", "zod"))),
        ("@//packages/lib:node_modules/zod", Some(("packages/lib", "zod"))),
        ("@@//packages/lib:node_modules/zod", Some(("packages/lib", "zod"))),
        (
            "@npm//packages/app:esbuild/package_json.bzl",
            Some(("packages/app", "esbuild")),
        ),
        ("@npm//:@babel/cli/package_json.bzl", Some(("", "@babel/cli"))),
        ("@npm//packages/app:node_modules/esbuild", None),
        ("//packages/app:esbuild/package_json.bzl", None),
        ("@npm//:defs.bzl", None),
        ("@npm//:esbuild/dir/package_json.bzl", None),
        (":node_modules", None),
        (":node_modules/@scope", None),
        ("//packages/app/node_modules/esbuild", None),
        ("src/index.ts", None),
        ("@types/node", None),
    ] {
        let expected = found.map(|(package, name)| (package.to_owned(), name.to_owned()));
        assert_eq!(linked_dependency(label, "packages/app"), expected, "{label}");
    }
}

#[test]
fn dependencies_bazel_build_files_credit_and_unlisted_findings_at_them_split_off() {
    let rules_js = "bazel_dep(name = \"aspect_rules_js\", version = \"2.0.0\")\n";
    let root_files: BTreeMap<&str, &str> = [
        ("MODULE.bazel", rules_js),
        ("e2e/js/MODULE.bazel", rules_js),
        ("e2e/plain/MODULE.bazel", "module(name = \"plain\")\n"),
    ]
    .into_iter()
    .collect();
    let manifests = strings(&[
        "package.json",
        "packages/app/package.json",
        "e2e/js/package.json",
        "e2e/plain/package.json",
    ]);
    let is_file = |file: &str| root_files.contains_key(file) || manifests.contains(file);
    let read = |file: &str| root_files.get(file).map(|source| (*source).to_owned());
    let build_files: Vec<(String, String)> = [
        (
            "packages/app/BUILD.bazel",
            "load(\"@npm//packages/app:esbuild/package_json.bzl\", esbuild = \"bin\")\n\
                 # \":node_modules/commented\" is no label\n\
                 js_library(name = \"lib\", srcs = [\"src/index.ts\"], deps = [\":node_modules/zod\", \
                 \"//:node_modules/typescript\", '//packages/other:node_modules/left-pad'])\n",
        ),
        (
            "packages/app/src/BUILD",
            "deps = [\"//packages/app:node_modules/@types/node\"]\n",
        ),
        (
            "tools/BUILD.bazel",
            "deps = [\"//packages/app:node_modules/react\", \"//:node_modules/prettier\"]\n",
        ),
        ("e2e/js/BUILD.bazel", "deps = [\":node_modules/uvu\"]\n"),
        ("e2e/plain/BUILD.bazel", "deps = [\":node_modules/jasmine\"]\n"),
    ]
    .into_iter()
    .map(|(path, source)| (path.to_owned(), source.to_owned()))
    .collect();
    let credits = bazel_credits(&build_files, &is_file, &read);
    assert_eq!(
        credits,
        strings(&[
            "e2e/js/package.json|uvu",
            "package.json|prettier",
            "package.json|typescript",
            "packages/app/package.json|@types/node",
            "packages/app/package.json|esbuild",
            "packages/app/package.json|zod",
        ]),
        "a label of another package's directory, a package without a manifest, a comment and a Bazel root \
             without rules_js credit nothing"
    );

    let manifest = |dir: &str| {
        manifests
            .contains(&joined(dir, "package.json"))
            .then(|| Rc::new(json!({ "name": dir })))
    };
    let layout = Layout::new(strings(&["packages/app"]), &manifest, &is_file);
    let classifier = Classifier {
        bazel_credits: &credits,
        ..bare(&layout)
    };
    for (side, category, key) in [
        (Side::Tool, "devDependencies", "packages/app/package.json|zod"),
        (Side::Tool, "dependencies", "package.json|typescript"),
        (
            Side::Tool,
            "optionalPeerDependencies",
            "packages/app/package.json|esbuild",
        ),
        (Side::Skif, "unlisted", "packages/app/BUILD.bazel|lodash"),
        (Side::Skif, "unlisted", "packages/app/src/BUILD|lodash"),
    ] {
        assert_eq!(
            classifier.classify(side, category, key),
            Some(BAZEL_BUILD_FILE),
            "{side:?} {category} {key}"
        );
    }
    for (side, category, key) in [
        (Side::Tool, "dependencies", "packages/app/package.json|react"),
        (Side::Skif, "dependencies", "packages/app/package.json|zod"),
        (Side::Tool, "unlisted", "packages/app/BUILD.bazel|lodash"),
        (Side::Skif, "unlisted", "packages/app/src/index.ts|lodash"),
        (Side::Skif, "unlisted", "packages/app/BUILD.bazel.bak|lodash"),
    ] {
        assert_eq!(
            classifier.classify(side, category, key),
            None,
            "{side:?} {category} {key}"
        );
    }
}

#[test]
fn the_peers_of_a_dependency_a_build_file_credits_are_credited_too() {
    let manifests: BTreeMap<&str, Value> = [
        (
            "",
            json!({ "devDependencies": {
                    "@angular/build": "21", "@angular/localize": "21", "@angular/compiler-cli": "21",
                    "@types/foo": "1", "left-pad": "1"
                } }),
        ),
        (
            "node_modules/@angular/build",
            json!({
                "peerDependencies": { "@angular/localize": "21", "@angular/core": "21", "foo": "1" },
                "peerDependenciesMeta": { "@angular/localize": { "optional": true } }
            }),
        ),
        (
            "node_modules/@angular/localize",
            json!({ "peerDependencies": { "@angular/compiler-cli": "21" } }),
        ),
        ("node_modules/host-x", json!({ "peerDependencies": { "peer-y": "1" } })),
        (
            "modules",
            json!({ "dependencies": { "@angular/localize": "21", "@angular/compiler-cli": "21" } }),
        ),
        (
            "modules/node_modules/@angular/localize",
            json!({ "peerDependencies": { "@angular/compiler-cli": "21" } }),
        ),
        (
            "packages/b",
            json!({
                "dependencies": { "@angular/build": "21" },
                "peerDependencies": { "@angular/localize": "21" }
            }),
        ),
        (
            "packages/a",
            json!({ "dependencies": { "host-x": "1", "peer-y": "1" } }),
        ),
    ]
    .into_iter()
    .collect();
    let manifest = |dir: &str| manifests.get(dir).cloned().map(Rc::new);
    let credits = strings(&[
        "package.json|@angular/build",
        "modules/package.json|@angular/localize",
        "packages/b/package.json|@angular/build",
    ]);
    assert_eq!(
        with_peer_credits(credits, &manifest),
        strings(&[
            "modules/package.json|@angular/compiler-cli",
            "modules/package.json|@angular/localize",
            "package.json|@angular/build",
            "package.json|@angular/compiler-cli",
            "package.json|@angular/localize",
            "package.json|@types/foo",
            "packages/b/package.json|@angular/build",
            "packages/b/package.json|@angular/localize",
        ]),
        "an optional peer, the peer of a credited peer, an `@types/` twin of a peer and a peer installed above the \
             package are credited; a peer the manifest does not list and the peers of an uncredited host are not"
    );
}

#[test]
fn the_harness_reads_the_build_files_of_a_bazel_checkout_where_skif_discovery_looks() {
    let root = std::env::temp_dir().join(format!("skif-bench-parity-bazel-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    for dir in [
        "packages/a/src/coverage",
        "packages/a/coverage",
        "node_modules/x",
        ".cache",
        "docs/build",
        "coverage",
        "tools/coverage/bundle",
        "tools/out",
    ] {
        fs::create_dir_all(root.join(dir)).unwrap();
    }
    for file in [
        "BUILD.bazel",
        "packages/a/package.json",
        "packages/a/BUILD",
        "packages/a/src/BUILD.bazel",
        "packages/a/src/coverage/BUILD",
        "packages/a/coverage/BUILD.bazel",
        "node_modules/x/BUILD.bazel",
        ".cache/BUILD",
        "docs/build/notes.md",
        "coverage/BUILD.bazel",
        "tools/coverage/bundle/BUILD.bazel",
        "tools/out/BUILD",
    ] {
        fs::write(root.join(file), format!("# {file}\n")).unwrap();
    }
    let is_package = |dir: &str| root.join(dir).join("package.json").is_file();
    assert!(read_build_files(&root, &is_package).is_empty(), "no Bazel root file");
    fs::write(root.join("MODULE.bazel"), "bazel_dep(name = \"aspect_rules_js\")\n").unwrap();
    let found = read_build_files(&root, &is_package);
    assert_eq!(
        found.iter().map(|(path, _)| path.as_str()).collect::<Vec<_>>(),
        [
            "BUILD.bazel",
            "packages/a/BUILD",
            "packages/a/src/BUILD.bazel",
            "packages/a/src/coverage/BUILD",
            "tools/coverage/bundle/BUILD.bazel",
            "tools/out/BUILD",
        ],
        "`coverage`, `out` and `target` are skipped only directly in the root or a package directory"
    );
    assert_eq!(found[1].1, "# packages/a/BUILD\n");
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn knip_only_findings_in_the_files_of_a_rush_root_split_off() {
    let read: BTreeMap<String, Rc<Value>> = [
        ("apps/heft", json!({ "name": "@rushstack/heft" })),
        (
            "common/autoinstallers/lint",
            json!({ "name": "lint", "dependencies": { "eslint": "9" } }),
        ),
    ]
    .into_iter()
    .map(|(dir, manifest)| (dir.to_owned(), Rc::new(manifest)))
    .collect();
    let manifest = move |dir: &str| read.get(dir).cloned();
    let every_file = |_: &str| true;
    let layout = Layout::new(strings(&["apps/heft"]), &manifest, &every_file);
    let classifier = |rush_root: bool| Classifier {
        rush_root,
        ..bare(&layout)
    };
    let rush = classifier(true);
    for (category, key) in [
        ("files", ".prettierrc.js"),
        ("files", "common/scripts/install-run-rush.js"),
        ("files", "common/config/subspaces/default/.pnpmfile.cjs"),
        ("unlisted", "common/scripts/install-run.js|semver"),
        ("exports", "common/scripts/install-run.js|run"),
    ] {
        assert_eq!(
            rush.classify(Side::Tool, category, key),
            Some(RUSH_ROOT_FILE),
            "{category} {key}"
        );
    }
    for (side, category, key) in [
        (Side::Tool, "files", "apps/heft/src/orphan.ts"),
        (Side::Tool, "unlisted", "apps/heft/src/a.ts|semver"),
        (Side::Skif, "files", ".prettierrc.js"),
    ] {
        assert_eq!(rush.classify(side, category, key), None, "{side:?} {category} {key}");
    }
    assert_eq!(
        rush.classify(Side::Tool, "files", "common/autoinstallers/lint/index.js"),
        Some(NON_MEMBER_PROJECT),
        "a project of its own below the root is not a root file"
    );
    assert_eq!(
        classifier(false).classify(Side::Tool, "files", ".prettierrc.js"),
        None,
        "only a Rush root"
    );
}

#[test]
fn a_marker_only_manifest_does_not_own_the_findings_below_it() {
    let read: BTreeMap<String, Rc<Value>> = [
        ("", json!({ "name": "root" })),
        (
            "packages/a",
            json!({ "name": "a", "exports": { "./esm/*": { "default": "./dist/esm/*.js" } } }),
        ),
        ("packages/a/src/esm", json!({ "type": "module" })),
        ("packages/b", json!({ "name": "b" })),
        ("packages/b/src/esm", json!({ "type": "module" })),
        ("packages/c", json!({ "name": "c" })),
        ("packages/c/src/esm", json!({ "type": "module" })),
    ]
    .into_iter()
    .map(|(dir, manifest)| (dir.to_owned(), Rc::new(manifest)))
    .collect();
    let manifest = move |dir: &str| read.get(dir).cloned();
    let every_file = |_: &str| true;
    let layout = Layout::new(
        strings(&["packages/a", "packages/b", "packages/c"]),
        &manifest,
        &every_file,
    );
    let not_judged_dirs = BTreeMap::from([(
        "packages/a".to_owned(),
        NotJudgedScope::within(vec!["src/esm/*.stories.ts".to_owned()]),
    )]);
    let knip_failed = strings(&["packages/b"]);
    let classifier = Classifier {
        not_judged: &not_judged_dirs,
        knip_failed: &knip_failed,
        ..bare(&layout)
    };
    assert_eq!(
        classifier.classify(Side::Tool, "files", "packages/a/src/esm/button.stories.ts"),
        Some(NOT_JUDGED),
        "the narrowed scope is matched relative to the package, not to the marker's directory"
    );
    assert_eq!(
        classifier.classify(Side::Tool, "files", "packages/a/src/esm/index.ts"),
        Some(PUBLISHED_SOURCE),
        "the package publishes the source below the marker"
    );
    assert_eq!(
        classifier.classify(Side::Tool, "exports", "packages/b/src/esm/index.ts|unused"),
        Some(KNIP_CONFIG_ERROR),
        "the config error of the package covers the files below the marker"
    );
    assert_eq!(
        classifier.classify(Side::Tool, "files", "packages/a/src/other.ts"),
        None
    );
    assert_eq!(
        classifier.classify(Side::Tool, "files", "packages/c/src/esm/coverage/report.ts"),
        None,
        "a marker directory is no package root for `coverage`"
    );
}

#[test]
fn earlier_deviations_take_precedence_over_the_layout_ones() {
    let manifests = [
        ("", json!({ "name": "root" })),
        ("examples/app", json!({ "name": "app" })),
    ];
    let read: BTreeMap<String, Rc<Value>> = manifests
        .iter()
        .map(|(dir, manifest)| ((*dir).to_owned(), Rc::new(manifest.clone())))
        .collect();
    let manifest = move |dir: &str| read.get(dir).cloned();
    let every_file = |_: &str| true;
    let layout = Layout::new(BTreeSet::new(), &manifest, &every_file);
    let unread = ["examples/app/docs/".to_owned()];
    let not_judged_dirs = BTreeMap::from([("examples/app".to_owned(), NotJudgedScope::Package)]);
    let classifier = Classifier {
        knip_unread: &unread,
        not_judged: &not_judged_dirs,
        ..bare(&layout)
    };
    assert_eq!(
        classifier.classify(Side::Tool, "files", "examples/app/docs/a.ts"),
        Some(KNIP_UNREAD)
    );
    assert_eq!(
        classifier.classify(Side::Tool, "files", "examples/app/test/a.ts"),
        Some(TEST_FILE)
    );
    assert_eq!(
        classifier.classify(Side::Tool, "files", "examples/app/src/a.ts"),
        Some(NOT_JUDGED),
        "skif names the package it did not judge"
    );
    assert_eq!(
        classifier.classify(Side::Tool, "files", "examples/app/.cache/a.ts"),
        Some(NOT_JUDGED)
    );
}

#[test]
fn the_declaration_files_a_workspace_tsconfig_compiles_are_the_ones_knip_makes_entries() {
    let sources = BTreeMap::from([
        ("tsconfig.json", r#"{ "files": ["typings/listed.d.ts"] }"#),
        (
            "packages/types/tsconfig.json",
            r#"{ // inherits the selection
                    "extends": "./tsconfig.base.json", "exclude": ["legacy"] }"#,
        ),
        (
            "packages/types/tsconfig.base.json",
            r#"{ "include": ["."], "compilerOptions": { "outDir": "build" } }"#,
        ),
        (
            "packages/app/tsconfig.json",
            r#"{ "include": ["src", "../../shared/api.d.ts"] }"#,
        ),
        ("packages/plain/tsconfig.json", "{}"),
        ("tools/tsconfig.json", "{}"),
    ]);
    let manifest = manifests_of(&[
        ("", json!({ "name": "root", "workspaces": ["packages/*"] })),
        ("packages/types", json!({ "name": "@acme/types" })),
        ("packages/app", json!({ "name": "@acme/app" })),
        ("packages/bare", json!({ "name": "@acme/bare" })),
        ("packages/plain", json!({ "name": "@acme/plain" })),
    ]);
    let is_file = |file: &str| sources.contains_key(file);
    let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
    let layout = Layout::new(
        strings(&["packages/types", "packages/app", "packages/bare", "packages/plain"]),
        &manifest,
        &is_file,
    );
    let files: Vec<String> = [
        "typings/listed.d.ts",
        "typings/other.d.ts",
        "packages/types/Calendar.d.ts",
        "packages/types/nested/schedule.d.mts",
        "packages/types/utils.ts",
        "packages/types/legacy/old.d.ts",
        "packages/types/build/index.d.ts",
        "packages/types/.cache/cached.d.ts",
        "packages/app/src/env.d.ts",
        "packages/app/other.d.ts",
        "shared/api.d.ts",
        "packages/bare/bare.d.ts",
        "packages/plain/plain.d.ts",
        "tools/tool.d.ts",
    ]
    .iter()
    .map(|file| (*file).to_owned())
    .collect();
    assert_eq!(
        workspace_declarations(&files, &layout, &read),
        strings(&[
            "typings/listed.d.ts",
            "packages/types/Calendar.d.ts",
            "packages/types/nested/schedule.d.mts",
            "packages/app/src/env.d.ts",
            "shared/api.d.ts",
            "packages/plain/plain.d.ts",
        ]),
        "`files` alone selects no more; an inherited `include` stays relative to its configuration, less \
             `exclude`, the `outDir` and hidden directories; `include` may name files outside the workspace; no \
             `include` and no `files` select everything; a workspace without a `tsconfig.json` and a project that \
             is not a workspace select nothing"
    );
}

#[test]
fn declaration_files_no_code_file_imports_are_told_apart_from_imported_ones() {
    let sources = BTreeMap::from([
        (
            "packages/types/Calendar.d.ts",
            "export interface Calendar { id: string }\n",
        ),
        (
            "packages/types/AppGetServerSideProps.d.ts",
            "import type { Session } from 'next-auth';\nexport type Props = Session;\n",
        ),
        ("src/api/git.d.ts", "export interface API { state: string }\n"),
        ("src/typings/index.d.ts", "export type Typing = 1;\n"),
        ("src/lone.d.ts", "import 'missing';\nexport {};\n"),
        ("src/button/index.ts", "export const Button = 1;\n"),
        (
            "src/button/index.d.ts",
            "import { Base } from '../component';\nexport declare class Button extends Base {}\n",
        ),
        (
            "apps/web/page.ts",
            "import type { Calendar } from '@acme/types/Calendar';\nexport const page: Calendar = { id: '' };\n",
        ),
        (
            "src/main.ts",
            "import type { API } from './api/git.js';\nimport type { Typing } from './typings';\n\
                 import { Button } from './button';\nexport const main = Button;\n",
        ),
        ("src/mentions.ts", "export const lone = 'lone';\n"),
    ]);
    let manifest = manifests_of(&[
        ("", json!({ "name": "root", "workspaces": ["packages/*", "apps/*"] })),
        ("packages/types", json!({ "name": "@acme/types" })),
        ("apps/web", json!({ "name": "web" })),
    ]);
    let is_file = |file: &str| sources.contains_key(file);
    let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
    let layout = Layout::new(strings(&["packages/types", "apps/web"]), &manifest, &is_file);
    let resolver = Resolver::new(&layout, &read);
    let files: Vec<String> = sources.keys().map(|file| (*file).to_owned()).collect();
    let declarations = strings(&[
        "packages/types/Calendar.d.ts",
        "packages/types/AppGetServerSideProps.d.ts",
        "src/api/git.d.ts",
        "src/typings/index.d.ts",
        "src/lone.d.ts",
        "src/button/index.d.ts",
    ]);
    assert_eq!(
        unimported_declarations(&declarations, &files, &resolver, &read),
        strings(&[
            "packages/types/AppGetServerSideProps.d.ts",
            "src/lone.d.ts",
            "src/button/index.d.ts"
        ]),
        "a workspace package subpath, a built extension and a directory's `index` import a declaration file; a \
             file that only names one does not, and a source beside it takes the import"
    );
    assert!(unimported_declarations(&BTreeSet::new(), &files, &resolver, &read).is_empty());
}

#[test]
fn findings_that_knip_s_tsconfig_declaration_entries_explain_split_off() {
    let sources = BTreeMap::from([
        (
            "src/golden.d.ts",
            "import { SomeDirective } from './some.directive';\nexport declare class Golden {}\n",
        ),
        (
            "src/some.directive.ts",
            "import { base } from './base';\nexport class SomeDirective { b = base; }\n",
        ),
        ("src/base.ts", "export const base = 1;\n"),
        ("src/other.ts", "export const other = 1;\n"),
    ]);
    let is_file = |file: &str| sources.contains_key(file);
    let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
    let only: BTreeSet<&str> = ["src/some.directive.ts", "src/base.ts", "src/other.ts"]
        .into_iter()
        .collect();
    let reached = reach(vec!["src/golden.d.ts"], &only, &is_file, &read);
    assert_eq!(reached, strings(&["src/some.directive.ts", "src/base.ts"]));
    let manifest = manifests_of(&[
        ("", json!({ "name": "root", "workspaces": ["packages/*"] })),
        ("packages/types", json!({ "name": "@acme/types" })),
    ]);
    let layout = Layout::new(strings(&["packages/types"]), &manifest, &is_file);
    let declarations = strings(&[
        "packages/types/Calendar.d.ts",
        "packages/types/AppGetServerSideProps.d.ts",
        "src/golden.d.ts",
    ]);
    let unimported = strings(&["packages/types/AppGetServerSideProps.d.ts"]);
    let classifier = Classifier {
        tsconfig_declarations: &declarations,
        unimported_declarations: &unimported,
        declaration_reach: &reached,
        ..bare(&layout)
    };
    for (side, category, key, expected) in [
        (
            Side::Skif,
            "types",
            "packages/types/Calendar.d.ts|Calendar",
            Some(TSCONFIG_DECLARATION_FILE),
        ),
        (
            Side::Skif,
            "exports",
            "packages/types/Calendar.d.ts|ProviderResult",
            Some(TSCONFIG_DECLARATION_FILE),
        ),
        (
            Side::Skif,
            "enumMembers",
            "src/golden.d.ts|Auto",
            Some(TSCONFIG_DECLARATION_FILE),
        ),
        (
            Side::Skif,
            "files",
            "src/some.directive.ts",
            Some(TSCONFIG_DECLARATION_FILE),
        ),
        (Side::Skif, "files", "src/base.ts", Some(TSCONFIG_DECLARATION_FILE)),
        (
            Side::Tool,
            "unlisted",
            "packages/types/AppGetServerSideProps.d.ts|next-auth",
            Some(TSCONFIG_DECLARATION_FILE),
        ),
        (
            Side::Tool,
            "unresolved",
            "packages/types/AppGetServerSideProps.d.ts|./dist/index.js",
            Some(TSCONFIG_DECLARATION_FILE),
        ),
        (Side::Skif, "files", "src/other.ts", None),
        (Side::Skif, "types", "src/types.ts|Calendar", None),
        (Side::Skif, "duplicates", "packages/types/Calendar.d.ts|a|b", None),
        (
            Side::Skif,
            "unlisted",
            "packages/types/AppGetServerSideProps.d.ts|next-auth",
            None,
        ),
        (Side::Tool, "unlisted", "packages/types/Calendar.d.ts|next", None),
        (Side::Tool, "types", "packages/types/Calendar.d.ts|Calendar", None),
    ] {
        assert_eq!(
            classifier.classify(side, category, key),
            expected,
            "{side:?} {category} {key}"
        );
    }
}

#[test]
fn types_of_a_module_an_import_uses_as_a_whole_object_split_off_on_skif_side() {
    let sources = BTreeMap::from([
        (
            "app-store/crm.apps.generated.ts",
            "export const CrmServiceMap = {\n  bigin: import('./zoho-bigin/lib/CrmService'),\n  \
                 video: import('./barrel'),\n};\n",
        ),
        (
            "app-store/zoho-bigin/lib/CrmService.ts",
            "export type BiginToken = { token: string };\nexport interface BiginContact { id: string }\n\
                 export default class BiginCrmService {}\n",
        ),
        ("app-store/barrel.ts", "export * from './video/VideoApiAdapter';\n"),
        (
            "app-store/video/VideoApiAdapter.ts",
            "export type DailyEventResult = { id: string };\nexport const adapter = 1;\n",
        ),
        (
            "app-store/static.ts",
            "import { helper } from './helpers';\nexport const used = helper;\n",
        ),
        (
            "app-store/helpers.ts",
            "export type HelperOptions = { on: boolean };\nexport const helper = 1;\n",
        ),
        ("app-store/lonely.ts", "export type LonelyType = string;\n"),
    ]);
    let is_file = |file: &str| sources.contains_key(file);
    let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
    let layout = Layout::new(BTreeSet::new(), &no_manifest, &is_file);
    let resolver = Resolver::new(&layout, &read);
    let files: Vec<String> = sources.keys().map(|file| (*file).to_owned()).collect();
    let types = strings(&[
        "app-store/zoho-bigin/lib/CrmService.ts|BiginContact",
        "app-store/zoho-bigin/lib/CrmService.ts|BiginToken",
        "app-store/video/VideoApiAdapter.ts|DailyEventResult",
        "app-store/helpers.ts|HelperOptions",
        "app-store/lonely.ts|LonelyType",
    ]);
    let credits = module_object_credits(&files, &resolver, &read);
    let found = module_objects(&types, &credits);
    assert_eq!(
        found,
        strings(&[
            "app-store/zoho-bigin/lib/CrmService.ts|BiginContact",
            "app-store/zoho-bigin/lib/CrmService.ts|BiginToken",
            "app-store/video/VideoApiAdapter.ts|DailyEventResult",
        ]),
        "an `import()` uses its module as a whole, also through `export *`; a named import and no import do not"
    );
    assert!(module_objects(&BTreeSet::new(), &credits).is_empty());
    let classifier = Classifier {
        module_object_types: &found,
        ..bare(&layout)
    };
    for (side, category, key, expected) in [
        (
            Side::Skif,
            "types",
            "app-store/zoho-bigin/lib/CrmService.ts|BiginContact",
            Some(MODULE_OBJECT_TYPES),
        ),
        (
            Side::Skif,
            "nsTypes",
            "app-store/zoho-bigin/lib/CrmService.ts|BiginToken",
            Some(MODULE_OBJECT_TYPES),
        ),
        (
            Side::Skif,
            "exports",
            "app-store/zoho-bigin/lib/CrmService.ts|BiginContact",
            None,
        ),
        (
            Side::Tool,
            "types",
            "app-store/zoho-bigin/lib/CrmService.ts|BiginContact",
            None,
        ),
        (Side::Skif, "types", "app-store/helpers.ts|HelperOptions", None),
    ] {
        assert_eq!(
            classifier.classify(side, category, key),
            expected,
            "{side:?} {category} {key}"
        );
    }
}

/// The manifests among `sources`, by directory: what `<dir>/package.json` holds, parsed.
fn manifests_in(sources: &BTreeMap<&str, &str>) -> BTreeMap<String, Rc<Value>> {
    sources
        .iter()
        .filter_map(|(path, text)| {
            let dir = path.strip_suffix("package.json")?.trim_end_matches('/');
            Some((dir.to_owned(), Rc::new(serde_json::from_str(text).unwrap())))
        })
        .collect()
}

/// Asserts how `classifier` classifies each `(side, category, key)` of `cases`.
fn assert_classified(classifier: &Classifier<'_>, cases: &[(Side, &str, &str, Option<&str>)]) {
    for (side, category, key, expected) in cases {
        assert_eq!(
            classifier.classify(*side, category, key),
            *expected,
            "{side:?} {category} {key}"
        );
    }
}

#[test]
fn dependencies_heft_configs_and_rig_profiles_credit_split_off_on_knip_side() {
    let sources = BTreeMap::from([
        (
            "app/package.json",
            r#"{ "name": "app", "devDependencies": { "@rushstack/heft": "1", "@rushstack/heft-lint-plugin": "1", "local-rig": "1", "left-pad": "1" } }"#,
        ),
        (
            "app/config/heft.json",
            r#"{ "phasesByName": { "build": { "tasksByName": { "lint": { "taskPlugin": { "pluginPackage": "@rushstack/heft-lint-plugin" } } } } } }"#,
        ),
        ("app/config/rig.json", r#"{ "rigPackageName": "local-rig" }"#),
        (
            "rig/package.json",
            r#"{ "name": "local-rig", "dependencies": { "@rushstack/heft": "1", "@rushstack/heft-jest-plugin": "1", "jest-environment-node": "1", "unused-dep": "1" } }"#,
        ),
        (
            "rig/profiles/default/config/heft.json",
            r#"{ "phasesByName": { "test": { "tasksByName": { "jest": { "taskPlugin": { "pluginPackage": "@rushstack/heft-jest-plugin" } } } } } }"#,
        ),
        (
            "rig/node_modules/@rushstack/heft-jest-plugin/package.json",
            r#"{ "name": "@rushstack/heft-jest-plugin", "peerDependencies": { "jest-environment-node": "*" } }"#,
        ),
        (
            "plain/package.json",
            r#"{ "name": "plain", "dependencies": { "@rushstack/heft-lint-plugin": "1" } }"#,
        ),
        (
            "plain/config/heft.json",
            r#"{ "heftPlugins": [{ "pluginPackage": "@rushstack/heft-lint-plugin" }] }"#,
        ),
    ]);
    let mut memory = MemoryFs::new();
    for (path, source) in &sources {
        memory = memory.with_file(format!("/repo/{path}"), source);
    }
    let manifests = manifests_in(&sources);
    let manifest = |dir: &str| manifests.get(dir).cloned();
    let credits = heft_credits(
        ["app", "rig", "plain", "missing"],
        &memory,
        Path::new("/repo"),
        &manifest,
    );
    assert_eq!(
        credits,
        strings(&[
            "app/package.json|@rushstack/heft-lint-plugin",
            "app/package.json|local-rig",
            "rig/package.json|@rushstack/heft",
            "rig/package.json|@rushstack/heft-jest-plugin",
            "rig/package.json|jest-environment-node",
        ]),
        "a project's Heft configs, a rig's own profiles and the peers of what they load; a project that neither \
             depends on Heft nor has a `config/rig.json` enables no `heft` plugin"
    );
    for (path, expected) in [
        ("profiles/default/config/heft.json", true),
        ("node_modules/@acme/web-rig/profiles/app/tsconfig-base.json", true),
        ("node_modules/web-rig/profiles/app/tsconfig-base.json", true),
        ("config/heft.json", false),
        ("src/profiles/default/config.json", false),
        ("profiles/default", false),
        ("node_modules/@acme/web-rig/config/heft.json", false),
    ] {
        assert_eq!(in_rig_profile(path), expected, "{path}");
    }
    let is_file = |file: &str| sources.contains_key(file);
    let layout = Layout::new(strings(&["app", "rig", "plain"]), &manifest, &is_file);
    let classifier = Classifier {
        heft_credits: &credits,
        ..bare(&layout)
    };
    assert_classified(
        &classifier,
        &[
            (
                Side::Tool,
                "devDependencies",
                "app/package.json|@rushstack/heft-lint-plugin",
                Some(HEFT_CONFIG),
            ),
            (
                Side::Tool,
                "dependencies",
                "rig/package.json|jest-environment-node",
                Some(HEFT_CONFIG),
            ),
            (Side::Tool, "devDependencies", "app/package.json|left-pad", None),
            (Side::Tool, "dependencies", "rig/package.json|unused-dep", None),
            (
                Side::Tool,
                "dependencies",
                "plain/package.json|@rushstack/heft-lint-plugin",
                None,
            ),
            (
                Side::Skif,
                "devDependencies",
                "app/package.json|@rushstack/heft-lint-plugin",
                None,
            ),
        ],
    );
}

#[test]
fn findings_knip_misses_for_files_a_nested_dot_slash_ignore_hides_split_off_on_knip_side() {
    let sources = BTreeMap::from([
        ("packages/core/.gitignore", "./memory\n"),
        (
            "packages/core/src/processors/memory/cache.ts",
            "import { LRUCache } from 'lru-cache';\nimport { topic } from '../../agent/topic';\n\
                 console.log(LRUCache, topic);\n",
        ),
        (
            "packages/core/src/agent/topic.ts",
            "export const topic = 1;\nexport const other = 2;\n",
        ),
        (
            "packages/core/src/index.ts",
            "import { z } from 'zod';\nconsole.log(z);\n",
        ),
    ]);
    let is_file = |file: &str| sources.contains_key(file);
    let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
    let layout = Layout::new(BTreeSet::new(), &no_manifest, &is_file);
    let resolver = Resolver::new(&layout, &read);
    let ignores = dot_slash_ignores(&[("packages/core/.gitignore".to_owned(), "./memory\n".to_owned())]);
    let hidden: BTreeSet<String> = sources
        .keys()
        .filter(|file| in_dot_slash_ignore(&ignores, file))
        .map(|file| (*file).to_owned())
        .collect();
    assert_eq!(hidden, strings(&["packages/core/src/processors/memory/cache.ts"]));
    let workspace_of = |_: &str| "packages/core".to_owned();
    let imports = unused_file_imports(&hidden, &BTreeSet::new(), &read, &workspace_of);
    let credits = unused_file_credits(&hidden, &BTreeSet::new(), &resolver, &read);
    let classifier = Classifier {
        knip_dot_ignore_imports: &imports,
        knip_dot_ignore_credits: &credits,
        ..bare(&layout)
    };
    assert_classified(
        &classifier,
        &[
            (
                Side::Tool,
                "dependencies",
                "packages/core/package.json|lru-cache",
                Some(KNIP_DOT_IGNORE),
            ),
            (
                Side::Tool,
                "exports",
                "packages/core/src/agent/topic.ts|topic",
                Some(KNIP_DOT_IGNORE),
            ),
            (Side::Tool, "dependencies", "packages/core/package.json|zod", None),
            (Side::Tool, "exports", "packages/core/src/agent/topic.ts|other", None),
        ],
    );
    assert_ne!(
        classifier.classify(Side::Skif, "dependencies", "packages/core/package.json|lru-cache"),
        Some(KNIP_DOT_IGNORE),
        "skif's own findings are not set apart"
    );
}

#[test]
fn requires_a_typescript_as_asserts_a_type_of_are_read_from_the_text() {
    let text = "const a = require('./a') as typeof import('./a');\n\
                    const b = (require(\"./b\")\n  as typeof import('./b'));\n\
                    const c = require('./c');\n\
                    const d = myrequire('./d') as D;\n\
                    const e = require('./e') asserted;\n\
                    const f = require(name) as F;\n\
                    const g = require(\n  `./g`\n) as typeof import('./g');\n\
                    const h = require(`./${name}`) as H;\n";
    assert_eq!(type_asserted_requires(text), ["./a", "./b", "./g"]);
}

#[test]
fn exports_of_a_module_a_type_asserted_require_loads_split_off_on_knip_side() {
    let sources = BTreeMap::from([
        (
            "src/stream-ops.ts",
            "let m = require('./stream-ops.node') as typeof import('./stream-ops.node');\nexport const go = m.go;\n",
        ),
        (
            "src/stream-ops.node.ts",
            "export const go = 1;\nexport * from './shared';\n",
        ),
        ("src/shared.ts", "export const helper = 1;\n"),
        (
            "src/plain.ts",
            "const p = require('./other');\nconsole.log(p.lonely);\n",
        ),
        ("src/other.ts", "export const lonely = 1;\n"),
    ]);
    let is_file = |file: &str| sources.contains_key(file);
    let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
    let layout = Layout::new(BTreeSet::new(), &no_manifest, &is_file);
    let resolver = Resolver::new(&layout, &read);
    let files: Vec<String> = sources.keys().map(|file| (*file).to_owned()).collect();
    let credits = type_asserted_require_credits(&files, &resolver, &read);
    assert_eq!(credits, strings(&["src/shared.ts|*", "src/stream-ops.node.ts|*"]));
    let classifier = Classifier {
        type_asserted_requires: &credits,
        ..bare(&layout)
    };
    assert_classified(
        &classifier,
        &[
            (
                Side::Tool,
                "exports",
                "src/stream-ops.node.ts|go",
                Some(TYPE_ASSERTED_REQUIRE),
            ),
            (
                Side::Tool,
                "nsExports",
                "src/shared.ts|helper",
                Some(TYPE_ASSERTED_REQUIRE),
            ),
            (Side::Tool, "exports", "src/other.ts|lonely", None),
            (Side::Tool, "types", "src/stream-ops.node.ts|go", None),
            (Side::Skif, "exports", "src/stream-ops.node.ts|go", None),
        ],
    );
}

#[test]
fn files_a_template_literal_require_matches_and_what_they_reach_split_off_on_knip_side() {
    let sources = BTreeMap::from([
        (
            "lib/npm.js",
            "const load = (c) => require(`./commands/${c}`);\nmodule.exports = load;\n",
        ),
        (
            "lib/commands/access.js",
            "const base = require('../base-cmd.js');\nmodule.exports = base;\n",
        ),
        ("lib/commands/stage/index.js", "module.exports = 1;\n"),
        ("lib/commands/stage/list.js", "module.exports = 2;\n"),
        ("lib/base-cmd.js", "module.exports = 3;\n"),
        ("lib/meta.js", "const all = import.meta.glob(`./meta/*.js`);\n"),
        ("lib/meta/one.js", "module.exports = 4;\n"),
        ("lib/orphan.js", "module.exports = 5;\n"),
    ]);
    let is_file = |file: &str| sources.contains_key(file);
    let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
    let files: Vec<String> = sources.keys().map(|file| (*file).to_owned()).collect();
    let targets = template_import_targets(&files, &read);
    assert_eq!(
        targets,
        strings(&["lib/commands/access.js", "lib/commands/stage/index.js"]),
        "a substitution stands for one path segment, or a directory's `index`; an `import.meta.glob` is no \
             template literal call"
    );
    let knip_unused = strings(&[
        "lib/base-cmd.js",
        "lib/commands/access.js",
        "lib/commands/stage/index.js",
        "lib/commands/stage/list.js",
        "lib/meta/one.js",
        "lib/orphan.js",
    ]);
    let reached = reach_from_sources(&targets, (&knip_unused, &BTreeSet::new()), &is_file, &read);
    assert_eq!(
        reached,
        strings(&[
            "lib/base-cmd.js",
            "lib/commands/access.js",
            "lib/commands/stage/index.js"
        ])
    );
    let layout = Layout::new(BTreeSet::new(), &no_manifest, &is_file);
    let classifier = Classifier {
        template_import_reach: &reached,
        ..bare(&layout)
    };
    assert_classified(
        &classifier,
        &[
            (
                Side::Tool,
                "files",
                "lib/commands/access.js",
                Some(TEMPLATE_LITERAL_IMPORT),
            ),
            (Side::Tool, "files", "lib/base-cmd.js", Some(TEMPLATE_LITERAL_IMPORT)),
            (Side::Tool, "files", "lib/commands/stage/list.js", None),
            (Side::Tool, "files", "lib/orphan.js", None),
            (Side::Skif, "files", "lib/commands/access.js", None),
        ],
    );
}

#[test]
fn exports_a_default_import_of_a_commonjs_module_reads_split_off_on_knip_side() {
    let sources = BTreeMap::from([
        (
            "scripts/routing.cjs",
            "function a() {}\nfunction b() {}\nfunction c() {}\nmodule.exports = { a, b, c };\n",
        ),
        (
            "scripts/routing.test.ts",
            "import routing from './routing.cjs';\nconst { a } = routing;\nrouting.b();\n",
        ),
        ("scripts/esm.ts", "export default {};\nexport const d = 1;\n"),
        (
            "scripts/esm.test.ts",
            "import esm from './esm';\nconst { d } = esm;\nconsole.log(d);\n",
        ),
    ]);
    let is_file = |file: &str| sources.contains_key(file);
    let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
    let layout = Layout::new(BTreeSet::new(), &no_manifest, &is_file);
    let resolver = Resolver::new(&layout, &read);
    let files: Vec<String> = sources.keys().map(|file| (*file).to_owned()).collect();
    let exports = strings(&[
        "scripts/esm.ts|d",
        "scripts/routing.cjs|a",
        "scripts/routing.cjs|b",
        "scripts/routing.cjs|c",
    ]);
    let found = commonjs_default_members(&exports, &files, &resolver, &read);
    assert_eq!(
        found,
        strings(&["scripts/routing.cjs|a", "scripts/routing.cjs|b"]),
        "the properties read through the default import of `module.exports`; an ES module's default is no \
             module object"
    );
    let classifier = Classifier {
        commonjs_default_members: &found,
        ..bare(&layout)
    };
    assert_classified(
        &classifier,
        &[
            (
                Side::Tool,
                "exports",
                "scripts/routing.cjs|a",
                Some(COMMONJS_DEFAULT_IMPORT),
            ),
            (
                Side::Tool,
                "nsExports",
                "scripts/routing.cjs|b",
                Some(COMMONJS_DEFAULT_IMPORT),
            ),
            (Side::Tool, "exports", "scripts/routing.cjs|c", None),
            (Side::Tool, "exports", "scripts/esm.ts|d", None),
            (Side::Skif, "exports", "scripts/routing.cjs|a", None),
        ],
    );
}

#[test]
fn files_an_installed_main_imports_and_what_they_reach_split_off_on_knip_side() {
    let sources = BTreeMap::from([
        (
            "examples/rn/package.json",
            r#"{ "name": "rn", "main": "node_modules/expo/AppEntry.js" }"#,
        ),
        (
            "examples/rn/node_modules/expo/AppEntry.js",
            "import App from '../../App';\nregister(App);\n",
        ),
        (
            "examples/rn/App.tsx",
            "import { List } from './src/List';\nexport default function App() { return List; }\n",
        ),
        ("examples/rn/src/List.tsx", "export const List = 1;\n"),
        ("examples/rn/src/Orphan.tsx", "export const Orphan = 1;\n"),
        (
            "examples/web/package.json",
            r#"{ "name": "web", "main": "dist/index.js" }"#,
        ),
        ("examples/web/dist/index.js", "import '../src/page';\n"),
        ("examples/web/src/page.ts", "export const page = 1;\n"),
    ]);
    let manifests = manifests_in(&sources);
    let manifest = |dir: &str| manifests.get(dir).cloned();
    let is_file = |file: &str| sources.contains_key(file);
    let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
    let layout = Layout::new(strings(&["examples/rn", "examples/web"]), &manifest, &is_file);
    let imports = installed_main_imports(["examples/rn", "examples/web", "missing"], &layout, &read);
    assert_eq!(
        imports,
        strings(&["examples/rn/App.tsx"]),
        "a `main` outside `node_modules` is the project's own file"
    );
    let knip_unused = strings(&[
        "examples/rn/App.tsx",
        "examples/rn/src/List.tsx",
        "examples/rn/src/Orphan.tsx",
        "examples/web/src/page.ts",
    ]);
    let reached = reach_from_sources(&imports, (&knip_unused, &BTreeSet::new()), &is_file, &read);
    assert_eq!(reached, strings(&["examples/rn/App.tsx", "examples/rn/src/List.tsx"]));
    let classifier = Classifier {
        installed_main_reach: &reached,
        ..bare(&layout)
    };
    assert_classified(
        &classifier,
        &[
            (Side::Tool, "files", "examples/rn/src/List.tsx", Some(INSTALLED_MAIN)),
            (Side::Tool, "files", "examples/rn/src/Orphan.tsx", None),
            (Side::Tool, "files", "examples/web/src/page.ts", None),
            (Side::Skif, "files", "examples/rn/App.tsx", None),
        ],
    );
}

#[test]
fn value_exports_of_a_module_used_as_a_whole_object_split_off_on_knip_side() {
    let sources = BTreeMap::from([
        (
            "nodes/methods/listSearch.ts",
            "export async function searchA() {}\nexport async function searchB() {}\n",
        ),
        (
            "nodes/Node.ts",
            "import * as listSearch from './methods/listSearch';\nexport const methods = { listSearch };\n",
        ),
        (
            "nodes/helpers.ts",
            "export const used = 1;\nexport const unusedHelper = 2;\n",
        ),
        (
            "nodes/user.ts",
            "import * as helpers from './helpers';\nconsole.log(helpers.used);\n",
        ),
    ]);
    let is_file = |file: &str| sources.contains_key(file);
    let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
    let layout = Layout::new(BTreeSet::new(), &no_manifest, &is_file);
    let resolver = Resolver::new(&layout, &read);
    let files: Vec<String> = sources.keys().map(|file| (*file).to_owned()).collect();
    let credits = module_object_credits(&files, &resolver, &read);
    let values = module_objects(
        &strings(&["nodes/helpers.ts|unusedHelper", "nodes/methods/listSearch.ts|searchA"]),
        &credits,
    );
    assert_eq!(values, strings(&["nodes/methods/listSearch.ts|searchA"]));
    let classifier = Classifier {
        module_object_values: &values,
        ..bare(&layout)
    };
    assert_classified(
        &classifier,
        &[
            (
                Side::Tool,
                "nsExports",
                "nodes/methods/listSearch.ts|searchA",
                Some(MODULE_OBJECT_VALUES),
            ),
            (Side::Tool, "exports", "nodes/helpers.ts|unusedHelper", None),
            (Side::Tool, "types", "nodes/methods/listSearch.ts|searchA", None),
            (Side::Skif, "nsExports", "nodes/methods/listSearch.ts|searchA", None),
        ],
    );
}

#[test]
fn members_of_an_export_an_importer_refers_to_as_a_type_split_off_on_skif_side() {
    let sources = BTreeMap::from([
        (
            "common/charCode.ts",
            "export const enum CharCode { Hash = 35, Digit0 = 48 }\n",
        ),
        (
            "common/color.ts",
            "import { CharCode } from './charCode';\n\
                 export function digit(code: CharCode): number { return code === CharCode.Digit0 ? 0 : 1; }\n",
        ),
        (
            "common/result.ts",
            "export type CompareResult = number;\n\
                 export namespace CompareResult { export function isLess(r: CompareResult) { return r < 0; } }\n\
                 export enum Mode { A, B }\n",
        ),
        (
            "common/compare.ts",
            "import { CompareResult, Mode } from './result';\n\
                 export const g = (r: CompareResult) => Mode.A;\n",
        ),
        ("common/flags.ts", "export enum Flag { On, Off }\n"),
        (
            "common/use-flag.ts",
            "import { Flag as F } from './flags';\nexport const h = (f: F) => f;\n",
        ),
    ]);
    let is_file = |file: &str| sources.contains_key(file);
    let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
    let layout = Layout::new(BTreeSet::new(), &no_manifest, &is_file);
    let resolver = Resolver::new(&layout, &read);
    let files: Vec<String> = sources.keys().map(|file| (*file).to_owned()).collect();
    assert_eq!(
        type_reference_names(
            "common/use-flag.ts",
            "type T = typeof Q;\nlet x: Array<P> = [];\nlet y = z as R.S;\n"
        ),
        strings(&["Array", "P", "Q"])
    );
    let members = strings(&[
        "common/charCode.ts|Hash",
        "common/flags.ts|Off",
        "common/result.ts|B",
        "common/result.ts|isLess",
    ]);
    let found = type_referenced_members(&members, &files, &resolver, &read);
    assert_eq!(
        found,
        strings(&["common/charCode.ts|Hash", "common/result.ts|isLess"]),
        "an enum used only as a value and one imported under an alias stay compared"
    );
    let classifier = Classifier {
        type_referenced_members: &found,
        ..bare(&layout)
    };
    assert_classified(
        &classifier,
        &[
            (
                Side::Skif,
                "enumMembers",
                "common/charCode.ts|Hash",
                Some(TYPE_REFERENCED_MEMBERS),
            ),
            (
                Side::Skif,
                "namespaceMembers",
                "common/result.ts|isLess",
                Some(TYPE_REFERENCED_MEMBERS),
            ),
            (Side::Skif, "enumMembers", "common/result.ts|B", None),
            (Side::Skif, "enumMembers", "common/flags.ts|Off", None),
            (Side::Tool, "enumMembers", "common/charCode.ts|Hash", None),
        ],
    );
}

#[test]
fn members_a_file_only_knip_lists_unused_reads_split_off_on_knip_side() {
    let sources = BTreeMap::from([
        (
            "nodes/Brevo/GenericFunctions.ts",
            "export namespace BrevoNode {\n  export const INTERCEPTORS = new Map();\n  export const OTHER = 1;\n}\n",
        ),
        (
            "nodes/Brevo/AttributeDescription.ts",
            "import { BrevoNode } from './GenericFunctions';\nconsole.log(BrevoNode.INTERCEPTORS);\n",
        ),
    ]);
    let is_file = |file: &str| sources.contains_key(file);
    let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
    let layout = Layout::new(BTreeSet::new(), &no_manifest, &is_file);
    let resolver = Resolver::new(&layout, &read);
    let members = strings(&[
        "nodes/Brevo/GenericFunctions.ts|INTERCEPTORS",
        "nodes/Brevo/GenericFunctions.ts|OTHER",
    ]);
    let used = members_used_by(&members, ["nodes/Brevo/AttributeDescription.ts"], &resolver, &read);
    assert_eq!(used, strings(&["nodes/Brevo/GenericFunctions.ts|INTERCEPTORS"]));
    let classifier = Classifier {
        knip_unused_members: &used,
        ..bare(&layout)
    };
    assert_classified(
        &classifier,
        &[
            (
                Side::Tool,
                "namespaceMembers",
                "nodes/Brevo/GenericFunctions.ts|INTERCEPTORS",
                Some(IMPORTED_BY_KNIP_UNUSED_FILE),
            ),
            (
                Side::Tool,
                "namespaceMembers",
                "nodes/Brevo/GenericFunctions.ts|OTHER",
                None,
            ),
            (
                Side::Skif,
                "namespaceMembers",
                "nodes/Brevo/GenericFunctions.ts|INTERCEPTORS",
                None,
            ),
        ],
    );
}

#[test]
fn peers_of_a_package_a_file_only_knip_lists_unused_imports_are_used_in_that_file() {
    let sources = BTreeMap::from([
        (
            "app/package.json",
            r#"{ "name": "app", "dependencies": { "@react-navigation/stack": "1", "react-native-screens": "1", "left-pad": "1" } }"#,
        ),
        (
            "app/node_modules/@react-navigation/stack/package.json",
            r#"{ "name": "@react-navigation/stack", "peerDependencies": { "react-native-screens": "*" } }"#,
        ),
        (
            "app/App.tsx",
            "import { createStackNavigator } from '@react-navigation/stack';\nconsole.log(createStackNavigator);\n",
        ),
    ]);
    let manifests = manifests_in(&sources);
    let manifest = |dir: &str| manifests.get(dir).cloned();
    let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
    let workspace_of = |_: &str| "app".to_owned();
    let imports = with_peer_credits(
        unused_file_imports(&strings(&["app/App.tsx"]), &BTreeSet::new(), &read, &workspace_of),
        &manifest,
    );
    assert_eq!(
        imports,
        strings(&[
            "app/package.json|@react-navigation/stack",
            "app/package.json|react-native-screens",
        ])
    );
    let is_file = |file: &str| sources.contains_key(file);
    let layout = Layout::new(strings(&["app"]), &manifest, &is_file);
    let classifier = Classifier {
        knip_unused_imports: &imports,
        ..bare(&layout)
    };
    assert_classified(
        &classifier,
        &[
            (
                Side::Tool,
                "dependencies",
                "app/package.json|react-native-screens",
                Some(USED_IN_KNIP_UNUSED_FILE),
            ),
            (Side::Tool, "dependencies", "app/package.json|left-pad", None),
        ],
    );
}

#[test]
fn findings_in_paths_knip_takes_for_node_modules_split_off_on_skif_side() {
    for (path, expected) in [
        ("js/private/test/node_modules_in_sandbox.test.mjs", true),
        ("src/my_node_modules/index.js", true),
        ("app/node_modules/pkg/node_modules_x.js", false),
        ("src/modules.js", false),
    ] {
        assert_eq!(named_like_node_modules(path), expected, "{path}");
    }
    with_classifier(&[], &[], &[], |classifier| {
        assert_classified(
            classifier,
            &[
                (
                    Side::Skif,
                    "files",
                    "js/private/test/node_modules_in_sandbox.test.mjs",
                    Some(KNIP_NODE_MODULES_NAME),
                ),
                (
                    Side::Skif,
                    "exports",
                    "src/my_node_modules/index.js|x",
                    Some(KNIP_NODE_MODULES_NAME),
                ),
                (Side::Skif, "files", "src/modules.js", None),
                (Side::Skif, "files", "app/node_modules/pkg/node_modules_x.js", None),
            ],
        );
        assert_ne!(
            classifier.classify(Side::Tool, "files", "js/private/test/node_modules_in_sandbox.test.mjs"),
            Some(KNIP_NODE_MODULES_NAME),
            "knip's own findings are not set apart"
        );
    });
}

#[test]
fn files_an_import_meta_glob_reads_as_text_split_off_on_skif_side() {
    let sources = BTreeMap::from([
        (
            "src/app/views.ts",
            "const raw = import.meta.glob('../components/*.vue', { query: '?raw' });\n\
                 const modules = import.meta.glob('./*.vue');\nconsole.log(raw, modules);\n",
        ),
        ("src/components/Button.vue", "<template><div /></template>\n"),
        ("src/app/Local.vue", "<template><span /></template>\n"),
        ("src/other/Lonely.vue", "<template><p /></template>\n"),
    ]);
    let is_file = |file: &str| sources.contains_key(file);
    let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
    let files: Vec<String> = sources.keys().map(|file| (*file).to_owned()).collect();
    let targets = raw_glob_targets(&files, &read);
    assert_eq!(
        targets,
        strings(&["src/components/Button.vue"]),
        "a glob loading modules reads no text"
    );
    let skif_unused = strings(&["src/components/Button.vue", "src/other/Lonely.vue"]);
    let reached = reach_from_sources(&targets, (&skif_unused, &BTreeSet::new()), &is_file, &read);
    let layout = Layout::new(BTreeSet::new(), &no_manifest, &is_file);
    let classifier = Classifier {
        raw_glob_reach: &reached,
        ..bare(&layout)
    };
    assert_classified(
        &classifier,
        &[
            (Side::Skif, "files", "src/components/Button.vue", Some(RAW_GLOB_READ)),
            (Side::Skif, "files", "src/other/Lonely.vue", None),
            (Side::Tool, "files", "src/components/Button.vue", None),
        ],
    );
}

#[test]
fn javascript_files_named_beside_a_typescript_twin_split_off_on_skif_side() {
    let sources = BTreeMap::from([
        ("ex/sum.ts", "export default 1;\n"),
        ("ex/sum.js", "module.exports = 1;\n"),
        (
            "ex/__tests__/sum.test.js",
            "const a = require('../sum.ts');\nconst b = require('../sum.js');\nconst c = require('../lone.js');\n",
        ),
        ("ex/other.ts", "export default 2;\n"),
        ("ex/other.js", "module.exports = 2;\n"),
        ("ex/user.js", "const o = require('./other');\n"),
        ("ex/lone.js", "module.exports = 3;\n"),
    ]);
    let is_file = |file: &str| sources.contains_key(file);
    let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
    let files: Vec<String> = sources.keys().map(|file| (*file).to_owned()).collect();
    let unused: BTreeSet<&str> = ["ex/sum.js", "ex/other.js", "ex/lone.js"].into_iter().collect();
    let twins = typescript_twin_files(&unused, &files, &is_file, &read);
    assert_eq!(
        twins,
        strings(&["ex/sum.js"]),
        "a twin no specifier names with its extension, and a file without a twin, stay compared"
    );
    let layout = Layout::new(BTreeSet::new(), &no_manifest, &is_file);
    let classifier = Classifier {
        typescript_twins: &twins,
        ..bare(&layout)
    };
    assert_classified(
        &classifier,
        &[
            (Side::Skif, "files", "ex/sum.js", Some(TYPESCRIPT_TWIN)),
            (Side::Skif, "files", "ex/other.js", None),
            (Side::Tool, "files", "ex/sum.js", None),
        ],
    );
}

/// The [`KnipReadSets`] that [`knip_read_credits`] reads from the code files of `sources`, resolved by relative
/// path only.
fn knip_read_sets(sources: &BTreeMap<&str, &str>) -> KnipReadSets {
    let is_file = |file: &str| sources.contains_key(file);
    let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
    let layout = Layout::new(BTreeSet::new(), &no_manifest, &is_file);
    let resolver = Resolver::new(&layout, &read);
    let files: Vec<String> = sources.keys().map(|file| (*file).to_owned()).collect();
    knip_read_credits(&files, &resolver, &read)
}

#[test]
fn import_expressions_knip_binds_and_those_it_reads_as_opaque_are_told_apart() {
    let source = "import { vi } from 'vitest';\n\
                      import('./opaque');\n\
                      const { a } = await import('./destructured');\n\
                      const one = await import('./one');\n\
                      const notAwaited = import('./not-awaited');\n\
                      import('./then').then((m) => m.x);\n\
                      import('./then-object').then(({ x }) => x);\n\
                      const [p, { q }] = await Promise.all([import('./p'), import('./q')]);\n\
                      vi.mock(import('./mocked'), () => ({ x: 1 }));\n\
                      vi.mock(import('./spread'), () => ({ ...base }));\n\
                      lazy(() => import('./lazy'));\n\
                      import(name);\n";
    let module = parse_module(
        Path::new("src/load.ts"),
        source,
        &ParseOptions {
            in_file_references: false,
            member_accesses: true,
        },
    )
    .ok();
    let reads = knip_reads("src/load.ts", source, module.as_ref()).unwrap();
    assert_eq!(reads.opaque, ["./opaque", "./not-awaited", "./spread"]);
    assert_eq!(reads.defaults, ["./one", "./then", "./p"]);
}

#[test]
fn exports_of_a_module_an_opaque_or_bound_import_expression_loads_split_off_on_skif_side() {
    let sources = BTreeMap::from([
        (
            "src/load.ts",
            "import('./opaque');\nconst one = await import('./one');\n",
        ),
        (
            "src/opaque.ts",
            "export const a = 1;\nexport type T = 1;\nexport * from './deep';\n",
        ),
        ("src/deep.ts", "export const deep = 1;\n"),
        ("src/one.ts", "export default 1;\nexport const other = 2;\n"),
        ("src/lonely.ts", "export const lonely = 1;\n"),
    ]);
    let reads = knip_read_sets(&sources);
    assert_eq!(reads.opaque_imports, strings(&["src/deep.ts|*", "src/opaque.ts|*"]));
    assert_eq!(reads.import_defaults, strings(&["src/one.ts|default"]));
    let is_file = |file: &str| sources.contains_key(file);
    let layout = Layout::new(BTreeSet::new(), &no_manifest, &is_file);
    let classifier = Classifier {
        knip_reads: &reads,
        ..bare(&layout)
    };
    assert_classified(
        &classifier,
        &[
            (Side::Skif, "exports", "src/opaque.ts|a", Some(OPAQUE_DYNAMIC_IMPORT)),
            (Side::Skif, "types", "src/opaque.ts|T", Some(OPAQUE_DYNAMIC_IMPORT)),
            (Side::Skif, "exports", "src/deep.ts|deep", Some(OPAQUE_DYNAMIC_IMPORT)),
            (
                Side::Skif,
                "exports",
                "src/one.ts|default",
                Some(DYNAMIC_IMPORT_DEFAULT),
            ),
            (Side::Skif, "exports", "src/one.ts|other", None),
            (Side::Skif, "exports", "src/lonely.ts|lonely", None),
            (Side::Skif, "files", "src/opaque.ts", None),
            (Side::Tool, "exports", "src/opaque.ts|a", None),
        ],
    );
}

#[test]
fn exports_of_a_module_jest_require_actual_loads_split_off_on_skif_side() {
    let sources = BTreeMap::from([
        (
            "src/a.test.ts",
            "jest.mock('./utils', () => ({ ...jest.requireActual('./utils') }));\n",
        ),
        (
            "src/b.test.ts",
            "import { jest as j } from '@jest/globals';\nj.requireActual('./aliased');\n\
                 other.requireActual('./not-jest');\n",
        ),
        ("src/utils.ts", "export type Options = 1;\n"),
        ("src/aliased.ts", "export const aliased = 1;\n"),
        ("src/not-jest.ts", "export const notJest = 1;\n"),
    ]);
    let reads = knip_read_sets(&sources);
    assert_eq!(reads.required_actuals, strings(&["src/aliased.ts|*", "src/utils.ts|*"]));
    let is_file = |file: &str| sources.contains_key(file);
    let layout = Layout::new(BTreeSet::new(), &no_manifest, &is_file);
    let classifier = Classifier {
        knip_reads: &reads,
        ..bare(&layout)
    };
    assert_classified(
        &classifier,
        &[
            (Side::Skif, "types", "src/utils.ts|Options", Some(JEST_REQUIRE_ACTUAL)),
            (
                Side::Skif,
                "exports",
                "src/aliased.ts|aliased",
                Some(JEST_REQUIRE_ACTUAL),
            ),
            (Side::Skif, "exports", "src/not-jest.ts|notJest", None),
            (Side::Tool, "types", "src/utils.ts|Options", None),
        ],
    );
}

#[test]
fn exports_of_a_module_whose_namespace_imports_only_spy_calls_read_split_off_on_skif_side() {
    let sources = BTreeMap::from([
        (
            "src/a.test.ts",
            "import * as mail from './mail';\nimport * as util from './util';\nimport * as both from './both';\n\
                 vi.spyOn(mail, 'send');\nvi.spyOn(util, 'go');\nutil.go();\nspyOn(both, 'x');\n",
        ),
        ("src/b.ts", "import * as again from './both';\nconsole.log(again.y);\n"),
        ("src/mail.ts", "export function send() {}\nexport type Mail = 1;\n"),
        ("src/util.ts", "export function go() {}\n"),
        ("src/both.ts", "export const x = 1;\nexport const y = 2;\n"),
    ]);
    let reads = knip_read_sets(&sources);
    assert_eq!(
        reads.spied_namespaces,
        strings(&["src/mail.ts|*"]),
        "a namespace also read through a member, here or in another file, is read as knip reads one"
    );
    let is_file = |file: &str| sources.contains_key(file);
    let layout = Layout::new(BTreeSet::new(), &no_manifest, &is_file);
    let classifier = Classifier {
        knip_reads: &reads,
        ..bare(&layout)
    };
    assert_classified(
        &classifier,
        &[
            (Side::Skif, "exports", "src/mail.ts|send", Some(SPIED_NAMESPACE)),
            (Side::Skif, "types", "src/mail.ts|Mail", Some(SPIED_NAMESPACE)),
            (Side::Skif, "exports", "src/util.ts|go", None),
            (Side::Skif, "exports", "src/both.ts|x", None),
            (Side::Tool, "exports", "src/mail.ts|send", None),
        ],
    );
}

#[test]
fn exports_of_a_module_whose_namespace_name_an_array_or_object_keys_holds_split_off_on_skif_side() {
    let sources = BTreeMap::from([
        (
            "src/git.ts",
            "import * as path from './path';\nimport * as keys from './keys';\nimport * as plain from './plain';\n\
                 export function lsTree(path: string) { return ['ls-tree', path]; }\n\
                 export const names = Object.keys(keys);\nexport const a = plain.a;\n",
        ),
        (
            "src/path.ts",
            "export function format() {}\nexport function toNamespacedPath() {}\n",
        ),
        ("src/keys.ts", "export const k = 1;\n"),
        ("src/plain.ts", "export const a = 1;\nexport const b = 2;\n"),
    ]);
    let reads = knip_read_sets(&sources);
    assert_eq!(
        reads.enumerated_namespaces,
        strings(&["src/keys.ts|*", "src/path.ts|*"]),
        "a parameter that shadows the namespace still counts, as in knip"
    );
    let is_file = |file: &str| sources.contains_key(file);
    let layout = Layout::new(BTreeSet::new(), &no_manifest, &is_file);
    let classifier = Classifier {
        knip_reads: &reads,
        ..bare(&layout)
    };
    assert_classified(
        &classifier,
        &[
            (Side::Skif, "exports", "src/path.ts|format", Some(ENUMERATED_NAMESPACE)),
            (Side::Skif, "exports", "src/keys.ts|k", Some(ENUMERATED_NAMESPACE)),
            (Side::Skif, "exports", "src/plain.ts|b", None),
            (Side::Tool, "exports", "src/path.ts|format", None),
        ],
    );
}

#[test]
fn types_a_qualified_import_type_names_split_off_on_knip_side() {
    let sources = BTreeMap::from([
        (
            "src/main.ts",
            "type W = import('./window').Window;\ntype N = import('./ns').A.B;\n\
                 type Q = typeof import('./whole');\n",
        ),
        (
            "src/window.ts",
            "export interface Window {}\nexport interface Other {}\n",
        ),
        ("src/ns.ts", "export namespace A { export type B = 1; }\n"),
        ("src/whole.ts", "export const whole = 1;\n"),
    ]);
    let reads = knip_read_sets(&sources);
    assert_eq!(reads.import_types, strings(&["src/ns.ts|A", "src/window.ts|Window"]));
    assert!(
        reads.opaque_imports.is_empty(),
        "an import type is no import expression"
    );
    let is_file = |file: &str| sources.contains_key(file);
    let layout = Layout::new(BTreeSet::new(), &no_manifest, &is_file);
    let classifier = Classifier {
        knip_reads: &reads,
        ..bare(&layout)
    };
    assert_classified(
        &classifier,
        &[
            (Side::Tool, "types", "src/window.ts|Window", Some(IMPORT_TYPE_QUALIFIER)),
            (Side::Tool, "nsTypes", "src/ns.ts|A", Some(IMPORT_TYPE_QUALIFIER)),
            (Side::Tool, "types", "src/window.ts|Other", None),
            (Side::Tool, "exports", "src/whole.ts|whole", None),
            (Side::Skif, "types", "src/window.ts|Window", None),
        ],
    );
}

#[test]
fn namespace_re_exports_and_commonjs_exports_of_typescript_files_split_off_on_skif_side() {
    let sources = BTreeMap::from([
        (
            "src/index.ts",
            "export * as Config from './config';\nexport * as self from './index';\nexport const plain = 1;\n",
        ),
        ("src/join.ts", "exports.joinPath = join;\nmodule.exports.other = 1;\n"),
        ("src/obj.ts", "const a = 1;\nconst b = 2;\nmodule.exports = { a, b };\n"),
        ("src/mixed.ts", "const a = 1;\nmodule.exports = { a, b: 2 };\n"),
        ("lib/plain.js", "exports.js = 1;\n"),
    ]);
    let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
    let keys = strings(&[
        "src/index.ts|Config",
        "src/index.ts|self",
        "src/index.ts|plain",
        "src/join.ts|joinPath",
        "src/join.ts|other",
        "src/join.ts|missing",
        "src/obj.ts|a",
        "src/mixed.ts|a",
        "lib/plain.js|js",
    ]);
    let reads = KnipReadSets {
        namespace_reexports: namespace_reexport_keys(&keys, &read),
        typescript_commonjs: typescript_commonjs_keys(&keys, &read),
        ..KnipReadSets::default()
    };
    assert_eq!(
        reads.namespace_reexports,
        strings(&["src/index.ts|Config", "src/index.ts|self"])
    );
    assert_eq!(
        reads.typescript_commonjs,
        strings(&["src/join.ts|joinPath", "src/join.ts|other", "src/obj.ts|a"]),
        "a JavaScript file, and an object literal with a property that is not shorthand, are read as knip reads \
             them"
    );
    let is_file = |file: &str| sources.contains_key(file);
    let layout = Layout::new(BTreeSet::new(), &no_manifest, &is_file);
    let classifier = Classifier {
        knip_reads: &reads,
        ..bare(&layout)
    };
    assert_classified(
        &classifier,
        &[
            (Side::Skif, "nsExports", "src/index.ts|Config", Some(NAMESPACE_REEXPORT)),
            (Side::Skif, "exports", "src/index.ts|self", Some(NAMESPACE_REEXPORT)),
            (Side::Skif, "types", "src/index.ts|Config", None),
            (Side::Skif, "exports", "src/index.ts|plain", None),
            (
                Side::Skif,
                "exports",
                "src/join.ts|joinPath",
                Some(TYPESCRIPT_COMMONJS_EXPORT),
            ),
            (Side::Skif, "exports", "src/mixed.ts|a", None),
            (Side::Tool, "exports", "src/index.ts|Config", None),
        ],
    );
}

#[test]
fn files_knip_s_node_plugin_matches_are_read_as_its_patterns_read_them() {
    for (path, expected) in [
        ("test/helpers.ts", true),
        ("src/test/setup.js", true),
        ("a.test.ts", true),
        ("a-test.mjs", true),
        ("a_test.cts", true),
        ("src/test-run.ts", true),
        ("test.js", true),
        ("src/a.spec.ts", false),
        ("testing/a.ts", false),
        ("src/contest.ts", false),
        ("src/.test.ts", false),
        ("a.test.tsx", false),
        ("test/data.json", false),
    ] {
        assert_eq!(is_node_test_file(path), expected, "{path}");
    }
    for (script, expected) in [
        ("node --test", true),
        ("NODE_ENV=test node --import tsx --test test/*.ts", true),
        ("pnpm build && nub --test", true),
        ("tsc; node --test", true),
        ("node test.js", false),
        ("vitest --test", false),
        ("echo node --test", false),
    ] {
        assert_eq!(
            runs_node_test(&json!({ "scripts": { "test": script } })),
            expected,
            "{script}"
        );
    }
    assert!(!runs_node_test(&json!({ "name": "no-scripts" })));
}

#[test]
fn exports_of_files_knip_s_node_plugin_makes_entries_split_off_on_skif_side() {
    let keys = strings(&[
        "packages/a/test/helpers.ts|help",
        "packages/a/src/x.ts|x",
        "packages/b/test/helpers.ts|help",
    ]);
    let is_file = |_: &str| false;
    let own = manifests_of(&[
        ("", json!({ "name": "root", "workspaces": ["packages/*"] })),
        (
            "packages/a",
            json!({ "name": "a", "scripts": { "test": "node --test" } }),
        ),
        ("packages/b", json!({ "name": "b" })),
    ]);
    let layout = Layout::new(strings(&["packages/a", "packages/b"]), &own, &is_file);
    let files = node_test_files(&keys, &layout);
    assert_eq!(files, strings(&["packages/a/test/helpers.ts"]));
    let root = manifests_of(&[
        ("", json!({ "name": "root", "scripts": { "test": "node --test" } })),
        ("packages/a", json!({ "name": "a" })),
        ("packages/b", json!({ "name": "b" })),
    ]);
    let root_layout = Layout::new(strings(&["packages/a", "packages/b"]), &root, &is_file);
    assert_eq!(
        node_test_files(&keys, &root_layout),
        strings(&["packages/a/test/helpers.ts", "packages/b/test/helpers.ts"]),
        "a root script turns the plugin on in every workspace"
    );
    let reads = KnipReadSets {
        node_test_files: files,
        ..KnipReadSets::default()
    };
    let classifier = Classifier {
        knip_reads: &reads,
        ..bare(&layout)
    };
    assert_classified(
        &classifier,
        &[
            (
                Side::Skif,
                "exports",
                "packages/a/test/helpers.ts|help",
                Some(NODE_TEST_ENTRY),
            ),
            (
                Side::Skif,
                "types",
                "packages/a/test/helpers.ts|Help",
                Some(NODE_TEST_ENTRY),
            ),
            (Side::Skif, "exports", "packages/a/src/x.ts|x", None),
            (Side::Skif, "exports", "packages/b/test/helpers.ts|help", None),
            (Side::Tool, "exports", "packages/a/test/helpers.ts|help", None),
        ],
    );
}

#[test]
fn exports_a_tsconfig_declaration_entry_re_exports_split_off_on_skif_side() {
    let sources = BTreeMap::from([
        ("src/module.compiled.d.ts", "export * from './module';\n"),
        ("src/module.ts", "export type RouteModule = 1;\n"),
        ("src/other.ts", "export type Other = 1;\n"),
    ]);
    let is_file = |file: &str| sources.contains_key(file);
    let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
    let layout = Layout::new(BTreeSet::new(), &no_manifest, &is_file);
    let resolver = Resolver::new(&layout, &read);
    let declarations = strings(&["src/module.compiled.d.ts"]);
    let reads = KnipReadSets {
        declaration_reexports: entry_reexports(declarations.iter().cloned().collect(), &resolver, &read),
        ..KnipReadSets::default()
    };
    let classifier = Classifier {
        tsconfig_declarations: &declarations,
        knip_reads: &reads,
        ..bare(&layout)
    };
    assert_classified(
        &classifier,
        &[
            (
                Side::Skif,
                "types",
                "src/module.ts|RouteModule",
                Some(TSCONFIG_DECLARATION_FILE),
            ),
            (Side::Skif, "types", "src/other.ts|Other", None),
            (Side::Skif, "files", "src/module.ts", None),
            (Side::Tool, "types", "src/module.ts|RouteModule", None),
        ],
    );
}

#[test]
fn bazel_labels_name_the_package_skif_s_bazel_plugin_reads() {
    for (label, expected) in [
        ("//packages/core", Some("packages/core")),
        ("//packages/core:lib", Some("packages/core")),
        ("@//packages/core", Some("packages/core")),
        (":core", Some("here")),
        ("core", Some("here")),
        ("@npm//packages/core", None),
        ("", None),
    ] {
        assert_eq!(bazel_label_package(label, "here").as_deref(), expected, "{label}");
    }
    assert_eq!(within_package("packages/core", "packages/core"), Some(""));
    assert_eq!(within_package("packages/core/a", "packages/core"), Some("a"));
    assert_eq!(within_package("packages/core/a", ""), Some("packages/core/a"));
    assert_eq!(within_package("packages/corex", "packages/core"), None);
}

#[test]
fn exports_an_ng_package_entry_point_re_exports_split_off_on_knip_side() {
    let sources = BTreeMap::from([
        (
            "MODULE.bazel",
            "bazel_dep(name = \"aspect_rules_js\", version = \"2.0.0\")\n",
        ),
        ("packages/core/package.json", r#"{ "name": "@angular/core" }"#),
        ("packages/core/index.ts", "export * from './src/api';\n"),
        ("packages/core/src/api.ts", "export const api = 1;\n"),
        (
            "packages/core/primitives/signals/index.ts",
            "export const signal = 1;\n",
        ),
        ("packages/core/src/lonely.ts", "export const lonely = 1;\n"),
        ("packages/other/index.ts", "export const other = 1;\n"),
    ]);
    let build_files = vec![(
        "packages/core/BUILD.bazel".to_owned(),
        "ng_package(name = \"npm_package\", deps = [\":core\", \"//packages/core/primitives/signals\", \
             \"//packages/core/missing\", \"//packages/other:lib\", \"@npm//packages/core/src\"])\n"
            .to_owned(),
    )];
    let manifests = manifests_in(&sources);
    let manifest = |dir: &str| manifests.get(dir).cloned();
    let is_file = |file: &str| sources.contains_key(file);
    let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
    let layout = Layout::new(strings(&["packages/core"]), &manifest, &is_file);
    let is_project = |dir: &str| layout.is_project(dir);
    let entries = ng_package_entries(&build_files, &is_project, &is_file, &read);
    assert_eq!(
        entries,
        ["packages/core/index.ts", "packages/core/primitives/signals/index.ts"],
        "a label outside the project, of another repository or of a package without an entry file names none"
    );
    let resolver = Resolver::new(&layout, &read);
    let reads = KnipReadSets {
        ng_package_reexports: entry_reexports(entries, &resolver, &read),
        ..KnipReadSets::default()
    };
    let classifier = Classifier {
        knip_reads: &reads,
        ..bare(&layout)
    };
    assert_classified(
        &classifier,
        &[
            (
                Side::Tool,
                "exports",
                "packages/core/index.ts|api",
                Some(NG_PACKAGE_ENTRY),
            ),
            (
                Side::Tool,
                "exports",
                "packages/core/src/api.ts|api",
                Some(NG_PACKAGE_ENTRY),
            ),
            (
                Side::Tool,
                "nsExports",
                "packages/core/primitives/signals/index.ts|signal",
                Some(NG_PACKAGE_ENTRY),
            ),
            (Side::Tool, "exports", "packages/core/src/lonely.ts|lonely", None),
            (Side::Tool, "exports", "packages/other/index.ts|other", None),
            (Side::Tool, "files", "packages/core/src/lonely.ts", None),
            (Side::Skif, "exports", "packages/core/src/api.ts|api", None),
        ],
    );
}

#[test]
fn a_workspace_subpath_into_unbuilt_output_resolves_to_its_source() {
    let sources = BTreeMap::from([
        ("packages/next/src/server/web/types.ts", "export type Req = 1;\n"),
        ("packages/next/src/index.ts", "export const next = 1;\n"),
    ]);
    let manifest = manifests_of(&[(
        "packages/next",
        json!({ "name": "next", "exports": { ".": "./dist/index.js" } }),
    )]);
    let is_file = |file: &str| sources.contains_key(file);
    let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
    let layout = Layout::new(strings(&["packages/next"]), &manifest, &is_file);
    let resolver = Resolver::new(&layout, &read);
    for (specifier, expected) in [
        (
            "next/dist/server/web/types",
            Some("packages/next/src/server/web/types.ts"),
        ),
        ("next", Some("packages/next/src/index.ts")),
        ("next/dist/server/missing", None),
        ("other/dist/server/web/types", None),
    ] {
        assert_eq!(
            resolver.resolve("apps/web/page.ts", specifier).as_deref(),
            expected,
            "{specifier}"
        );
    }
}

#[test]
fn exports_a_published_declaration_file_re_exports_split_off_on_knip_side() {
    let sources = BTreeMap::from([
        (
            "packages/next/server.d.ts",
            "export * from 'next/dist/server/web/types';\nexport { local } from './src/local';\n",
        ),
        ("packages/next/src/server/web/types.ts", "export type Req = 1;\n"),
        (
            "packages/next/src/local.ts",
            "export const local = 1;\nexport const unshared = 2;\n",
        ),
        ("packages/next/src/lonely.ts", "export const lonely = 1;\n"),
    ]);
    let manifest = manifests_of(&[("packages/next", json!({ "name": "next", "files": ["server.d.ts"] }))]);
    let is_file = |file: &str| sources.contains_key(file);
    let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
    let layout = Layout::new(strings(&["packages/next"]), &manifest, &is_file);
    let resolver = Resolver::new(&layout, &read);
    let files: Vec<String> = sources.keys().map(|file| (*file).to_owned()).collect();
    let keys = strings(&[
        "packages/next/src/server/web/types.ts|Req",
        "packages/next/src/local.ts|local",
        "packages/next/src/lonely.ts|lonely",
    ]);
    let (_, published) = published_file_entries(&keys, &files, &layout, &read);
    assert_eq!(published, ["packages/next/server.d.ts"]);
    let credits = published_file_reexports(published, &resolver, &read);
    assert_eq!(
        credits,
        strings(&[
            "packages/next/src/local.ts|local",
            "packages/next/src/server/web/types.ts|*"
        ]),
        "the published file's own findings are `published-file`"
    );
    let classifier = Classifier {
        published_reexports: &credits,
        ..bare(&layout)
    };
    assert_classified(
        &classifier,
        &[
            (
                Side::Tool,
                "types",
                "packages/next/src/server/web/types.ts|Req",
                Some(PUBLISHED_SOURCE),
            ),
            (
                Side::Tool,
                "exports",
                "packages/next/src/local.ts|local",
                Some(PUBLISHED_SOURCE),
            ),
            (Side::Tool, "exports", "packages/next/src/local.ts|unshared", None),
            (Side::Tool, "exports", "packages/next/src/lonely.ts|lonely", None),
            (Side::Skif, "exports", "packages/next/src/local.ts|local", None),
        ],
    );
}

#[test]
fn value_exports_a_named_import_of_a_namespace_re_export_uses_as_a_whole_split_off_on_knip_side() {
    let sources = BTreeMap::from([
        (
            "nodes/methods/index.ts",
            "export * as listSearch from './listSearch';\nexport * as loadOptions from './loadOptions';\n",
        ),
        ("nodes/methods/listSearch.ts", "export async function searchA() {}\n"),
        ("nodes/methods/loadOptions.ts", "export async function loadA() {}\n"),
        (
            "nodes/Node.ts",
            "import { listSearch, loadOptions } from './methods';\nexport const methods = { listSearch };\n\
                 console.log(loadOptions.loadA);\n",
        ),
    ]);
    let is_file = |file: &str| sources.contains_key(file);
    let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
    let layout = Layout::new(BTreeSet::new(), &no_manifest, &is_file);
    let resolver = Resolver::new(&layout, &read);
    let files: Vec<String> = sources.keys().map(|file| (*file).to_owned()).collect();
    let credits = namespace_reexport_object_credits(&files, &resolver, &read);
    assert_eq!(
        credits,
        strings(&["nodes/methods/listSearch.ts|*"]),
        "a namespace re-export read through a member is no use as a whole"
    );
    let values = module_objects(
        &strings(&[
            "nodes/methods/listSearch.ts|searchA",
            "nodes/methods/loadOptions.ts|loadA",
        ]),
        &credits,
    );
    let classifier = Classifier {
        module_object_values: &values,
        ..bare(&layout)
    };
    assert_classified(
        &classifier,
        &[
            (
                Side::Tool,
                "exports",
                "nodes/methods/listSearch.ts|searchA",
                Some(MODULE_OBJECT_VALUES),
            ),
            (Side::Tool, "exports", "nodes/methods/loadOptions.ts|loadA", None),
        ],
    );
}

/// Asserts the deviation [`dependency_reads`] finds for each `(side, category, key)` of `cases`, each judged alone
/// as the one finding of its side, in a checkout of `sources` (the manifests among them, [`manifests_in`]) with these workspace members,
/// where git ignores the paths at or below those of `ignored`.
fn assert_dependency_reads(
    sources: &BTreeMap<&str, &str>,
    (members, ignored): (&[&str], &[&str]),
    cases: &[(Side, &str, &str, Option<&str>)],
) {
    let manifests = manifests_in(sources);
    let manifest = |dir: &str| manifests.get(dir).cloned();
    let is_file = |file: &str| sources.contains_key(file);
    let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
    let is_ignored = |path: &str| {
        ignored
            .iter()
            .any(|dir| path == *dir || path.starts_with(&format!("{dir}/")))
    };
    let layout = Layout::new(strings(members), &manifest, &is_file);
    let resolver = Resolver::new(&layout, &read);
    let files: Vec<String> = sources.keys().map(|file| (*file).to_owned()).collect();
    let build_files: Vec<(String, String)> = sources
        .iter()
        .filter(|(path, _)| is_build_file(path))
        .map(|(path, source)| ((*path).to_owned(), (*source).to_owned()))
        .collect();
    let outputs = bazel_outputs(&build_files, &is_file, &read);
    let checkout = DependencyCheckout {
        layout: &layout,
        resolver: &resolver,
        files: &files,
        read: &read,
        is_ignored: &is_ignored,
        bazel_outputs: &outputs,
    };
    for (side, category, key, expected) in cases {
        let finding = Keys::from([((*category).to_owned(), BTreeSet::from([(*key).to_owned()]))]);
        let reads = match side {
            Side::Skif => dependency_reads((&finding, &Keys::new()), &checkout),
            Side::Tool => dependency_reads((&Keys::new(), &finding), &checkout),
        };
        assert_eq!(
            reads.deviation(*side, category, key),
            *expected,
            "{side:?} {category} {key}"
        );
    }
}

#[test]
fn unlisted_dependencies_knip_reads_from_data_uris_augmentations_mocks_and_configs_split_off_on_knip_side() {
    let sources = BTreeMap::from([
        ("package.json", r#"{ "name": "root" }"#),
        (
            "src/data.mjs",
            "import a from 'data:text/javascript,export default 1';\nconsole.log(a);\n",
        ),
        ("src/plain.mjs", "import a from 'database';\nconsole.log(a);\n"),
        (
            "src/augment.ts",
            "import { x } from './x';\ndeclare module 'expect' {\n  interface Matchers<R> {\n    toBeFoo(): R;\n  }\n}\nexport const y = x;\n",
        ),
        ("src/x.ts", "export const x = 1;\n"),
        ("src/uses.ts", "import { expect } from 'expect';\nexpect(1);\n"),
        (
            "src/mock.test.ts",
            "import { vi } from 'vitest';\njest.mock('virtual-mod');\njest.requireActual('@scope/real/sub');\nvi.mock('vi-mocked');\n",
        ),
        (
            "docs/next.config.mjs",
            "import nextra from 'nextra';\nexport default nextra({})({ reactStrictMode: true });\n",
        ),
        (
            "web/next.config.mjs",
            "import mdx from '@next/mdx';\nexport default mdx()({});\n",
        ),
    ]);
    assert_dependency_reads(
        &sources,
        (&[], &[]),
        &[
            (Side::Tool, "unlisted", "src/data.mjs|data", Some(DATA_URI)),
            (Side::Tool, "unlisted", "src/plain.mjs|data", None),
            (Side::Skif, "unlisted", "src/data.mjs|data", None),
            (
                Side::Tool,
                "unlisted",
                "src/augment.ts|expect",
                Some(MODULE_AUGMENTATION),
            ),
            (Side::Tool, "unlisted", "src/uses.ts|expect", None),
            (
                Side::Tool,
                "unlisted",
                "src/mock.test.ts|virtual-mod",
                Some(JEST_MOCK_SPECIFIER),
            ),
            (
                Side::Tool,
                "unlisted",
                "src/mock.test.ts|@scope/real",
                Some(JEST_MOCK_SPECIFIER),
            ),
            (Side::Tool, "unlisted", "src/mock.test.ts|vi-mocked", None),
            (Side::Tool, "unlisted", "src/mock.test.ts|vitest", None),
            (
                Side::Tool,
                "unlisted",
                "docs/next.config.mjs|@vercel/turbopack-next",
                Some(NEXTRA_ALIAS),
            ),
            (
                Side::Tool,
                "unresolved",
                "docs/next.config.mjs|./content/*",
                Some(NEXTRA_ALIAS),
            ),
            (Side::Tool, "unlisted", "docs/next.config.mjs|nextra", None),
            (
                Side::Tool,
                "unlisted",
                "web/next.config.mjs|@vercel/turbopack-next",
                None,
            ),
            (Side::Tool, "unlisted", "app/app.json|expo-updates", Some(EXPO_UPDATES)),
            (
                Side::Tool,
                "unlisted",
                "app/app.config.ts|expo-updates",
                Some(EXPO_UPDATES),
            ),
            (Side::Tool, "unlisted", "app/app.json|expo-notifications", None),
            (Side::Tool, "unlisted", "app/other.json|expo-updates", None),
        ],
    );
}

#[test]
fn unresolved_imports_knip_resolves_otherwise_split_off_on_knip_side() {
    let sources = BTreeMap::from([
        (
            "MODULE.bazel",
            "bazel_dep(name = \"aspect_rules_js\", version = \"2.0.0\")\n",
        ),
        (
            "pkg/BUILD.bazel",
            "genrule(\n    name = \"sources\",\n    outs = [\n        \"dist/index.d.ts\",\n        \"LICENSE\",\n    ],\n    cmd = \"cp $< $@\",\n)\n",
        ),
        ("pkg/index.d.ts", "import './dist/index.js';\nimport './gone.js';\n"),
        ("db/index.ts", "export * from './types';\nexport * from './schema';\n"),
        ("rigs/web-rig/package.json", r#"{ "name": "web-rig" }"#),
        (
            "packages/style-config/package.json",
            r#"{ "name": "@acme/style-config", "dependencies": { "postcss-html": "1" } }"#,
        ),
        (
            "app/package.json",
            r#"{ "name": "app", "devDependencies": { "@acme/style-config": "1" } }"#,
        ),
        (
            "app/stylelint.config.mjs",
            "import { base } from '@acme/style-config/base';\nexport default { ...base, plugins: ['stylelint-order'] };\n",
        ),
        ("desktop/package.json", r#"{ "name": "desktop" }"#),
        ("desktop/script/copy.ts", "export {};\n"),
        ("desktop/script/other.ts", "export {};\n"),
        (
            "desktop/script/build.ts",
            "import { $ } from 'bun';\nawait $`bun ./script/copy.ts ${channel}`;\nawait import('./script/other.ts');\n",
        ),
        (
            "vitest.config.ts",
            "export default { test: { projects: await discover() } };\n",
        ),
        (
            "stores/pg/vitest.config.ts",
            "export default { test: { globalSetup: ['src/__tests__/global-setup.ts'] } };\n",
        ),
        ("stores/pg/src/__tests__/global-setup.ts", "export default () => {};\n"),
        ("tooling.config.ts", "export default {};\n"),
    ]);
    assert_dependency_reads(
        &sources,
        (&["packages/style-config", "app"], &["db/types.ts"]),
        &[
            (
                Side::Tool,
                "unresolved",
                "pkg/index.d.ts|./dist/index.js",
                Some(BAZEL_BUILD_FILE),
            ),
            (Side::Tool, "unresolved", "pkg/index.d.ts|./gone.js", None),
            (Side::Tool, "unresolved", "db/index.ts|./types", Some(GITIGNORED_IMPORT)),
            (Side::Tool, "unresolved", "db/index.ts|./schema", None),
            (
                Side::Tool,
                "unresolved",
                "rigs/web-rig/profiles/library/tsconfig-base.json|tslib",
                Some(HEFT_CONFIG),
            ),
            (
                Side::Tool,
                "unresolved",
                "rigs/web-rig/config/tsconfig.json|tslib",
                None,
            ),
            (
                Side::Tool,
                "unresolved",
                "rigs/web-rig/profiles/library/tsconfig-base.json|./local",
                None,
            ),
            (
                Side::Tool,
                "unresolved",
                "app/stylelint.config.mjs|postcss-html",
                Some(SHARED_CONFIG_REFERENCE),
            ),
            (Side::Tool, "unresolved", "app/stylelint.config.mjs|postcss-scss", None),
            (
                Side::Tool,
                "unresolved",
                "app/stylelint.config.mjs|stylelint-order",
                None,
            ),
            (
                Side::Tool,
                "unresolved",
                "desktop/script/build.ts|./script/copy.ts",
                Some(SHELL_TEMPLATE_DIRECTORY),
            ),
            (
                Side::Tool,
                "unresolved",
                "desktop/script/build.ts|./script/missing.ts",
                None,
            ),
            (
                Side::Tool,
                "unresolved",
                "desktop/script/build.ts|./script/other.ts",
                None,
            ),
            (
                Side::Tool,
                "unresolved",
                "vitest.config.ts|src/__tests__/global-setup.ts",
                Some(NESTED_VITEST_SETUP),
            ),
            (Side::Tool, "unresolved", "vitest.config.ts|src/missing-setup.ts", None),
            (
                Side::Tool,
                "unresolved",
                "tooling.config.ts|src/__tests__/global-setup.ts",
                None,
            ),
            (Side::Skif, "unresolved", "db/index.ts|./types", None),
        ],
    );
}

#[test]
fn unlisted_binaries_of_commands_knip_runs_elsewhere_split_off_on_knip_side() {
    let workflow = "name: ci\njobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      \
                        - run: pnpm -C packages/zone electrontest\n      \
                        - name: Copilot\n        working-directory: tools/copilot\n        run: pnpm exec tsx check.ts\n      \
                        - name: Members\n        working-directory: packages/app\n        run: pnpm exec vitest\n      \
                        - name: Plain\n        run: pnpm exec prettier --check .\n      \
                        - name: Computed\n        working-directory: ${{ steps.info.outputs.dir }}\n        run: pnpm napi create-npm-dirs\n  \
                        publish:\n    runs-on: ubuntu-latest\n    defaults:\n      run:\n        shell: bash\n        working-directory: tools/copilot\n    \
                        steps:\n      - run: pnpm update-templates\n";
    let sources = BTreeMap::from([
        (
            "package.json",
            r#"{ "name": "root", "scripts": { "serve": "cd build/rspack && pnpm exec rspack serve", "lint": "pnpm exec eslint ." } }"#,
        ),
        ("build/rspack/package.json", r#"{ "name": "rspack-build" }"#),
        ("tools/copilot/package.json", r#"{ "name": "copilot" }"#),
        ("docs/package.json", r#"{ "name": "docs" }"#),
        (
            "packages/zone/package.json",
            r#"{ "name": "zone", "scripts": { "electrontest": "node test.js" } }"#,
        ),
        (
            "packages/app/package.json",
            "{ \"name\": \"app\", \"scripts\": {\n  \"dev:api\": \"pnpm --dir ../zone electrontest\",\n  \
                 \"fmt\": \"cd ../../docs && pnpm oxfmt-mdx --write\",\n  \"watch-fix\": \"watch -n 1 'sed -i x'\"\n} }",
        ),
        ("packages/sdk/package.json", r#"{ "name": "sdk" }"#),
        (
            "packages/sdk/script/verify.ts",
            "import { $ } from 'bun';\nawait $`bun imports.mjs`.cwd(consumer);\nawait $`node other.mjs`;\n",
        ),
        (
            "scripts/build.mjs",
            "import { $ } from 'zx';\nawait $`cd ${config.rootDir} && pnpm generate-licenses`;\n",
        ),
        (".github/workflows/ci.yml", workflow),
    ]);
    let members = ["packages/zone", "packages/app", "packages/sdk", "docs"];
    let ci = ".github/workflows/ci.yml";
    let binary = |file: &str, name: &str| format!("{file}|{name}");
    let keys = [
        (binary(ci, "electrontest"), Some(COMMAND_DIRECTORY)),
        (binary(ci, "tsx"), Some(COMMAND_DIRECTORY)),
        (binary(ci, "vitest"), None),
        (binary(ci, "prettier"), None),
        (binary(ci, "napi"), Some(COMMAND_DIRECTORY)),
        (binary(ci, "update-templates"), Some(COMMAND_DIRECTORY)),
        (binary("package.json", "rspack"), Some(COMMAND_DIRECTORY)),
        (binary("package.json", "eslint"), None),
        (
            binary("packages/app/package.json", "electrontest"),
            Some(COMMAND_DIRECTORY),
        ),
        (
            binary("packages/app/package.json", "oxfmt-mdx"),
            Some(COMMAND_DIRECTORY),
        ),
        (binary("packages/app/package.json", "watch"), Some(PASSTHROUGH_COMMAND)),
        (binary("packages/app/package.json", "sed"), None),
        (
            binary("packages/sdk/script/verify.ts", "imports.mjs"),
            Some(BUN_FILE_ARGUMENT),
        ),
        (binary("packages/sdk/script/verify.ts", "other.mjs"), None),
        (
            binary("scripts/build.mjs", "generate-licenses"),
            Some(COMMAND_DIRECTORY),
        ),
    ];
    let cases: Vec<(Side, &str, &str, Option<&str>)> = keys
        .iter()
        .map(|(key, expected)| (Side::Tool, "binaries", key.as_str(), *expected))
        .chain([(Side::Skif, "binaries", "package.json|rspack", None)])
        .collect();
    assert_dependency_reads(&sources, (&members, &[]), &cases);
}

#[test]
fn commands_run_where_their_directory_options_and_workflow_keys_say() {
    assert_eq!(
        run_directory(&command_words("cd ../../docs && pnpm "), "packages/app"),
        RunDirectory::At("docs".to_owned())
    );
    assert_eq!(
        run_directory(&command_words("pnpm -C a build && pnpm "), ""),
        RunDirectory::Same,
        "a directory option holds for its own command only"
    );
    assert_eq!(
        run_directory(&command_words("git -C a log && "), ""),
        RunDirectory::Same,
        "only pnpm's -C names the directory"
    );
    assert_eq!(
        run_directory(&command_words("pnpm --dir=tools/x exec "), ""),
        RunDirectory::At("tools/x".to_owned())
    );
    for computed in ["$`cd ${root} && pnpm ", "cd /tmp && ", "cd ../.. && "] {
        assert_eq!(
            run_directory(&command_words(computed), "a"),
            RunDirectory::Computed,
            "{computed}"
        );
    }
    assert_eq!(output_stem("pkg/dist/index.d.ts"), "pkg/dist/index");
    assert_eq!(output_stem("pkg/dist/index.js"), "pkg/dist/index");
    assert_eq!(output_stem("pkg/LICENSE"), "pkg/LICENSE");
    assert_eq!(output_stem("pkg/.env"), "pkg/.env");
    let lines = [
        "jobs:",
        "  a:",
        "    steps:",
        "      - name: x",
        "        with:",
        "          working-directory: nested",
        "        run: |",
        "          pnpm x",
    ];
    assert_eq!(workflow_directory(&lines, 7), None, "a deeper key is not the step's");
}

#[test]
fn unlisted_dependencies_and_binaries_only_skif_reports_from_optional_peers_and_required_child_process_split_off() {
    let sources = BTreeMap::from([
        (
            "package.json",
            r#"{ "name": "root", "devDependencies": { "vitest": "4" } }"#,
        ),
        (
            "node_modules/vitest/package.json",
            r#"{ "name": "vitest", "peerDependencies": { "@vitest/coverage-v8": "*", "jsdom": "*", "happy-dom": "*" }, "peerDependenciesMeta": { "@vitest/coverage-v8": { "optional": true }, "jsdom": { "optional": true } } }"#,
        ),
        ("packages/utils/package.json", r#"{ "name": "utils" }"#),
        (
            "scripts/release.js",
            "const { execSync } = require('child_process');\nexecSync('sentry-cli releases new 1');\n",
        ),
        (
            "scripts/esm.mjs",
            "import { execSync } from 'node:child_process';\nconst cp = require('child_process');\nexecSync('sqlite3 db', cp);\n",
        ),
        ("scripts/plain.js", "run('x');\n"),
    ]);
    assert_dependency_reads(
        &sources,
        (&["packages/utils"], &[]),
        &[
            (
                Side::Skif,
                "unlisted",
                "packages/utils/vite.config.ts|@vitest/coverage-v8",
                Some(OPTIONAL_PEER_HOST),
            ),
            (
                Side::Skif,
                "unlisted",
                "packages/utils/a.test.ts|jsdom",
                Some(OPTIONAL_PEER_HOST),
            ),
            (Side::Skif, "unlisted", "packages/utils/a.test.ts|happy-dom", None),
            (Side::Skif, "unlisted", "packages/utils/a.test.ts|left-pad", None),
            (Side::Tool, "unlisted", "packages/utils/a.test.ts|jsdom", None),
            (
                Side::Skif,
                "binaries",
                "scripts/release.js|sentry-cli",
                Some(REQUIRED_CHILD_PROCESS),
            ),
            (Side::Skif, "binaries", "scripts/esm.mjs|sqlite3", None),
            (Side::Skif, "binaries", "scripts/plain.js|x", None),
            (Side::Skif, "binaries", "package.json|sentry-cli", None),
            (Side::Tool, "binaries", "scripts/release.js|sentry-cli", None),
        ],
    );
}

#[test]
fn unlisted_dependencies_below_nested_projects_of_members_or_listed_above_split_off_on_knip_side() {
    let manifests = [
        (
            "",
            json!({ "name": "root", "workspaces": ["packages/*", "extensions"] }),
        ),
        ("packages/runtime", json!({ "name": "runtime" })),
        (
            "packages/runtime/src/__tests__/test_root",
            json!({ "name": "test_root", "dependencies": {} }),
        ),
        ("extensions", json!({ "name": "extensions" })),
        (
            "extensions/copilot",
            json!({ "name": "copilot", "devDependencies": { "vitest": "1" } }),
        ),
        ("extensions/copilot/src/plugin", json!({ "name": "plugin" })),
        (
            "templates/agent",
            json!({ "name": "agent", "devDependencies": { "@playwright/test": "1" } }),
        ),
        ("templates/agent/client", json!({ "name": "client" })),
        ("examples/app", json!({ "name": "app" })),
    ];
    let read: BTreeMap<String, Rc<Value>> = manifests
        .iter()
        .map(|(dir, manifest)| ((*dir).to_owned(), Rc::new(manifest.clone())))
        .collect();
    let manifest = move |dir: &str| read.get(dir).cloned();
    let every_file = |_: &str| true;
    let layout = Layout::new(strings(&["packages/runtime", "extensions"]), &manifest, &every_file);
    let reads = DependencyReads {
        tool: BTreeMap::from([(
            "unlisted|packages/runtime/src/__tests__/test_root/data.mjs|data".to_owned(),
            DATA_URI,
        )]),
        skif: BTreeMap::from([(
            "binaries|scripts/release.js|sentry-cli".to_owned(),
            REQUIRED_CHILD_PROCESS,
        )]),
    };
    let classifier = Classifier {
        dependency_reads: &reads,
        ..bare(&layout)
    };
    assert_classified(
        &classifier,
        &[
            (
                Side::Tool,
                "unlisted",
                "packages/runtime/src/__tests__/test_root/root.js|RegularModule",
                Some(NON_MEMBER_PROJECT),
            ),
            (
                Side::Tool,
                "unlisted",
                "extensions/copilot/src/plugin/test/a.spec.ts|vitest",
                Some(NON_MEMBER_PROJECT),
            ),
            (
                Side::Tool,
                "unlisted",
                "templates/agent/client/test/e2e/a.spec.ts|@playwright/test",
                Some(NON_MEMBER_PROJECT),
            ),
            (
                Side::Tool,
                "unlisted",
                "templates/agent/client/test/e2e/a.spec.ts|zod",
                None,
            ),
            (Side::Tool, "unlisted", "examples/app/src/a.ts|zod", None),
            (Side::Tool, "unlisted", "packages/runtime/src/a.ts|zod", None),
            (
                Side::Tool,
                "unlisted",
                "packages/runtime/src/__tests__/test_root/data.mjs|data",
                Some(DATA_URI),
            ),
            (
                Side::Skif,
                "binaries",
                "scripts/release.js|sentry-cli",
                Some(REQUIRED_CHILD_PROCESS),
            ),
            (Side::Tool, "binaries", "scripts/release.js|sentry-cli", None),
        ],
    );
}

#[test]
fn tsconfig_paths_into_ignored_generated_code_are_the_importer_s_own_paths() {
    let sources = BTreeMap::from([
        (
            "tsconfig.json",
            r#"{ "compilerOptions": { "paths": { "electron": ["./.build/typings/electron.d.ts"], "@db/*": ["./db/*"] } } }"#,
        ),
        (
            "src/a.ts",
            "import { app } from 'electron';\nimport { Role } from '@db/enums';\nimport { real } from '@db/real';\n\
                 import { gone } from '@db/gone';\nimport z from 'zod';\nconsole.log(app, Role, real, gone, z);\n",
        ),
        ("db/real.ts", "export const real = 1;\n"),
    ]);
    let is_file = |file: &str| sources.contains_key(file);
    let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
    let layout = Layout::new(BTreeSet::new(), &no_manifest, &is_file);
    let resolver = Resolver::new(&layout, &read);
    let unlisted = strings(&[
        "src/a.ts|electron",
        "src/a.ts|@db/enums",
        "src/a.ts|@db/real",
        "src/a.ts|@db/gone",
        "src/a.ts|zod",
    ]);
    let ignored = |path: &str| path.starts_with(".build/") || path == "db/enums";
    assert_eq!(
        own_paths_keys(&unlisted, &resolver, &read, &ignored),
        strings(&["src/a.ts|@db/enums", "src/a.ts|@db/real", "src/a.ts|electron"])
    );
    assert_eq!(
        own_paths_keys(&unlisted, &resolver, &read, &|_: &str| false),
        strings(&["src/a.ts|@db/real"]),
        "a missing target git does not ignore is no generated code"
    );
}
