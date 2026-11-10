//! A cache of parse results keyed by everything a parse depends on (ADR 0066).
//!
//! A [`ModuleInfo`] is a pure function of the file name, the text and the [`ParseOptions`], so a key over those
//! inputs can never return a stale result: a changed file has a new key. The cache itself is supplied by the host
//! through [`ModuleCache`]; this crate only owns the key, the stored value, an in-memory implementation and the
//! [`ParseCoalescer`] that makes the threads of one run parse each key once (ADR 0100).

use std::collections::{BTreeMap, HashMap};
use std::hash::Hasher as _;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, ThreadId};

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

/// Parses each key once while several threads ask for it: a lookup of a key another thread is parsing waits for
/// that parse and is given its outcome instead of parsing the file again (ADR 0100).
///
/// It is not a [`ModuleCache`] and holds no result: it sits beside the cache a run parses through, and
/// [`ParseCoalescer::parse_cached`] takes that cache, so coalescing does not depend on how the cache is wrapped. It
/// holds one slot per key being parsed, with its own lock and condition variable, so a wait for one key neither
/// blocks nor wakes the lookups of another. The thread that claims a key parses it, stores the result in the cache
/// and hands the result, or the parse error, to the threads that waited; a parse that panics hands them nothing, and
/// one of them parses the key again.
///
/// Deadlock freedom rests on one invariant: a parse never waits for another lookup through the coalescer. The
/// parsers run on the calling thread, use no thread pool and do not read the cache. A lookup of a key the calling
/// thread has claimed itself parses inline instead of waiting for itself.
#[derive(Default)]
pub struct ParseCoalescer {
    slots: Mutex<HashMap<ModuleKey, Arc<Slot>>>,
}

/// The parse of one key in flight.
struct Slot {
    /// The thread parsing the key.
    claimer: ThreadId,
    /// The threads waiting for the outcome. It changes only under the lock of [`ParseCoalescer::slots`], so it is
    /// final once the claimer has taken the slot out of the map.
    waiters: AtomicUsize,
    outcome: Mutex<Option<Outcome>>,
    ready: Condvar,
}

/// What the claimer of a key hands the threads that waited for it.
#[derive(Clone)]
enum Outcome {
    Parsed(Arc<CachedModule>),
    Failed(SyntaxError),
    /// The parse panicked: the waiters look the key up again.
    Abandoned,
}

/// The answer of [`ParseCoalescer::join_or_claim`].
enum Turn<'a> {
    /// Another thread parses the key; wait for this slot.
    Wait(Arc<Slot>),
    /// The calling thread parses the key.
    Parse(Claim<'a>),
    /// The calling thread is already parsing the key further up its stack.
    Inline,
}

impl ParseCoalescer {
    /// A coalescer with no parse in flight.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// As [`parse_cached`] through `cache`, with a lookup of a key another thread is parsing waiting for that parse.
    ///
    /// # Errors
    /// As [`crate::parse_module`], also when the error comes from the parse another thread ran for the same key.
    pub fn parse_cached(
        &self,
        cache: &dyn ModuleCache,
        path: &Path,
        source: &str,
        options: &ParseOptions,
    ) -> Result<ModuleInfo, SyntaxError> {
        let key = ModuleKey::new(path, source, options);
        self.get_or_parse(cache, key, &mut || {
            parse_uncached(path, source, *options).map(CachedModule)
        })
        .map(|CachedModule(info)| info)
    }

    /// The result stored under `key` in `cache`, or the outcome of the one parse of `key` that runs while several
    /// threads ask for it. A fresh result is stored before any waiter is woken.
    fn get_or_parse(
        &self,
        cache: &dyn ModuleCache,
        key: ModuleKey,
        parse: &mut dyn FnMut() -> Result<CachedModule, SyntaxError>,
    ) -> Result<CachedModule, SyntaxError> {
        loop {
            if let Some(found) = cache.get(key) {
                return Ok(found);
            }
            let slot = match self.join_or_claim(key) {
                Turn::Wait(slot) => slot,
                Turn::Parse(mut claim) => {
                    // Looked up again with the key claimed: a parse that ended since the lookup above stored its
                    // result before it released the key.
                    let result = match cache.get(key) {
                        Some(found) => Ok(found),
                        None => parse().inspect(|module| cache.put(key, module.clone())),
                    };
                    claim.release(|| match &result {
                        Ok(module) => Outcome::Parsed(Arc::new(module.clone())),
                        Err(error) => Outcome::Failed(error.clone()),
                    });
                    return result;
                }
                Turn::Inline => return parse(),
            };
            match slot.wait() {
                Outcome::Parsed(module) => return Ok(Arc::unwrap_or_clone(module)),
                Outcome::Failed(error) => return Err(error),
                Outcome::Abandoned => {}
            }
        }
    }

    fn slots(&self) -> MutexGuard<'_, HashMap<ModuleKey, Arc<Slot>>> {
        self.slots.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Joins the parse of `key` in flight, or claims `key` for the calling thread.
    fn join_or_claim(&self, key: ModuleKey) -> Turn<'_> {
        let current = thread::current().id();
        let mut slots = self.slots();
        if let Some(slot) = slots.get(&key) {
            if slot.claimer == current {
                return Turn::Inline;
            }
            slot.waiters.fetch_add(1, Ordering::Relaxed);
            return Turn::Wait(Arc::clone(slot));
        }
        let slot = Arc::new(Slot {
            claimer: current,
            waiters: AtomicUsize::new(0),
            outcome: Mutex::new(None),
            ready: Condvar::new(),
        });
        slots.insert(key, Arc::clone(&slot));
        Turn::Parse(Claim {
            coalescer: self,
            key,
            slot,
            released: false,
        })
    }
}

impl std::fmt::Debug for ParseCoalescer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParseCoalescer")
            .field("parsing", &self.slots().len())
            .finish_non_exhaustive()
    }
}

impl Slot {
    /// Blocks until the claimer hands over the outcome.
    fn wait(&self) -> Outcome {
        let mut outcome = self.outcome.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if let Some(outcome) = outcome.as_ref() {
                return outcome.clone();
            }
            outcome = self.ready.wait(outcome).unwrap_or_else(PoisonError::into_inner);
        }
    }
}

/// A key claimed by [`ParseCoalescer::join_or_claim`]. Dropping it unreleased, as a panicking parse does, releases
/// it with [`Outcome::Abandoned`].
struct Claim<'a> {
    coalescer: &'a ParseCoalescer,
    key: ModuleKey,
    slot: Arc<Slot>,
    released: bool,
}

impl Claim<'_> {
    /// Takes the slot out of the map, so later lookups go to the cache, and hands the waiters `outcome`, which is
    /// built only when a thread waits.
    fn release(&mut self, outcome: impl FnOnce() -> Outcome) {
        self.released = true;
        self.coalescer.slots().remove(&self.key);
        if self.slot.waiters.load(Ordering::Relaxed) == 0 {
            return;
        }
        *self.slot.outcome.lock().unwrap_or_else(PoisonError::into_inner) = Some(outcome());
        self.slot.ready.notify_all();
    }
}

impl Drop for Claim<'_> {
    fn drop(&mut self) {
        if !self.released {
            self.release(|| Outcome::Abandoned);
        }
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
    let Some(cache) = cache else {
        return parse_uncached(path, source, *options);
    };
    let key = ModuleKey::new(path, source, options);
    if let Some(CachedModule(info)) = cache.get(key) {
        return Ok(info);
    }
    let info = parse_uncached(path, source, *options)?;
    cache.put(key, CachedModule(info.clone()));
    Ok(info)
}

/// Parses `source`, the text of `path`, with the parser its name selects.
fn parse_uncached(path: &Path, source: &str, options: ParseOptions) -> Result<ModuleInfo, SyntaxError> {
    if is_stylesheet(path) {
        Ok(parse_stylesheet(path, source))
    } else {
        crate::parse_module(path, source, &options)
    }
}

const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ModuleKey>();
    assert_send_sync::<CachedModule>();
    assert_send_sync::<MemoryModuleCache>();
    assert_send_sync::<ParseCoalescer>();
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

    type Parse = Result<CachedModule, SyntaxError>;

    /// A parse result of its own, so a lookup that returns it was handed this parse.
    fn module(source: &str) -> CachedModule {
        CachedModule(crate::parse_module(Path::new("a.ts"), source, &ParseOptions::default()).unwrap())
    }

    fn unsupported() -> SyntaxError {
        SyntaxError::Unsupported {
            extension: "txt".to_owned(),
        }
    }

    /// The threads registered as waiting for the parse of `key` in flight.
    fn waiting(coalescer: &ParseCoalescer, key: ModuleKey) -> usize {
        coalescer
            .slots()
            .get(&key)
            .map_or(0, |slot| slot.waiters.load(Ordering::SeqCst))
    }

    /// Returns once `count` threads wait for the parse of `key`, from when each of them can only be handed the
    /// claimer's outcome, or once `gave_up` holds. The deadline only bounds a failing test.
    fn until_waiting(coalescer: &ParseCoalescer, key: ModuleKey, count: usize, gave_up: impl Fn() -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while waiting(coalescer, key) < count && !gave_up() {
            assert!(
                std::time::Instant::now() < deadline,
                "{count} lookups never waited for the parse"
            );
            std::thread::yield_now();
        }
    }

    /// The parse of a lookup that must be handed another thread's outcome.
    fn must_not_parse() -> Parse {
        panic!("a waiting lookup parsed the key itself")
    }

    /// Lets one thread claim `key` and start parsing, then a second look the key up with `waiter_parse`, and ends
    /// the first parse with `finish` only once the second is registered as waiting. Returns the outcome of the
    /// first, an `Err` when its parse panicked, and of the second.
    fn claimer_and_waiter(
        coalescer: &ParseCoalescer,
        cache: &dyn ModuleCache,
        key: ModuleKey,
        finish: impl FnOnce() -> Parse + Send,
        mut waiter_parse: impl FnMut() -> Parse + Send,
    ) -> (std::thread::Result<Parse>, Parse) {
        let (started, has_started) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel::<()>();
        std::thread::scope(|scope| {
            let first = scope.spawn(move || {
                let mut finish = Some(finish);
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    coalescer.get_or_parse(cache, key, &mut || {
                        started.send(()).unwrap();
                        released.recv().unwrap();
                        finish.take().unwrap()()
                    })
                }))
            });
            has_started.recv().unwrap();
            let second = scope.spawn(move || coalescer.get_or_parse(cache, key, &mut waiter_parse));
            until_waiting(coalescer, key, 1, || second.is_finished());
            release.send(()).unwrap();
            (first.join().unwrap(), second.join().unwrap())
        })
    }

    #[test]
    fn a_lookup_of_a_key_being_parsed_is_handed_that_parse() {
        let store = Counting::default();
        let coalescer = ParseCoalescer::new();
        let key = key("a.ts", "a", ParseOptions::default());
        let parsed = module("export const parsed = 1;");
        let (first, second) = claimer_and_waiter(&coalescer, &store, key, || Ok(parsed.clone()), must_not_parse);
        assert_eq!(first.unwrap(), Ok(parsed.clone()));
        assert_eq!(second, Ok(parsed.clone()));
        // One parse, stored once; the waiter was handed it, not served by the store.
        assert_eq!(store.puts.load(Ordering::Relaxed), 1);
        assert_eq!(store.hits.load(Ordering::Relaxed), 0);
        assert_eq!(store.inner.get(key), Some(parsed));
        assert_eq!(waiting(&coalescer, key), 0);
        assert!(coalescer.slots().is_empty());
    }

    #[test]
    fn a_failed_parse_is_handed_to_its_waiters_and_stores_nothing() {
        let store = Counting::default();
        let coalescer = ParseCoalescer::new();
        let key = key("a.ts", "a", ParseOptions::default());
        let (first, second) = claimer_and_waiter(&coalescer, &store, key, || Err(unsupported()), must_not_parse);
        assert_eq!(first.unwrap(), Err(unsupported()));
        assert_eq!(second, Err(unsupported()));
        assert_eq!(store.puts.load(Ordering::Relaxed), 0);
        assert!(store.inner.is_empty());
        // The key was released: the next lookup parses it.
        let mut parses = 0;
        let found = coalescer.get_or_parse(&store, key, &mut || {
            parses += 1;
            Ok(module("export const later = 1;"))
        });
        assert_eq!(parses, 1);
        assert_eq!(found, Ok(module("export const later = 1;")));
        assert_eq!(store.inner.len(), 1);
    }

    #[test]
    fn a_panicking_parse_leaves_one_waiter_to_parse_the_key_again() {
        let store = Counting::default();
        let coalescer = ParseCoalescer::new();
        let key = key("a.ts", "a", ParseOptions::default());
        let waiter_parses = AtomicUsize::new(0);
        let (first, second) = claimer_and_waiter(
            &coalescer,
            &store,
            key,
            || -> Parse { panic!("the parser failed") },
            || {
                waiter_parses.fetch_add(1, Ordering::SeqCst);
                Ok(module("export const retried = 1;"))
            },
        );
        assert!(first.is_err());
        // The panic handed the waiter nothing; it parsed the key itself, once, and stored that result.
        assert_eq!(second, Ok(module("export const retried = 1;")));
        assert_eq!(waiter_parses.load(Ordering::SeqCst), 1);
        assert_eq!(store.puts.load(Ordering::Relaxed), 1);
        assert_eq!(store.inner.len(), 1);
        assert_eq!(store.inner.get(key), Some(module("export const retried = 1;")));
        assert!(coalescer.slots().is_empty());
    }

    #[test]
    fn many_threads_asking_for_one_key_parse_it_once() {
        let store = MemoryModuleCache::new();
        let coalescer = ParseCoalescer::new();
        let key = key("a.ts", "a", ParseOptions::default());
        let parses = AtomicUsize::new(0);
        let barrier = std::sync::Barrier::new(8);
        let found: Vec<Parse> = std::thread::scope(|scope| {
            let mut lookups = Vec::new();
            for _ in 0..8 {
                lookups.push(scope.spawn(|| {
                    barrier.wait();
                    coalescer.get_or_parse(&store, key, &mut || {
                        parses.fetch_add(1, Ordering::SeqCst);
                        // The claimer holds the key until the seven others wait for it.
                        until_waiting(&coalescer, key, 7, || false);
                        Ok(module("export const once = 1;"))
                    })
                }));
            }
            lookups.into_iter().map(|lookup| lookup.join().unwrap()).collect()
        });
        assert_eq!(parses.load(Ordering::SeqCst), 1);
        assert!(found.iter().all(|found| *found == Ok(module("export const once = 1;"))));
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn a_lookup_of_a_key_its_own_thread_is_parsing_parses_inline() {
        let store = MemoryModuleCache::new();
        let coalescer = ParseCoalescer::new();
        let key = key("a.ts", "a", ParseOptions::default());
        let found = coalescer.get_or_parse(&store, key, &mut || {
            coalescer.get_or_parse(&store, key, &mut || Ok(module("export const inner = 1;")))
        });
        assert_eq!(found, Ok(module("export const inner = 1;")));
        assert_eq!(store.get(key), Some(module("export const inner = 1;")));
        assert!(coalescer.slots().is_empty());
    }

    #[test]
    fn parse_cached_through_a_coalescer_stores_once_and_reports_parse_errors() {
        let store = Counting::default();
        let coalescer = ParseCoalescer::new();
        let options = ParseOptions::default();
        let path = Path::new("a.ts");
        let first = coalescer
            .parse_cached(&store, path, "export const a = 1;", &options)
            .unwrap();
        let second = coalescer
            .parse_cached(&store, path, "export const a = 1;", &options)
            .unwrap();
        assert_eq!(first, second);
        assert_eq!(
            first,
            parse_cached(None, path, "export const a = 1;", &options).unwrap()
        );
        assert_eq!(store.puts.load(Ordering::Relaxed), 1);
        assert_eq!(store.hits.load(Ordering::Relaxed), 1);
        assert_eq!(
            coalescer.parse_cached(&store, Path::new("a.txt"), "", &options),
            Err(unsupported())
        );
        assert_eq!(store.puts.load(Ordering::Relaxed), 1);
    }
}
