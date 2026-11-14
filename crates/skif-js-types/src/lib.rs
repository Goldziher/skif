//! Type-aware facts for skif's JS/TS analysis, from the TypeScript checker of tsrs (ADR 0043).
//!
//! skif's module graph and syntax facts come from oxc. Where a project has types, [`analyze`] answers what oxc
//! cannot: which member a read reaches through a typed receiver, an override or an implemented interface. The answer
//! is owned data ([`TypeFacts`]); this crate is the only one that depends on tsrs (ADR 0044).

mod error;
mod facts;

use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use skif_core::FileSystem;

pub use error::TypeError;
pub use facts::{
    CoverageReason, CoverageState, DeclId, DeclKind, DeclQuery, PackageCoverage, TypeFacts, TypeQuery, TypeTimings,
};

/// The engine and version recorded in [`TypeFacts::engine`].
pub const ENGINE: &str = "tsrs 7.1.0-beta.tsrs1";

/// A package whose judged files the analysis covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypePackage {
    /// The package directory, absolute.
    pub dir: PathBuf,
    /// The package name, when it has one.
    pub name: Option<String>,
    /// The absolute paths of the package's judged files.
    pub files: Vec<PathBuf>,
}

/// What [`analyze`] works on.
#[derive(Clone)]
#[non_exhaustive]
pub struct TypeInput<'a> {
    /// The file system every file and config is read through.
    pub fs: Arc<dyn FileSystem>,
    /// The workspace root, absolute.
    pub root: &'a Path,
    /// The packages with judged files.
    pub packages: &'a [TypePackage],
    /// A `tsconfig` that replaces discovery for the root package (`[js] tsconfig`).
    pub tsconfig_override: Option<&'a Path>,
    /// The declarations to answer for.
    pub query: &'a TypeQuery,
    /// The threads the checker may use; `None` for all cores.
    pub threads: Option<NonZeroUsize>,
    /// Set by the host to stop the analysis.
    pub cancel: Option<&'a AtomicBool>,
}

impl<'a> TypeInput<'a> {
    /// An input over `packages` for `query`, with discovered `tsconfig` files, all cores and no cancellation.
    #[must_use]
    pub fn new(fs: Arc<dyn FileSystem>, root: &'a Path, packages: &'a [TypePackage], query: &'a TypeQuery) -> Self {
        Self {
            fs,
            root,
            packages,
            tsconfig_override: None,
            query,
            threads: None,
            cancel: None,
        }
    }

    /// Uses `tsconfig` for the root package instead of discovering one.
    #[must_use]
    pub fn with_tsconfig_override(mut self, tsconfig: &'a Path) -> Self {
        self.tsconfig_override = Some(tsconfig);
        self
    }

    /// Limits the checker to `threads` threads.
    #[must_use]
    pub fn with_threads(mut self, threads: NonZeroUsize) -> Self {
        self.threads = Some(threads);
        self
    }

    /// Stops the analysis once `cancel` is set.
    #[must_use]
    pub fn with_cancel(mut self, cancel: &'a AtomicBool) -> Self {
        self.cancel = Some(cancel);
        self
    }
}

/// Builds the programs of the packages in `input`, asks the checker about the declarations of its query and returns
/// the facts with the coverage of every package.
///
/// # Errors
/// [`TypeError::Config`] when an explicit `tsconfig` cannot be read, [`TypeError::Internal`] when the checker fails
/// unexpectedly and [`TypeError::Cancelled`] when the host cancels. A package without usable types is no error.
pub fn analyze(input: &TypeInput<'_>) -> Result<TypeFacts, TypeError> {
    let coverage = input
        .packages
        .iter()
        .map(|package| PackageCoverage {
            dir: package.dir.clone(),
            name: package.name.clone(),
            state: CoverageState::Untyped,
            reason: Some(CoverageReason::NotBuilt),
            untyped_files: u32::try_from(package.files.len()).unwrap_or(u32::MAX),
        })
        .collect();
    Ok(TypeFacts {
        engine: ENGINE,
        coverage,
        ..TypeFacts::default()
    })
}

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<TypeFacts>();
    assert_send_sync::<TypeQuery>();
    assert_send_sync::<TypeError>();
};
