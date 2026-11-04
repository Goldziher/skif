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
