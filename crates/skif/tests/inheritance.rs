#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use skif::{
    Ceiling, ConfigCache, ConfigOverrides, DirEntry, FileSystem, Level, MemoryFs, SourceKind, Target, discover_all,
    find_target, load_target,
};

const PACKAGE: &str = r#"{"name": "NAME", "version": "1.0.0"}"#;

fn package(name: &str) -> String {
    PACKAGE.replace("NAME", name)
}

/// A repository with an organisation file, a pnpm project with its own file, and a standalone package inside the
/// project that opts out of the project but extends the organisation file.
fn acme() -> MemoryFs {
    MemoryFs::new()
        .with_file("/acme/.git/HEAD", "ref")
        .with_file(
            "/acme/skif.toml",
            r#"
[lint]
fail-on-warnings = true

[js]
ignore-binaries = ["docker"]

[[version-groups]]
label = "org"
dependencies = ["moment"]
policy = "banned"

[[overrides]]
packages = ["./legacy/**"]
max-level = "warn"
"#,
        )
        .with_file("/acme/apps/site/package.json", package("site"))
        .with_file(
            "/acme/services/billing/package.json",
            r#"{"name": "billing", "version": "1.0.0", "workspaces": ["packages/*"]}"#,
        )
        .with_file(
            "/acme/services/billing/skif.toml",
            r#"
[js]
extend-ignore-binaries = ["prisma"]

[[version-groups]]
label = "stripe"
dependencies = ["stripe"]
policy = "ignored"
"#,
        )
        .with_file("/acme/services/billing/packages/api/package.json", package("api"))
        .with_file("/acme/services/billing/tools/codegen/package.json", package("codegen"))
        .with_file(
            "/acme/services/billing/tools/codegen/skif.toml",
            r#"
root = true
extends = ["../../../../skif.toml"]

[js]
entry = ["bin/codegen.mjs"]
"#,
        )
}

fn labels(target: &Target) -> Vec<&str> {
    target
        .config
        .version_groups
        .iter()
        .filter_map(|group| group.label.as_deref())
        .collect()
}

fn target<'a>(targets: &'a [Target], root: &str) -> &'a Target {
    targets.iter().find(|target| target.root == Path::new(root)).unwrap()
}

#[test]
fn each_root_gets_the_files_that_govern_it() {
    let targets = discover_all(Path::new("/acme"), &acme(), &ConfigOverrides::default()).unwrap();
    let roots: Vec<_> = targets.iter().map(|target| target.root.to_str().unwrap()).collect();
    assert_eq!(
        roots,
        [
            "/acme/apps/site",
            "/acme/services/billing",
            "/acme/services/billing/tools/codegen"
        ]
    );

    let site = target(&targets, "/acme/apps/site");
    assert!(site.config.lint.fail_on_warnings);
    assert_eq!(labels(site), ["org"]);
    assert_eq!(site.config.js.ignore_binaries, ["docker"]);
    assert_eq!(site.config.overrides.len(), 1);
    assert_eq!(site.sources.len(), 1);
    assert_eq!(site.config_path(), Some(PathBuf::from("../../skif.toml")));

    let billing = target(&targets, "/acme/services/billing");
    assert!(billing.config.lint.fail_on_warnings);
    assert_eq!(labels(billing), ["stripe", "org"]);
    assert_eq!(billing.config.js.ignore_binaries, ["docker", "prisma"]);
    assert_eq!(billing.sources.len(), 2);
    assert_eq!(billing.config_path(), Some(PathBuf::from("skif.toml")));
    let origin = billing.origins.last("version-groups[0]").unwrap();
    assert_eq!(
        billing.sources[origin.source].path,
        Path::new("/acme/services/billing/skif.toml")
    );

    let codegen = target(&targets, "/acme/services/billing/tools/codegen");
    assert!(codegen.config.lint.fail_on_warnings);
    assert_eq!(labels(codegen), ["org"]);
    assert_eq!(codegen.config.js.ignore_binaries, ["docker"]);
    assert_eq!(codegen.config.js.entry, ["bin/codegen.mjs"]);
    let kinds: Vec<_> = codegen.sources.iter().map(|source| source.kind.clone()).collect();
    assert_eq!(
        kinds,
        [
            SourceKind::Extends {
                by: PathBuf::from("/acme/services/billing/tools/codegen/skif.toml")
            },
            SourceKind::Root
        ]
    );
    assert!(codegen.workspace.is_nested());
}

#[test]
fn a_project_has_the_same_configuration_whichever_way_it_is_reached() {
    let fs = acme();
    let overrides = ConfigOverrides::default();
    let from_top = discover_all(Path::new("/acme"), &fs, &overrides).unwrap();
    let from_project = discover_all(Path::new("/acme/services/billing"), &fs, &overrides).unwrap();
    let direct = find_target(Path::new("/acme/services/billing/packages/api"), &fs, &overrides).unwrap();
    let billing = target(&from_top, "/acme/services/billing");
    assert_eq!(target(&from_project, "/acme/services/billing").config, billing.config);
    assert_eq!(direct.root, billing.root);
    assert_eq!(direct.config, billing.config);
    assert_eq!(direct.sources, billing.sources);

    let codegen = find_target(Path::new("/acme/services/billing/tools/codegen"), &fs, &overrides).unwrap();
    assert_eq!(
        codegen.config,
        target(&from_top, "/acme/services/billing/tools/codegen").config
    );
}

#[test]
fn overrides_are_stored_with_their_anchor() {
    let targets = discover_all(Path::new("/acme"), &acme(), &ConfigOverrides::default()).unwrap();
    let billing = target(&targets, "/acme/services/billing");
    let scoped = &billing.config.overrides[0];
    assert_eq!(scoped.max_level, Some(Level::Warn));
    assert_eq!(scoped.scope.as_ref().unwrap().declared_in, Path::new("/acme"));
    assert!(scoped.applies_to(Some(("old", Path::new("/acme/legacy/old"))), None));
    assert!(!scoped.applies_to(Some(("api", Path::new("/acme/services/billing/packages/api"))), None));
}

#[test]
fn no_config_and_an_explicit_file_bypass_discovery() {
    let fs = acme().with_file(
        "/elsewhere/ci.toml",
        "[lint]\nfail-on-warnings = false\n[js]\nentry = [\"ci\"]\n",
    );
    let none = ConfigOverrides {
        no_config: true,
        ..ConfigOverrides::default()
    };
    for target in discover_all(Path::new("/acme"), &fs, &none).unwrap() {
        assert!(target.sources.is_empty() && !target.config.lint.fail_on_warnings);
        assert_eq!(target.config_path(), None);
    }
    let explicit = ConfigOverrides {
        config_file: Some(PathBuf::from("/elsewhere/ci.toml")),
        ..ConfigOverrides::default()
    };
    for target in discover_all(Path::new("/acme"), &fs, &explicit).unwrap() {
        assert!(!target.config.lint.fail_on_warnings);
        assert_eq!(target.config.js.entry, ["ci"]);
        assert_eq!(target.sources.len(), 1);
    }
}

#[test]
fn load_target_honours_the_ceiling() {
    let fs = acme();
    let load = |ceiling: Ceiling| {
        load_target(
            Path::new("/acme/services/billing"),
            &ceiling,
            &fs,
            &ConfigOverrides::default(),
            &ConfigCache::new(),
        )
        .unwrap()
    };
    assert_eq!(load(Ceiling::Vcs).sources.len(), 2);
    assert_eq!(load(Ceiling::RootOnly).sources.len(), 1);
    assert_eq!(load(Ceiling::Dir(PathBuf::from("/acme/services"))).sources.len(), 1);
    assert_eq!(load(Ceiling::Dir(PathBuf::from("/acme"))).sources.len(), 2);
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

    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
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
fn each_config_file_is_read_once_per_run() {
    let counting = Counting {
        inner: acme(),
        reads: Mutex::new(BTreeMap::new()),
    };
    let targets = discover_all(Path::new("/acme"), &counting, &ConfigOverrides::default()).unwrap();
    assert_eq!(targets.len(), 3);
    let reads = counting.reads.lock().unwrap();
    let configs: Vec<_> = reads
        .iter()
        .filter(|(path, _)| path.file_name().is_some_and(|name| name == "skif.toml"))
        .collect();
    assert_eq!(configs.len(), 3, "{reads:?}");
    assert!(configs.iter().all(|(_, count)| **count == 1), "{configs:?}");
}

/// A workspace whose members `a` and `b` can carry package files.
fn workspace_with(a_file: Option<&str>, root_file: &str) -> MemoryFs {
    let fs = MemoryFs::new()
        .with_file("/ws/skif.toml", root_file)
        .with_file("/ws/package.json", r#"{"name": "ws", "private": true, "workspaces": ["packages/*"]}"#)
        .with_file("/ws/packages/a/package.json", package("a"))
        .with_file("/ws/packages/b/package.json", package("b"));
    match a_file {
        Some(text) => fs.with_file("/ws/packages/a/skif.toml", text),
        None => fs,
    }
}

fn load_ws(fs: &MemoryFs, overrides: &ConfigOverrides) -> Result<Target, skif::Error> {
    load_target(Path::new("/ws"), &Ceiling::RootOnly, fs, overrides, &ConfigCache::new())
}

#[test]
fn a_package_file_becomes_an_override_scoped_to_its_package() {
    let fs = workspace_with(
        Some("[rules]\nempty-dependencies = \"off\"\n\n[lint]\nmax-level = \"warn\"\n\n[[ignore]]\nrules = [\"unused-files\"]\n"),
        "[rules]\nempty-dependencies = \"error\"\n",
    );
    let target = load_ws(&fs, &ConfigOverrides::default()).unwrap();
    let [scoped] = target.config.overrides.as_slice() else {
        panic!("{:?}", target.config.overrides)
    };
    assert_eq!(scoped.max_level, Some(Level::Warn));
    assert_eq!(scoped.rules["empty-dependencies"].severity, Some(Level::Off));
    assert_eq!(scoped.ignore.len(), 1);
    assert!(scoped.applies_to(Some(("a", Path::new("/ws/packages/a"))), None));
    assert!(!scoped.applies_to(Some(("b", Path::new("/ws/packages/b"))), None));
    assert!(!scoped.applies_to(None, Some(Path::new("/ws/package.json"))));

    let last = target.sources.last().unwrap();
    assert_eq!(last.kind, SourceKind::Package);
    assert_eq!(last.path, Path::new("/ws/packages/a/skif.toml"));
    assert_eq!(target.config_file, Some(PathBuf::from("/ws/skif.toml")));
    assert_eq!(target.config_path(), Some(PathBuf::from("skif.toml")));
    assert_eq!(target.origins.last("overrides[0]").unwrap().source, target.sources.len() - 1);
    assert_eq!(target.config.rules["empty-dependencies"].severity, Some(Level::Error));
}

#[test]
fn package_files_apply_after_the_overrides_of_the_chain_outermost_first() {
    let fs = workspace_with(
        Some("[rules]\nempty-dependencies = \"info\"\n"),
        "[[overrides]]\npackages = [\"b\"]\nmax-level = \"warn\"\n",
    )
    .with_file("/ws/packages/b/skif.toml", "[rules]\nempty-dependencies = \"warn\"\n");
    let target = load_ws(&fs, &ConfigOverrides::default()).unwrap();
    let origins: Vec<_> = target
        .config
        .overrides
        .iter()
        .map(|scoped| scoped.scope.as_ref().unwrap().declared_in.clone())
        .collect();
    assert_eq!(
        origins,
        [
            PathBuf::from("/ws"),
            PathBuf::from("/ws/packages/a"),
            PathBuf::from("/ws/packages/b")
        ]
    );
}

#[test]
fn a_package_file_with_a_root_scoped_key_is_a_scope_error() {
    for (text, key) in [
        ("[workspace]\nignore-packages = [\"x\"]\n", "workspace"),
        ("[lint]\nfail-on-warnings = true\n", "lint.fail-on-warnings"),
        ("[js]\nproduction = true\n", "js.production"),
        ("[rules.empty-dependencies]\nlevel = \"warn\"\nfoo = 1\n", "rules.empty-dependencies.foo"),
        ("extends = [\"../../skif.toml\"]\n", "extends"),
        ("root = true\n", "root"),
        ("[[version-groups]]\ndependencies = [\"x\"]\n", "version-groups"),
    ] {
        let fs = workspace_with(Some(text), "");
        let error = load_ws(&fs, &ConfigOverrides::default()).unwrap_err();
        match error {
            skif::Error::Config(skif::ConfigError::Scope { key: found, path, line, .. }) => {
                assert_eq!(found, key, "{text}");
                assert_eq!(path.as_deref(), Some(Path::new("/ws/packages/a/skif.toml")));
                assert!(line.is_some(), "{text}");
            }
            other => panic!("{text}: {other:?}"),
        }
    }
}

#[test]
fn no_config_skips_package_files() {
    let fs = workspace_with(Some("[rules]\nempty-dependencies = \"off\"\n"), "");
    let overrides = ConfigOverrides {
        no_config: true,
        ..ConfigOverrides::default()
    };
    assert!(load_ws(&fs, &overrides).unwrap().config.overrides.is_empty());
}

#[test]
fn a_file_in_a_directory_that_is_not_a_member_is_not_a_package_file() {
    let fs = workspace_with(None, "").with_file("/ws/packages/c/skif.toml", "[workspace]\nignore-packages = [\"x\"]\n");
    assert!(load_ws(&fs, &ConfigOverrides::default()).unwrap().config.overrides.is_empty());
}

#[test]
fn a_member_directory_is_not_a_root_of_its_own() {
    let fs = workspace_with(Some("[rules]\nempty-dependencies = \"off\"\n"), "");
    let targets = discover_all(Path::new("/ws"), &fs, &ConfigOverrides::default()).unwrap();
    let roots: Vec<_> = targets.iter().map(|target| target.root.clone()).collect();
    assert_eq!(roots, [PathBuf::from("/ws")]);
    let direct = find_target(Path::new("/ws/packages/a"), &fs, &ConfigOverrides::default()).unwrap();
    assert_eq!(direct.root, Path::new("/ws"));
    assert_eq!(direct.config.overrides, targets[0].config.overrides);
}
