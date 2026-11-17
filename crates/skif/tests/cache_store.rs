//! ADR 0007 through the facade: whatever the cache store of a linter holds or returns, the report is the same.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use skif::cache::{CacheCounters, CacheNamespace, CacheStore, MODULES, MemoryStore};
use skif::{ConfigOverrides, FileSystem, Linter, MemoryFs, Report, Workspace, load_config};

// The resolver only applies a `tsconfig` found below an absolute path, and `/ws` has no drive on Windows.
const ROOT: &str = if cfg!(windows) { "C:/ws" } else { "/ws" };
const ELSEWHERE: &str = if cfg!(windows) {
    "C:/other/checkout"
} else {
    "/other/checkout"
};

/// The JS analysis fixtures of `skif-js`, each copied into memory below `root`.
fn fixtures(root: &str) -> Vec<(String, MemoryFs)> {
    fn load(dir: &Path, base: &Path, root: &str, fs: &mut MemoryFs) {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                load(&path, base, root, fs);
            } else if let Ok(text) = std::fs::read_to_string(&path) {
                let rel = path.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/");
                fs.insert(Path::new(root).join(rel), text);
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
            load(&case, &case, root, &mut fs);
            (case.file_name().unwrap().to_string_lossy().into_owned(), fs)
        })
        .collect()
}

/// The report of the workspace at `root` read through `fs`, cached through `store` when given.
fn report(root: &str, fs: &Arc<dyn FileSystem>, store: Option<Arc<dyn CacheStore>>, salt: &[u8]) -> Option<Report> {
    let config = load_config(Path::new(root), &**fs, &ConfigOverrides::default()).ok()?;
    let workspace = Workspace::discover_with_fs(Path::new(root), &config, &**fs).ok()?;
    let linter = Linter::with_fs(&config, Arc::clone(fs)).unwrap();
    let linter = match store {
        Some(store) => linter.with_cache_store(store).with_cache_salt(salt),
        None => linter,
    };
    Some(linter.run(&workspace).unwrap())
}

/// The report as a host prints it, and the report without its cache counters: the two things a store must not change.
fn printed(report: &Report) -> (String, Report) {
    let mut bare = report.clone();
    bare.cache.clear();
    (serde_json::to_string(report).unwrap(), bare)
}

fn modules(report: &Report) -> CacheCounters {
    report.cache.get(MODULES.name).copied().unwrap_or_default()
}

#[test]
fn no_store_a_cold_store_and_a_warm_store_report_alike_and_the_warm_run_only_hits() {
    let mut linted = 0;
    let mut cached = 0;
    for (name, fs) in fixtures(ROOT) {
        let fs: Arc<dyn FileSystem> = Arc::new(fs);
        let Some(uncached) = report(ROOT, &fs, None, b"") else {
            continue;
        };
        assert!(uncached.cache.is_empty(), "{name}: counters without a store");
        let store = Arc::new(MemoryStore::new());
        let cold = report(ROOT, &fs, Some(store.clone()), b"").unwrap();
        let warm = report(ROOT, &fs, Some(store.clone()), b"").unwrap();
        assert_eq!(printed(&cold), printed(&uncached), "{name}: cold");
        assert_eq!(printed(&warm), printed(&uncached), "{name}: warm");
        linted += 1;

        let (cold, warm) = (modules(&cold), modules(&warm));
        assert_eq!(cold.hits + cold.misses, warm.hits + warm.misses, "{name}: lookups");
        // A file that does not parse is never stored, so it misses on every run.
        let stored = store.len() as u64;
        assert!(cold.misses >= stored, "{name}: {cold:?}");
        assert_eq!(warm.bytes_written, 0, "{name}: a warm run wrote");
        // Files with one name and one text share a key: parsed in parallel on the cold run, each may miss before the
        // first is stored, so the cold run can miss more often than it stored and the warm run then hits them all.
        assert!(warm.misses <= cold.misses - stored, "{name}: {warm:?} after {cold:?}");
        if stored > 0 {
            assert!(cold.bytes_written > 0, "{name}");
            assert!(warm.hits >= stored, "{name}: {warm:?}");
            cached += 1;
        }
    }
    assert!(linted >= 20, "only {linted} fixtures linted");
    assert!(cached >= 20, "only {cached} fixtures stored an entry");
}

#[test]
fn a_warm_run_of_a_project_whose_files_all_parse_is_all_hits() {
    let fs: Arc<dyn FileSystem> = Arc::new(project(ROOT));
    let store = Arc::new(MemoryStore::new());
    let cold = modules(&report(ROOT, &fs, Some(store.clone()), b"").unwrap());
    assert_eq!(cold.hits, 0);
    // The entry file and the module it imports are parsed; the orphan is reported without being parsed.
    assert_eq!(cold.misses, PARSED, "{cold:?}");
    assert!(cold.bytes_written > 0);
    let warm = modules(&report(ROOT, &fs, Some(store.clone()), b"").unwrap());
    assert_eq!(warm, CacheCounters::new(PARSED, 0, 0));
    assert_eq!(store.len() as u64, PARSED);
}

/// The files of [`project`] a run parses.
const PARSED: u64 = 2;

fn project(root: &str) -> MemoryFs {
    let root = Path::new(root);
    MemoryFs::new()
        .with_file(
            root.join("package.json"),
            r#"{"name": "app", "version": "1.0.0", "main": "src/index.ts", "dependencies": {"left-pad": "1.0.0"}}"#,
        )
        .with_file(
            root.join("src/index.ts"),
            "import pad from 'left-pad';\nimport { used } from './util';\nexport default pad(used());\n",
        )
        .with_file(
            root.join("src/util.ts"),
            "export const used = () => '1';\nexport const unused = 2;\n",
        )
        .with_file(root.join("src/orphan.ts"), "export const orphan = 3;\n")
}

#[test]
fn the_same_tree_at_another_absolute_path_hits_every_entry() {
    let here: Arc<dyn FileSystem> = Arc::new(project(ROOT));
    let there: Arc<dyn FileSystem> = Arc::new(project(ELSEWHERE));
    let store = Arc::new(MemoryStore::new());
    let first = report(ROOT, &here, Some(store.clone()), b"").unwrap();
    assert_eq!(modules(&first).hits, 0);
    let second = report(ELSEWHERE, &there, Some(store.clone()), b"").unwrap();
    assert_eq!(modules(&second), CacheCounters::new(PARSED, 0, 0));
    assert_eq!(printed(&second), printed(&first));
    assert_eq!(
        printed(&second),
        printed(&report(ELSEWHERE, &there, None, b"").unwrap())
    );
}

#[test]
fn another_salt_reads_nothing_the_first_wrote() {
    let fs: Arc<dyn FileSystem> = Arc::new(project(ROOT));
    let store = Arc::new(MemoryStore::new());
    let first = report(ROOT, &fs, Some(store.clone()), b"one").unwrap();
    let other = report(ROOT, &fs, Some(store.clone()), b"two").unwrap();
    assert_eq!(modules(&other).hits, 0);
    assert_eq!(printed(&other).1, printed(&first).1);
    assert_eq!(modules(&report(ROOT, &fs, Some(store), b"one").unwrap()).misses, 0);
}

#[test]
fn an_edit_misses_once_and_the_report_follows_it() {
    let store = Arc::new(MemoryStore::new());
    let before: Arc<dyn FileSystem> = Arc::new(project(ROOT));
    report(ROOT, &before, Some(store.clone()), b"").unwrap();
    let mut edited = project(ROOT);
    edited.insert(
        Path::new(ROOT).join("src/util.ts"),
        "export const used = () => '1';\nexport const unused = 2;\nexport const another = 3;\n",
    );
    let edited: Arc<dyn FileSystem> = Arc::new(edited);
    let cached = report(ROOT, &edited, Some(store), b"").unwrap();
    assert_eq!(
        modules(&cached),
        CacheCounters::new(PARSED - 1, 1, modules(&cached).bytes_written)
    );
    let uncached = report(ROOT, &edited, None, b"").unwrap();
    assert_eq!(printed(&cached), printed(&uncached));
    assert_ne!(
        printed(&cached).0,
        printed(&report(ROOT, &before, None, b"").unwrap()).0
    );
}

/// How an [`Adversarial`] store answers a lookup.
#[derive(Debug, Clone, Copy)]
enum Damage {
    /// The value stored under another key.
    OtherKey,
    /// The right value with one byte flipped, at a place that moves with the key.
    Flipped,
    /// The right value cut short.
    Truncated,
    /// Bytes that were never a value.
    Random,
    /// An empty value.
    Empty,
}

/// A store that keeps what it is given and answers every lookup with damage.
struct Adversarial {
    inner: MemoryStore,
    damage: Damage,
}

impl CacheStore for Adversarial {
    fn get(&self, namespace: CacheNamespace, key: &[u8; 32]) -> Option<Arc<[u8]>> {
        let entries = self.inner.entries();
        let seed = usize::from(key[0]) << 8 | usize::from(key[1]);
        let own = || self.inner.get(namespace, key).map(|value| value.to_vec());
        let bytes = match self.damage {
            Damage::OtherKey => entries.iter().find(|(_, other, _)| other != key)?.2.to_vec(),
            Damage::Flipped => {
                let mut bytes = own()?;
                let at = seed % bytes.len();
                bytes[at] ^= 1 << (seed % 8);
                bytes
            }
            Damage::Truncated => {
                let mut bytes = own()?;
                bytes.truncate(seed % bytes.len());
                bytes
            }
            Damage::Random => {
                let mut state = u64::from_le_bytes(key[..8].try_into().unwrap()) | 1;
                (0..seed % 2048)
                    .map(|_| {
                        state ^= state << 13;
                        state ^= state >> 7;
                        state ^= state << 17;
                        state.to_le_bytes()[0]
                    })
                    .collect()
            }
            Damage::Empty => Vec::new(),
        };
        Some(Arc::from(bytes))
    }

    fn put(&self, namespace: CacheNamespace, key: &[u8; 32], value: Arc<[u8]>) {
        self.inner.put(namespace, key, value);
    }
}

#[test]
fn a_store_that_returns_damage_reports_like_no_store() {
    let mut linted = 0;
    for (name, fs) in fixtures(ROOT) {
        let fs: Arc<dyn FileSystem> = Arc::new(fs);
        let Some(uncached) = report(ROOT, &fs, None, b"") else {
            continue;
        };
        for damage in [
            Damage::OtherKey,
            Damage::Flipped,
            Damage::Truncated,
            Damage::Random,
            Damage::Empty,
        ] {
            let store = Arc::new(Adversarial {
                inner: MemoryStore::new(),
                damage,
            });
            // The first run fills the store; the second is answered with damage on every lookup.
            for run in ["cold", "warm"] {
                let damaged = report(ROOT, &fs, Some(store.clone()), b"").unwrap();
                assert_eq!(printed(&damaged), printed(&uncached), "{name}: {damage:?}, {run}");
                assert_eq!(
                    modules(&damaged).hits,
                    0,
                    "{name}: {damage:?}, {run}: a damaged value was a hit"
                );
            }
        }
        linted += 1;
    }
    assert!(linted >= 20, "only {linted} fixtures linted");
}
