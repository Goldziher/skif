#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use skif_config::{Config, Selector};
use skif_core::PathSeg;
use skif_workspace::{DependencyKind, MemoryFs, PackageId, PackageManager, Workspace, WorkspaceError};

fn memory(files: &[(&str, &str)]) -> MemoryFs {
    let mut fs = MemoryFs::new();
    for (path, content) in files {
        fs.insert(Path::new("/ws").join(path), *content);
    }
    fs
}

fn discover(files: &[(&str, &str)], config: &Config) -> Result<Workspace, WorkspaceError> {
    Workspace::discover_with_fs(Path::new("/ws"), config, &memory(files))
}

fn dirs(workspace: &Workspace) -> Vec<String> {
    workspace
        .packages
        .iter()
        .map(|p| p.dir.to_str().unwrap().to_owned())
        .collect()
}

fn pkg(name: &str) -> String {
    format!(r#"{{"name":"{name}"}}"#)
}

fn root(workspaces: &str) -> String {
    format!(r#"{{"name":"root","workspaces":{workspaces}}}"#)
}

/// A tree used by the glob table.
fn tree() -> Vec<(&'static str, String)> {
    vec![
        ("packages/a/package.json", pkg("a")),
        ("packages/abc/package.json", pkg("abc")),
        ("packages/foo-bar/package.json", pkg("foo-bar")),
        ("packages/foo-baz/package.json", pkg("foo-baz")),
        ("packages/a/nested/package.json", pkg("nested")),
        ("packages/group/x/package.json", pkg("x")),
        ("packages/node_modules/dep/package.json", pkg("dep")),
        ("packages/.hidden/package.json", pkg("hidden")),
        ("apps/one/web/package.json", pkg("one-web")),
        ("apps/two/api/package.json", pkg("two-api")),
        ("pk/p/package.json", pkg("pk-p")),
        ("tool/package.json", pkg("tool")),
        ("scripts/run.sh", String::new()),
    ]
}

fn run(patterns: &[&str]) -> Workspace {
    let quoted: Vec<String> = patterns.iter().map(|p| format!("\"{p}\"")).collect();
    let manifest = root(&format!("[{}]", quoted.join(",")));
    let tree = tree();
    let mut files: Vec<(&str, &str)> = tree.iter().map(|(p, c)| (*p, c.as_str())).collect();
    files.push(("package.json", manifest.as_str()));
    discover(&files, &Config::default()).unwrap()
}

#[test]
fn glob_semantics() {
    let cases: &[(&[&str], &[&str])] = &[
        (&["*"], &["tool"]),
        (&["pk/*"], &["pk/p"]),
        (
            &["packages/*"],
            &["packages/a", "packages/abc", "packages/foo-bar", "packages/foo-baz"],
        ),
        (&["packages/foo-*"], &["packages/foo-bar", "packages/foo-baz"]),
        (&["packages/f*"], &["packages/foo-bar", "packages/foo-baz"]),
        (
            &["packages/**"],
            &[
                "packages/a",
                "packages/a/nested",
                "packages/abc",
                "packages/foo-bar",
                "packages/foo-baz",
                "packages/group/x",
            ],
        ),
        (
            &["packages/**/*"],
            &[
                "packages/a",
                "packages/a/nested",
                "packages/abc",
                "packages/foo-bar",
                "packages/foo-baz",
                "packages/group/x",
            ],
        ),
        (
            &["packages/a", "packages/**", "!packages/a"],
            &[
                "packages/a/nested",
                "packages/abc",
                "packages/foo-bar",
                "packages/foo-baz",
                "packages/group/x",
            ],
        ),
        (
            &["packages/*", "!packages/a"],
            &["packages/abc", "packages/foo-bar", "packages/foo-baz"],
        ),
        (&["!packages/*", "packages/*"], &[]),
        (
            &["packages/**", "!packages/**/nested"],
            &[
                "packages/a",
                "packages/abc",
                "packages/foo-bar",
                "packages/foo-baz",
                "packages/group/x",
            ],
        ),
        (&["apps/*/web"], &["apps/one/web"]),
        (&["apps/**/api"], &["apps/two/api"]),
        (
            &["packages/a", "packages/*"],
            &["packages/a", "packages/abc", "packages/foo-bar", "packages/foo-baz"],
        ),
    ];
    for (patterns, expected) in cases {
        assert_eq!(dirs(&run(patterns)), *expected, "patterns {patterns:?}");
    }
}

#[test]
fn bare_star_package_includes_nothing_without_manifest() {
    let workspace = run(&["*"]);
    assert_eq!(workspace.facts.dirs_without_manifest, [] as [std::path::PathBuf; 0]);
}

#[test]
fn partial_pattern_reported_once_not_per_directory() {
    let workspace = run(&["apps/*/web", "apps/*/missing", "nothing/*"]);
    let matched: Vec<(&str, usize)> = workspace
        .facts
        .patterns
        .iter()
        .map(|f| (f.pattern.as_str(), f.matched))
        .collect();
    assert_eq!(matched, [("apps/*/web", 1), ("apps/*/missing", 0), ("nothing/*", 0)]);
}

#[test]
fn negated_fact_counts_removed_directories() {
    let workspace = run(&["packages/*", "!packages/a"]);
    assert!(workspace.facts.patterns[1].negated);
    assert_eq!(workspace.facts.patterns[1].matched, 1);
}

#[test]
fn pattern_spans_in_package_json() {
    let workspace = run(&["packages/a", "tool"]);
    let fact = &workspace.facts.patterns[1];
    let span = fact.span.unwrap();
    let source = &workspace.root_package.manifest.source;
    assert_eq!(&source[span.start as usize..span.end as usize], "\"tool\"");
    assert_eq!(fact.file, PathBuf::from("package.json"));
}

#[test]
fn pnpm_yaml_wins_for_pnpm_and_catalogs_merge() {
    let yaml = "packages:\n  - \"pkgs/*\"  # comment\n  - '!pkgs/x'\ncatalog:\n  react: 18\ncatalogs:\n  old:\n    react: ^17.0.0\n";
    let manifest = r#"{"name":"root","packageManager":"pnpm@9.0.0","workspaces":["other/*"],"catalog":{"react":"19.0.0","vue":"3.0.0"}}"#;
    let workspace = discover(
        &[
            ("package.json", manifest),
            ("pnpm-workspace.yaml", yaml),
            ("pkgs/y/package.json", r#"{"name":"y"}"#),
            ("pkgs/x/package.json", r#"{"name":"x"}"#),
            ("other/z/package.json", r#"{"name":"z"}"#),
        ],
        &Config::default(),
    )
    .unwrap();
    assert_eq!(dirs(&workspace), ["pkgs/y"]);
    let first = workspace.facts.patterns[0].span.unwrap();
    let source = &workspace.pnpm_workspace.as_ref().unwrap().source;
    assert_eq!(&source[first.start as usize..first.end as usize], "\"pkgs/*\"");
    assert_eq!(workspace.catalogs.resolve("react", "catalog:"), Some("18"));
    assert_eq!(workspace.catalogs.resolve("react", "catalog:old"), Some("^17.0.0"));
    assert_eq!(workspace.catalogs.resolve("vue", "catalog:default"), Some("3.0.0"));
    assert_eq!(workspace.catalogs.source(), Some(Path::new("pnpm-workspace.yaml")));
}

#[test]
fn invalid_pnpm_workspace_is_typed() {
    let error = discover(
        &[("package.json", "{}"), ("pnpm-workspace.yaml", "packages: nope\n")],
        &Config::default(),
    )
    .unwrap_err();
    assert!(matches!(error, WorkspaceError::InvalidPnpmWorkspace { .. }));
}

#[test]
fn bun_catalogs_under_workspaces_object() {
    let manifest = r#"{"name":"r","workspaces":{"packages":["a"],"catalog":{"react":"^19"},"catalogs":{"legacy":{"react":"^17"}}}}"#;
    let workspace = discover(
        &[("package.json", manifest), ("a/package.json", "{}")],
        &Config::default(),
    )
    .unwrap();
    assert_eq!(workspace.catalogs.resolve("react", "catalog:"), Some("^19"));
    assert_eq!(workspace.catalogs.resolve("react", "catalog:legacy"), Some("^17"));
    assert_eq!(workspace.catalogs.default_catalog().len(), 1);
    assert_eq!(workspace.catalogs.named().len(), 1);
}

#[test]
fn catalog_resolution() {
    let manifest = r#"{"workspaces":["a"],"catalog":{"x":"1"},"catalogs":{"default":{"x":"2","y":"3"},"n":{"x":"4"}}}"#;
    let workspace = discover(
        &[("package.json", manifest), ("a/package.json", "{}")],
        &Config::default(),
    )
    .unwrap();
    let catalogs = &workspace.catalogs;
    assert_eq!(catalogs.resolve("x", "catalog:"), Some("1"));
    assert_eq!(catalogs.resolve("y", "catalog:default"), Some("3"));
    assert_eq!(catalogs.resolve("x", "catalog:n"), Some("4"));
    assert_eq!(catalogs.resolve("x", "catalog:missing"), None);
    assert_eq!(catalogs.resolve("z", "catalog:"), None);
    assert_eq!(catalogs.resolve("x", "^1.0.0"), None);
}

#[test]
fn package_manager_precedence() {
    let ws = |manifest: &str, extra: &[(&str, &str)], config: &Config| {
        let mut files = vec![("package.json", manifest), ("a/package.json", "{}")];
        files.extend_from_slice(extra);
        discover(&files, config).unwrap().package_manager
    };
    let config = Config::default();
    let field = r#"{"workspaces":["a"],"packageManager":"yarn@4.0.0"}"#;
    let dev = r#"{"workspaces":["a"],"devEngines":{"packageManager":[{"name":"x"},{"name":"bun"}]}}"#;
    let plain = r#"{"workspaces":["a"]}"#;

    assert_eq!(
        ws(field, &[("package-lock.json", "")], &config),
        Some(PackageManager::Yarn)
    );
    assert_eq!(ws(dev, &[("yarn.lock", "")], &config), Some(PackageManager::Bun));
    assert_eq!(ws(plain, &[], &config), None);
    assert_eq!(
        ws(plain, &[("package-lock.json", "")], &config),
        Some(PackageManager::Npm)
    );
    assert_eq!(
        ws(plain, &[("yarn.lock", ""), ("package-lock.json", "")], &config),
        Some(PackageManager::Yarn)
    );
    assert_eq!(
        ws(plain, &[("bun.lockb", ""), ("yarn.lock", "")], &config),
        Some(PackageManager::Bun)
    );
    assert_eq!(
        ws(plain, &[("bun.lock", ""), ("yarn.lock", "")], &config),
        Some(PackageManager::Bun)
    );
    assert_eq!(
        ws(plain, &[("pnpm-lock.yaml", ""), ("bun.lock", "")], &config),
        Some(PackageManager::Pnpm)
    );
    assert_eq!(
        ws(plain, &[("pnpm-workspace.yaml", "packages: []\n")], &config),
        Some(PackageManager::Pnpm)
    );

    let mut forced = Config::default();
    forced.workspace.package_manager = Some(PackageManager::Npm);
    assert_eq!(ws(field, &[("pnpm-lock.yaml", "")], &forced), Some(PackageManager::Npm));
}

#[test]
fn dependency_entry_spans() {
    let manifest = r#"{
  "name": "root",
  "workspaces": ["a"],
  "devDependencies": { "is-number": "^7.0.0", "left-pad": null },
  "dependencies": { "react": "18" }
}"#;
    let workspace = discover(
        &[("package.json", manifest), ("a/package.json", "{}")],
        &Config::default(),
    )
    .unwrap();
    let root = &workspace.root_package;
    let source = &root.manifest.source;
    let text = |span: skif_core::Span| source[span.start as usize..span.end as usize].to_owned();

    let entries: Vec<(DependencyKind, &str)> = root.dependencies.iter().map(|d| (d.kind, d.name.as_str())).collect();
    assert_eq!(
        entries,
        [
            (DependencyKind::DevDependencies, "is-number"),
            (DependencyKind::Dependencies, "react")
        ]
    );
    let first = &root.dependencies[0];
    assert_eq!(text(first.key_span), "\"is-number\"");
    assert_eq!(text(first.value_span), "\"^7.0.0\"");
    assert_eq!((first.key_span.start_line, first.key_span.start_col), (4, 24));
    assert_eq!(first.specifier, "^7.0.0");
    assert_eq!(root.dependency_map(DependencyKind::DevDependencies).unwrap().len(), 2);
    assert!(root.dependency_map(DependencyKind::PeerDependencies).is_none());
}

#[test]
fn field_span_addresses_entries_and_elements() {
    let workspace = discover(
        &[
            ("package.json", r#"{"workspaces":["a","b"],"name":"r"}"#),
            ("a/package.json", "{}"),
        ],
        &Config::default(),
    )
    .unwrap();
    let manifest = &workspace.root_package.manifest;
    let text = |pointer: &[PathSeg]| {
        let span = manifest.field_span(pointer).unwrap();
        manifest.source[span.start as usize..span.end as usize].to_owned()
    };
    assert_eq!(text(&[PathSeg::Key("name".into())]), "\"name\":\"r\"");
    assert_eq!(text(&[PathSeg::Key("workspaces".into()), PathSeg::Index(1)]), "\"b\"");
    assert!(manifest.field_span(&[PathSeg::Key("missing".into())]).is_none());
}

#[test]
fn crlf_and_bom_manifests() {
    let manifest = "\u{feff}{\r\n  \"name\": \"r\",\r\n  \"workspaces\": [\"a\"],\r\n  \"dependencies\": {\r\n    \"is-number\": \"1\"\r\n  }\r\n}\r\n";
    let workspace = discover(
        &[("package.json", manifest), ("a/package.json", "{}")],
        &Config::default(),
    )
    .unwrap();
    let root = &workspace.root_package;
    let entry = &root.dependencies[0];
    let source = &root.manifest.source;
    assert_eq!(
        &source[entry.key_span.start as usize..entry.key_span.end as usize],
        "\"is-number\""
    );
    assert_eq!(
        &source[entry.value_span.start as usize..entry.value_span.end as usize],
        "\"1\""
    );
    assert_eq!(entry.key_span.start_line, 5);
    assert_eq!(root.name.as_deref(), Some("r"));
    let pattern = workspace.facts.patterns[0].span.unwrap();
    assert_eq!(&source[pattern.start as usize..pattern.end as usize], "\"a\"");
}

#[test]
fn invalid_root_json_reports_line_and_column() {
    let error = discover(
        &[(
            "package.json",
            "{\n  \"name\": \"r\",\n  \"workspaces\": [\"a\"]\n  oops\n}",
        )],
        &Config::default(),
    )
    .unwrap_err();
    let WorkspaceError::InvalidManifest { path, line, column, .. } = error else {
        panic!("expected InvalidManifest, got {error:?}");
    };
    assert_eq!(path, PathBuf::from("package.json"));
    // jsonc-parser accepts the bare word `oops` as a property name, so the error is the missing `:` at `}`.
    assert_eq!((line, column), (5, 1));
}

#[test]
fn invalid_member_json_is_collected_not_fatal() {
    let workspace = discover(
        &[
            ("package.json", r#"{"workspaces":["a","b"]}"#),
            ("a/package.json", "{ nope"),
            ("b/package.json", r#"{"name":"b","dependencies":{"x":null,"y":"1"}}"#),
        ],
        &Config::default(),
    )
    .unwrap();
    assert_eq!(dirs(&workspace), ["b"]);
    assert_eq!(workspace.manifest_errors.len(), 1);
    assert!(
        matches!(&workspace.manifest_errors[0], WorkspaceError::InvalidManifest { path, .. } if path == Path::new("a/package.json"))
    );
    assert_eq!(workspace.packages[0].dependencies.len(), 1);
}

#[test]
fn package_matching_and_ignore() {
    let config: Config =
        Config::from_toml_str("[workspace]\nignore-packages = [\"@repo/legacy-*\", \"tools/old\", \"b*\"]\n").unwrap();
    let workspace = discover(
        &[
            ("package.json", r#"{"workspaces":["pkgs/*","tools/*"]}"#),
            ("pkgs/a/package.json", r#"{"name":"@repo/legacy-a"}"#),
            ("pkgs/c/package.json", r#"{"name":"@repo/c"}"#),
            ("tools/old/package.json", "{}"),
            ("tools/new/package.json", "{}"),
            ("pkgs/b/package.json", r#"{"name":"beta"}"#),
        ],
        &config,
    )
    .unwrap();
    let ignored: Vec<&str> = workspace
        .packages
        .iter()
        .filter(|p| workspace.is_ignored(p.id))
        .map(skif_workspace::Package::display_name)
        .collect();
    assert_eq!(ignored, ["@repo/legacy-a", "beta", "tools/old"]);
    assert_eq!(workspace.all_packages().count(), 6);
    assert!(workspace.all_packages().next().unwrap().is_root);
    assert!(workspace.package_by_name("@repo/c").is_some());
    assert!(workspace.packages[0].is_selected_by(&Selector::new(["pkgs/**"]).unwrap()));
    assert!(!workspace.packages[0].is_selected_by(&Selector::new(["tools/**"]).unwrap()));
}

#[test]
fn private_and_version() {
    let workspace = discover(
        &[
            ("package.json", r#"{"workspaces":["a"],"private":true}"#),
            ("a/package.json", r#"{"name":"a","version":"1.2.3"}"#),
        ],
        &Config::default(),
    )
    .unwrap();
    assert!(workspace.root_package.private);
    assert_eq!(workspace.root_package.display_name(), ".");
    assert_eq!(workspace.packages[0].version.as_deref(), Some("1.2.3"));
    assert!(!workspace.packages[0].private);
}

fn text_of(manifest: &skif_workspace::Manifest, pointer: &[PathSeg]) -> Option<String> {
    let span = manifest.field_span(pointer)?;
    Some(manifest.source[span.start as usize..span.end as usize].to_owned())
}

fn key(name: &str) -> PathSeg {
    PathSeg::Key(name.into())
}

#[test]
fn span_table_covers_sections_entries_and_the_whole_document() {
    let source = r#"{
  "name": "r",
  "workspaces": ["a"],
  "dependencies": { "react": "18", "react": "19" },
  "pnpm": { "overrides": { "x": "1" }, "deep": { "deeper": { "deepest": 1 } } }
}"#;
    let workspace = discover(
        &[("package.json", source), ("a/package.json", "{}")],
        &Config::default(),
    )
    .unwrap();
    let manifest = &workspace.root_package.manifest;
    let cases: Vec<(Vec<PathSeg>, Option<&str>)> = vec![
        (vec![], Some(source)),
        (vec![key("name")], Some("\"name\": \"r\"")),
        (
            vec![key("dependencies")],
            Some("\"dependencies\": { \"react\": \"18\", \"react\": \"19\" }"),
        ),
        (vec![key("dependencies"), key("react")], Some("\"react\": \"19\"")),
        (vec![key("workspaces"), PathSeg::Index(0)], Some("\"a\"")),
        (vec![key("pnpm"), key("overrides"), key("x")], Some("\"x\": \"1\"")),
        (vec![key("pnpm"), key("deep"), key("deeper"), key("deepest")], None),
        (vec![key("workspaces"), PathSeg::Index(1)], None),
    ];
    for (pointer, expected) in cases {
        assert_eq!(text_of(manifest, &pointer).as_deref(), expected, "{pointer:?}");
    }
}

#[test]
fn span_lookup_does_not_depend_on_reparsing_the_source() {
    let workspace = discover(
        &[("package.json", r#"{"workspaces":["a"],"name":"r"}"#), ("a/package.json", "{}")],
        &Config::default(),
    )
    .unwrap();
    let mut manifest = workspace.root_package.manifest.clone();
    manifest.source = std::sync::Arc::from("not json at all");
    assert!(manifest.field_span(&[key("name")]).is_some());
}

#[test]
fn dependency_entries_carry_their_parsed_specifier() {
    let workspace = discover(
        &[
            ("package.json", r#"{"workspaces":["a"]}"#),
            (
                "a/package.json",
                r#"{"name":"a","dependencies":{"b":"workspace:*","c":"^1.2.3","d":"file:../d"}}"#,
            ),
        ],
        &Config::default(),
    )
    .unwrap();
    let entries = &workspace.packages[0].dependencies;
    let parsed: Vec<(&str, bool)> = entries
        .iter()
        .map(|entry| (entry.parsed.get_raw(), entry.parsed.is_workspace_protocol()))
        .collect();
    assert_eq!(parsed, [("workspace:*", true), ("^1.2.3", false), ("file:../d", false)]);
    assert_eq!(entries[1].parsed.get_semver_number(), Some("1.2.3"));
    assert_eq!(entries[2].parsed.get_semver_number(), None);
}

#[test]
fn package_ids_are_dense_and_follow_directory_order_even_with_broken_members() {
    let mut files = vec![("package.json".to_owned(), r#"{"name":"root","workspaces":["p/*"]}"#.to_owned())];
    for index in (0..40).rev() {
        let body = if index % 7 == 3 {
            "{ broken".to_owned()
        } else {
            format!(r#"{{"name":"p{index:02}"}}"#)
        };
        files.push((format!("p/d{index:02}/package.json"), body));
    }
    files.push(("p/empty-dir/placeholder.txt".to_owned(), String::new()));
    let borrowed: Vec<(&str, &str)> = files.iter().map(|(p, c)| (p.as_str(), c.as_str())).collect();
    let larger: Vec<(String, String)> = (0..50)
        .map(|index| (format!("p/e{index:02}/package.json"), "{}".to_owned()))
        .chain([("package.json".to_owned(), r#"{"workspaces":["p/*"]}"#.to_owned())])
        .collect();
    let larger: Vec<(&str, &str)> = larger.iter().map(|(p, c)| (p.as_str(), c.as_str())).collect();
    let foreign = discover(&larger, &Config::default()).unwrap().package_ids().last().unwrap();
    for _ in 0..5 {
        let workspace = discover(&borrowed, &Config::default()).unwrap();
        let names: Vec<&str> = workspace.packages.iter().filter_map(|p| p.name.as_deref()).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted);
        assert_eq!(workspace.manifest_errors.len(), 6);
        assert_eq!(workspace.packages.len(), 34);
        assert_eq!(workspace.root_package.id, PackageId::ROOT);
        let ids: Vec<usize> = workspace.package_ids().map(PackageId::index).collect();
        assert_eq!(ids, (0..=34).collect::<Vec<_>>());
        for (id, package) in workspace.iter() {
            assert_eq!(package.id, id);
            assert_eq!(workspace.package(id).map(|p| p.dir.clone()), Some(package.dir.clone()));
        }
        assert!(workspace.package(foreign).is_none());
    }
}

#[test]
fn ignore_packages_is_resolved_per_id() {
    let config = Config::from_toml_str("[workspace]\nignore-packages = [\"@repo/*\", \"!@repo/keep\"]\n").unwrap();
    let workspace = discover(
        &[
            ("package.json", r#"{"name":"root","workspaces":["p/*"]}"#),
            ("p/a/package.json", r#"{"name":"@repo/a"}"#),
            ("p/keep/package.json", r#"{"name":"@repo/keep"}"#),
            ("p/x/package.json", r#"{"name":"x"}"#),
        ],
        &config,
    )
    .unwrap();
    let ignored: Vec<&str> = workspace
        .iter()
        .filter(|(id, _)| workspace.is_ignored(*id))
        .map(|(_, package)| package.display_name())
        .collect();
    assert_eq!(ignored, ["@repo/a"]);
    assert!(!workspace.is_ignored(PackageId::ROOT));
}
