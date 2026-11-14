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
    /// The UTF-8 byte offset of the declaration's name in the file text, as oxc spans count; for an anonymous
    /// `export default class {}`, which has no name, the offset just past its closing brace.
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

impl CoverageReason {
    /// The reason as skif's report spells it: `no-tsconfig`, `unresolved-modules: 3`.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Disabled => "disabled".to_owned(),
            Self::NotBuilt => "not-built".to_owned(),
            Self::NoTsconfig => "no-tsconfig".to_owned(),
            Self::TsconfigError => "tsconfig-error".to_owned(),
            Self::NotTypescript => "not-typescript".to_owned(),
            Self::UnresolvedModules(count) => format!("unresolved-modules: {count}"),
            Self::FilesOutsidePrograms(count) => format!("files-outside-programs: {count}"),
            Self::CheckerFailed => "checker-failed".to_owned(),
            Self::Cancelled => "cancelled".to_owned(),
        }
    }
}

/// How long the type-aware stages took.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct TypeTimings {
    /// Finding the config of every judged file, before any program is built.
    pub planning: Duration,
    /// Reading the configs of the programs, building and binding them, summed over the programs: several build at
    /// once, so the sum can exceed the wall time.
    pub programs: Duration,
    /// Walking the files and asking the checker, summed over the programs as [`TypeTimings::programs`] is.
    pub queries: Duration,
    /// The wall time from the start of the first program to the end of the last.
    pub wall: Duration,
    /// The threads the programs shared.
    pub threads: usize,
    /// Every built program, in the order of their configs.
    pub per_program: Vec<ProgramTiming>,
}

impl TypeTimings {
    /// The `count` programs that took longest to build and walk, slowest first; programs that took as long come in
    /// the order of their configs.
    #[must_use]
    pub fn slowest(&self, count: usize) -> Vec<&ProgramTiming> {
        let mut programs: Vec<&ProgramTiming> = self.per_program.iter().collect();
        programs.sort_by(|a, b| b.total().cmp(&a.total()).then_with(|| a.config.cmp(&b.config)));
        programs.truncate(count);
        programs
    }
}

/// How long one program took, with its size and the checkers it ran on.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ProgramTiming {
    /// The config the program was built from, absolute.
    pub config: PathBuf,
    /// The judged files planned into the program.
    pub judged_files: usize,
    /// Every file of the program, with the declarations of its dependencies and the default libraries.
    pub files: usize,
    /// The checkers skif gave it; tsrs makes no more than the program has files.
    pub checkers: usize,
    /// Reading its config, building and binding it.
    pub build: Duration,
    /// Walking its judged files on its checkers.
    pub query: Duration,
}

impl ProgramTiming {
    fn total(&self) -> Duration {
        self.build.saturating_add(self.query)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use super::{CoverageReason, ProgramTiming, TypeTimings};

    fn program(config: &str, build: u64, query: u64) -> ProgramTiming {
        ProgramTiming {
            config: PathBuf::from(config),
            judged_files: 1,
            files: 1,
            checkers: 1,
            build: Duration::from_millis(build),
            query: Duration::from_millis(query),
        }
    }

    #[test]
    fn the_slowest_programs_come_first_by_build_and_query_time_with_ties_in_config_order() {
        let timings = TypeTimings {
            per_program: vec![
                program("/ws/a/tsconfig.json", 10, 10),
                program("/ws/b/tsconfig.json", 5, 40),
                program("/ws/c/tsconfig.json", 20, 0),
                program("/ws/d/tsconfig.json", 1, 1),
            ],
            ..TypeTimings::default()
        };
        let configs: Vec<&str> = timings
            .slowest(3)
            .iter()
            .map(|program| program.config.to_str().unwrap_or_default())
            .collect();
        assert_eq!(
            configs,
            ["/ws/b/tsconfig.json", "/ws/a/tsconfig.json", "/ws/c/tsconfig.json"]
        );
        assert_eq!(timings.slowest(10).len(), 4);
        assert_eq!(TypeTimings::default().slowest(5), Vec::<&ProgramTiming>::new());
    }

    #[test]
    fn reasons_are_labelled_as_the_report_spells_them() {
        let cases = [
            (CoverageReason::Disabled, "disabled"),
            (CoverageReason::NotBuilt, "not-built"),
            (CoverageReason::NoTsconfig, "no-tsconfig"),
            (CoverageReason::TsconfigError, "tsconfig-error"),
            (CoverageReason::NotTypescript, "not-typescript"),
            (CoverageReason::UnresolvedModules(3), "unresolved-modules: 3"),
            (CoverageReason::FilesOutsidePrograms(2), "files-outside-programs: 2"),
            (CoverageReason::CheckerFailed, "checker-failed"),
            (CoverageReason::Cancelled, "cancelled"),
        ];
        for (reason, label) in cases {
            assert_eq!(reason.label(), label);
        }
    }
}
