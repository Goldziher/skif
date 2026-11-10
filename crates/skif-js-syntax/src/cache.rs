//! A cache of parse results keyed by everything a parse depends on (ADR 0066).
//!
//! A [`ModuleInfo`] is a pure function of the file name, the text and the [`ParseOptions`], so a key over those
//! inputs can never return a stale result: a changed file has a new key. The cache itself is supplied by the host
//! through [`ModuleCache`]; this crate only owns the key, the stored value and an in-memory implementation.

use std::collections::{BTreeMap, BTreeSet};
use std::hash::Hasher as _;
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

use serde::{Deserialize, Serialize};
use siphasher::sip128::{Hasher128 as _, SipHasher13};

use crate::error::SyntaxError;
use crate::model::{ModuleInfo, ParseOptions};
use crate::style::{is_stylesheet, parse_stylesheet};

/// The version of the parse output. Raise it whenever [`ModuleInfo`] or what the parsers put in it changes, so a
/// cache written by an older build at the same crate version is not trusted; the test
/// `the_cache_schema_matches_the_parse_output` fails until [`CACHE_SCHEMA_DIGEST`] is updated with it.
pub const CACHE_SCHEMA: u32 = 3;

/// The digest of the serialized parse results of a fixed probe corpus at [`CACHE_SCHEMA`]. It is a test's guard,
/// not part of any key: when the parse output changes, the test reports the new digest and both constants move.
pub const CACHE_SCHEMA_DIGEST: u128 = 0x4d7a_4f2c_aa76_8aea_43af_7cad_cc2b_d9de;

/// The version a persisted cache records, `<crate version>+<CACHE_SCHEMA>`: a store written under another version
/// is discarded as a whole.
#[must_use]
pub fn cache_version() -> String {
    format!("{}+{CACHE_SCHEMA}", env!("CARGO_PKG_VERSION"))
}

/// The key of one parse result: a 128-bit `SipHash-1-3` of the schema, the crate version, the parser, the parse
/// options, the file name and the text. The directory is not part of it, so identical files share an entry and a
/// moved checkout keeps its cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ModuleKey(u128);

impl ModuleKey {
    /// The key for parsing `source`, the text of `path`, with `options`.
    #[must_use]
    pub fn new(path: &Path, source: &str, options: &ParseOptions) -> Self {
        // Destructured without `..`, so a new parse option does not compile until it is part of the key.
        let ParseOptions {
            in_file_references,
            member_accesses,
        } = *options;
        let mut hasher = SipHasher13::new_with_keys(0, 0);
        hasher.write(&CACHE_SCHEMA.to_le_bytes());
        write_field(&mut hasher, env!("CARGO_PKG_VERSION").as_bytes());
        hasher.write_u8(u8::from(is_stylesheet(path)));
        hasher.write_u8(u8::from(in_file_references));
        hasher.write_u8(u8::from(member_accesses));
        write_field(
            &mut hasher,
            path.file_name()
                .map(std::ffi::OsStr::as_encoded_bytes)
                .unwrap_or_default(),
        );
        write_field(&mut hasher, source.as_bytes());
        Self(hasher.finish128().as_u128())
    }

    /// The key as 16 little-endian bytes, for a host that persists it.
    #[must_use]
    pub const fn to_bytes(self) -> [u8; 16] {
        self.0.to_le_bytes()
    }

    /// A key from the bytes [`ModuleKey::to_bytes`] gave.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(u128::from_le_bytes(bytes))
    }
}

/// Writes a variable-length field with its length first, so two different splits of the same bytes never hash alike.
fn write_field(hasher: &mut SipHasher13, bytes: &[u8]) {
    hasher.write(&(bytes.len() as u64).to_le_bytes());
    hasher.write(bytes);
}

/// One cached parse result. Its fields are private: a host stores and serializes it without depending on the shape
/// of [`ModuleInfo`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CachedModule(ModuleInfo);

/// A store of parse results the host owns and gives the analysis.
///
/// Both methods are called from the parallel parse step. A failed lookup, or a stored value the implementation
/// cannot decode, is a miss (`None`), never an error.
pub trait ModuleCache: Send + Sync {
    /// The result stored under `key`, if any.
    fn get(&self, key: ModuleKey) -> Option<CachedModule>;
    /// Stores `module` under `key`.
    fn put(&self, key: ModuleKey, module: CachedModule);

    /// The result stored under `key`, or else the one `parse` returns, stored before it is returned. `parse`
    /// returns `None` for a file it cannot parse, which is not stored. [`parse_cached`] goes through this method, so
    /// an implementation that coordinates concurrent parses of one key ([`CoalescingModuleCache`]) overrides it; a
    /// wrapper that forwards to another cache forwards this method too.
    fn get_or_insert_with(
        &self,
        key: ModuleKey,
        parse: &mut dyn FnMut() -> Option<CachedModule>,
    ) -> Option<CachedModule> {
        if let Some(found) = self.get(key) {
            return Some(found);
        }
        let module = parse()?;
        self.put(key, module.clone());
        Some(module)
    }
}

/// An in-process [`ModuleCache`] for long-lived hosts (watch mode, the language server, the MCP server).
///
/// Every entry carries a mark set when it is read or written; [`MemoryModuleCache::prune_unused`] drops the entries
/// without a mark and clears the marks, so a host that prunes after each run keeps only what its last run used.
#[derive(Debug, Default)]
pub struct MemoryModuleCache {
    entries: Mutex<BTreeMap<ModuleKey, (Arc<CachedModule>, bool)>>,
}

impl MemoryModuleCache {
    /// An empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The number of entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner).len()
    }

    /// Whether the cache holds no entry.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Drops every entry not read or written since the last prune, and clears the marks of the others.
    pub fn prune_unused(&self) {
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        entries.retain(|_, (_, used)| std::mem::replace(used, false));
    }
}

impl ModuleCache for MemoryModuleCache {
    fn get(&self, key: ModuleKey) -> Option<CachedModule> {
        let found = {
            let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
            let (module, used) = entries.get_mut(&key)?;
            *used = true;
            Arc::clone(module)
        };
        // Cloned outside the lock, so parallel lookups do not wait on each other's copies.
        Some((*found).clone())
    }

    fn put(&self, key: ModuleKey, module: CachedModule) {
        self.entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key, (Arc::new(module), true));
    }
}

/// A [`ModuleCache`] in front of another that parses each key once while several threads ask for it: a lookup of a
/// key another thread is parsing waits for that parse and returns its result instead of parsing the file again.
///
/// The roots of a multi-root run are linted in parallel, and files outside them that several roots import are
/// reached by all of them at about the same time; without this, each of those lookups misses and parses. It holds
/// only the keys being parsed, never an entry: the results live in the cache it wraps, once each. A parse that fails
/// or panics releases its key, and the next lookup parses again.
pub struct CoalescingModuleCache {
    inner: Arc<dyn ModuleCache>,
    parsing: Mutex<BTreeSet<ModuleKey>>,
    parsed: Condvar,
}

impl CoalescingModuleCache {
    /// Coalesces the parses that go through `inner`.
    #[must_use]
    pub fn new(inner: Arc<dyn ModuleCache>) -> Self {
        Self {
            inner,
            parsing: Mutex::new(BTreeSet::new()),
            parsed: Condvar::new(),
        }
    }

    fn parsing(&self) -> MutexGuard<'_, BTreeSet<ModuleKey>> {
        self.parsing.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The result stored under `key` once no other thread parses it, or `None` after claiming the key for the
    /// calling thread, which then parses it.
    fn wait_or_claim(&self, key: ModuleKey) -> Option<CachedModule> {
        let mut parsing = self.parsing();
        while parsing.contains(&key) {
            parsing = self.parsed.wait(parsing).unwrap_or_else(PoisonError::into_inner);
        }
        // Looked up again under the lock: a parse that ended since the caller's lookup stored its result before it
        // released the key.
        let found = self.inner.get(key);
        if found.is_none() {
            parsing.insert(key);
        }
        found
    }
}

impl std::fmt::Debug for CoalescingModuleCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CoalescingModuleCache")
            .field("parsing", &self.parsing().len())
            .finish_non_exhaustive()
    }
}

/// Releases a key claimed by [`CoalescingModuleCache::wait_or_claim`] and wakes the threads waiting for it, also when
/// the parse panics.
struct Claim<'a> {
    cache: &'a CoalescingModuleCache,
    key: ModuleKey,
}

impl Drop for Claim<'_> {
    fn drop(&mut self) {
        self.cache.parsing().remove(&self.key);
        self.cache.parsed.notify_all();
    }
}

impl ModuleCache for CoalescingModuleCache {
    fn get(&self, key: ModuleKey) -> Option<CachedModule> {
        self.inner.get(key)
    }

    fn put(&self, key: ModuleKey, module: CachedModule) {
        self.inner.put(key, module);
    }

    fn get_or_insert_with(
        &self,
        key: ModuleKey,
        parse: &mut dyn FnMut() -> Option<CachedModule>,
    ) -> Option<CachedModule> {
        if let Some(found) = self.inner.get(key) {
            return Some(found);
        }
        if let Some(found) = self.wait_or_claim(key) {
            return Some(found);
        }
        let _claim = Claim { cache: self, key };
        let module = parse()?;
        self.inner.put(key, module.clone());
        Some(module)
    }
}

/// Parses `source`, the text of `path`, with `options`, through `cache`: a stored result is returned as it is, and
/// a fresh parse is stored before it is returned. Stylesheets go to [`parse_stylesheet`] and everything else to
/// [`crate::parse_module`]. Without a cache this is a plain parse.
///
/// # Errors
/// As [`crate::parse_module`]. An unsupported file is never stored.
pub fn parse_cached(
    cache: Option<&dyn ModuleCache>,
    path: &Path,
    source: &str,
    options: &ParseOptions,
) -> Result<ModuleInfo, SyntaxError> {
    let parse = || {
        if is_stylesheet(path) {
            Ok(parse_stylesheet(path, source))
        } else {
            crate::parse_module(path, source, options)
        }
    };
    let Some(cache) = cache else {
        return parse();
    };
    let key = ModuleKey::new(path, source, options);
    let mut failure = None;
    let found = cache.get_or_insert_with(key, &mut || match parse() {
        Ok(info) => Some(CachedModule(info)),
        Err(error) => {
            failure = Some(error);
            None
        }
    });
    match (found, failure) {
        (Some(CachedModule(info)), _) => Ok(info),
        (None, Some(error)) => Err(error),
        // A cache that answered nothing without calling `parse`.
        (None, None) => parse(),
    }
}

const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ModuleKey>();
    assert_send_sync::<CachedModule>();
    assert_send_sync::<MemoryModuleCache>();
    assert_send_sync::<CoalescingModuleCache>();
};

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// One source per language the parsers know and one of test-runner mocks, plus the bench corpus: the probe whose
    /// parse output the schema digest pins.
    fn probe_corpus() -> Vec<(PathBuf, String)> {
        let inline: [(&str, &str); 16] = [
            (
                "a.ts",
                "import { a, type B } from './b';\nimport * as ns from 'ns';\nexport const c = a + ns.x;\n\
                 export enum E { A, B }\nexport function f(): B { return E.A as never; }\n\
                 export default class K {}\n",
            ),
            (
                "b.tsx",
                "'use client';\nimport React from 'react';\nexport const App = () => <div>{React.version}</div>;\n",
            ),
            (
                "c.js",
                "const x = require('x');\nmodule.exports = { x };\nimport('./lazy');\nimport(`./dir/${x}`);\n",
            ),
            (
                "d.d.ts",
                "declare module 'virtual:thing' {\n  export const v: number;\n}\n",
            ),
            (
                "e.mjs",
                "export * from './e2';\nexport { default as g } from './g';\nimport.meta.glob('./*.js');\n",
            ),
            (
                "f.vue",
                "<script setup lang=\"ts\">\nimport Child from './Child.vue';\nimport { ref } from 'vue';\n\
                 const n = ref(0);\n</script>\n<template><Child :n=\"n\" /><app-card v-for=\"item in items\" />\
                 <component is=\"AppBadge\" /></template>\n<style lang=\"scss\">@use './theme';</style>\n",
            ),
            (
                "g.svelte",
                "<script>\n  import Button from './Button.svelte';\n  export let label;\n</script>\n\
                 <Button>{label}</Button>\n",
            ),
            (
                "h.astro",
                "---\nimport Layout from '../layouts/Layout.astro';\nconst title = 'x';\n---\n\
                 <Layout title={title}><p>hi</p></Layout>\n",
            ),
            (
                "i.mdx",
                "import { Chart } from './chart';\nexport const meta = { title: 'x' };\n\n# Title\n\n<Chart />\n",
            ),
            (
                "j.tsrx",
                "import { thing } from './thing';\nexport component App() { <div>{thing}</div> }\n",
            ),
            (
                "k.css",
                "@import './base.css';\n.a { composes: b from './other.css'; }\n",
            ),
            (
                "l.scss",
                "@use 'sass:math';\n@use './vars' as v;\n@forward './mixins';\n.a { b: v.$c; }\n",
            ),
            (
                "n.marko",
                "import format from './format';\nimport Card from '<card>';\n<div>${format(input.value)}</div>\n\
                 style.less {\n  @import './theme.less';\n}\n",
            ),
            (
                "m.test.ts",
                "vi.mock('./x');\nvi.mock(import('./y'), () => ({}));\njest.requireActual('./z');\n\
                 jest.mock('v', () => ({}), { virtual: true });\n",
            ),
            (
                "n.ts",
                "import * as ns from './ns';\nimport * as chars from './chars';\n\
                 export const read = ns.E.A + chars.a;\nexport function scan(chars: string) { return chars.length; }\n\
                 export async function lazy() { const ns = await import('./ns'); return ns.b; }\n\
                 export interface Options { a: string }\nexport const Service = make();\n\
                 export type Service = { run(options: Options): void };\n",
            ),
            (
                "o.mjs",
                "import { execSync, spawn } from 'node:child_process';\nimport path from 'node:path';\n\
                 execSync('npm run build', { cwd: '/tmp/app' });\nspawn('tsc', ['-b'], { cwd: process.cwd() });\n\
                 execSync('pnpm exec rspack', { cwd: path.join(import.meta.dirname, '../sub') });\n",
            ),
        ];
        let mut corpus: Vec<(PathBuf, String)> = inline
            .iter()
            .map(|(name, source)| (PathBuf::from(name), (*source).to_owned()))
            .collect();
        let bench = Path::new(env!("CARGO_MANIFEST_DIR")).join("benches").join("corpus");
        let mut files: Vec<PathBuf> = std::fs::read_dir(&bench)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        files.sort();
        assert!(!files.is_empty(), "the bench corpus is empty");
        for file in files {
            let source = std::fs::read_to_string(&file).unwrap();
            corpus.push((PathBuf::from(file.file_name().unwrap()), source));
        }
        // A file the parser cannot recover from is cached too.
        corpus.push((PathBuf::from("broken.ts"), "export const = ;;; {{{".to_owned()));
        corpus
    }

    fn key(name: &str, source: &str, options: ParseOptions) -> ModuleKey {
        ModuleKey::new(Path::new(name), source, &options)
    }

    #[test]
    fn the_key_changes_with_each_input() {
        let options = ParseOptions::default();
        let base = key("a.ts", "export const a = 1;", options);
        let others = [
            key("a.ts", "export const a = 2;", options),
            key("b.ts", "export const a = 1;", options),
            key("a.d.ts", "export const a = 1;", options),
            key("a.tsx", "export const a = 1;", options),
            key(
                "a.ts",
                "export const a = 1;",
                ParseOptions {
                    in_file_references: false,
                    ..options
                },
            ),
            key(
                "a.ts",
                "export const a = 1;",
                ParseOptions {
                    member_accesses: false,
                    ..options
                },
            ),
        ];
        for other in others {
            assert_ne!(base, other);
        }
        // The parser: the same file name stem as a stylesheet and as a script.
        assert_ne!(key("a.css", "", options), key("a.js", "", options));
    }

    #[test]
    fn the_key_ignores_the_directory() {
        let options = ParseOptions::default();
        assert_eq!(
            ModuleKey::new(Path::new("/one/src/a.ts"), "x", &options),
            ModuleKey::new(Path::new("/two/lib/a.ts"), "x", &options)
        );
    }

    #[test]
    fn the_key_round_trips_through_bytes() {
        let key = key("a.ts", "x", ParseOptions::default());
        assert_eq!(ModuleKey::from_bytes(key.to_bytes()), key);
    }

    #[test]
    fn parse_results_round_trip_through_serde() {
        for (path, source) in probe_corpus() {
            let info = parse_cached(None, &path, &source, &ParseOptions::default()).unwrap();
            let cached = CachedModule(info);
            let json = serde_json::to_string(&cached).unwrap();
            let back: CachedModule = serde_json::from_str(&json).unwrap();
            assert_eq!(back, cached, "{}", path.display());
        }
    }

    #[test]
    fn the_cache_schema_matches_the_parse_output() {
        let mut hasher = SipHasher13::new_with_keys(0, 0);
        for (path, source) in probe_corpus() {
            for options in [
                ParseOptions::default(),
                ParseOptions {
                    in_file_references: false,
                    member_accesses: false,
                },
            ] {
                let info = parse_cached(None, &path, &source, &options).unwrap();
                write_field(&mut hasher, path.as_os_str().as_encoded_bytes());
                write_field(&mut hasher, &serde_json::to_vec(&info).unwrap());
            }
        }
        let digest = hasher.finish128().as_u128();
        assert_eq!(
            digest, CACHE_SCHEMA_DIGEST,
            "the parse output changed: raise CACHE_SCHEMA (now {CACHE_SCHEMA}) and set CACHE_SCHEMA_DIGEST to \
             {digest:#034x}"
        );
    }

    /// Counts the lookups that hit and the stores of a [`MemoryModuleCache`].
    #[derive(Default)]
    struct Counting {
        inner: MemoryModuleCache,
        hits: AtomicUsize,
        puts: AtomicUsize,
    }

    impl ModuleCache for Counting {
        fn get(&self, key: ModuleKey) -> Option<CachedModule> {
            let found = self.inner.get(key);
            if found.is_some() {
                self.hits.fetch_add(1, Ordering::Relaxed);
            }
            found
        }

        fn put(&self, key: ModuleKey, module: CachedModule) {
            self.puts.fetch_add(1, Ordering::Relaxed);
            self.inner.put(key, module);
        }
    }

    #[test]
    fn parse_cached_stores_on_a_miss_and_returns_the_stored_result_on_a_hit() {
        let cache = Counting::default();
        let options = ParseOptions::default();
        let path = Path::new("a.ts");
        let first = parse_cached(Some(&cache), path, "import x from 'x'; export { x };", &options).unwrap();
        let second = parse_cached(Some(&cache), path, "import x from 'x'; export { x };", &options).unwrap();
        assert_eq!(first, second);
        assert_eq!(cache.puts.load(Ordering::Relaxed), 1);
        assert_eq!(cache.hits.load(Ordering::Relaxed), 1);
        // An unsupported file is an error and is not stored.
        assert!(parse_cached(Some(&cache), Path::new("a.txt"), "", &options).is_err());
        assert_eq!(cache.puts.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn prune_unused_keeps_only_the_entries_used_since_the_last_prune() {
        let cache = MemoryModuleCache::new();
        let options = ParseOptions::default();
        let (a, b) = (key("a.ts", "a", options), key("b.ts", "b", options));
        cache.put(a, CachedModule(ModuleInfo::default()));
        cache.put(b, CachedModule(ModuleInfo::default()));
        assert_eq!(cache.len(), 2);
        // Both were written since the last prune.
        cache.prune_unused();
        assert_eq!(cache.len(), 2);
        assert!(cache.get(a).is_some());
        cache.prune_unused();
        assert_eq!(cache.len(), 1);
        assert!(cache.get(a).is_some());
        assert!(cache.get(b).is_none());
        cache.prune_unused();
        cache.prune_unused();
        assert!(cache.is_empty());
    }

    #[test]
    fn parse_options_that_differ_do_not_share_an_entry() {
        let cache = Counting::default();
        let path = Path::new("a.ts");
        let source = "const local = 1;\nexport const a = local;\nexport const b = a;\n";
        let full = ParseOptions::default();
        let bare = ParseOptions {
            in_file_references: false,
            member_accesses: false,
        };
        let with_references = parse_cached(Some(&cache), path, source, &full).unwrap();
        let without = parse_cached(Some(&cache), path, source, &bare).unwrap();
        assert_eq!(cache.puts.load(Ordering::Relaxed), 2);
        assert_eq!(cache.hits.load(Ordering::Relaxed), 0);
        assert_eq!(cache.inner.len(), 2);
        // Each lookup is served what its own options parse to, from the entry of those options.
        assert_eq!(
            parse_cached(Some(&cache), path, source, &full).unwrap(),
            with_references
        );
        assert_eq!(parse_cached(Some(&cache), path, source, &bare).unwrap(), without);
        assert_eq!(with_references, parse_cached(None, path, source, &full).unwrap());
        assert_eq!(without, parse_cached(None, path, source, &bare).unwrap());
        assert_eq!(cache.hits.load(Ordering::Relaxed), 2);
    }

    fn empty_module() -> CachedModule {
        CachedModule(ModuleInfo::default())
    }

    #[test]
    fn a_lookup_of_a_key_being_parsed_waits_for_that_parse() {
        let store = Arc::new(Counting::default());
        let cache = CoalescingModuleCache::new(Arc::clone(&store) as Arc<dyn ModuleCache>);
        let key = key("a.ts", "a", ParseOptions::default());
        let parses = AtomicUsize::new(0);
        let (started, has_started) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel::<()>();
        let (cache, parses) = (&cache, &parses);
        std::thread::scope(|scope| {
            let first = scope.spawn(move || {
                cache.get_or_insert_with(key, &mut || {
                    parses.fetch_add(1, Ordering::SeqCst);
                    started.send(()).unwrap();
                    released.recv().unwrap();
                    Some(empty_module())
                })
            });
            has_started.recv().unwrap();
            let second = scope.spawn(move || {
                cache.get_or_insert_with(key, &mut || {
                    parses.fetch_add(1, Ordering::SeqCst);
                    Some(empty_module())
                })
            });
            // Time for the second lookup to start a parse of its own if it did not wait.
            std::thread::sleep(std::time::Duration::from_millis(50));
            release.send(()).unwrap();
            assert_eq!(first.join().unwrap(), Some(empty_module()));
            assert_eq!(second.join().unwrap(), Some(empty_module()));
        });
        assert_eq!(parses.load(Ordering::SeqCst), 1);
        assert_eq!(store.puts.load(Ordering::Relaxed), 1);
        assert_eq!(store.hits.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn many_threads_asking_for_one_key_parse_it_once() {
        let store = Arc::new(MemoryModuleCache::new());
        let cache = CoalescingModuleCache::new(Arc::clone(&store) as Arc<dyn ModuleCache>);
        let key = key("a.ts", "a", ParseOptions::default());
        let parses = AtomicUsize::new(0);
        let barrier = std::sync::Barrier::new(8);
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    barrier.wait();
                    cache.get_or_insert_with(key, &mut || {
                        parses.fetch_add(1, Ordering::SeqCst);
                        std::thread::sleep(std::time::Duration::from_millis(20));
                        Some(empty_module())
                    })
                });
            }
        });
        assert_eq!(parses.load(Ordering::SeqCst), 1);
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn a_failed_or_panicking_parse_releases_its_key_and_stores_nothing() {
        let cache = CoalescingModuleCache::new(Arc::new(MemoryModuleCache::new()));
        let key = key("a.ts", "a", ParseOptions::default());
        assert_eq!(cache.get_or_insert_with(key, &mut || None), None);
        assert_eq!(cache.get(key), None);
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            cache.get_or_insert_with(key, &mut || -> Option<CachedModule> { panic!("the parser failed") })
        }));
        assert!(panicked.is_err());
        assert_eq!(cache.get(key), None);
        // Neither left the key claimed: this lookup parses instead of waiting forever.
        let mut parsed = false;
        let found = cache.get_or_insert_with(key, &mut || {
            parsed = true;
            Some(empty_module())
        });
        assert!(parsed);
        assert_eq!(found, Some(empty_module()));
        assert_eq!(cache.get(key), Some(empty_module()));
    }

    #[test]
    fn parse_cached_through_a_coalescing_cache_stores_once_and_reports_parse_errors() {
        let store = Arc::new(Counting::default());
        let cache = CoalescingModuleCache::new(Arc::clone(&store) as Arc<dyn ModuleCache>);
        let options = ParseOptions::default();
        let path = Path::new("a.ts");
        let first = parse_cached(Some(&cache), path, "export const a = 1;", &options).unwrap();
        let second = parse_cached(Some(&cache), path, "export const a = 1;", &options).unwrap();
        assert_eq!(first, second);
        assert_eq!(store.puts.load(Ordering::Relaxed), 1);
        assert!(matches!(
            parse_cached(Some(&cache), Path::new("a.txt"), "", &options),
            Err(SyntaxError::Unsupported { .. })
        ));
        assert_eq!(store.puts.load(Ordering::Relaxed), 1);
    }
}
