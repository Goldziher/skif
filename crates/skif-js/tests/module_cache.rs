//! The module cache of ADR 0029: lookups per run, unchanged results, and parse results that survive serialization.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use skif_config::Config;
use skif_core::{Diagnostic, FileSystem, MemoryFs};
use skif_js::{CachedModule, JsProvider, MemoryModuleCache, ModuleCache, ModuleKey};
use skif_js_syntax::{ModuleInfo, ParseOptions, is_stylesheet, parse_module, parse_stylesheet};
use skif_lint::{Registry, RunOptions, run_unsorted};
use skif_workspace::Workspace;

/// A [`MemoryModuleCache`] that counts the lookups that hit and miss.
#[derive(Default)]
struct Counting {
    inner: MemoryModuleCache,
    hits: AtomicUsize,
    misses: AtomicUsize,
}

impl Counting {
    /// The hits and misses since the last call.
    fn take(&self) -> (usize, usize) {
        (
            self.hits.swap(0, Ordering::Relaxed),
            self.misses.swap(0, Ordering::Relaxed),
        )
    }
}

impl ModuleCache for Counting {
    fn get(&self, key: ModuleKey) -> Option<CachedModule> {
        let found = self.inner.get(key);
        let counter = if found.is_some() { &self.hits } else { &self.misses };
        counter.fetch_add(1, Ordering::Relaxed);
        found
    }

    fn put(&self, key: ModuleKey, module: CachedModule) {
        self.inner.put(key, module);
    }
}

/// A workspace whose five source files all differ, so no two share a cache entry.
fn project() -> MemoryFs {
    MemoryFs::new()
        .with_file(
            "/ws/package.json",
            r#"{"name": "root", "private": true, "workspaces": ["packages/*"]}"#,
        )
        .with_file(
            "/ws/packages/app/package.json",
            r#"{"name": "app", "private": true, "main": "src/index.ts", "dependencies": {"is-number": "7.0.0"}}"#,
        )
        .with_file(
            "/ws/packages/app/src/index.ts",
            "import { used } from './util';\nimport 'left-pad';\nexport const main = used();\n",
        )
        .with_file(
            "/ws/packages/app/src/util.ts",
            "export const used = () => 1;\nexport const unused = 2;\n",
        )
        .with_file("/ws/packages/app/src/orphan.ts", "export const orphan = 3;\n")
        .with_file("/ws/packages/app/src/style.css", "@import './base.css';\n")
        .with_file("/ws/packages/app/src/types.d.ts", "declare module 'virtual:x' {}\n")
}

fn lint(fs: &MemoryFs, modules: Option<Arc<dyn ModuleCache>>) -> Vec<Diagnostic> {
    let config = Config::default();
    let workspace = Workspace::discover_with_fs(Path::new("/ws"), &config, fs).unwrap();
    let mut provider = JsProvider::new(Arc::new(fs.clone()) as Arc<dyn FileSystem>);
    if let Some(modules) = modules {
        provider = provider.with_module_cache(modules);
    }
    let registry = Registry::new(vec![Box::new(provider)]).unwrap();
    let mut diagnostics = run_unsorted(&registry, &workspace, &config, &RunOptions::default()).unwrap();
    diagnostics.sort_by(|a, b| {
        (a.rule.as_str(), &a.location.path, &a.message).cmp(&(b.rule.as_str(), &b.location.path, &b.message))
    });
    diagnostics
}

#[test]
fn a_warm_cache_hits_every_module_and_an_edit_misses_only_the_edited_file() {
    let cache = Arc::new(Counting::default());
    let fs = project();
    let uncached = lint(&fs, None);
    assert_ne!(uncached, []);

    let cold = lint(&fs, Some(Arc::clone(&cache) as Arc<dyn ModuleCache>));
    let (hits, misses) = cache.take();
    assert_eq!(hits, 0);
    assert!(misses >= 3, "{misses}");
    assert_eq!(cold, uncached);

    let warm = lint(&fs, Some(Arc::clone(&cache) as Arc<dyn ModuleCache>));
    assert_eq!(cache.take(), (misses, 0));
    assert_eq!(warm, uncached);

    let mut edited = fs.clone();
    edited.insert(
        "/ws/packages/app/src/util.ts",
        "export const used = () => 1;\nexport const unused = 2;\nexport const another = 3;\n",
    );
    let after_edit = lint(&edited, Some(Arc::clone(&cache) as Arc<dyn ModuleCache>));
    assert_eq!(cache.take(), (misses - 1, 1));
    assert_eq!(after_edit, lint(&edited, None));
}

fn files_below(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            files_below(&path, out);
        } else {
            out.push(path);
        }
    }
}

#[test]
fn the_parse_result_of_every_fixture_source_round_trips_through_json() {
    let mut files = Vec::new();
    files_below(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures"),
        &mut files,
    );
    let mut parsed = 0;
    for path in files {
        let Ok(source) = std::fs::read_to_string(&path) else {
            continue;
        };
        let info: ModuleInfo = if is_stylesheet(&path) {
            parse_stylesheet(&path, &source)
        } else {
            match parse_module(&path, &source, &ParseOptions::default()) {
                Ok(info) => info,
                Err(_) => continue,
            }
        };
        let json = serde_json::to_string(&info).unwrap();
        let back: ModuleInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(back, info, "{}", path.display());
        parsed += 1;
    }
    assert!(parsed > 100, "only {parsed} fixture sources parsed");
}
