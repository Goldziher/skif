//! The question skif asks the checker and the facts it gets back. Every type is owned skif data; no tsrs type leaks.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::Duration;

/// The declarations skif wants typed facts for. The index of a declaration in [`TypeQuery::decls`] is its
/// [`DeclId`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TypeQuery {
    /// The declarations, in the order skif numbers them.
    pub decls: Vec<DeclQuery>,
}

/// One declaration whose member reads skif asks about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclQuery {
    /// The absolute path of the file declaring it.
    pub file: PathBuf,
    /// The UTF-8 byte offset of the declaration's name in the file text, as oxc spans count.
    pub name_start: u32,
    /// What it declares.
    pub kind: DeclKind,
    /// The names of the members skif judges; reads of other members are not reported.
    pub members: Vec<String>,
}

/// The kind of a [`DeclQuery`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum DeclKind {
    /// A class: its methods, fields and accessors.
    Class,
    /// An enum: its members.
    Enum,
    /// A namespace: its exported members.
    Namespace,
}

/// The index of a declaration in [`TypeQuery::decls`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeclId(pub u32);

/// What the checker found for a [`TypeQuery`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct TypeFacts {
    /// The engine that produced the facts, with its version (`tsrs 7.1.0-beta.tsrs1`).
    pub engine: &'static str,
    /// The members of each declaration that a typed read reaches, directly or through a base class or an interface
    /// the declaration implements.
    pub reads: BTreeMap<DeclId, BTreeSet<String>>,
    /// The declarations used as a whole (`keyof C`, `Object.values(E)`, an alias passed on), whose members all count
    /// as used.
    pub whole: BTreeSet<DeclId>,
    /// Per file, the member names read through a receiver the checker could not type (`any`, an error type, an
    /// unresolved module): these names count as read on every declaration, as without types.
    pub unresolved_names: BTreeMap<PathBuf, BTreeSet<String>>,
    /// The absolute paths of the files the checker covered. A file outside every program keeps the untyped analysis.
    pub covered: BTreeSet<PathBuf>,
    /// The type coverage of every package that has judged files.
    pub coverage: Vec<PackageCoverage>,
    /// How long the stages took.
    pub timings: TypeTimings,
}

/// The type coverage of one package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageCoverage {
    /// The package directory, absolute.
    pub dir: PathBuf,
    /// The package name, when it has one.
    pub name: Option<String>,
    /// How much of the package is typed.
    pub state: CoverageState,
    /// Why the package is not fully typed; `None` when it is.
    pub reason: Option<CoverageReason>,
    /// The number of judged files of the package that stay untyped.
    pub untyped_files: u32,
}

/// How much of a package the checker covered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum CoverageState {
    /// Every judged file is in a program and its imports resolve.
    Typed,
    /// Some files are untyped or some imports do not resolve.
    Partial,
    /// No file of the package is typed.
    Untyped,
}

/// Why a package is not fully typed.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum CoverageReason {
    /// Type awareness is turned off.
    Disabled,
    /// skif was built without the `type-aware` feature.
    NotBuilt,
    /// No `tsconfig.json` or `jsconfig.json` applies to the package.
    NoTsconfig,
    /// The package's `tsconfig` cannot be parsed.
    TsconfigError,
    /// The package has no TypeScript or checked JavaScript files.
    NotTypescript,
    /// This many imports resolve to no module, mostly dependencies that are not installed.
    UnresolvedModules(u32),
    /// This many judged files are in no program.
    FilesOutsidePrograms(u32),
    /// The checker failed on the package's program.
    CheckerFailed,
    /// The host cancelled the analysis.
    Cancelled,
}

/// How long the type-aware stages took.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TypeTimings {
    /// Reading configs and building programs.
    pub programs: Duration,
    /// Walking the files and asking the checker.
    pub queries: Duration,
}
