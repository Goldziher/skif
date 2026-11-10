//! The module cache the linters of one run parse through (ADR 0100): a host's cache, which holds every file, or the
//! cache of a multi-root run, which holds only the files more than one root can reach.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use skif_js_syntax::{
    CachedModule, MemoryModuleCache, ModuleCache, ModuleInfo, ModuleKey, ParseCoalescer, ParseOptions, SyntaxError,
    parse_cached,
};

/// The module cache the linters of one run share, built by [`SharedModules::for_run`] and given to each linter with
/// [`JsProvider::with_shared_modules`](crate::JsProvider::with_shared_modules). Cloning it shares the same cache.
#[derive(Clone)]
pub struct SharedModules {
    cache: Arc<dyn ModuleCache>,
    /// Present when several roots are linted: a lookup of a file another root is parsing waits for that parse.
    coalescer: Option<Arc<ParseCoalescer>>,
    /// The roots of the run when the cache is the run's own, which holds only the files that are not local to the
    /// root parsing them; `None` for a host's cache, which holds every file.
    roots: Option<Arc<[PathBuf]>>,
}

impl SharedModules {
    /// The module cache for linting `roots`, the workspace roots of a run's targets, through `host` when the host
    /// keeps a cache (`--cache`, watch mode, an editor), or `None` when there is nothing to share: one root and no
    /// host cache.
    ///
    /// * With `host`, every file is parsed through it, as with
    ///   [`JsProvider::with_module_cache`](crate::JsProvider::with_module_cache).
    /// * Without one, several roots parse through one in-memory cache that lives as long as the linters holding it.
    ///   It keeps only the files that can be shared. A file below the root parsing it, and below no other root of
    ///   the run nested in that root, is local to that root: it is parsed outside the cache, so a run whose roots
    ///   share no code stores, copies and hashes nothing.
    /// * With several roots, a lookup of a file another root is parsing waits for that parse instead of repeating
    ///   it. One root takes no lock for that.
    #[must_use]
    pub fn for_run(roots: Vec<PathBuf>, host: Option<Arc<dyn ModuleCache>>) -> Option<Self> {
        let several = roots.len() > 1;
        let coalescer = several.then(|| Arc::new(ParseCoalescer::new()));
        match host {
            Some(cache) => Some(Self {
                cache,
                coalescer,
                roots: None,
            }),
            None if several => Some(Self {
                cache: Arc::new(MemoryModuleCache::new()),
                coalescer,
                roots: Some(roots.into()),
            }),
            None => None,
        }
    }

    /// A host's cache for one linter: every file goes through it, with no coalescing.
    pub(crate) fn host(cache: Arc<dyn ModuleCache>) -> Self {
        Self {
            cache,
            coalescer: None,
            roots: None,
        }
    }

    /// How the run of the workspace at `root` parses.
    pub(crate) fn parser<'a>(&'a self, root: &'a Path) -> ModuleParser<'a> {
        let local = self.roots.as_deref().map(|roots| Local {
            root,
            nested: roots
                .iter()
                .map(PathBuf::as_path)
                .filter(|other| *other != root && other.starts_with(root))
                .collect(),
        });
        ModuleParser {
            cache: self.cache.as_ref(),
            coalescer: self.coalescer.as_deref(),
            local,
        }
    }
}

impl std::fmt::Debug for SharedModules {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedModules")
            .field("coalesced", &self.coalescer.is_some())
            .field("roots", &self.roots)
            .finish_non_exhaustive()
    }
}

/// The files local to the root a run lints: below it and below none of the other roots of the run nested in it.
struct Local<'a> {
    root: &'a Path,
    nested: Vec<&'a Path>,
}

impl Local<'_> {
    fn contains(&self, path: &Path) -> bool {
        path.starts_with(self.root) && !self.nested.iter().any(|nested| path.starts_with(nested))
    }
}

/// How one run parses through its [`SharedModules`].
pub(crate) struct ModuleParser<'a> {
    cache: &'a dyn ModuleCache,
    coalescer: Option<&'a ParseCoalescer>,
    /// For the run's own cache, the files it leaves out.
    local: Option<Local<'a>>,
}

/// How a parse through a [`ModuleParser`] was answered, for the timings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Served {
    /// A local file, parsed outside the cache.
    Uncached,
    /// From the cache.
    Hit,
    /// Parsed by this lookup, or a parse error.
    Parsed,
    /// Handed the parse another root ran while this lookup waited.
    Waited,
}

impl ModuleParser<'_> {
    /// Parses `source`, the text of `path`, with `options`, as [`parse_cached`] does, and how it was answered.
    pub(crate) fn parse(
        &self,
        path: &Path,
        source: &str,
        options: ParseOptions,
    ) -> (Result<ModuleInfo, SyntaxError>, Served) {
        if self.local.as_ref().is_some_and(|local| local.contains(path)) {
            return (parse_cached(None, path, source, &options), Served::Uncached);
        }
        let probe = Probe {
            cache: self.cache,
            found: AtomicBool::new(false),
            stored: AtomicBool::new(false),
        };
        let parsed = match self.coalescer {
            Some(coalescer) => coalescer.parse_cached(&probe, path, source, &options),
            None => parse_cached(Some(&probe), path, source, &options),
        };
        let served = if probe.stored.into_inner() || parsed.is_err() {
            Served::Parsed
        } else if probe.found.into_inner() {
            Served::Hit
        } else {
            Served::Waited
        };
        (parsed, served)
    }
}

/// The cache of one lookup, noting whether it answered and whether the lookup stored a fresh parse.
struct Probe<'a> {
    cache: &'a dyn ModuleCache,
    found: AtomicBool,
    stored: AtomicBool,
}

impl ModuleCache for Probe<'_> {
    fn get(&self, key: ModuleKey) -> Option<CachedModule> {
        let found = self.cache.get(key);
        if found.is_some() {
            self.found.store(true, Ordering::Relaxed);
        }
        found
    }

    fn put(&self, key: ModuleKey, module: CachedModule) {
        self.stored.store(true, Ordering::Relaxed);
        self.cache.put(key, module);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// A [`MemoryModuleCache`] that records each key stored.
    #[derive(Default)]
    struct Recording {
        inner: MemoryModuleCache,
        stored: Mutex<Vec<ModuleKey>>,
    }

    impl ModuleCache for Recording {
        fn get(&self, key: ModuleKey) -> Option<CachedModule> {
            self.inner.get(key)
        }

        fn put(&self, key: ModuleKey, module: CachedModule) {
            self.stored.lock().unwrap().push(key);
            self.inner.put(key, module);
        }
    }

    const SOURCE: &str = "export const a = 1;\n";

    fn roots(paths: &[&str]) -> Vec<PathBuf> {
        paths.iter().map(PathBuf::from).collect()
    }

    /// How the run of `root` answers each of `paths`, parsed in order.
    fn served(modules: &SharedModules, root: &str, paths: &[&str]) -> Vec<Served> {
        let parser = modules.parser(Path::new(root));
        paths
            .iter()
            .map(|path| {
                let (parsed, served) = parser.parse(Path::new(path), SOURCE, ParseOptions::default());
                assert_eq!(
                    parsed.unwrap(),
                    parse_cached(None, Path::new(path), SOURCE, &ParseOptions::default()).unwrap()
                );
                served
            })
            .collect()
    }

    #[test]
    fn one_root_without_a_host_cache_shares_nothing() {
        assert!(SharedModules::for_run(roots(&["/r/a"]), None).is_none());
        assert!(SharedModules::for_run(Vec::new(), None).is_none());
    }

    #[test]
    fn only_a_run_over_several_roots_coalesces() {
        let host = || Arc::new(MemoryModuleCache::new()) as Arc<dyn ModuleCache>;
        let one = SharedModules::for_run(roots(&["/r/a"]), Some(host())).unwrap();
        assert!(one.coalescer.is_none());
        assert!(one.roots.is_none());
        let several = SharedModules::for_run(roots(&["/r/a", "/r/b"]), Some(host())).unwrap();
        assert!(several.coalescer.is_some());
        // A host's cache holds every file.
        assert!(several.roots.is_none());
        let own = SharedModules::for_run(roots(&["/r/a", "/r/b"]), None).unwrap();
        assert!(own.coalescer.is_some());
        assert_eq!(own.roots.as_deref(), Some(roots(&["/r/a", "/r/b"]).as_slice()));
    }

    #[test]
    fn the_cache_of_a_run_holds_only_the_files_a_root_does_not_own() {
        let modules = SharedModules::for_run(roots(&["/r", "/r/nested", "/r/sibling"]), None).unwrap();
        // For `/r`: its own file is local; a file of the root nested in it and one outside every root are shared.
        assert_eq!(
            served(
                &modules,
                "/r",
                &["/r/src/own.ts", "/r/nested/src/n.ts", "/shared/lib.ts"]
            ),
            [Served::Uncached, Served::Parsed, Served::Parsed]
        );
        // For `/r/nested`: its own file is local, though `/r` encloses it. It finds what `/r` stored.
        assert_eq!(
            served(
                &modules,
                "/r/nested",
                &["/r/nested/src/n.ts", "/shared/lib.ts", "/r/src/own.ts"]
            ),
            [Served::Uncached, Served::Hit, Served::Parsed]
        );
        // `/r/sibling` is nested in `/r` but not in `/r/nested`, so for it a file of `/r/nested` is shared.
        assert_eq!(
            served(&modules, "/r/sibling", &["/r/sibling/x.ts", "/r/nested/src/n.ts"]),
            [Served::Uncached, Served::Hit]
        );
    }

    #[test]
    fn a_host_cache_holds_every_file() {
        let host = Arc::new(Recording::default());
        let modules = SharedModules::for_run(
            roots(&["/r/a", "/r/b"]),
            Some(Arc::clone(&host) as Arc<dyn ModuleCache>),
        )
        .unwrap();
        assert_eq!(
            served(&modules, "/r/a", &["/r/a/own.ts", "/shared/lib.ts"]),
            [Served::Parsed, Served::Parsed]
        );
        assert_eq!(
            served(&modules, "/r/b", &["/r/a/own.ts", "/shared/lib.ts"]),
            [Served::Hit, Served::Hit]
        );
        // Both texts are equal and the directory is not part of the key, but the file names differ.
        assert_eq!(host.stored.lock().unwrap().len(), 2);
        assert_eq!(host.inner.len(), 2);
    }

    #[test]
    fn a_parse_error_is_a_miss_that_stores_nothing() {
        let host = Arc::new(Recording::default());
        let modules = SharedModules::host(Arc::clone(&host) as Arc<dyn ModuleCache>);
        let (parsed, served) =
            modules
                .parser(Path::new("/r"))
                .parse(Path::new("/r/notes.txt"), "", ParseOptions::default());
        assert!(matches!(parsed, Err(SyntaxError::Unsupported { .. })));
        assert_eq!(served, Served::Parsed);
        assert!(host.stored.lock().unwrap().is_empty());
    }
}
