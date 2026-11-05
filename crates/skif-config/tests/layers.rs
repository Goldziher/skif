#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use skif_config::{
    Ceiling, Config, ConfigCache, ConfigError, ConfigLoader, ConfigOverrides, Level, Op, PluginConfig, ResolvedConfig,
    SourceKind, VersionPolicy, discover,
};
use skif_core::{FileSystem, MemoryFs};

fn fs(files: &[(&str, &str)]) -> MemoryFs {
    files
        .iter()
        .fold(MemoryFs::new(), |fs, (path, content)| fs.with_file(*path, content))
}

fn try_resolve(fs: &dyn FileSystem, root: &str, ceiling: &Ceiling) -> Result<ResolvedConfig, ConfigError> {
    let cache = ConfigCache::new();
    ConfigLoader::new(fs, &cache).resolve(Path::new(root), ceiling)
}

fn resolve(files: &[(&str, &str)], root: &str) -> ResolvedConfig {
    try_resolve(&fs(files), root, &Ceiling::Vcs).unwrap()
}

fn error(files: &[(&str, &str)], root: &str) -> ConfigError {
    try_resolve(&fs(files), root, &Ceiling::Vcs).unwrap_err()
}

/// A repository at /r whose org file is `org` and whose project at /r/p has `project`.
fn two(org: &str, project: &str) -> ResolvedConfig {
    resolve(
        &[("/r/.git/HEAD", ""), ("/r/skif.toml", org), ("/r/p/skif.toml", project)],
        "/r/p",
    )
}

#[test]
fn a_single_file_resolves_to_what_it_parses_to() {
    for text in [
        "",
        "[lint]\nfail-on-warnings = true\n[rules]\na = \"warn\"\nb = { severity = \"error\", mode = \"x\" }\nc = { level = \"off\" }\n",
        "[workspace]\nignore-packages = [\"a\"]\n[[ignore]]\nrules = [\"x\"]\n[[version-groups]]\ndependencies = [\"a\"]\npolicy = \"banned\"\n",
        "[js]\nentry = [\"a\"]\n[js.plugins]\nvitest = true\njest = { config = [\"j.js\"] }\n[js.workspaces.\"p/a\"]\nentry = [\"x\"]\n",
        "[[dependency-families]]\nname = \"react\"\npackages = [\"react\"]\n",
    ] {
        let resolved = resolve(&[("/r/skif.toml", text)], "/r");
        assert_eq!(resolved.config, Config::from_toml_str(text).unwrap(), "{text}");
    }
}

#[test]
fn the_fixtures_resolve_like_discovery() {
    for name in ["basic", "full"] {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures").join(name);
        let (_, expected) = discover(&dir).unwrap().unwrap();
        let cache = ConfigCache::new();
        let resolved = ConfigLoader::new(&skif_core::OsFs, &cache)
            .resolve(&dir, &Ceiling::RootOnly)
            .unwrap();
        assert_eq!(resolved.config, expected, "{name}");
    }
}

#[test]
fn scalars_replace_and_unset_keys_are_inherited() {
    let config = two(
        "[lint]\nfail-on-warnings = true\n[workspace]\npackage-manager = \"pnpm\"\n",
        "[lint]\nfail-on-warnings = false\n",
    )
    .config;
    assert!(!config.lint.fail_on_warnings);
    assert_eq!(
        config.workspace.package_manager,
        Some(skif_config::PackageManager::Pnpm)
    );
}

#[test]
fn tables_merge_key_by_key() {
    let config = two(
        "[catalog]\nmode = \"enforce\"\nmin-packages = 5\n",
        "[catalog]\nmode = \"prefer\"\n",
    )
    .config;
    assert_eq!(config.catalog.mode, skif_config::CatalogMode::Prefer);
    assert_eq!(config.catalog.min_packages, 5);
}

#[test]
fn scalar_arrays_replace() {
    let config = two(
        "[js]\nentry = [\"a\"]\nignore-dependencies = [\"x\"]\n[workspace]\nignore-packages = [\"o\"]\n",
        "[js]\nentry = [\"b\"]\n[workspace]\nignore-packages = [\"p\"]\n",
    )
    .config;
    assert_eq!(config.js.entry, ["b"]);
    assert_eq!(config.js.ignore_dependencies, ["x"]);
    assert_eq!(config.workspace.ignore_packages, ["p"]);
}

#[test]
fn extend_keys_append_to_the_inherited_list() {
    let config = two(
        "[workspace]\nignore-packages = [\"o\"]\n[js]\nignore = [\"i\"]\nignore-dependencies = [\"d\"]\nignore-binaries = [\"b\"]\n[catalog]\nexclude = [\"c\"]\n",
        "[workspace]\nextend-ignore-packages = [\"p\"]\n[js]\nextend-ignore = [\"i2\"]\nextend-ignore-dependencies = [\"d2\"]\nextend-ignore-binaries = [\"b2\"]\n[catalog]\nextend-exclude = [\"c2\"]\n",
    )
    .config;
    assert_eq!(config.workspace.ignore_packages, ["o", "p"]);
    assert_eq!(config.js.ignore, ["i", "i2"]);
    assert_eq!(config.js.ignore_dependencies, ["d", "d2"]);
    assert_eq!(config.js.ignore_binaries, ["b", "b2"]);
    assert_eq!(config.catalog.exclude, ["c", "c2"]);
    assert!(config.js.extend_ignore.is_empty() && config.catalog.extend_exclude.is_empty());
    assert!(config.workspace.extend_ignore_packages.is_empty());
}

#[test]
fn extend_ignore_unresolved_appends_to_the_inherited_list() {
    let config = two(
        "[js]\nignore-unresolved = [\"a\"]\n",
        "[js]\nextend-ignore-unresolved = [\"b\"]\n",
    )
    .config;
    assert_eq!(config.js.ignore_unresolved, ["a", "b"]);
    assert_eq!(config.js.extend_ignore_unresolved, [] as [std::string::String; 0]);
}

#[test]
fn within_one_file_the_key_applies_before_its_extend_key() {
    let config = two(
        "[js]\nignore-dependencies = [\"inherited\"]\n",
        "[js]\nextend-ignore-dependencies = [\"added\"]\nignore-dependencies = [\"replaced\"]\n",
    )
    .config;
    assert_eq!(config.js.ignore_dependencies, ["replaced", "added"]);
}

#[test]
fn extend_keys_work_in_a_standalone_file_too() {
    let text = "[js]\nignore = [\"a\"]\nextend-ignore = [\"b\"]\n";
    assert_eq!(Config::from_toml_str(text).unwrap().js.ignore, ["a", "b"]);
    assert_eq!(resolve(&[("/r/skif.toml", text)], "/r").config.js.ignore, ["a", "b"]);
}

#[test]
fn a_rule_level_keeps_the_inherited_options_and_options_replace_as_a_whole() {
    let org = "[rules]\nunused-exports = { level = \"warn\", mode = \"a\", extra = 1 }\n";
    let level = two(org, "[rules]\nunused-exports = \"error\"\n").config;
    let rule = level.rule("unused-exports").unwrap();
    assert_eq!(rule.severity, Some(Level::Error));
    assert_eq!(rule.options.get("mode").and_then(|value| value.as_str()), Some("a"));

    let options = two(org, "[rules]\nunused-exports = { mode = \"b\" }\n").config;
    let rule = options.rule("unused-exports").unwrap();
    assert_eq!(rule.severity, Some(Level::Warn));
    assert_eq!(rule.options.get("mode").and_then(|value| value.as_str()), Some("b"));
    assert!(!rule.options.contains_key("extra"));

    let only_level = two(org, "[rules]\nunused-exports = { level = \"off\" }\n").config;
    let rule = only_level.rule("unused-exports").unwrap();
    assert_eq!(rule.severity, Some(Level::Off));
    assert!(rule.options.contains_key("extra"));
}

#[test]
fn a_plugin_switch_keeps_the_options_and_a_table_replaces_them() {
    let org = "[js.plugins]\nvitest = { config = [\"a.ts\"], entry = [\"e\"] }\n";
    let off = two(org, "[js.plugins]\nvitest = false\n").config;
    let PluginConfig::Options(options) = &off.js.plugins["vitest"] else {
        panic!("expected options");
    };
    assert_eq!(options.enabled, Some(false));
    assert_eq!(options.config, ["a.ts"]);

    let table = resolve(
        &[
            ("/r/.git/HEAD", ""),
            ("/r/skif.toml", org),
            ("/r/p/skif.toml", "[js.plugins]\nvitest = false\n"),
            ("/r/p/q/skif.toml", "[js.plugins]\nvitest = { project = [\"p\"] }\n"),
        ],
        "/r/p/q",
    )
    .config;
    let PluginConfig::Options(options) = &table.js.plugins["vitest"] else {
        panic!("expected options");
    };
    assert_eq!(options.enabled, Some(false));
    assert!(options.config.is_empty() && options.entry.is_empty());
    assert_eq!(options.project, ["p"]);

    let bare = two("[js.plugins]\nvitest = true\n", "[js.plugins]\nvitest = false\n").config;
    assert_eq!(bare.js.plugins["vitest"], PluginConfig::Enabled(false));
}

#[test]
fn ignore_entries_accumulate_farthest_first() {
    let config = two(
        "[[ignore]]\nrules = [\"a\"]\n",
        "[[ignore]]\nrules = [\"b\"]\n[[ignore]]\nrules = [\"c\"]\n",
    )
    .config;
    let rules: Vec<_> = config.ignore.iter().map(|entry| entry.rules[0].as_str()).collect();
    assert_eq!(rules, ["a", "b", "c"]);
}

#[test]
fn the_nearer_file_s_groups_come_first() {
    let config = two(
        "[[version-groups]]\nlabel = \"org\"\ndependencies = [\"react\"]\npolicy = \"banned\"\n[[semver-groups]]\nlabel = \"org\"\nrange = \"^\"\n",
        "[[version-groups]]\nlabel = \"p1\"\ndependencies = [\"stripe\"]\npolicy = \"ignored\"\n[[version-groups]]\nlabel = \"p2\"\npolicy = \"banned\"\n[[semver-groups]]\nlabel = \"p\"\nrange = \"~\"\n",
    )
    .config;
    let labels: Vec<_> = config
        .version_groups
        .iter()
        .map(|g| g.label.as_deref().unwrap())
        .collect();
    assert_eq!(labels, ["p1", "p2", "org"]);
    assert_eq!(config.version_groups[0].policy, VersionPolicy::Ignored);
    let labels: Vec<_> = config
        .semver_groups
        .iter()
        .map(|g| g.label.as_deref().unwrap())
        .collect();
    assert_eq!(labels, ["p", "org"]);
}

#[test]
fn families_merge_by_name() {
    let config = two(
        "[[dependency-families]]\nname = \"react\"\npackages = [\"react\", \"react-dom\"]\n[[dependency-families]]\nname = \"babel\"\npackages = [\"@babel/*\"]\n",
        "[[dependency-families]]\nname = \"react\"\npackages = [\"preact\"]\nenabled = false\n[[dependency-families]]\nname = \"mine\"\npackages = [\"x\"]\n",
    )
    .config;
    let families: Vec<_> = config
        .dependency_families
        .iter()
        .map(|family| (family.name.as_str(), family.enabled))
        .collect();
    assert_eq!(families, [("react", false), ("babel", true), ("mine", true)]);
    assert_eq!(config.dependency_families[0].packages, ["preact"]);
}

#[test]
fn reset_discards_the_inherited_lists() {
    let org = "[[version-groups]]\nlabel = \"org\"\ndependencies = [\"a\"]\npolicy = \"banned\"\n[[ignore]]\nrules = [\"x\"]\n[[overrides]]\nfiles = [\"**\"]\n[[dependency-families]]\nname = \"f\"\npackages = [\"a\"]\n[[semver-groups]]\nrange = \"^\"\n";
    let project = "reset = [\"version-groups\", \"ignore\", \"overrides\", \"dependency-families\", \"semver-groups\"]\n[[ignore]]\nrules = [\"y\"]\n";
    let resolved = two(org, project);
    let config = &resolved.config;
    assert!(config.version_groups.is_empty() && config.overrides.is_empty());
    assert!(config.dependency_families.is_empty() && config.semver_groups.is_empty());
    assert_eq!(config.ignore.len(), 1);
    assert_eq!(config.ignore[0].rules, ["y"]);
    assert_eq!(resolved.origins.last("version-groups").unwrap().op, Op::Reset);
}

#[test]
fn reset_rejects_unknown_keys() {
    let error = error(&[("/r/skif.toml", "reset = [\"rules\"]\n")], "/r");
    assert!(matches!(error, ConfigError::Parse { .. }), "{error}");
}

#[test]
fn unknown_keys_still_fail_with_the_file_and_line() {
    let error = error(
        &[
            ("/r/.git/HEAD", ""),
            ("/r/skif.toml", ""),
            ("/r/p/skif.toml", "\n[lint]\nbogus = 1\n"),
        ],
        "/r/p",
    );
    let ConfigError::Parse { path, line, .. } = error else {
        panic!("expected a parse error");
    };
    assert_eq!(path, Some(PathBuf::from("/r/p/skif.toml")));
    assert_eq!(line, Some(3));
}

#[test]
fn ancestors_up_to_the_git_directory_contribute_and_nothing_above_it() {
    let files = [
        ("/skif.toml", "[lint]\nfail-on-warnings = true\n"),
        ("/r/.git/HEAD", ""),
        ("/r/skif.toml", "[js]\nentry = [\"r\"]\n"),
        ("/r/a/b/skif.toml", "[js]\nproject = [\"b\"]\n"),
    ];
    let resolved = resolve(&files, "/r/a/b");
    assert_eq!(resolved.config.js.entry, ["r"]);
    assert_eq!(resolved.config.js.project, ["b"]);
    assert!(!resolved.config.lint.fail_on_warnings);
    let kinds: Vec<_> = resolved.sources.iter().map(|source| source.kind.clone()).collect();
    assert_eq!(kinds, [SourceKind::Ancestor, SourceKind::Root]);
    assert_eq!(resolved.nearest_file(), Some(Path::new("/r/a/b/skif.toml")));
}

#[test]
fn a_git_file_is_a_repository_marker_too() {
    let files = [
        ("/skif.toml", "[lint]\nfail-on-warnings = true\n"),
        ("/r/.git", "gitdir: /elsewhere"),
        ("/r/a/skif.toml", ""),
    ];
    assert!(!resolve(&files, "/r/a").config.lint.fail_on_warnings);
}

#[test]
fn root_true_ends_the_search_at_that_file() {
    let files = [
        ("/r/.git/HEAD", ""),
        ("/r/skif.toml", "[lint]\nfail-on-warnings = true\n"),
        ("/r/a/skif.toml", "root = true\n[js]\nentry = [\"a\"]\n"),
        ("/r/a/b/skif.toml", ""),
    ];
    let resolved = resolve(&files, "/r/a/b");
    assert!(!resolved.config.lint.fail_on_warnings);
    assert_eq!(resolved.config.js.entry, ["a"]);
    assert!(!resolved.config.root);
    assert_eq!(resolved.sources.len(), 2);
}

#[test]
fn without_a_repository_the_ceiling_is_the_fallback() {
    let files = [
        ("/r/skif.toml", "[lint]\nfail-on-warnings = true\n"),
        ("/r/a/b/skif.toml", ""),
    ];
    let fs = fs(&files);
    let fail = |ceiling: Ceiling| {
        try_resolve(&fs, "/r/a/b", &ceiling)
            .unwrap()
            .config
            .lint
            .fail_on_warnings
    };
    assert!(!fail(Ceiling::Vcs));
    assert!(!fail(Ceiling::RootOnly));
    assert!(!fail(Ceiling::VcsOr(PathBuf::from("/r/a/b"))));
    assert!(fail(Ceiling::VcsOr(PathBuf::from("/r"))));
    assert!(fail(Ceiling::Dir(PathBuf::from("/r"))));
    assert!(!fail(Ceiling::Dir(PathBuf::from("/r/a"))));
}

#[test]
fn a_ceiling_that_is_not_above_the_root_is_an_error_rather_than_no_configuration() {
    let files = [("/r/a/skif.toml", "[lint]\nfail-on-warnings = true\n")];
    let fs = fs(&files);
    for ceiling in [
        Ceiling::Dir(PathBuf::from("/elsewhere")),
        Ceiling::Dir(PathBuf::from("/r/a/b")),
        Ceiling::VcsOr(PathBuf::from("/elsewhere")),
    ] {
        let error = try_resolve(&fs, "/r/a", &ceiling).unwrap_err();
        assert!(matches!(error, ConfigError::Invalid { .. }), "{ceiling:?}: {error}");
        assert!(error.to_string().contains("ceiling"), "{error}");
    }
}

#[test]
fn a_relative_root_is_an_error() {
    let error = try_resolve(&fs(&[]), "r/a", &Ceiling::Vcs).unwrap_err();
    assert!(matches!(error, ConfigError::Invalid { .. }), "{error}");
    assert!(error.to_string().contains("not an absolute path"), "{error}");
}

#[test]
fn dot_segments_in_the_root_do_not_change_what_is_read() {
    let files = [
        ("/r/.git/HEAD", ""),
        ("/r/skif.toml", "[lint]\nfail-on-warnings = true\n"),
        ("/r/a/skif.toml", ""),
    ];
    let fs = fs(&files);
    let plain = try_resolve(&fs, "/r/a", &Ceiling::Vcs).unwrap();
    let dotted = try_resolve(&fs, "/r/./a/b/..", &Ceiling::Vcs).unwrap();
    assert_eq!(dotted.sources, plain.sources);
    assert_eq!(dotted.config, plain.config);
    assert_eq!(dotted.sources.len(), 2);
}

#[test]
fn a_repository_wins_over_the_fallback() {
    let files = [
        ("/s/skif.toml", "[lint]\nfail-on-warnings = true\n"),
        ("/s/r/.git/HEAD", ""),
        ("/s/r/a/skif.toml", ""),
    ];
    let fs = fs(&files);
    let resolved = try_resolve(&fs, "/s/r/a", &Ceiling::VcsOr(PathBuf::from("/s"))).unwrap();
    assert!(!resolved.config.lint.fail_on_warnings);
}

#[test]
fn both_file_names_in_one_directory_are_ambiguous() {
    let error = error(&[("/r/skif.toml", ""), ("/r/.skif.toml", "")], "/r");
    assert!(matches!(error, ConfigError::Ambiguous { .. }));
}

#[test]
fn extends_resolves_relative_to_the_declaring_file_depth_first() {
    let files = [
        (
            "/r/skif.toml",
            "extends = [\"presets/a.toml\", \"presets/b.toml\"]\n[js]\nentry = [\"own\"]\n",
        ),
        (
            "/r/presets/a.toml",
            "extends = [\"base.toml\"]\n[js]\nentry = [\"a\"]\nproject = [\"a\"]\n",
        ),
        (
            "/r/presets/base.toml",
            "[js]\nproject = [\"base\"]\nignore = [\"base\"]\n",
        ),
        ("/r/presets/b.toml", "[js]\nproject = [\"b\"]\n"),
    ];
    let resolved = resolve(&files, "/r");
    let order: Vec<_> = resolved.sources.iter().map(|source| source.path.clone()).collect();
    assert_eq!(
        order,
        [
            "/r/presets/base.toml",
            "/r/presets/a.toml",
            "/r/presets/b.toml",
            "/r/skif.toml"
        ]
        .map(PathBuf::from)
    );
    assert_eq!(
        resolved.sources[0].kind,
        SourceKind::Extends {
            by: PathBuf::from("/r/presets/a.toml")
        }
    );
    assert_eq!(resolved.sources[3].kind, SourceKind::Root);
    assert_eq!(resolved.config.js.entry, ["own"]);
    assert_eq!(resolved.config.js.project, ["b"]);
    assert_eq!(resolved.config.js.ignore, ["base"]);
    assert_eq!(resolved.config.extends, [] as [std::string::String; 0]);
}

#[test]
fn extends_may_leave_the_directory() {
    let files = [
        ("/org/skif.toml", "[lint]\nfail-on-warnings = true\n"),
        ("/org/r/skif.toml", "root = true\nextends = [\"../skif.toml\"]\n"),
    ];
    let resolved = resolve(&files, "/org/r");
    assert!(resolved.config.lint.fail_on_warnings);
}

#[test]
fn a_cycle_is_an_error_naming_the_files() {
    let files = [
        ("/r/skif.toml", "extends = [\"a.toml\"]\n"),
        ("/r/a.toml", "extends = [\"b.toml\"]\n"),
        ("/r/b.toml", "extends = [\"a.toml\"]\n"),
    ];
    let ConfigError::ExtendsCycle { chain } = error(&files, "/r") else {
        panic!("expected a cycle");
    };
    assert_eq!(chain, ["/r/a.toml", "/r/b.toml", "/r/a.toml"].map(PathBuf::from));
    let itself = error(&[("/r/skif.toml", "extends = [\"./skif.toml\"]\n")], "/r");
    assert!(matches!(itself, ConfigError::ExtendsCycle { .. }), "{itself}");
}

#[test]
fn a_diamond_applies_the_shared_file_once() {
    let files = [
        ("/r/skif.toml", "extends = [\"a.toml\", \"b.toml\"]\n"),
        ("/r/a.toml", "extends = [\"base.toml\"]\n"),
        (
            "/r/b.toml",
            "extends = [\"base.toml\"]\n[js]\nextend-ignore = [\"b\"]\n",
        ),
        (
            "/r/base.toml",
            "[js]\nextend-ignore = [\"base\"]\n[[ignore]]\nrules = [\"x\"]\n",
        ),
    ];
    let resolved = resolve(&files, "/r");
    assert_eq!(resolved.sources.len(), 4);
    assert_eq!(resolved.config.js.ignore, ["base", "b"]);
    assert_eq!(resolved.config.ignore.len(), 1);
}

#[test]
fn an_extended_file_that_is_also_an_ancestor_applies_once() {
    let files = [
        ("/r/.git/HEAD", ""),
        ("/r/skif.toml", "[js]\nextend-ignore = [\"org\"]\n"),
        ("/r/p/skif.toml", "extends = [\"../skif.toml\"]\n"),
    ];
    let resolved = resolve(&files, "/r/p");
    assert_eq!(resolved.config.js.ignore, ["org"]);
    assert_eq!(resolved.sources.len(), 2);
}

#[test]
fn a_missing_target_names_the_declaring_file_and_line() {
    let error = error(&[("/r/skif.toml", "\nextends = [\"nope.toml\"]\n")], "/r");
    let ConfigError::Io { path, source } = &error else {
        panic!("expected an io error, got {error:?}");
    };
    assert_eq!(path, Path::new("/r/nope.toml"));
    assert_eq!(source.kind(), io::ErrorKind::NotFound);
    assert!(error.to_string().contains("/r/skif.toml:2:"), "{error}");
}

#[test]
fn extends_is_limited_to_sixteen_files_deep() {
    let chain = |files: usize| -> Vec<(String, String)> {
        (0..files)
            .map(|index| {
                let path = if index == 0 {
                    "/r/skif.toml".to_owned()
                } else {
                    format!("/r/d{index}.toml")
                };
                let content = if index + 1 < files {
                    format!("extends = [\"d{}.toml\"]\n", index + 1)
                } else {
                    String::new()
                };
                (path, content)
            })
            .collect()
    };
    let resolve_chain = |files: usize| {
        let owned = chain(files);
        let refs: Vec<(&str, &str)> = owned.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        try_resolve(&fs(&refs), "/r", &Ceiling::Vcs)
    };
    assert_eq!(resolve_chain(16).unwrap().sources.len(), 16);
    let error = resolve_chain(17).unwrap_err();
    assert!(error.to_string().contains("16 files deep"), "{error}");
}

#[test]
fn root_is_rejected_in_an_extended_file() {
    let files = [
        ("/r/skif.toml", "extends = [\"a.toml\"]\n"),
        ("/r/a.toml", "\nroot = true\n"),
    ];
    let ConfigError::Scope { key, path, line, .. } = error(&files, "/r") else {
        panic!("expected a scope error");
    };
    assert_eq!(key, "root");
    assert_eq!(path, Some(PathBuf::from("/r/a.toml")));
    assert_eq!(line, Some(2));
}

#[test]
fn root_scoped_keys_in_an_override_are_scope_errors_with_a_position() {
    let cases: &[(&str, &str, usize)] = &[
        (
            "[[overrides]]\npackages = [\"a\"]\n[overrides.workspace]\npackage-manager = \"npm\"\n",
            "overrides[0].workspace",
            3,
        ),
        (
            "[[overrides]]\npackages = [\"a\"]\n[overrides.lint]\nfail-on-warnings = true\n",
            "overrides[0].lint",
            3,
        ),
        (
            "[[overrides]]\npackages = [\"a\"]\n[[overrides.version-groups]]\npolicy = \"banned\"\n",
            "overrides[0].version-groups",
            3,
        ),
        (
            "[[overrides]]\npackages = [\"a\"]\n[overrides.js]\nenabled = false\n",
            "overrides[0].js.enabled",
            4,
        ),
        (
            "[[overrides]]\npackages = [\"a\"]\n[overrides.js]\nproduction = true\n",
            "overrides[0].js.production",
            4,
        ),
        (
            "[[overrides]]\npackages = [\"a\"]\n[overrides.rules]\nx = { level = \"warn\", mode = \"a\" }\n",
            "overrides[0].rules.x.mode",
            4,
        ),
        (
            "[[overrides]]\npackages = [\"a\"]\nroot = true\n",
            "overrides[0].root",
            3,
        ),
        (
            "[[overrides]]\npackages = [\"a\"]\nextends = []\n",
            "overrides[0].extends",
            3,
        ),
    ];
    for &(text, expected_key, expected_line) in cases {
        let ConfigError::Scope {
            key, line, path, hint, ..
        } = error(&[("/r/skif.toml", text)], "/r")
        else {
            panic!("expected a scope error for {text}");
        };
        assert_eq!(key, expected_key, "{text}");
        assert_eq!(line, Some(expected_line), "{text}");
        assert_eq!(path, Some(PathBuf::from("/r/skif.toml")));
        assert_ne!(hint, "");
    }
}

#[test]
fn inconsistent_overrides_are_rejected() {
    for text in [
        "[[overrides]]\nmax-level = \"warn\"\n",
        "[[overrides]]\nfiles = [\"**\"]\n[overrides.js]\nentry = [\"a\"]\n",
        "[[overrides]]\npackages = [\"a\"]\nfiles = [\"**\"]\n[overrides.js]\nentry = [\"a\"]\n",
    ] {
        let error = error(&[("/r/skif.toml", text)], "/r");
        assert!(matches!(error, ConfigError::Invalid { .. }), "{text}: {error}");
        assert!(error.to_string().contains("overrides[0]"), "{error}");
    }
    let fine = resolve(
        &[(
            "/r/skif.toml",
            "[[overrides]]\npackages = [\"a\"]\nmax-level = \"warn\"\n[overrides.rules]\nx = \"off\"\n[overrides.js]\nentry = [\"a\"]\nextend-ignore = [\"b\"]\n[[overrides.ignore]]\nrules = [\"y\"]\n",
        )],
        "/r",
    );
    let scoped = &fine.config.overrides[0];
    assert_eq!(scoped.max_level, Some(Level::Warn));
    assert_eq!(scoped.rules["x"].severity, Some(Level::Off));
    assert_eq!(scoped.js.entry.as_deref(), Some(&["a".to_owned()][..]));
    assert_eq!(scoped.js.extend_ignore, ["b"]);
    assert_eq!(scoped.ignore[0].rules, ["y"]);
}

#[test]
fn lint_max_level_is_a_base_setting() {
    let config = two("[lint]\nmax-level = \"warn\"\n", "").config;
    assert_eq!(config.lint.max_level, Some(Level::Warn));
}

fn applies(config: &Config, index: usize, package: Option<(&str, &str)>, file: Option<&str>) -> bool {
    config.overrides[index].applies_to(package.map(|(name, dir)| (name, Path::new(dir))), file.map(Path::new))
}

#[test]
fn override_patterns_are_anchored_at_the_declaring_file() {
    let org = "[[overrides]]\npackages = [\"./legacy/**\", \"old-*\"]\n[[overrides]]\nfiles = [\"**/generated/**\"]\n[[overrides]]\npackages = [\"../outside\"]\n";
    let resolved = resolve(
        &[
            ("/acme/.git/HEAD", ""),
            ("/acme/skif.toml", org),
            ("/acme/svc/skif.toml", ""),
        ],
        "/acme/svc",
    );
    let config = &resolved.config;
    assert_eq!(config.overrides.len(), 3);

    assert!(applies(config, 0, Some(("x", "/acme/legacy/a")), None));
    assert!(!applies(config, 0, Some(("x", "/acme/svc/legacy/a")), None));
    assert!(applies(config, 0, Some(("old-thing", "/acme/svc/p")), None));
    assert!(!applies(config, 0, None, None));

    assert!(applies(config, 1, None, Some("/acme/svc/p/src/generated/a.ts")));
    assert!(!applies(config, 1, None, Some("/other/generated/a.ts")));
    assert!(!applies(config, 1, Some(("x", "/acme/svc/p")), None));

    assert!(!applies(config, 2, Some(("x", "/outside")), None));
    assert!(!applies(config, 2, Some(("outside", "/acme/outside")), None));

    let scope = config.overrides[0].scope.as_ref().unwrap();
    assert_eq!(scope.declared_in, Path::new("/acme"));
    assert_eq!(scope.source, 0);
}

#[test]
fn root_anchors_follow_the_linted_root_not_the_declaring_file() {
    let preset = "[[overrides]]\npackages = [\"$ROOT/\", \"$ROOT/legacy/**\"]\n[overrides.js]\nextend-ignore-binaries = [\"node\"]\n";
    let files = [
        ("/acme/presets/node.toml", preset),
        ("/acme/svc/skif.toml", "extends = [\"../presets/node.toml\"]\n"),
    ];
    let config = resolve(&files, "/acme/svc").config;
    assert!(applies(&config, 0, Some(("svc", "/acme/svc")), None));
    assert!(applies(&config, 0, Some(("l", "/acme/svc/legacy/a")), None));
    assert!(!applies(&config, 0, Some(("p", "/acme/presets")), None));
    let scope = config.overrides[0].scope.as_ref().unwrap();
    assert_eq!(scope.declared_in, Path::new("/acme/presets"));
}

#[test]
fn glob_characters_in_directory_names_are_literal() {
    let files = [("/a[1]/skif.toml", "[[overrides]]\npackages = [\"./p\"]\n")];
    let config = resolve(&files, "/a[1]").config;
    assert!(applies(&config, 0, Some(("p", "/a[1]/p")), None));
    assert!(!applies(&config, 0, Some(("p", "/a1/p")), None));
}

#[test]
fn overrides_keep_the_order_of_the_chain() {
    let config = two(
        "[[overrides]]\npackages = [\"a\"]\nmax-level = \"info\"\n",
        "[[overrides]]\npackages = [\"b\"]\nmax-level = \"warn\"\n",
    )
    .config;
    let levels: Vec<_> = config.overrides.iter().map(|scoped| scoped.max_level).collect();
    assert_eq!(levels, [Some(Level::Info), Some(Level::Warn)]);
    let sources: Vec<_> = config
        .overrides
        .iter()
        .map(|scoped| scoped.scope.as_ref().unwrap().source)
        .collect();
    assert_eq!(sources, [0, 1]);
}

#[test]
fn origins_record_the_file_line_and_operation() {
    let resolved = two(
        "[lint]\nfail-on-warnings = true\n[js]\nignore-dependencies = [\"a\"]\n[rules]\nx = { level = \"warn\", mode = \"m\" }\n[[version-groups]]\ndependencies = [\"a\"]\npolicy = \"banned\"\n",
        "\n[lint]\nfail-on-warnings = false\n[js]\nextend-ignore-dependencies = [\"b\"]\n[rules]\nx = \"error\"\n[[version-groups]]\ndependencies = [\"b\"]\npolicy = \"banned\"\n",
    );
    let origins = &resolved.origins;
    assert_eq!(resolved.sources.len(), 2);

    let all = origins.get("lint.fail-on-warnings");
    assert_eq!(all.len(), 2);
    assert_eq!(
        (all[0].source, all[0].line, all[0].column, all[0].op),
        (0, 2, 1, Op::Set)
    );
    assert_eq!(
        (all[1].source, all[1].line, all[1].column, all[1].op),
        (1, 3, 1, Op::Set)
    );

    let extended = origins.get("js.ignore-dependencies");
    assert_eq!(
        extended.iter().map(|a| (a.source, a.op)).collect::<Vec<_>>(),
        [(0, Op::Set), (1, Op::Append)]
    );
    assert_eq!(extended[1].line, 5);

    assert_eq!(
        origins
            .get("rules.x.level")
            .iter()
            .map(|a| a.source)
            .collect::<Vec<_>>(),
        [0, 1]
    );
    assert_eq!(
        origins
            .get("rules.x.options")
            .iter()
            .map(|a| a.source)
            .collect::<Vec<_>>(),
        [0]
    );

    let first = origins.last("version-groups[0]").unwrap();
    assert_eq!((first.source, first.op), (1, Op::Prepend));
    let second = origins.last("version-groups[1]").unwrap();
    assert_eq!((second.source, second.op), (0, Op::Prepend));
    assert_eq!(origins.get("catalog.mode"), []);
}

#[test]
fn origin_keys_quote_names_that_contain_dots() {
    let resolved = resolve(
        &[(
            "/r/skif.toml",
            "[js.workspaces.\"pkg/a.b\"]\nentry = [\"x\"]\n[js.workspaces.\"pkg/c\"]\nentry = [\"y\"]\n",
        )],
        "/r",
    );
    let dotted = resolved.origins.last("js.workspaces.\"pkg/a.b\".entry").unwrap();
    assert_eq!((dotted.line, dotted.column), (2, 1));
    let plain = resolved.origins.last("js.workspaces.pkg/c.entry").unwrap();
    assert_eq!(plain.line, 4);
    assert_eq!(resolved.origins.get("js.workspaces.pkg/a.b.entry"), []);
}

#[test]
fn origins_of_extended_files_point_at_them() {
    let files = [
        ("/r/skif.toml", "extends = [\"base.toml\"]\n"),
        ("/r/base.toml", "\n\n[catalog]\n  mode = \"prefer\"\n"),
    ];
    let resolved = resolve(&files, "/r");
    let origin = resolved.origins.last("catalog.mode").unwrap();
    assert_eq!(resolved.sources[origin.source].path, Path::new("/r/base.toml"));
    assert_eq!((origin.line, origin.column), (4, 3));
}

#[test]
fn no_config_and_an_explicit_file_replace_discovery() {
    let fs = fs(&[
        ("/r/.git/HEAD", ""),
        ("/r/skif.toml", "[lint]\nfail-on-warnings = true\n"),
        ("/elsewhere/custom.toml", "[js]\nentry = [\"custom\"]\n"),
    ]);
    let cache = ConfigCache::new();
    let loader = ConfigLoader::new(&fs, &cache);
    let root = Path::new("/r");

    let none = loader
        .load(
            root,
            &Ceiling::Vcs,
            &ConfigOverrides::default()
                .with_no_config(true)
                .with_fail_on_warnings(Some(false)),
        )
        .unwrap();
    assert!(none.sources.is_empty() && !none.config.lint.fail_on_warnings);

    let explicit = loader
        .load(
            root,
            &Ceiling::Vcs,
            &ConfigOverrides::default().with_config_file(Some(PathBuf::from("/elsewhere/custom.toml"))),
        )
        .unwrap();
    assert_eq!(explicit.config.js.entry, ["custom"]);
    assert!(!explicit.config.lint.fail_on_warnings);
    assert_eq!(explicit.sources[0].kind, SourceKind::Explicit);

    let cli = loader
        .load(
            root,
            &Ceiling::Vcs,
            &ConfigOverrides::default().with_ignore_rules(vec!["x".into()]),
        )
        .unwrap();
    assert!(cli.config.lint.fail_on_warnings);
    assert_eq!(cli.config.rule("x").unwrap().severity, Some(Level::Off));
}

struct Counting {
    inner: MemoryFs,
    reads: Mutex<BTreeMap<PathBuf, usize>>,
}

impl FileSystem for Counting {
    fn read_to_string(&self, path: &Path) -> io::Result<Arc<str>> {
        *self.reads.lock().unwrap().entry(path.to_path_buf()).or_default() += 1;
        self.inner.read_to_string(path)
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<skif_core::DirEntry>> {
        self.inner.read_dir(path)
    }

    fn is_file(&self, path: &Path) -> bool {
        self.inner.is_file(path)
    }

    fn is_dir(&self, path: &Path) -> bool {
        self.inner.is_dir(path)
    }
}

#[test]
fn every_file_is_read_once_however_many_roots_and_extends_reach_it() {
    let counting = Counting {
        inner: fs(&[
            ("/r/.git/HEAD", ""),
            ("/r/skif.toml", "extends = [\"preset.toml\"]\n"),
            ("/r/preset.toml", "[lint]\nfail-on-warnings = true\n"),
            ("/r/a/skif.toml", "extends = [\"../preset.toml\"]\n"),
            ("/r/b/skif.toml", ""),
        ]),
        reads: Mutex::new(BTreeMap::new()),
    };
    let cache = ConfigCache::new();
    let loader = ConfigLoader::new(&counting, &cache);
    let loader = &loader;
    std::thread::scope(|scope| {
        for root in ["/r", "/r/a", "/r/b", "/r/a", "/r/b", "/r"] {
            scope.spawn(move || loader.resolve(Path::new(root), &Ceiling::Vcs).unwrap());
        }
    });
    let reads = counting.reads.lock().unwrap();
    assert_eq!(reads.len(), 4);
    assert!(reads.values().all(|&count| count == 1), "{reads:?}");
}

const NODE_TYPE: &str = "[[custom-types]]\nname = \"node\"\npath = \"engines.node\"\nstrategy = \"version\"\n";

#[test]
fn a_layer_may_select_a_custom_type_an_inherited_layer_defines() {
    let config = two(
        NODE_TYPE,
        "[[version-groups]]\ndependency-types = [\"node\"]\npolicy = \"ignored\"\n\n[policy]\ndependency-types = [\"prod\", \"node\"]\n",
    )
    .config;
    assert_eq!(config.custom_types.len(), 1);
    assert_eq!(
        config.version_groups[0]
            .dependency_types
            .custom_names()
            .collect::<Vec<_>>(),
        ["node"]
    );
    assert_eq!(config.policy.dependency_types, ["prod", "node"]);
}

#[test]
fn a_custom_type_nobody_defines_is_still_rejected_after_the_merge() {
    let error = try_resolve(
        &fs(&[
            ("/r/.git/HEAD", ""),
            ("/r/skif.toml", NODE_TYPE),
            (
                "/r/p/skif.toml",
                "[[version-groups]]\ndependency-types = [\"nodes\"]\npolicy = \"ignored\"\n",
            ),
        ]),
        "/r/p",
        &Ceiling::Vcs,
    )
    .unwrap_err();
    let message = error.to_string();
    assert!(message.contains("unknown dependency type \"nodes\""), "{message}");
    assert!(message.contains("did you mean \"node\""), "{message}");
    assert!(
        message.starts_with("/r/p/skif.toml"),
        "the file that names the type is reported: {message}"
    );
}

#[test]
fn a_single_file_still_needs_its_own_custom_types() {
    let error = try_resolve(
        &fs(&[
            ("/r/.git/HEAD", ""),
            ("/r/skif.toml", "[policy]\ndependency-types = [\"node\"]\n"),
        ]),
        "/r",
        &Ceiling::Vcs,
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("unknown dependency type \"node\""),
        "{error}"
    );
}

#[test]
fn a_layer_replaces_an_inherited_custom_type_of_the_same_name() {
    let config = two(
        NODE_TYPE,
        "[[custom-types]]\nname = \"node\"\npath = \"volta.node\"\nstrategy = \"version\"\n",
    )
    .config;
    assert_eq!(config.custom_types.len(), 1);
    assert_eq!(config.custom_types[0].path, "volta.node");
}
