//! A cache of parse results keyed by everything a parse depends on (ADR 0066).
//!
//! A [`ModuleInfo`] is a pure function of the file name, the text and the [`ParseOptions`], so a key over those
//! inputs can never return a stale result: a changed file has a new key. The cache itself is supplied by the host
//! through [`ModuleCache`]; this crate only owns the key, the stored value and an in-memory implementation.

use std::collections::BTreeMap;
use std::hash::Hasher as _;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use serde::{Deserialize, Serialize};
use siphasher::sip128::{Hasher128 as _, SipHasher13};

use crate::error::SyntaxError;
use crate::model::{ModuleInfo, ParseOptions};
use crate::style::{is_stylesheet, parse_stylesheet};

/// The version of the parse output. Raise it whenever [`ModuleInfo`] or what the parsers put in it changes, so a
/// cache written by an older build at the same crate version is not trusted; the test
/// `the_cache_schema_matches_the_parse_output` fails until [`CACHE_SCHEMA_DIGEST`] is updated with it.
pub const CACHE_SCHEMA: u32 = 2;

/// The digest of the serialized parse results of a fixed probe corpus at [`CACHE_SCHEMA`]. It is a test's guard,
/// not part of any key: when the parse output changes, the test reports the new digest and both constants move.
pub const CACHE_SCHEMA_DIGEST: u128 = 0xf26e_dd18_7be1_af36_b887_4249_c76f_a21b;

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
    if let Some(CachedModule(info)) = cache.get(key) {
        return Ok(info);
    }
    let info = parse()?;
    cache.put(key, CachedModule(info.clone()));
    Ok(info)
}

const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ModuleKey>();
    assert_send_sync::<CachedModule>();
    assert_send_sync::<MemoryModuleCache>();
};

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// One source per language the parsers know, plus the bench corpus: the probe whose parse output the schema
    /// digest pins.
    fn probe_corpus() -> Vec<(PathBuf, String)> {
        let inline: [(&str, &str); 12] = [
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
                 const n = ref(0);\n</script>\n<template><Child :n=\"n\" /></template>\n\
                 <style lang=\"scss\">@use './theme';</style>\n",
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
}
