//! ADR 0029 through the facade: a module cache and an overlay of unsaved buffers change no report.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use skif::{
    BufferFs, CachedModule, ConfigOverrides, FileSystem, Linter, MemoryFs, MemoryModuleCache, ModuleCache, ModuleKey,
    Report, Workspace, discover_targets, load_config,
};

// The resolver only applies a `tsconfig` found below an absolute path, and `/ws` has no drive on Windows.
const ROOT: &str = if cfg!(windows) { "C:/ws" } else { "/ws" };

/// The JS analysis fixtures of `skif-js`, each copied into memory below [`ROOT`].
fn fixtures() -> Vec<(String, MemoryFs)> {
    fn load(dir: &Path, base: &Path, fs: &mut MemoryFs) {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                load(&path, base, fs);
            } else if let Ok(text) = std::fs::read_to_string(&path) {
                let rel = path.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/");
                fs.insert(Path::new(ROOT).join(rel), text);
            }
        }
    }
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../skif-js/tests/fixtures");
    let mut cases: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_dir())
        .collect();
    cases.sort();
    cases
        .into_iter()
        .map(|case| {
            let mut fs = MemoryFs::new();
            load(&case, &case, &mut fs);
            (case.file_name().unwrap().to_string_lossy().into_owned(), fs)
        })
        .collect()
}

/// The report of the workspace at [`ROOT`] read through `fs`, with `modules` when given.
fn report(fs: Arc<dyn FileSystem>, modules: Option<Arc<dyn ModuleCache>>) -> Option<Report> {
    let config = load_config(Path::new(ROOT), &*fs, &ConfigOverrides::default()).ok()?;
    let workspace = Workspace::discover_with_fs(Path::new(ROOT), &config, &*fs).ok()?;
    let linter = match modules {
        Some(modules) => Linter::with_module_cache(&config, fs, modules),
        None => Linter::with_fs(&config, fs),
    }
    .unwrap();
    Some(linter.run(&workspace).unwrap())
}

#[test]
fn no_cache_a_cold_cache_and_a_warm_cache_report_alike() {
    let mut linted = 0;
    for (name, fs) in fixtures() {
        let fs: Arc<dyn FileSystem> = Arc::new(fs);
        let Some(uncached) = report(Arc::clone(&fs), None) else {
            continue;
        };
        let cache = Arc::new(MemoryModuleCache::new());
        let cold = report(Arc::clone(&fs), Some(Arc::clone(&cache) as Arc<dyn ModuleCache>)).unwrap();
        assert!(!cache.is_empty(), "{name}: the cold run stored nothing");
        let warm = report(Arc::clone(&fs), Some(Arc::clone(&cache) as Arc<dyn ModuleCache>)).unwrap();
        assert_eq!(cold, uncached, "{name}: cold");
        assert_eq!(warm, uncached, "{name}: warm");
        linted += 1;
    }
    assert!(linted >= 20, "only {linted} fixtures linted");
}

fn manifest_project() -> MemoryFs {
    MemoryFs::new()
        .with_file(
            Path::new(ROOT).join("package.json"),
            r#"{"name": "root", "private": true, "workspaces": ["packages/*"]}"#,
        )
        .with_file(
            Path::new(ROOT).join("packages/a/package.json"),
            "{\n  \"name\": \"a\",\n  \"version\": \"1.0.0\"\n}\n",
        )
}

#[test]
fn an_overlay_without_buffers_reports_like_its_base() {
    let base = manifest_project();
    let overlay = BufferFs::new(base.clone(), BTreeMap::new());
    assert_eq!(
        report(Arc::new(overlay), None).unwrap(),
        report(Arc::new(base), None).unwrap()
    );
}

#[test]
fn a_manifest_buffer_reports_like_the_saved_file() {
    let manifest = Path::new(ROOT).join("packages/a/package.json");
    let edited = "{\n  \"name\": \"a\",\n  \"version\": \"1.0.0\",\n  \"devDependencies\": {}\n}\n";
    let base = manifest_project();
    let mut saved = base.clone();
    saved.insert(&manifest, edited);
    let overlay = BufferFs::new(base.clone(), BTreeMap::new()).with_buffer(&manifest, edited);

    let buffered = report(Arc::new(overlay), None).unwrap();
    assert_eq!(buffered, report(Arc::new(saved), None).unwrap());
    assert_ne!(buffered, report(Arc::new(base), None).unwrap());
    assert!(
        buffered
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.rule.as_str() == "empty-dependencies"),
        "{buffered:#?}"
    );
}

/// Three standalone packages below [`ROOT`] whose entry files import `shared/lib.ts`, which lies outside all of them.
fn shared_closure() -> MemoryFs {
    let mut fs = MemoryFs::new().with_file(Path::new(ROOT).join("shared/lib.ts"), "export const shared = 1;\n");
    for (name, offset) in [("a", 1), ("b", 2), ("c", 3)] {
        fs.insert(
            Path::new(ROOT).join(name).join("package.json"),
            format!(r#"{{"name": "{name}", "version": "1.0.0", "main": "index.ts"}}"#),
        );
        fs.insert(
            Path::new(ROOT).join(name).join("index.ts"),
            format!("import {{ shared }} from '../shared/lib';\nexport const {name} = shared + {offset};\n"),
        );
    }
    fs
}

/// A [`MemoryModuleCache`] that counts the stores of each key: one store is one parse.
#[derive(Default)]
struct Parses {
    inner: MemoryModuleCache,
    stores: Mutex<BTreeMap<ModuleKey, usize>>,
}

impl ModuleCache for Parses {
    fn get(&self, key: ModuleKey) -> Option<CachedModule> {
        self.inner.get(key)
    }

    fn put(&self, key: ModuleKey, module: CachedModule) {
        *self.stores.lock().unwrap().entry(key).or_default() += 1;
        self.inner.put(key, module);
    }
}

/// The report of each target, the targets linted in parallel by `linters`.
fn lint_in_parallel(targets: &[skif::Target], linters: &[Linter]) -> Vec<Report> {
    std::thread::scope(|scope| {
        let runs: Vec<_> = targets
            .iter()
            .zip(linters)
            .map(|(target, linter)| scope.spawn(move || linter.run(&target.workspace).unwrap()))
            .collect();
        runs.into_iter().map(|run| run.join().unwrap()).collect()
    })
}

#[test]
fn the_linters_of_several_roots_parse_a_file_they_share_once() {
    let fs = shared_closure();
    let discovery = discover_targets(Path::new(ROOT), &fs, &ConfigOverrides::default()).unwrap();
    assert_eq!(discovery.targets.len(), 3);
    let store = Arc::new(Parses::default());
    let linters = Linter::for_targets_with_cache(
        &discovery.targets,
        Arc::new(fs.clone()),
        Arc::clone(&store) as Arc<dyn ModuleCache>,
    )
    .unwrap();
    lint_in_parallel(&discovery.targets, &linters);
    let stores = store.stores.lock().unwrap();
    // The three entry files and the shared file, each parsed by one root only.
    assert_eq!(stores.len(), 4, "{stores:?}");
    assert!(stores.values().all(|count| *count == 1), "{stores:?}");
}

#[test]
fn linters_for_targets_report_like_a_linter_per_target() {
    let fs = shared_closure();
    let discovery = discover_targets(Path::new(ROOT), &fs, &ConfigOverrides::default()).unwrap();
    let separate: Vec<Linter> = discovery
        .targets
        .iter()
        .map(|target| Linter::with_fs(&target.config, Arc::new(fs.clone())).unwrap())
        .collect();
    let shared = Linter::for_targets(&discovery.targets, Arc::new(fs.clone())).unwrap();
    assert_eq!(shared.len(), 3);
    let expected = lint_in_parallel(&discovery.targets, &separate);
    assert!(
        expected.iter().all(|report| report.summary.packages == 1),
        "{expected:#?}"
    );
    assert_eq!(lint_in_parallel(&discovery.targets, &shared), expected);
}

#[test]
fn only_several_targets_or_a_host_cache_share_a_module_cache() {
    let fs = shared_closure();
    let discovery = discover_targets(Path::new(ROOT), &fs, &ConfigOverrides::default()).unwrap();
    let host = || Some(Arc::new(MemoryModuleCache::new()) as Arc<dyn ModuleCache>);
    assert!(Linter::shared_modules(&discovery.targets, None).is_some());
    assert!(Linter::shared_modules(&discovery.targets[..1], None).is_none());
    assert!(Linter::shared_modules(&discovery.targets[..1], host()).is_some());
}
