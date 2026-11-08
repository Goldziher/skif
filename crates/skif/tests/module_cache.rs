//! ADR 0066 through the facade: a module cache and an overlay of unsaved buffers change no report.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use skif::{
    BufferFs, ConfigOverrides, FileSystem, Linter, MemoryFs, MemoryModuleCache, ModuleCache, Report, Workspace,
    load_config,
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
