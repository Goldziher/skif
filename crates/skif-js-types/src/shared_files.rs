//! The declaration files the programs of a run share (ADR 0034): the bundled `lib.*.d.ts` files and the declaration
//! files below `node_modules` are read, parsed and bound once for the run instead of once per program.
//!
//! # Why sharing is sound
//!
//! - **Ownership.** A shared file is parsed and bound in a registered region of its own ([`Region::new`]), which the
//!   run's [`SharedFiles`] holds. Every program that reads the file holds the cache through its compiler host and its
//!   [`Resident`](crate::resident::Resident), so the region outlives every program that points into it. Freeing a
//!   program (`free_unshared_program`) frees the program, its processed-file data and its mapper, which are heap
//!   boxes; its loader regions are dropped after it. None of them owns a shared file, so freeing one program never
//!   frees or touches a file another program reads.
//! - **Identical parse.** In tsrs a parse is a function of the file's text and its [`SourceFileParseOptions`] (name,
//!   path and the external-module-indicator options); the script kind and language variant come from the name, and
//!   no compiler option (target, JSX, module detection, `isolatedModules`, `allowJs`, `checkJs`) reaches the parser or
//!   the binder otherwise. The cache is keyed on the whole parse options, so two requests share a file only when tsrs
//!   would parse it the same way for both. For a declaration file the external-module-indicator options are always
//!   the default (`get_external_module_indicator_options`), so every program that reads one shares it. The programs
//!   of a run read one file system (the condition [`TypePrograms`](crate::TypePrograms) already sets).
//! - **Binding.** The binder needs only the file and binds it once (`SourceFile::bind_once`); it runs here, on
//!   insertion, inside the file's region, so the file is bound before any program sees it and every program's
//!   `bind_source_files` skips it.
//! - **Checkers.** Each program's checkers keep what they work out in link stores of their own (node links, symbol
//!   links and merged symbols are tables of the checker keyed by node or symbol, in the checker's region) and write
//!   into symbols they create (transient ones), never into a parsed file or a bound symbol. The writes into a file a
//!   checker does make are those it already makes concurrently with the other checkers of its program: the lazily
//!   computed caches of the file (JSDoc, line map, name table), each set once from the file's text alone, and a
//!   symbol's or node's id, set once by compare-and-swap from a process-wide counter. tsrs allocates those caches in
//!   the region that owns the file (`SourceFile::owner_region`, `enter_owner`), found through the region registry:
//!   registering the file's region keeps them there, whichever checker fills them, instead of in the region that
//!   happens to be current on the filling thread, which a program frees. This is how tsrs itself shares declaration
//!   files between the projects of `tsc -b` (`bind_in_file_regions`).
//! - **Threads.** The cache is `Send + Sync`. Concurrent requests for one file wait on its slot while one of them
//!   parses; parsing and binding run on the calling thread and need no thread pool, so a pool thread that waits
//!   blocks nothing the parse needs. A thread inside a checker's scratch region (one that took a loader job while it
//!   waited) parses the file for its program alone, as before: the parser moves the `SourceFile` out of a scratch
//!   region, which would put it outside the file's region.

use std::fmt::Display;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use tsrs_ast::{SourceFile, SourceFileParseOptions};
use tsrs_compiler::CompilerHost;
use tsrs_contentmapper::{self as contentmapper, Project};
use tsrs_core::arena::Region;
use tsrs_core::{P, tspath};
use tsrs_diagnostics::Message;
use tsrs_tsoptions::{Mapper, ParsedCommandLine};
use tsrs_vfs::FS;

use crate::SharedFileCounts;
use crate::vfs::BomFiles;

/// The first chunk of a shared file's region; it grows as the parse needs and is trimmed to what parse and bind used.
const FIRST_CHUNK: usize = 64 << 10;

/// The shared files of a run, by the parse options tsrs read each with.
#[derive(Default)]
pub(crate) struct SharedFiles {
    slots: Mutex<skif_core::HashMap<SourceFileParseOptions, Arc<Slot>>>,
    counts: Counts,
}

/// One file: empty while no request has parsed it; `None` once a request found it unreadable.
type Slot = OnceLock<Option<SharedFile>>;

/// A parsed and bound file with the region that holds it, freed with the cache.
struct SharedFile {
    file: P<SourceFile>,
    /// Whether its text starts with a byte order mark, which the file system that read it dropped.
    bom: bool,
    _region: Region,
}

impl SharedFiles {
    /// The cache's counts for the run so far.
    pub(crate) fn counts(&self) -> SharedFileCounts {
        self.counts.snapshot()
    }

    /// The file `options` name for a program reading through `host`, whose file system records the files with a byte
    /// order mark in `boms`: the shared one, parsed by this request or an earlier one, or, on a thread inside a
    /// scratch region, one parsed for the program alone. The request counts in `counts` (the analysis's) and in the
    /// cache's own.
    fn source_file(
        &self,
        options: SourceFileParseOptions,
        host: &dyn CompilerHost,
        boms: &BomFiles,
        counts: &Counts,
    ) -> Option<P<SourceFile>> {
        let slot = Arc::clone(
            lock(&self.slots)
                .entry(options.clone())
                .or_insert_with(|| Arc::new(OnceLock::new())),
        );
        let (shared, parsed) = match slot.get() {
            Some(shared) => (shared, false),
            None if tsrs_core::arena::scratch_active() => {
                self.counts.unshared.fetch_add(1, Ordering::Relaxed);
                counts.unshared.fetch_add(1, Ordering::Relaxed);
                return host.get_source_file(options);
            }
            None => {
                let mut parsed = false;
                let shared = slot.get_or_init(|| {
                    parsed = true;
                    parse(options, host, boms)
                });
                (shared, parsed)
            }
        };
        let count = |counts: &Counts| {
            let counter = if parsed { &counts.parsed } else { &counts.reused };
            counter.fetch_add(1, Ordering::Relaxed);
        };
        count(&self.counts);
        count(counts);
        let shared = shared.as_ref()?;
        if shared.bom {
            boms.record(shared.file.file_name());
        }
        Some(shared.file)
    }
}

/// Reads, parses and binds the file `options` name through `host` in a region of its own.
fn parse(options: SourceFileParseOptions, host: &dyn CompilerHost, boms: &BomFiles) -> Option<SharedFile> {
    let region = Region::new(FIRST_CHUNK);
    let file = {
        let _scope = region.enter();
        let file = host.get_source_file(options)?;
        tsrs_binder::bind_source_file(file);
        file
    };
    region.trim();
    Some(SharedFile {
        file,
        bom: boms.offset(file.file_name()) != 0,
        _region: region,
    })
}

/// Whether the file `file_name` is shared: a bundled lib file or a declaration file below `node_modules`. A project's
/// own files, declaration files included, are parsed for each program that reads them.
fn shareable(file_name: &str) -> bool {
    tsrs_vfs::bundled::embedded_file(file_name).is_some()
        || (file_name.contains("/node_modules/") && tspath::is_declaration_file_name(file_name))
}

/// Files parsed, reused and parsed unshared, counted with relaxed atomics: each count is read after the programs that
/// add to it are done.
#[derive(Debug, Default)]
pub(crate) struct Counts {
    parsed: AtomicUsize,
    reused: AtomicUsize,
    unshared: AtomicUsize,
}

impl Counts {
    pub(crate) fn snapshot(&self) -> SharedFileCounts {
        SharedFileCounts {
            parsed: self.parsed.load(Ordering::Relaxed),
            reused: self.reused.load(Ordering::Relaxed),
            unshared: self.unshared.load(Ordering::Relaxed),
        }
    }
}

/// The compiler host of one program: `inner`'s, except that a shared file comes from the run's cache.
pub(crate) struct SharingHost {
    inner: Arc<dyn CompilerHost>,
    files: Arc<SharedFiles>,
    boms: Arc<BomFiles>,
    counts: Arc<Counts>,
}

impl SharingHost {
    /// `inner`, reading the shared files from `files`; `boms` is the record of `inner`'s file system, and `counts`
    /// those of the analysis that builds the program.
    pub(crate) fn new(
        inner: Arc<dyn CompilerHost>,
        files: Arc<SharedFiles>,
        boms: Arc<BomFiles>,
        counts: Arc<Counts>,
    ) -> Self {
        Self {
            inner,
            files,
            boms,
            counts,
        }
    }
}

impl CompilerHost for SharingHost {
    fn fs(&self) -> &dyn FS {
        self.inner.fs()
    }

    fn default_library_path(&self) -> &str {
        self.inner.default_library_path()
    }

    fn get_current_directory(&self) -> &str {
        self.inner.get_current_directory()
    }

    fn trace(&self, msg: &'static Message, args: &[&dyn Display]) {
        self.inner.trace(msg, args);
    }

    fn get_source_file(&self, opts: SourceFileParseOptions) -> Option<P<SourceFile>> {
        if !shareable(&opts.file_name) {
            return self.inner.get_source_file(opts);
        }
        self.files
            .source_file(opts, self.inner.as_ref(), &self.boms, &self.counts)
    }

    fn get_content_mapped_source_files(
        &self,
        parse_options: SourceFileParseOptions,
        mapper: &Mapper,
    ) -> Result<contentmapper::SourceFiles, contentmapper::Error> {
        self.inner.get_content_mapped_source_files(parse_options, mapper)
    }

    fn content_mapper_project(&self) -> Option<Arc<dyn Project>> {
        self.inner.content_mapper_project()
    }

    fn get_resolved_project_reference(&self, file_name: &str, path: tspath::Path) -> Option<P<ParsedCommandLine>> {
        self.inner.get_resolved_project_reference(file_name, path)
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<SharedFiles>();
    assert_send_sync::<SharingHost>();
};

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::sync::Barrier;

    use skif_core::MemoryFs;
    use tsrs_ast::ExternalModuleIndicatorOptions;

    use super::*;
    use crate::vfs;

    const SHAPES: &str = "/ws/node_modules/shapes/index.d.ts";
    const SHAPES_TEXT: &str = "export interface Shape {\n  area(): number;\n}\n";

    /// A compiler host over `memory` as a program of one analysis reads through it, with the record of its file
    /// system.
    struct Reader {
        host: Arc<dyn CompilerHost>,
        boms: Arc<BomFiles>,
    }

    impl Reader {
        fn new(memory: &MemoryFs) -> Self {
            let (fs, boms) = vfs::tsrs_fs(Arc::new(memory.clone()));
            let host = tsrs_compiler::new_compiler_host("/ws", fs, &vfs::lib_path(), None, None, None);
            Self { host, boms }
        }

        /// `SHAPES` from `files`, with the external-module-indicator options `indicator`.
        fn read_with(
            &self,
            files: &SharedFiles,
            counts: &Counts,
            indicator: ExternalModuleIndicatorOptions,
        ) -> Option<P<SourceFile>> {
            let options = SourceFileParseOptions {
                file_name: SHAPES.to_owned(),
                path: tspath::to_path(SHAPES, "/ws", true),
                external_module_indicator_options: indicator,
            };
            files.source_file(options, self.host.as_ref(), &self.boms, counts)
        }

        /// `SHAPES` from `files`, with the options of a declaration file.
        fn read(&self, files: &SharedFiles, counts: &Counts) -> Option<P<SourceFile>> {
            self.read_with(files, counts, ExternalModuleIndicatorOptions::default())
        }
    }

    fn shapes() -> MemoryFs {
        MemoryFs::new().with_file(SHAPES, SHAPES_TEXT)
    }

    const fn counts(parsed: usize, reused: usize, unshared: usize) -> SharedFileCounts {
        SharedFileCounts {
            parsed,
            reused,
            unshared,
        }
    }

    #[test]
    fn lib_files_and_declaration_files_below_node_modules_are_shared_and_project_files_are_not() {
        let lib = format!("{}/lib.es5.d.ts", vfs::lib_path());
        let cases = [
            (lib.as_str(), true),
            ("/ws/node_modules/shapes/index.d.ts", true),
            ("/ws/node_modules/@types/node/fs.d.ts", true),
            ("/ws/packages/a/node_modules/shapes/index.d.mts", true),
            ("/ws/node_modules/shapes/index.js", false),
            ("/ws/node_modules/shapes/src/index.ts", false),
            ("/ws/src/globals.d.ts", false),
            ("/ws/src/widget.ts", false),
            ("/ws/my_node_modules/shapes/index.d.ts", false),
        ];
        for (file_name, expected) in cases {
            assert_eq!(shareable(file_name), expected, "{file_name}");
        }
    }

    #[test]
    fn a_second_request_with_the_same_options_reuses_the_bound_file() {
        let memory = shapes();
        let files = SharedFiles::default();
        let analysis = Counts::default();
        let first = Reader::new(&memory).read(&files, &analysis).expect("the file is read");
        let second = Reader::new(&memory).read(&files, &analysis).expect("the file is read");
        assert_eq!(first, second, "one parse for both requests");
        assert!(first.is_bound(), "bound on insertion");
        assert_eq!(first.text(), SHAPES_TEXT);
        assert_eq!(files.counts(), counts(1, 1, 0), "the run's counts");
        assert_eq!(analysis.snapshot(), counts(1, 1, 0), "the analysis's counts");
    }

    #[test]
    fn requests_with_other_parse_options_do_not_share_the_file() {
        let reader = Reader::new(&shapes());
        let files = SharedFiles::default();
        let analysis = Counts::default();
        let indicators = [
            ExternalModuleIndicatorOptions::default(),
            ExternalModuleIndicatorOptions {
                jsx: false,
                force: true,
            },
            ExternalModuleIndicatorOptions {
                jsx: true,
                force: false,
            },
        ];
        let parsed: Vec<P<SourceFile>> = indicators
            .into_iter()
            .map(|indicator| reader.read_with(&files, &analysis, indicator).unwrap())
            .collect();
        assert!(parsed[0] != parsed[1] && parsed[0] != parsed[2] && parsed[1] != parsed[2]);
        assert_eq!(files.counts(), counts(3, 0, 0));
    }

    #[test]
    fn a_reused_file_with_a_byte_order_mark_is_recorded_for_the_analysis_that_reuses_it() {
        let memory = MemoryFs::new().with_file(SHAPES, format!("\u{feff}{SHAPES_TEXT}"));
        let (first, second) = (Reader::new(&memory), Reader::new(&memory));
        let files = SharedFiles::default();
        let analysis = Counts::default();
        first.read(&files, &analysis).unwrap();
        assert_eq!(first.boms.offset(SHAPES), 3, "the parsing analysis read the mark");
        assert_eq!(
            second.boms.offset(SHAPES),
            0,
            "the other analysis has not read the file"
        );
        second.read(&files, &analysis).unwrap();
        assert_eq!(second.boms.offset(SHAPES), 3, "the reusing analysis knows of the mark");
        assert_eq!(files.counts(), counts(1, 1, 0));
    }

    #[test]
    fn an_unreadable_file_is_none_for_every_request() {
        let reader = Reader::new(&MemoryFs::new());
        let files = SharedFiles::default();
        let analysis = Counts::default();
        assert_eq!(reader.read(&files, &analysis), None);
        assert_eq!(reader.read(&files, &analysis), None);
        assert_eq!(files.counts(), counts(1, 1, 0));
    }

    #[test]
    fn concurrent_requests_for_one_file_parse_it_once() {
        const THREADS: usize = 8;
        let memory = shapes();
        let files = SharedFiles::default();
        let analysis = Counts::default();
        let start = Barrier::new(THREADS);
        let parsed: Vec<P<SourceFile>> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..THREADS)
                .map(|_| {
                    scope.spawn(|| {
                        let reader = Reader::new(&memory);
                        start.wait();
                        reader.read(&files, &analysis).unwrap()
                    })
                })
                .collect();
            handles.into_iter().map(|handle| handle.join().unwrap()).collect()
        });
        assert!(
            parsed.iter().all(|&file| file == parsed[0]),
            "every request gets the one parse"
        );
        assert_eq!(files.counts(), counts(1, THREADS - 1, 0));
    }

    #[test]
    fn a_request_inside_a_scratch_region_parses_the_file_for_its_program_alone() {
        let reader = Reader::new(&shapes());
        let files = SharedFiles::default();
        let analysis = Counts::default();
        let scratch = Region::new_scratch(FIRST_CHUNK);
        let alone = {
            let _scratch = scratch.enter_scratch();
            reader.read(&files, &analysis).unwrap()
        };
        let shared = reader.read(&files, &analysis).unwrap();
        assert_ne!(
            alone, shared,
            "the request inside the scratch region left the cache empty"
        );
        assert_eq!(files.counts(), counts(1, 0, 1));
    }
}
