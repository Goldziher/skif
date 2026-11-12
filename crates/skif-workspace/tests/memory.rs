#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use skif_config::{Config, Selector};
use skif_core::PathSeg;
use skif_workspace::{DependencyKind, MemoryFs, PackageId, PackageManager, Workspace, WorkspaceError, WorkspaceSource};

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
        .packages()
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
fn bun_keeps_a_directory_listed_by_its_literal_path_despite_a_negation() {
    let tree = [
        ("packages/a/package.json", pkg("a")),
        ("packages/literal/package.json", pkg("literal")),
        ("packages/globbed/package.json", pkg("globbed")),
    ];
    let run = |package_manager: &str| {
        let manifest = format!(
            r#"{{"name":"root","packageManager":"{package_manager}","workspaces":["packages/*","./packages/literal/","!packages/literal","!packages/glob*"]}}"#
        );
        let mut files: Vec<(&str, &str)> = tree.iter().map(|(p, c)| (*p, c.as_str())).collect();
        files.push(("package.json", manifest.as_str()));
        let workspace = discover(&files, &Config::default()).unwrap();
        let removed: Vec<usize> = workspace.facts.patterns[2..].iter().map(|fact| fact.matched).collect();
        (dirs(&workspace), removed)
    };
    assert_eq!(
        run("bun@1.4.4"),
        (vec!["packages/a".to_owned(), "packages/literal".to_owned()], vec![0, 1])
    );
    // npm lets a negation remove a directory whatever selected it.
    assert_eq!(run("npm@11.0.0"), (vec!["packages/a".to_owned()], vec![1, 1]));
}

#[test]
fn bun_selects_members_outside_the_root_without_loading_them() {
    let shared = pkg("shared");
    let lib = pkg("lib");
    let run = |package_manager: &str| {
        let manifest = format!(
            r#"{{"name":"root","packageManager":"{package_manager}","workspaces":["packages/*","../shared","../libs/*"]}}"#
        );
        let files = [
            ("package.json", manifest.as_str()),
            ("packages/a/package.json", r#"{"name":"a"}"#),
            ("../shared/package.json", shared.as_str()),
            ("../libs/lib/package.json", lib.as_str()),
            ("../libs/notes/readme.md", "x"),
        ];
        discover(&files, &Config::default()).unwrap()
    };
    let bun = run("bun@1.4.4");
    assert_eq!(dirs(&bun), ["packages/a"]);
    assert_eq!(
        bun.facts.outside_root_members,
        [PathBuf::from("../libs/lib"), PathBuf::from("../shared")]
    );
    let counts: Vec<(usize, bool)> = bun
        .facts
        .patterns
        .iter()
        .map(|fact| (fact.matched, fact.invalid))
        .collect();
    assert_eq!(counts, [(1, false), (1, false), (2, false)]);
    assert!(bun.manifest_errors.is_empty());
    assert!(bun.package_by_name("shared").is_none());
    // npm cannot use such a pattern.
    let npm = run("npm@11.0.0");
    assert_eq!(dirs(&npm), ["packages/a"]);
    assert_eq!(npm.facts.outside_root_members, [] as [std::path::PathBuf; 0]);
    let invalid: Vec<bool> = npm.facts.patterns.iter().map(|fact| fact.invalid).collect();
    assert_eq!(invalid, [false, true, true]);
    assert_eq!(npm.manifest_errors.len(), 2);
}

#[test]
fn bun_excludes_a_member_without_a_name_from_analysis() {
    let run = |package_manager: &str, ignore: &str| {
        let manifest = format!(
            r#"{{"name":"root","packageManager":"{package_manager}","workspaces":["packages/*","../outside","../named"]}}"#
        );
        let files = [
            ("package.json", manifest.as_str()),
            ("packages/a/package.json", r#"{"name":"a"}"#),
            ("packages/blank/package.json", r#"{"name":"","version":"1.0.0"}"#),
            ("packages/nameless/package.json", r#"{"version":"1.0.0"}"#),
            ("../outside/package.json", r#"{"version":"1.0.0"}"#),
            ("../named/package.json", r#"{"name":"named"}"#),
        ];
        let config = Config::from_toml_str(&format!("[workspace]\nignore-packages = [{ignore}]\n")).unwrap();
        let workspace = discover(&files, &config).unwrap();
        let ignored: Vec<bool> = workspace
            .package_ids()
            .skip(1)
            .map(|id| workspace.is_ignored(id))
            .collect();
        (workspace, ignored)
    };
    let (bun, ignored) = run("bun@1.4.4", "");
    assert_eq!(dirs(&bun), ["packages/a", "packages/blank", "packages/nameless"]);
    assert_eq!(ignored, [false, true, true]);
    assert_eq!(
        bun.facts.nameless_members,
        [PathBuf::from("packages/blank"), PathBuf::from("packages/nameless")]
    );
    assert_eq!(bun.facts.outside_root_members, [PathBuf::from("../named")]);
    // A member the configuration already ignores is not listed again.
    let (configured, _) = run("bun@1.4.4", r#""packages/nameless""#);
    assert_eq!(configured.facts.nameless_members, [PathBuf::from("packages/blank")]);
    // npm installs a member without a name.
    let (npm, ignored) = run("npm@11.0.0", "");
    assert_eq!(npm.facts.nameless_members, [] as [std::path::PathBuf; 0]);
    assert_eq!(ignored, [false, false, false]);
}

#[test]
fn pattern_spans_in_package_json() {
    let workspace = run(&["packages/a", "tool"]);
    let fact = &workspace.facts.patterns[1];
    let span = fact.span.unwrap();
    let source = workspace.root_package().manifest.source();
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
fn yarn_catalogs_come_from_yarnrc() {
    let manifest = r#"{"name":"r","packageManager":"yarn@4.10.0","workspaces":["a"]}"#;
    let yarnrc = "nodeLinker: node-modules\ncatalog:\n  react: ^18.3.1\ncatalogs:\n  legacy:\n    react: ^17.0.2\n";
    let workspace = discover(
        &[
            ("package.json", manifest),
            (".yarnrc.yml", yarnrc),
            ("a/package.json", "{}"),
        ],
        &Config::default(),
    )
    .unwrap();
    assert_eq!(workspace.catalogs.resolve("react", "catalog:"), Some("^18.3.1"));
    assert_eq!(workspace.catalogs.resolve("react", "catalog:legacy"), Some("^17.0.2"));
    assert_eq!(workspace.catalogs.source(), Some(Path::new(".yarnrc.yml")));
    assert_eq!(
        workspace.yarnrc.as_ref().map(|yarnrc| yarnrc.path.as_path()),
        Some(Path::new(".yarnrc.yml"))
    );
}

#[test]
fn a_yarnrc_without_catalogs_or_with_invalid_yaml_declares_none() {
    let manifest = r#"{"name":"r","packageManager":"yarn@4.10.0","workspaces":["a"]}"#;
    for yarnrc in ["nodeLinker: node-modules\n", "catalog: [unclosed\n"] {
        let workspace = discover(
            &[
                ("package.json", manifest),
                (".yarnrc.yml", yarnrc),
                ("a/package.json", "{}"),
            ],
            &Config::default(),
        )
        .unwrap();
        assert!(workspace.catalogs.is_empty(), "{yarnrc}");
        assert!(workspace.yarnrc.is_none(), "{yarnrc}");
        let invalid = yarnrc.starts_with("catalog: [");
        assert_eq!(
            workspace
                .manifest_errors
                .iter()
                .filter(|error| matches!(error, WorkspaceError::InvalidYarnrc { .. }))
                .count(),
            usize::from(invalid),
            "{yarnrc}"
        );
    }
}

#[test]
fn catalogs_come_only_from_the_files_of_the_active_package_manager() {
    let yaml = "packages:\n  - a\ncatalog:\n  react: ^18.0.0\n";
    let yarnrc = "catalog:\n  vue: ^3.0.0\n";
    let files = |manager: &'static str| {
        [
            ("package.json", manager),
            ("pnpm-workspace.yaml", yaml),
            (".yarnrc.yml", yarnrc),
            ("a/package.json", "{}"),
        ]
    };
    let yarn = discover(
        &files(r#"{"name":"r","packageManager":"yarn@4.10.0","workspaces":["a"]}"#),
        &Config::default(),
    )
    .unwrap();
    assert_eq!(yarn.catalogs.resolve("vue", "catalog:"), Some("^3.0.0"));
    assert_eq!(yarn.catalogs.resolve("react", "catalog:"), None);
    let pnpm = discover(
        &files(r#"{"name":"r","packageManager":"pnpm@9.0.0"}"#),
        &Config::default(),
    )
    .unwrap();
    assert_eq!(pnpm.catalogs.resolve("react", "catalog:"), Some("^18.0.0"));
    assert_eq!(pnpm.catalogs.resolve("vue", "catalog:"), None);
    assert!(pnpm.yarnrc.is_none());
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
    // `bunfig.toml` with an `[install]` table is the last resort: a lockfile, `pnpm-workspace.yaml` or the files Yarn
    // Berry leaves decide first.
    let bunfig = ("bunfig.toml", "[install]\nlinker = \"isolated\"\n");
    assert_eq!(ws(plain, &[bunfig], &config), Some(PackageManager::Bun));
    assert_eq!(
        ws(plain, &[("bunfig.toml", "install.linker = \"hoisted\"\n")], &config),
        Some(PackageManager::Bun)
    );
    assert_eq!(
        ws(plain, &[bunfig, ("pnpm-workspace.yaml", "packages: []\n")], &config),
        Some(PackageManager::Pnpm)
    );
    // A `bunfig.toml` for `bun test` or the runtime alone does not make an npm workspace a Bun one.
    for runtime_only in [
        "[test]\npreload = [\"./setup.ts\"]\n",
        "preload = [\"./setup.ts\"]\n",
        "",
        "[install\n",
    ] {
        assert_eq!(
            ws(plain, &[("bunfig.toml", runtime_only)], &config),
            None,
            "{runtime_only:?}"
        );
    }
    assert_eq!(
        ws(plain, &[bunfig, ("package-lock.json", "")], &config),
        Some(PackageManager::Npm)
    );
    assert_eq!(
        ws(plain, &[bunfig, (".yarnrc.yml", "")], &config),
        Some(PackageManager::Yarn)
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
    let root = workspace.root_package();
    let source = root.manifest.source();
    let text = |span: skif_core::Span| source[span.start as usize..span.end as usize].to_owned();

    let entries: Vec<(DependencyKind, &str)> = root.dependencies.iter().map(|d| (d.kind, &*d.name)).collect();
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
    assert_eq!(&*first.specifier, "^7.0.0");
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
    let manifest = &workspace.root_package().manifest;
    let text = |pointer: &[PathSeg]| {
        let span = manifest.field_span(pointer).unwrap();
        manifest.source()[span.start as usize..span.end as usize].to_owned()
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
    let root = workspace.root_package();
    let entry = &root.dependencies[0];
    let source = root.manifest.source();
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
    assert_eq!(workspace.packages()[0].dependencies.len(), 1);
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
        .packages()
        .iter()
        .filter(|p| workspace.is_ignored(p.id))
        .map(skif_workspace::Package::display_name)
        .collect();
    assert_eq!(ignored, ["@repo/legacy-a", "beta", "tools/old"]);
    assert_eq!(workspace.all_packages().count(), 6);
    assert!(workspace.all_packages().next().unwrap().is_root);
    assert!(workspace.package_by_name("@repo/c").is_some());
    assert!(workspace.packages()[0].is_selected_by(&Selector::new(["pkgs/**"]).unwrap()));
    assert!(!workspace.packages()[0].is_selected_by(&Selector::new(["tools/**"]).unwrap()));
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
    assert!(workspace.root_package().private);
    assert_eq!(workspace.root_package().display_name(), ".");
    assert_eq!(workspace.packages()[0].version.as_deref(), Some("1.2.3"));
    assert!(!workspace.packages()[0].private);
}

fn text_of(manifest: &skif_workspace::Manifest, pointer: &[PathSeg]) -> Option<String> {
    let span = manifest.field_span(pointer)?;
    Some(manifest.source()[span.start as usize..span.end as usize].to_owned())
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
    let manifest = &workspace.root_package().manifest;
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
fn span_lookups_agree_before_and_after_the_table_is_built_and_in_clones() {
    let workspace = discover(
        &[
            ("package.json", r#"{"workspaces":["a"],"name":"r"}"#),
            ("a/package.json", "{}"),
        ],
        &Config::default(),
    )
    .unwrap();
    let manifest = workspace.root_package().manifest.clone();
    let fresh = workspace.root_package().manifest.clone();
    let first = manifest.field_span(&[key("name")]);
    assert!(first.is_some());
    assert_eq!(manifest.field_span(&[key("name")]), first);
    assert_eq!(manifest.clone().field_span(&[key("name")]), first);
    assert_eq!(fresh.field_span(&[key("name")]), first);
    assert_eq!(manifest.field_span(&[key("missing")]), None);
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
    let entries = &workspace.packages()[0].dependencies;
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
    let mut files = vec![(
        "package.json".to_owned(),
        r#"{"name":"root","workspaces":["p/*"]}"#.to_owned(),
    )];
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
    let foreign = discover(&larger, &Config::default())
        .unwrap()
        .package_ids()
        .last()
        .unwrap();
    for _ in 0..5 {
        let workspace = discover(&borrowed, &Config::default()).unwrap();
        let names: Vec<&str> = workspace.packages().iter().filter_map(|p| p.name.as_deref()).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted);
        assert_eq!(workspace.manifest_errors.len(), 6);
        assert_eq!(workspace.packages().len(), 34);
        assert_eq!(workspace.root_package().id, PackageId::ROOT);
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

#[test]
fn field_key_and_value_spans_are_separate() {
    let workspace = discover(
        &[
            ("package.json", r#"{"workspaces":["a"],"name":"r","scripts":{"b":"x"}}"#),
            ("a/package.json", "{}"),
        ],
        &Config::default(),
    )
    .unwrap();
    let manifest = &workspace.root_package().manifest;
    let text = |span: skif_core::Span| manifest.source()[span.start as usize..span.end as usize].to_owned();
    let name = [PathSeg::Key("name".into())];
    assert_eq!(text(manifest.field_key_span(&name).unwrap()), "\"name\"");
    assert_eq!(text(manifest.field_value_span(&name).unwrap()), "\"r\"");
    assert_eq!(text(manifest.field_span(&name).unwrap()), "\"name\":\"r\"");
    let spans = manifest.field_spans(&name).unwrap();
    assert_eq!(Some(spans.entry), manifest.field_span(&name));
    let nested = [PathSeg::Key("scripts".into()), PathSeg::Key("b".into())];
    assert_eq!(text(manifest.field_key_span(&nested).unwrap()), "\"b\"");
    assert_eq!(text(manifest.field_value_span(&nested).unwrap()), "\"x\"");
    let element = [PathSeg::Key("workspaces".into()), PathSeg::Index(0)];
    assert!(manifest.field_key_span(&element).is_none());
    assert_eq!(text(manifest.field_value_span(&element).unwrap()), "\"a\"");
    assert!(manifest.field_key_span(&[]).is_none());
    assert!(manifest.field_key_span(&[PathSeg::Key("missing".into())]).is_none());
}

#[test]
fn pattern_facts_record_their_declared_index() {
    let json = discover(
        &[
            ("package.json", r#"{"name":"r","workspaces":["a",1,"b"]}"#),
            ("a/package.json", "{}"),
        ],
        &Config::default(),
    )
    .unwrap();
    let indices: Vec<(&str, usize)> = json
        .facts
        .patterns
        .iter()
        .map(|fact| (fact.pattern.as_str(), fact.index))
        .collect();
    assert_eq!(indices, [("a", 0), ("b", 2)]);

    let yaml = discover(
        &[
            ("package.json", r#"{"name":"r","packageManager":"pnpm@9.0.0"}"#),
            ("pnpm-workspace.yaml", "packages:\n  - 'a'\n  - 'b'\n  - '!c'\n"),
            ("a/package.json", "{}"),
        ],
        &Config::default(),
    )
    .unwrap();
    let indices: Vec<usize> = yaml.facts.patterns.iter().map(|fact| fact.index).collect();
    assert_eq!(indices, [0, 1, 2]);
}

fn pnpm_patterns(yaml: &str) -> Vec<(String, String)> {
    let workspace = discover(
        &[
            ("package.json", r#"{"name":"r","packageManager":"pnpm@9.0.0"}"#),
            ("pnpm-workspace.yaml", yaml),
            ("a/package.json", "{}"),
        ],
        &Config::default(),
    )
    .unwrap();
    let source = workspace.pnpm_workspace.as_ref().unwrap().source.clone();
    workspace
        .facts
        .patterns
        .iter()
        .map(|fact| {
            let span = fact.span.unwrap_or_else(|| panic!("no span for {}", fact.pattern));
            (
                fact.pattern.clone(),
                source[span.start as usize..span.end as usize].to_owned(),
            )
        })
        .collect()
}

#[test]
fn discover_with_fs_never_consults_the_current_directory() {
    let mut fs = MemoryFs::new();
    fs.insert("ws/package.json", r#"{"name":"r","workspaces":["a"]}"#);
    fs.insert("ws/a/package.json", r#"{"name":"a"}"#);
    let workspace = Workspace::discover_with_fs(Path::new("ws"), &Config::default(), &fs).unwrap();
    assert_eq!(workspace.root, Path::new("ws"));
    assert_eq!(dirs(&workspace), ["a"]);
}

#[test]
fn negations_ending_in_double_star_exclude_the_directory_itself() {
    let tree = [
        ("packages/a/package.json", pkg("a")),
        ("packages/a/test/package.json", pkg("a-test")),
        ("packages/test/package.json", pkg("test")),
        ("packages/x/package.json", pkg("x")),
        ("packages/x/y/package.json", pkg("y")),
    ];
    let run = |patterns: &str| {
        let mut files: Vec<(&str, &str)> = tree.iter().map(|(p, c)| (*p, c.as_str())).collect();
        let manifest = root(patterns);
        files.push(("package.json", manifest.as_str()));
        let workspace = discover(&files, &Config::default()).unwrap();
        (dirs(&workspace), workspace.facts.patterns.last().unwrap().matched)
    };
    assert_eq!(
        run(r#"["packages/**","!**/test/**"]"#),
        (
            vec![
                "packages/a".to_owned(),
                "packages/x".to_owned(),
                "packages/x/y".to_owned()
            ],
            2
        )
    );
    assert_eq!(
        run(r#"["packages/**","!packages/x/**"]"#),
        (
            vec![
                "packages/a".to_owned(),
                "packages/a/test".to_owned(),
                "packages/test".to_owned()
            ],
            2
        )
    );
    assert_eq!(
        run(r#"["packages/**","!packages/x/*"]"#),
        (
            vec![
                "packages/a".to_owned(),
                "packages/a/test".to_owned(),
                "packages/test".to_owned(),
                "packages/x".to_owned()
            ],
            1
        )
    );
}

#[test]
fn directories_with_only_generated_content_count_as_empty() {
    let workspace = discover(
        &[
            ("package.json", r#"{"name":"r","workspaces":["packages/*"]}"#),
            ("packages/a/package.json", r#"{"name":"a"}"#),
            ("packages/b/node_modules/dep/index.js", ""),
            ("packages/b/dist/index.js", ""),
            ("packages/b/build/out.js", ""),
            ("packages/b/coverage/lcov.info", ""),
            ("packages/b/.cache/x", ""),
            ("packages/c/src/index.js", ""),
        ],
        &Config::default(),
    )
    .unwrap();
    assert_eq!(workspace.facts.dirs_without_manifest, [PathBuf::from("packages/c")]);
}

#[test]
fn patterns_cannot_climb_out_of_the_root() {
    for pattern in ["../x", "packages/../../x", "!../x"] {
        let manifest = root(&format!(r#"["{pattern}"]"#));
        let workspace = discover(&[("package.json", manifest.as_str())], &Config::default()).unwrap();
        assert!(
            matches!(&workspace.manifest_errors[..], [WorkspaceError::InvalidPattern { pattern: written, .. }] if written == pattern),
            "{pattern}: {:?}",
            workspace.manifest_errors
        );
        assert_eq!(workspace.facts.patterns.len(), 1, "{pattern}");
        assert!(workspace.facts.patterns[0].invalid, "{pattern}");
    }
}

#[test]
fn an_unusable_pattern_does_not_stop_discovery() {
    let explosion = "{a,b}".repeat(9);
    let manifest = root(&format!(r#"["../x","{explosion}","packages/*"]"#));
    let workspace = discover(
        &[
            ("package.json", manifest.as_str()),
            ("packages/a/package.json", r#"{"name":"a"}"#),
        ],
        &Config::default(),
    )
    .unwrap();
    assert_eq!(dirs(&workspace), ["packages/a"]);
    assert_eq!(workspace.manifest_errors.len(), 2);
    assert!(
        workspace
            .manifest_errors
            .iter()
            .all(|error| matches!(error, WorkspaceError::InvalidPattern { .. }))
    );
    let invalid: Vec<bool> = workspace.facts.patterns.iter().map(|fact| fact.invalid).collect();
    assert_eq!(invalid, [true, true, false]);
}

#[test]
fn a_pnpm_workspace_without_packages_takes_every_directory_like_pnpm() {
    let workspace = discover(
        &[
            (
                "package.json",
                r#"{"name":"root","packageManager":"pnpm@9.0.0","workspaces":["other/*"]}"#,
            ),
            ("pnpm-workspace.yaml", "catalog:\n  react: 18\n"),
            ("packages/a/package.json", r#"{"name":"a"}"#),
            ("apps/b/c/package.json", r#"{"name":"c"}"#),
            ("packages/test/x/package.json", r#"{"name":"x"}"#),
            ("other/z/package.json", r#"{"name":"z"}"#),
        ],
        &Config::default(),
    )
    .unwrap();
    assert!(workspace.facts.implicit_patterns);
    assert_eq!(workspace.facts.source, WorkspaceSource::PnpmWorkspaceYaml);
    assert_eq!(dirs(&workspace), ["apps/b/c", "other/z", "packages/a"]);
}

#[test]
fn implicit_patterns_that_select_no_member_make_a_single_package() {
    let workspace = discover(
        &[
            ("package.json", r#"{"name":"app","packageManager":"pnpm@9.0.0"}"#),
            ("pnpm-workspace.yaml", "onlyBuiltDependencies:\n  - esbuild\n"),
            ("src/index.ts", ""),
        ],
        &Config::default(),
    )
    .unwrap();
    assert_eq!(workspace.facts.source, WorkspaceSource::SinglePackage);
    assert!(workspace.packages().is_empty());
}

#[test]
fn written_patterns_selecting_nothing_stay_a_pnpm_workspace() {
    let workspace = discover(
        &[
            ("package.json", r#"{"name":"root","packageManager":"pnpm@9.0.0"}"#),
            ("pnpm-workspace.yaml", "packages:\n  - \"pkgs/*\"\n"),
        ],
        &Config::default(),
    )
    .unwrap();
    assert_eq!(workspace.facts.source, WorkspaceSource::PnpmWorkspaceYaml);
}

#[test]
fn written_patterns_are_not_implicit() {
    let workspace = discover(
        &[
            ("package.json", r#"{"name":"root","packageManager":"pnpm@9.0.0"}"#),
            ("pnpm-workspace.yaml", "packages:\n  - \"pkgs/*\"\n"),
            ("pkgs/a/package.json", r#"{"name":"a"}"#),
        ],
        &Config::default(),
    )
    .unwrap();
    assert!(!workspace.facts.implicit_patterns);
}

#[test]
fn yarn_classic_is_recognised_from_the_package_manager_field_or_the_lockfile_header() {
    let cases: [(&str, Option<&str>, bool); 5] = [
        (r#""packageManager":"yarn@1.22.19","#, None, true),
        (r#""packageManager":"yarn@4.1.0","#, Some("# yarn lockfile v1\n"), false),
        (
            "",
            Some("# THIS IS AN AUTOGENERATED FILE.\n# yarn lockfile v1\n\nx@1:\n"),
            true,
        ),
        (
            "",
            Some("# This file is generated by running \"yarn install\"\n\n__metadata:\n  version: 8\n"),
            false,
        ),
        ("", None, false),
    ];
    for (field, lock, expected) in cases {
        let manifest = format!(r#"{{"name":"root",{field}"workspaces":["a"]}}"#);
        let mut files = vec![("package.json", manifest.as_str()), ("a/package.json", "{}")];
        if let Some(lock) = lock {
            files.push(("yarn.lock", lock));
        }
        let config = Config::from_toml_str("[workspace]\npackage-manager = \"yarn\"\n").unwrap();
        let workspace = discover(&files, &config).unwrap();
        assert_eq!(workspace.facts.yarn_classic, expected, "{field} {lock:?}");
    }
}

#[test]
fn a_dependency_declared_twice_in_a_section_is_collected_once_with_the_last_value() {
    let manifest = r#"{"name":"root","workspaces":["a"],"dependencies":{"x":"1.0.0","y":"1.0.0","x":"2.0.0"},"devDependencies":{"x":"3.0.0"}}"#;
    let workspace = discover(
        &[("package.json", manifest), ("a/package.json", "{}")],
        &Config::default(),
    )
    .unwrap();
    let entries: Vec<(DependencyKind, &str, &str)> = workspace
        .root_package()
        .dependencies
        .iter()
        .map(|entry| (entry.kind, &*entry.name, &*entry.specifier))
        .collect();
    assert_eq!(
        entries,
        [
            (DependencyKind::Dependencies, "y", "1.0.0"),
            (DependencyKind::Dependencies, "x", "2.0.0"),
            (DependencyKind::DevDependencies, "x", "3.0.0"),
        ]
    );
}

#[test]
fn brace_alternation_may_span_directories() {
    let found = run(&["{apps/*/web,tool}", "packages/{a,foo-*}"]);
    assert_eq!(
        dirs(&found),
        [
            "apps/one/web",
            "packages/a",
            "packages/foo-bar",
            "packages/foo-baz",
            "tool"
        ]
    );
    let found = run(&["{apps,packages}/{one,a}/*"]);
    assert_eq!(dirs(&found), ["apps/one/web", "packages/a/nested"]);
}

#[test]
fn pnpm_pattern_spans_follow_duplicates_flow_lists_and_bom() {
    let written = |pairs: &[(&str, &str)]| -> Vec<(String, String)> {
        pairs.iter().map(|(a, b)| ((*a).to_owned(), (*b).to_owned())).collect()
    };
    assert_eq!(
        pnpm_patterns("packages:\n  - a\n  - 'b'\n  - a  # again\n"),
        written(&[("a", "a"), ("b", "'b'"), ("a", "a")])
    );
    let spans = {
        let workspace = discover(
            &[
                ("package.json", r#"{"name":"r","packageManager":"pnpm@9.0.0"}"#),
                ("pnpm-workspace.yaml", "packages:\n  - a\n  - a\n"),
                ("a/package.json", "{}"),
            ],
            &Config::default(),
        )
        .unwrap();
        workspace
            .facts
            .patterns
            .iter()
            .map(|f| f.span.unwrap().start)
            .collect::<Vec<_>>()
    };
    assert_ne!(spans[0], spans[1], "duplicates must not share a span");
    assert_eq!(
        pnpm_patterns("packages: [a, \"b\", 'c']\n"),
        written(&[("a", "a"), ("b", "\"b\""), ("c", "'c'")])
    );
    assert_eq!(
        pnpm_patterns("packages: [\n  a,\n  '!b',\n]\ncatalog: {}\n"),
        written(&[("a", "a"), ("!b", "'!b'")])
    );
    assert_eq!(
        pnpm_patterns("\u{feff}packages:\n  - a\n  - \"b\"\n"),
        written(&[("a", "a"), ("b", "\"b\"")])
    );
}

#[test]
fn a_pnpm_workspace_file_alone_is_a_workspace_without_a_root_manifest() {
    let workspace = discover(
        &[
            ("pnpm-workspace.yaml", "packages:\n  - \"pkgs/*\"\n"),
            ("pkgs/a/package.json", r#"{"name":"a"}"#),
            ("pkgs/b/package.json", r#"{"name":"b"}"#),
        ],
        &Config::default(),
    )
    .unwrap();
    assert!(!workspace.has_root_manifest());
    assert_eq!(workspace.package_manager, Some(PackageManager::Pnpm));
    assert_eq!(workspace.facts.source, WorkspaceSource::PnpmWorkspaceYaml);
    assert_eq!(dirs(&workspace), ["pkgs/a", "pkgs/b"]);
    assert!(
        workspace.is_ignored(workspace.root_package().id),
        "the stand-in root is not analysed"
    );
    assert!(
        workspace
            .packages()
            .iter()
            .all(|package| !workspace.is_ignored(package.id))
    );
}

#[test]
fn a_root_with_a_manifest_reports_it() {
    let manifest = root(r#"["p/*"]"#);
    let member = pkg("a");
    let workspace = discover(
        &[
            ("package.json", manifest.as_str()),
            ("p/a/package.json", member.as_str()),
        ],
        &Config::default(),
    )
    .unwrap();
    assert!(workspace.has_root_manifest());
    assert!(!workspace.is_ignored(workspace.root_package().id));
}

const RUSH: &str = r#"
// Rush configuration
{
  "rushVersion": "5.100.0",
  "pnpmVersion": "9.1.0",
  /* members */
  "projects": [
    { "packageName": "a", "projectFolder": "apps/a" },
    { "packageName": "b", "projectFolder": "libs/b", },
    { "packageName": "gone", "projectFolder": "libs/gone" },
  ],
}
"#;

#[test]
fn rush_json_declares_the_members_and_the_package_manager() {
    let workspace = discover(
        &[
            ("rush.json", RUSH),
            ("apps/a/package.json", r#"{"name":"a"}"#),
            ("libs/b/package.json", r#"{"name":"b"}"#),
            ("libs/other/package.json", r#"{"name":"other"}"#),
        ],
        &Config::default(),
    )
    .unwrap();
    assert!(!workspace.has_root_manifest());
    assert_eq!(workspace.facts.source, WorkspaceSource::RushJson);
    assert_eq!(workspace.package_manager, Some(PackageManager::Pnpm));
    assert_eq!(dirs(&workspace), ["apps/a", "libs/b"]);
    let unmatched: Vec<&str> = workspace
        .facts
        .patterns
        .iter()
        .filter(|fact| fact.matched == 0)
        .map(|fact| fact.pattern.as_str())
        .collect();
    assert_eq!(unmatched, ["libs/gone"]);
    let fact = &workspace.facts.patterns[0];
    assert_eq!(fact.file, Path::new("rush.json"));
    let span = fact.span.unwrap();
    assert_eq!(&RUSH[span.start as usize..span.end as usize], "\"apps/a\"");
}

#[test]
fn the_config_override_beats_the_rush_package_manager() {
    let config = Config::from_toml_str("[workspace]\npackage-manager = \"npm\"\n").unwrap();
    let workspace = discover(
        &[("rush.json", RUSH), ("apps/a/package.json", r#"{"name":"a"}"#)],
        &config,
    )
    .unwrap();
    assert_eq!(workspace.package_manager, Some(PackageManager::Npm));
}

#[test]
fn a_broken_rush_json_is_an_invalid_manifest() {
    let error = discover(&[("rush.json", "{\"projects\": [")], &Config::default()).unwrap_err();
    assert!(matches!(error, WorkspaceError::InvalidManifest { .. }), "{error}");
}

#[test]
fn package_by_name_finds_the_root_and_members_and_the_first_of_a_duplicate_wins() {
    let workspace = discover(
        &[
            ("package.json", r#"{"name":"root","workspaces":["pkgs/*"]}"#),
            ("pkgs/a/package.json", r#"{"name":"dup","version":"1.0.0"}"#),
            ("pkgs/b/package.json", r#"{"name":"dup","version":"2.0.0"}"#),
            ("pkgs/c/package.json", r#"{"name":"root","version":"3.0.0"}"#),
            ("pkgs/d/package.json", r#"{"version":"4.0.0"}"#),
        ],
        &Config::default(),
    )
    .unwrap();
    let found = |name: &str| workspace.package_by_name(name).map(|package| package.dir.clone());
    assert_eq!(found("dup"), Some(PathBuf::from("pkgs/a")));
    assert_eq!(found("root"), Some(PathBuf::new()));
    assert_eq!(found("missing"), None);
    assert_eq!(found(""), None);
    for (id, package) in workspace.iter() {
        if let Some(name) = package.name.as_deref() {
            let first = workspace
                .all_packages()
                .find(|p| p.name.as_deref() == Some(name))
                .map(|p| p.id);
            assert_eq!(workspace.package_by_name(name).map(|p| p.id), first, "{name} {id}");
        }
    }
}

#[test]
fn equal_specifiers_are_parsed_once_per_workspace() {
    let workspace = discover(
        &[
            (
                "package.json",
                r#"{"workspaces":["pkgs/*"],"devDependencies":{"x":"^1.2.3"}}"#,
            ),
            (
                "pkgs/a/package.json",
                r#"{"name":"a","dependencies":{"x":"^1.2.3","y":"~2.0.0"}}"#,
            ),
            (
                "pkgs/b/package.json",
                r#"{"name":"b","dependencies":{"x":"^1.2.3"},"peerDependencies":{"y":"~2.0.0"}}"#,
            ),
        ],
        &Config::default(),
    )
    .unwrap();
    let parsed = |spec: &str| -> Vec<std::sync::Arc<skif_specifier::Specifier>> {
        workspace
            .all_packages()
            .flat_map(|package| package.dependencies.iter())
            .filter(|entry| &*entry.specifier == spec)
            .map(|entry| std::sync::Arc::clone(&entry.parsed))
            .collect()
    };
    for spec in ["^1.2.3", "~2.0.0"] {
        let all = parsed(spec);
        assert!(all.len() >= 2, "{spec}");
        assert!(
            all.windows(2).all(|pair| std::sync::Arc::ptr_eq(&pair[0], &pair[1])),
            "{spec}"
        );
    }
}

#[test]
fn a_section_written_twice_keeps_only_the_last_occurrence_like_the_json_value() {
    let workspace = discover(
        &[(
            "package.json",
            r#"{"name":"root","dependencies":{"a":"1.0.0","b":"1.0.0"},"devDependencies":{"d":"1.0.0"},"dependencies":{"c":"2.0.0"}}"#,
        )],
        &Config::default(),
    )
    .unwrap();
    let root = workspace.root_package();
    let production: Vec<&str> = root
        .dependencies
        .iter()
        .filter(|entry| entry.kind == DependencyKind::Dependencies)
        .map(|entry| &*entry.name)
        .collect();
    assert_eq!(production, ["c"]);
    assert_eq!(
        root.dependency_map(DependencyKind::Dependencies)
            .map(|map| map.keys().cloned().collect::<Vec<_>>()),
        Some(vec!["c".to_owned()])
    );
    assert_eq!(root.dependencies.len(), 2);
}

#[test]
fn a_later_section_that_is_not_an_object_drops_the_earlier_entries() {
    let workspace = discover(
        &[(
            "package.json",
            r#"{"name":"root","dependencies":{"a":"1.0.0"},"dependencies":null}"#,
        )],
        &Config::default(),
    )
    .unwrap();
    assert_eq!(workspace.root_package().dependencies.len(), 0);
}

#[test]
fn yarn_berry_files_identify_yarn_when_nothing_stronger_does() {
    let root = r#"{"name":"root","workspaces":["pkgs/*"]}"#;
    let with_yarnrc = discover(
        &[("package.json", root), (".yarnrc.yml", "nodeLinker: node-modules\n")],
        &Config::default(),
    )
    .unwrap();
    assert_eq!(with_yarnrc.package_manager, Some(PackageManager::Yarn));
    assert!(!with_yarnrc.facts.yarn_classic);
    let with_releases = discover(
        &[("package.json", root), (".yarn/releases/yarn-4.1.0.cjs", "//")],
        &Config::default(),
    )
    .unwrap();
    assert_eq!(with_releases.package_manager, Some(PackageManager::Yarn));
    let bare = discover(&[("package.json", root)], &Config::default()).unwrap();
    assert_eq!(bare.package_manager, None);
    let lockfile = discover(
        &[("package.json", root), (".yarnrc.yml", ""), ("package-lock.json", "{}")],
        &Config::default(),
    )
    .unwrap();
    assert_eq!(lockfile.package_manager, Some(PackageManager::Npm));
}

const LERNA: &str = r#"{
  "version": "independent",
  "npmClient": "yarn",
  "packages": ["packages/*", "tools/gone/*"]
}
"#;

#[test]
fn lerna_json_declares_the_members_and_the_package_manager_when_nothing_else_does() {
    let files = [
        ("package.json", r#"{"name":"root","private":true}"#),
        ("lerna.json", LERNA),
        ("packages/a/package.json", r#"{"name":"a"}"#),
        ("packages/b/package.json", r#"{"name":"b"}"#),
        ("other/c/package.json", r#"{"name":"c"}"#),
    ];
    let workspace = discover(&files, &Config::default()).unwrap();
    assert_eq!(workspace.facts.source, WorkspaceSource::LernaJson);
    assert_eq!(workspace.package_manager, Some(PackageManager::Yarn));
    assert_eq!(dirs(&workspace), ["packages/a", "packages/b"]);
    let fact = &workspace.facts.patterns[1];
    assert_eq!((fact.file.as_path(), fact.matched), (Path::new("lerna.json"), 0));
    let span = fact.span.unwrap();
    assert_eq!(&LERNA[span.start as usize..span.end as usize], "\"tools/gone/*\"");

    let with_workspaces = [
        (
            "package.json",
            r#"{"name":"root","private":true,"workspaces":["other/*"]}"#,
        ),
        ("lerna.json", LERNA),
        ("packages/a/package.json", r#"{"name":"a"}"#),
        ("other/c/package.json", r#"{"name":"c"}"#),
    ];
    let workspace = discover(&with_workspaces, &Config::default()).unwrap();
    assert_eq!(workspace.facts.source, WorkspaceSource::PackageJsonWorkspaces);
    assert_eq!(dirs(&workspace), ["other/c"]);
}

#[test]
fn lerna_json_is_not_read_when_something_else_declares_the_members() {
    let files = [
        ("package.json", r#"{"name":"root","private":true}"#),
        ("pnpm-workspace.yaml", "packages:\n  - packages/*\n"),
        ("lerna.json", "{\"packages\": ["),
        ("packages/a/package.json", r#"{"name":"a"}"#),
    ];
    let workspace = discover(&files, &Config::default()).unwrap();
    assert_eq!(workspace.facts.source, WorkspaceSource::PnpmWorkspaceYaml);
    assert_eq!(dirs(&workspace), ["packages/a"]);
}

fn lerna_paths(dirs: &[PathBuf]) -> Vec<String> {
    dirs.iter()
        .map(|dir| dir.to_str().unwrap().replace('\\', "/"))
        .collect()
}

#[test]
fn lerna_packages_that_differ_from_the_workspaces_are_recorded() {
    let lerna = r#"{"version": "1.0.0", "packages": ["packages/*", "tools/*", "!tools/skip"]}"#;
    let files = [
        (
            "package.json",
            r#"{"name":"root","private":true,"workspaces":["packages/*","apps/*"]}"#,
        ),
        ("lerna.json", lerna),
        ("packages/a/package.json", r#"{"name":"a"}"#),
        ("apps/web/package.json", r#"{"name":"web"}"#),
        ("tools/cli/package.json", r#"{"name":"cli"}"#),
        ("tools/skip/package.json", r#"{"name":"skip"}"#),
        ("tools/empty/readme.md", ""),
    ];
    let workspace = discover(&files, &Config::default()).unwrap();
    assert_eq!(workspace.facts.source, WorkspaceSource::PackageJsonWorkspaces);
    assert_eq!(dirs(&workspace), ["apps/web", "packages/a"]);
    let facts = workspace.facts.lerna.as_ref().unwrap();
    assert_eq!(facts.path, Path::new("lerna.json"));
    assert_eq!(
        facts.packages.as_deref(),
        Some(["packages/*".to_owned(), "tools/*".to_owned(), "!tools/skip".to_owned()].as_slice())
    );
    assert_eq!(lerna_paths(&facts.only_in_lerna), ["tools/cli"]);
    assert_eq!(lerna_paths(&facts.only_in_workspace), ["apps/web"]);
    let span = facts.packages_span.unwrap();
    assert_eq!(
        &lerna[span.start as usize..span.end as usize],
        r#""packages": ["packages/*", "tools/*", "!tools/skip"]"#
    );
}

#[test]
fn lerna_packages_that_select_the_same_members_differently_do_not_differ() {
    let files = [
        (
            "package.json",
            r#"{"name":"root","private":true,"workspaces":["packages/*"]}"#,
        ),
        (
            "lerna.json",
            r#"{"version": "1.0.0", "packages": ["./packages/a", "packages/b"]}"#,
        ),
        ("packages/a/package.json", r#"{"name":"a"}"#),
        ("packages/b/package.json", r#"{"name":"b"}"#),
    ];
    let workspace = discover(&files, &Config::default()).unwrap();
    let facts = workspace.facts.lerna.as_ref().unwrap();
    assert_eq!(facts.only_in_lerna, [] as [std::path::PathBuf; 0]);
    assert_eq!(facts.only_in_workspace, [] as [std::path::PathBuf; 0]);
}

#[test]
fn lerna_packages_that_declare_the_members_differ_from_nothing() {
    let files = [
        ("package.json", r#"{"name":"root","private":true}"#),
        ("lerna.json", LERNA),
        ("packages/a/package.json", r#"{"name":"a"}"#),
        ("other/c/package.json", r#"{"name":"c"}"#),
    ];
    let workspace = discover(&files, &Config::default()).unwrap();
    assert_eq!(workspace.facts.source, WorkspaceSource::LernaJson);
    let facts = workspace.facts.lerna.as_ref().unwrap();
    assert_eq!(facts.only_in_lerna, [] as [std::path::PathBuf; 0]);
    assert_eq!(facts.only_in_workspace, [] as [std::path::PathBuf; 0]);
}

#[test]
fn a_lerna_json_with_use_workspaces_is_recorded_while_the_workspaces_decide() {
    let files = [
        (
            "package.json",
            r#"{"name":"root","private":true,"workspaces":["packages/*"]}"#,
        ),
        ("lerna.json", r#"{"useWorkspaces": true, "npmClient": "pnpm"}"#),
        ("packages/a/package.json", r#"{"name":"a"}"#),
    ];
    let workspace = discover(&files, &Config::default()).unwrap();
    assert_eq!(workspace.facts.source, WorkspaceSource::PackageJsonWorkspaces);
    // `npmClient` names the manager only when `lerna.json` declares the members.
    assert_eq!(workspace.package_manager, None);
    let facts = workspace.facts.lerna.as_ref().unwrap();
    assert_eq!(facts.use_workspaces, Some(true));
    assert!(facts.use_workspaces_span.is_some());
    assert!(!facts.has_version);
    assert_eq!(facts.packages, None);
    assert_eq!(facts.only_in_lerna, [] as [std::path::PathBuf; 0]);
}

#[test]
fn without_lerna_json_there_are_no_lerna_facts() {
    let files = [
        (
            "package.json",
            r#"{"name":"root","private":true,"workspaces":["packages/*"]}"#,
        ),
        ("packages/a/package.json", r#"{"name":"a"}"#),
    ];
    let workspace = discover(&files, &Config::default()).unwrap();
    assert_eq!(workspace.facts.lerna, None);
}

#[test]
fn an_invalid_lerna_json_declares_nothing() {
    let files = [
        ("package.json", r#"{"name":"root","private":true}"#),
        ("lerna.json", "{\"packages\": ["),
        ("packages/a/package.json", r#"{"name":"a"}"#),
    ];
    let workspace = discover(&files, &Config::default()).unwrap();
    assert_eq!(workspace.facts.source, WorkspaceSource::SinglePackage);
}

fn package_manager_files(workspace: &Workspace) -> Vec<String> {
    workspace
        .facts
        .package_manager_files
        .iter()
        .map(|path| path.to_str().unwrap().replace('\\', "/"))
        .collect()
}

#[test]
fn package_manager_files_of_the_root_and_the_selected_directories_are_listed() {
    let manifest = root(r#"["packages/*"]"#);
    let a = pkg("a");
    let b = pkg("b");
    let workspace = discover(
        &[
            ("package.json", manifest.as_str()),
            ("pnpm-lock.yaml", ""),
            ("yarn.lock", ""),
            ("packages/a/package.json", a.as_str()),
            ("packages/a/package-lock.json", "{}"),
            ("packages/a/npm-shrinkwrap.json", "{}"),
            ("packages/a/bun.lock", ""),
            ("packages/a/bun.lockb", ""),
            ("packages/a/pnpm-lock.yaml", ""),
            ("packages/a/pnpm-workspace.yaml", "packages: []\n"),
            ("packages/a/package-lock.json.bak", ""),
            ("packages/a/sub/package-lock.json", "{}"),
            ("packages/b/package.json", b.as_str()),
            ("tools/x/package-lock.json", "{}"),
        ],
        &Config::default(),
    )
    .unwrap();
    assert_eq!(
        package_manager_files(&workspace),
        [
            "packages/a/bun.lock",
            "packages/a/bun.lockb",
            "packages/a/npm-shrinkwrap.json",
            "packages/a/package-lock.json",
            "packages/a/pnpm-lock.yaml",
            "packages/a/pnpm-workspace.yaml",
            "pnpm-lock.yaml",
            "yarn.lock",
        ]
    );
}

#[test]
fn ignored_package_manager_files_are_not_listed() {
    let manifest = root(r#"["packages/*"]"#);
    let a = pkg("a");
    let b = pkg("b");
    let c = pkg("c");
    let workspace = discover(
        &[
            (".git/HEAD", "ref: refs/heads/main\n"),
            (".git/info/exclude", "bun.lockb\n"),
            (".gitignore", "package-lock.json\npackages/c/\n"),
            ("package.json", manifest.as_str()),
            ("packages/a/package.json", a.as_str()),
            ("packages/a/package-lock.json", "{}"),
            ("packages/a/bun.lockb", ""),
            ("packages/a/yarn.lock", ""),
            ("packages/b/.gitignore", "yarn.lock\n!package-lock.json\n"),
            ("packages/b/package.json", b.as_str()),
            ("packages/b/package-lock.json", "{}"),
            ("packages/b/yarn.lock", ""),
            ("packages/c/package.json", c.as_str()),
            ("packages/c/pnpm-lock.yaml", ""),
        ],
        &Config::default(),
    )
    .unwrap();
    assert_eq!(
        package_manager_files(&workspace),
        ["packages/a/yarn.lock", "packages/b/package-lock.json"]
    );
}

/// Counts the probes of member manifests that reach the wrapped file system.
struct ManifestProbes {
    inner: MemoryFs,
    probes: std::sync::atomic::AtomicUsize,
}

impl skif_core::FileSystem for ManifestProbes {
    fn read_to_string(&self, path: &Path) -> std::io::Result<std::sync::Arc<str>> {
        self.inner.read_to_string(path)
    }

    fn read_dir(&self, path: &Path) -> std::io::Result<Vec<skif_core::DirEntry>> {
        self.inner.read_dir(path)
    }

    fn is_file(&self, path: &Path) -> bool {
        if path.starts_with("/ws/packages") && path.ends_with("package.json") {
            self.probes.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        self.inner.is_file(path)
    }

    fn is_dir(&self, path: &Path) -> bool {
        self.inner.is_dir(path)
    }
}

#[test]
fn a_caching_file_system_answers_member_manifest_probes_from_the_listing() {
    let manifest = root(r#"["packages/*"]"#);
    let (a, b) = (pkg("a"), pkg("b"));
    let inner = memory(&[
        ("package.json", manifest.as_str()),
        ("packages/a/package.json", a.as_str()),
        ("packages/b/package.json", b.as_str()),
        ("packages/c/readme.md", "no manifest"),
    ]);
    let fs = skif_core::CachedFs::new(ManifestProbes {
        inner,
        probes: std::sync::atomic::AtomicUsize::new(0),
    });
    let workspace = Workspace::discover_with_fs(Path::new("/ws"), &Config::default(), &fs).unwrap();
    assert_eq!(dirs(&workspace), ["packages/a", "packages/b"]);
    assert_eq!(workspace.facts.dirs_without_manifest, [PathBuf::from("packages/c")]);
    assert_eq!(fs.inner().probes.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[test]
fn claimed_dirs_without_loading_the_members_are_those_of_a_full_discovery() {
    let layouts: [&[(&str, &str)]; 6] = [
        &[
            ("package.json", r#"{"workspaces": ["p/*", "!p/skipped"]}"#),
            ("p/good/package.json", r#"{"name": "good"}"#),
            ("p/bad/package.json", "{ not json"),
            ("p/bare/README.md", "no manifest"),
            ("p/skipped/package.json", r#"{"name": "skipped"}"#),
        ],
        &[
            ("package.json", r#"{"name": "root"}"#),
            ("pnpm-workspace.yaml", "packages:\n  - apps/*\n"),
            ("apps/web/package.json", r#"{"name": "web"}"#),
            ("libs/ui/package.json", r#"{"name": "ui"}"#),
        ],
        &[
            ("package.json", r#"{"name": "single"}"#),
            ("sub/package.json", r#"{"name": "sub"}"#),
        ],
        &[("package.json", r#"{"workspaces": ["missing/*"]}"#)],
        &[("package.json", "{ not json")],
        &[("README.md", "no manifest")],
    ];
    for files in layouts {
        let fs = memory(files);
        let root = Path::new("/ws");
        let config = Config::default();
        let full = Workspace::discover_with_fs(root, &config, &fs).map(|workspace| workspace.claimed_dirs());
        let claimed = Workspace::claimed_dirs_with_fs(root, &config, &fs);
        match (full, claimed) {
            (Ok(full), Ok(claimed)) => assert_eq!(claimed, full, "{files:?}"),
            (Err(full), Err(claimed)) => assert_eq!(claimed.to_string(), full.to_string(), "{files:?}"),
            (full, claimed) => panic!("{files:?}: {full:?} against {claimed:?}"),
        }
    }
    let fs = memory(layouts[0]);
    assert_eq!(
        Workspace::claimed_dirs_with_fs(Path::new("/ws"), &Config::default(), &fs).unwrap(),
        BTreeSet::from([PathBuf::from("/ws/p/bad"), PathBuf::from("/ws/p/good")])
    );
}
