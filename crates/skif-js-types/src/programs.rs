//! Which programs to build: every judged file goes to the `tsconfig.json` or `jsconfig.json` TypeScript's own
//! discovery gives it (tsserver's rules, after tsrs-lint's `find_tsconfig`), each config is built once, and a file no
//! config includes stays untyped. skif builds no inferred program (ADR 0043).

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use skif_core::HashSet;
use tsrs_core::tspath;
use tsrs_tsoptions::{ParseConfigHost, ParsedCommandLine};
use tsrs_vfs::FS;

use crate::vfs::tsrs_path;
use crate::{TypeError, TypeInput};

/// The host configs are parsed with: the analysis's file system and the workspace root as the current directory.
pub(crate) struct ConfigHost {
    fs: Arc<dyn FS>,
    cwd: String,
}

impl ConfigHost {
    pub(crate) fn new(fs: Arc<dyn FS>, cwd: String) -> Self {
        Self { fs, cwd }
    }

    fn to_path(&self, file: &str) -> tspath::Path {
        tspath::to_path(file, &self.cwd, self.fs.use_case_sensitive_file_names())
    }
}

impl ParseConfigHost for ConfigHost {
    fn fs(&self) -> &dyn FS {
        &*self.fs
    }

    fn get_current_directory(&self) -> &str {
        &self.cwd
    }
}

/// The programs to build and where each judged file goes.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ProgramPlan {
    /// The programs, ordered by config path.
    pub(crate) programs: Vec<PlannedProgram>,
    /// Every judged file, package by package in input order.
    pub(crate) files: Vec<PlannedFile>,
}

/// A config to build, with the judged files it holds.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct PlannedProgram {
    /// The config, by its canonical path as tsrs names files.
    pub(crate) config: String,
    /// Indices into [`ProgramPlan::files`].
    pub(crate) files: Vec<usize>,
}

/// A judged file.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct PlannedFile {
    /// The index of its package in [`TypeInput::packages`].
    pub(crate) package: usize,
    /// The path skif gave.
    pub(crate) path: PathBuf,
    /// The path as tsrs names it.
    pub(crate) name: String,
    /// Where it goes.
    pub(crate) placement: Placement,
}

/// Where a judged file goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Placement {
    /// Into this program of [`ProgramPlan::programs`].
    Program(usize),
    /// No `tsconfig.json` or `jsconfig.json` is above it.
    NoConfig,
    /// No config holds it, and one on the way cannot be parsed.
    ConfigError,
    /// The configs above it parse, but none includes it.
    Outside,
}

/// What the search for a file's config found.
enum Lookup {
    Found(String),
    NoConfig,
    ConfigError,
    Outside,
}

/// Where the search for a file's config starts.
#[derive(Clone, Copy)]
enum Start<'a> {
    /// The nearest config above the file, then the configs above that one.
    Discover,
    /// This config and the configs it references, nothing above (`[js] tsconfig`).
    Given(&'a str),
}

/// The parsed configs of one planning, by path; `None` for a config that cannot be parsed.
type ConfigCache = skif_core::HashMap<tspath::Path, Option<ParsedCommandLine>>;

/// Plans the programs of `input`: per judged file its config, deduplicated by canonical path.
///
/// # Errors
/// [`TypeError::Config`] when the `tsconfig` given for the root package cannot be read or parsed.
pub(crate) fn plan_programs(input: &TypeInput<'_>, host: &ConfigHost) -> Result<ProgramPlan, TypeError> {
    let given = input
        .tsconfig_override
        .map(|config| given_config(input.root, config, host))
        .transpose()?;
    let mut cache = ConfigCache::default();
    let mut by_config: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut files = Vec::new();
    for (package_index, package) in input.packages.iter().enumerate() {
        let start = match &given {
            Some(config) if package.dir == input.root => Start::Given(config),
            _ => Start::Discover,
        };
        for path in &package.files {
            let name = tsrs_path(path);
            let placement = match find_config(&name, start, host, &mut cache) {
                Lookup::Found(config) => {
                    by_config
                        .entry(host.fs.realpath(&config))
                        .or_default()
                        .push(files.len());
                    // Replaced by the program index once every file is placed.
                    Placement::Program(usize::MAX)
                }
                Lookup::NoConfig => Placement::NoConfig,
                Lookup::ConfigError => Placement::ConfigError,
                Lookup::Outside => Placement::Outside,
            };
            files.push(PlannedFile {
                package: package_index,
                path: path.clone(),
                name,
                placement,
            });
        }
    }
    let programs = by_config
        .into_iter()
        .enumerate()
        .map(|(index, (config, members))| {
            for &file in &members {
                files[file].placement = Placement::Program(index);
            }
            PlannedProgram { config, files: members }
        })
        .collect();
    Ok(ProgramPlan { programs, files })
}

/// The `tsconfig` given for the root package, absolute, once it parses.
fn given_config(root: &Path, config: &Path, host: &ConfigHost) -> Result<String, TypeError> {
    let name = tsrs_path(&root.join(config));
    parse_config_reporting(&name, host).map_err(|message| TypeError::Config {
        path: config.to_path_buf(),
        message,
    })?;
    Ok(name)
}

/// Parses the config at `path`; `None` when it cannot be read or parsed.
pub(crate) fn parse_config(path: &str, host: &ConfigHost) -> Option<ParsedCommandLine> {
    parse_config_reporting(path, host).ok()
}

/// Parses the config at `path`, or says why it cannot be.
fn parse_config_reporting(path: &str, host: &ConfigHost) -> Result<ParsedCommandLine, String> {
    let (config, diagnostics) = tsrs_tsoptions::get_parsed_command_line_of_config_file(path, None, None, host, None);
    if !diagnostics.is_empty() {
        return Err(messages(diagnostics.iter().map(|diagnostic| diagnostic.localize())));
    }
    let config = config.ok_or_else(|| format!("cannot read {path}"))?;
    if let Some(file) = config.config_file {
        let syntax = file.source_file.diagnostics();
        if !syntax.is_empty() {
            return Err(messages(syntax.iter().map(|diagnostic| diagnostic.localize())));
        }
    }
    Ok(config)
}

fn messages(messages: impl Iterator<Item = String>) -> String {
    messages.collect::<Vec<_>>().join("; ")
}

/// The config whose program holds `file`, searched as tsserver does: the nearest config, then breadth first through
/// its project references, then the next config above it (unless `disableSolutionSearching` is set).
fn find_config(file: &str, start: Start<'_>, host: &ConfigHost, cache: &mut ConfigCache) -> Lookup {
    let mut next = match start {
        Start::Discover => compute_config_file_name(file, false, host),
        Start::Given(config) => Some(config.to_owned()),
    };
    let file_path = host.to_path(file);
    let mut visited = HashSet::default();
    let mut found_any = false;
    let mut errored = false;
    while let Some(config_path) = next {
        found_any = true;
        let mut queue = VecDeque::from([config_path.clone()]);
        let mut disable_solution_searching = false;
        while let Some(path) = queue.pop_front() {
            if !visited.insert(path.clone()) {
                continue;
            }
            let config = cache
                .entry(host.to_path(&path))
                .or_insert_with(|| parse_config(&path, host));
            let Some(config) = config else {
                errored = true;
                continue;
            };
            if path == config_path {
                disable_solution_searching = config
                    .compiler_options()
                    .is_some_and(|options| options.disable_solution_searching.is_true());
            }
            if holds(config, file, &file_path) {
                return Lookup::Found(path);
            }
            queue.extend(config.resolved_project_reference_paths().iter().cloned());
        }
        if disable_solution_searching || matches!(start, Start::Given(_)) {
            break;
        }
        next = compute_config_file_name(&config_path, true, host);
    }
    if !found_any {
        Lookup::NoConfig
    } else if errored {
        Lookup::ConfigError
    } else {
        Lookup::Outside
    }
}

/// Whether `config`'s program holds `file`: a solution config (no files) holds nothing, and a composite project only
/// what its `include` and `files` can match.
fn holds(config: &ParsedCommandLine, file: &str, file_path: &tspath::Path) -> bool {
    !config.file_names().is_empty()
        && (config
            .compiler_options()
            .is_none_or(|options| !options.composite.is_true())
            || config.possibly_matches_file_name(file))
        && config.file_names_by_path().contains_key(file_path)
}

/// The nearest `tsconfig.json` or `jsconfig.json` above `file`; with `skip_directory`, above the config `file` (a
/// `jsconfig.json` beside a `tsconfig.json` is still found).
fn compute_config_file_name(file: &str, skip_directory: bool, host: &ConfigHost) -> Option<String> {
    let mut directory = tspath::get_directory_path(file);
    // Whether the `tsconfig.json` and the `jsconfig.json` of the first directory are skipped.
    let mut skipped = [skip_directory, skip_directory && !file.ends_with("/tsconfig.json")];
    loop {
        for (name, skip) in ["tsconfig.json", "jsconfig.json"].into_iter().zip(skipped) {
            if skip {
                continue;
            }
            let path = tspath::combine_paths(&directory, &[name]);
            if host.fs.file_exists(&path) {
                return Some(path);
            }
        }
        if directory.ends_with("/node_modules") {
            return None;
        }
        let parent = tspath::get_directory_path(&directory);
        if parent == directory {
            return None;
        }
        directory = parent;
        skipped = [false; 2];
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use skif_core::MemoryFs;

    use super::*;
    use crate::{TypePackage, TypeQuery};

    const ROOT: &str = "/ws";

    fn package(dir: &str, files: &[&str]) -> TypePackage {
        TypePackage {
            dir: PathBuf::from(dir),
            name: None,
            files: files.iter().map(PathBuf::from).collect(),
        }
    }

    fn plan_with(memory: MemoryFs, packages: &[TypePackage], given: Option<&Path>) -> Result<ProgramPlan, TypeError> {
        let (fs, _) = crate::vfs::tsrs_fs(Arc::new(memory));
        let host = ConfigHost::new(fs, ROOT.to_owned());
        let query = TypeQuery::default();
        let mut input = TypeInput::new(Arc::new(MemoryFs::new()), Path::new(ROOT), packages, &query);
        if let Some(config) = given {
            input = input.with_tsconfig_override(config);
        }
        plan_programs(&input, &host)
    }

    fn plan(memory: MemoryFs, packages: &[TypePackage]) -> ProgramPlan {
        plan_with(memory, packages, None).unwrap()
    }

    /// Per judged file, the config of its program or how it stays untyped.
    fn placements(plan: &ProgramPlan) -> Vec<(String, String)> {
        plan.files
            .iter()
            .map(|file| {
                let place = match file.placement {
                    Placement::Program(index) => plan.programs[index].config.clone(),
                    other => format!("{other:?}"),
                };
                (file.name.clone(), place)
            })
            .collect()
    }

    fn pairs(expected: &[(&str, &str)]) -> Vec<(String, String)> {
        expected
            .iter()
            .map(|&(file, place)| (file.to_owned(), place.to_owned()))
            .collect()
    }

    #[test]
    fn each_file_goes_to_its_nearest_config() {
        let memory = MemoryFs::new()
            .with_file("/ws/tsconfig.json", r#"{ "include": ["src"] }"#)
            .with_file("/ws/src/root.ts", "")
            .with_file("/ws/packages/a/tsconfig.json", r#"{ "include": ["src"] }"#)
            .with_file("/ws/packages/a/src/a.ts", "")
            .with_file("/ws/packages/b/jsconfig.json", r#"{ "include": ["lib"] }"#)
            .with_file("/ws/packages/b/lib/b.js", "");
        let packages = [
            package("/ws", &["/ws/src/root.ts"]),
            package("/ws/packages/a", &["/ws/packages/a/src/a.ts"]),
            package("/ws/packages/b", &["/ws/packages/b/lib/b.js"]),
        ];
        let plan = plan(memory, &packages);
        assert_eq!(
            placements(&plan),
            pairs(&[
                ("/ws/src/root.ts", "/ws/tsconfig.json"),
                ("/ws/packages/a/src/a.ts", "/ws/packages/a/tsconfig.json"),
                ("/ws/packages/b/lib/b.js", "/ws/packages/b/jsconfig.json"),
            ])
        );
        let configs: Vec<&str> = plan.programs.iter().map(|program| program.config.as_str()).collect();
        assert_eq!(
            configs,
            [
                "/ws/packages/a/tsconfig.json",
                "/ws/packages/b/jsconfig.json",
                "/ws/tsconfig.json"
            ]
        );
    }

    #[test]
    fn a_solution_config_is_followed_to_the_referenced_config_that_holds_the_file() {
        let memory = MemoryFs::new()
            .with_file(
                "/ws/tsconfig.json",
                r#"{ "files": [], "references": [{ "path": "./tsconfig.node.json" }, { "path": "./tsconfig.app.json" }] }"#,
            )
            .with_file("/ws/tsconfig.node.json", r#"{ "compilerOptions": { "composite": true }, "include": ["vite.config.ts"] }"#)
            .with_file("/ws/tsconfig.app.json", r#"{ "compilerOptions": { "composite": true }, "include": ["src"] }"#)
            .with_file("/ws/vite.config.ts", "")
            .with_file("/ws/src/main.ts", "");
        let plan = plan(memory, &[package("/ws", &["/ws/src/main.ts", "/ws/vite.config.ts"])]);
        assert_eq!(
            placements(&plan),
            pairs(&[
                ("/ws/src/main.ts", "/ws/tsconfig.app.json"),
                ("/ws/vite.config.ts", "/ws/tsconfig.node.json"),
            ])
        );
    }

    #[test]
    fn a_config_that_excludes_the_file_hands_it_to_the_config_above() {
        let memory = MemoryFs::new()
            .with_file("/ws/tsconfig.json", r#"{ "include": ["packages/*/test"] }"#)
            .with_file("/ws/packages/a/tsconfig.json", r#"{ "include": ["src"] }"#)
            .with_file("/ws/packages/a/src/a.ts", "")
            .with_file("/ws/packages/a/test/a.test.ts", "");
        let plan = plan(
            memory,
            &[package(
                "/ws/packages/a",
                &["/ws/packages/a/src/a.ts", "/ws/packages/a/test/a.test.ts"],
            )],
        );
        assert_eq!(
            placements(&plan),
            pairs(&[
                ("/ws/packages/a/src/a.ts", "/ws/packages/a/tsconfig.json"),
                ("/ws/packages/a/test/a.test.ts", "/ws/tsconfig.json"),
            ])
        );
    }

    #[test]
    fn files_without_a_config_or_outside_every_config_stay_untyped() {
        let memory = MemoryFs::new()
            .with_file("/ws/packages/a/tsconfig.json", r#"{ "include": ["src"] }"#)
            .with_file("/ws/packages/a/src/a.ts", "")
            .with_file("/ws/packages/a/scripts/build.ts", "")
            .with_file("/ws/packages/b/b.ts", "");
        let packages = [
            package(
                "/ws/packages/a",
                &["/ws/packages/a/src/a.ts", "/ws/packages/a/scripts/build.ts"],
            ),
            package("/ws/packages/b", &["/ws/packages/b/b.ts"]),
        ];
        let plan = plan(memory, &packages);
        assert_eq!(
            placements(&plan),
            pairs(&[
                ("/ws/packages/a/src/a.ts", "/ws/packages/a/tsconfig.json"),
                ("/ws/packages/a/scripts/build.ts", "Outside"),
                ("/ws/packages/b/b.ts", "NoConfig"),
            ])
        );
    }

    #[test]
    fn a_config_that_does_not_parse_is_a_config_error() {
        let memory = MemoryFs::new()
            .with_file("/ws/tsconfig.json", r#"{ "include": ["src"" }"#)
            .with_file("/ws/src/a.ts", "");
        let plan = plan(memory, &[package("/ws", &["/ws/src/a.ts"])]);
        assert_eq!(placements(&plan), pairs(&[("/ws/src/a.ts", "ConfigError")]));
        assert_eq!(plan.programs.len(), 0);
    }

    #[test]
    fn packages_that_share_a_config_share_one_program() {
        let memory = MemoryFs::new()
            .with_file("/ws/tsconfig.json", r#"{ "include": ["packages"] }"#)
            .with_file("/ws/packages/a/a.ts", "")
            .with_file("/ws/packages/b/b.ts", "")
            .with_file("/ws/packages/c/tsconfig.json", r#"{ "include": ["src"] }"#)
            .with_file("/ws/packages/c/src/c.ts", "");
        let packages = [
            package("/ws/packages/a", &["/ws/packages/a/a.ts"]),
            package("/ws/packages/b", &["/ws/packages/b/b.ts"]),
        ];
        let plan = plan(memory, &packages);
        assert_eq!(
            plan.programs,
            [PlannedProgram {
                config: "/ws/tsconfig.json".to_owned(),
                files: vec![0, 1],
            }]
        );
    }

    #[test]
    fn the_given_tsconfig_replaces_discovery_for_the_root_package_only() {
        let memory = MemoryFs::new()
            .with_file("/ws/tsconfig.json", r#"{ "include": ["src", "packages"] }"#)
            .with_file("/ws/tsconfig.build.json", r#"{ "include": ["src"] }"#)
            .with_file("/ws/src/a.ts", "")
            .with_file("/ws/scripts/x.ts", "")
            .with_file("/ws/packages/b/b.ts", "");
        let packages = [
            package("/ws", &["/ws/src/a.ts", "/ws/scripts/x.ts"]),
            package("/ws/packages/b", &["/ws/packages/b/b.ts"]),
        ];
        let plan = plan_with(memory, &packages, Some(Path::new("tsconfig.build.json"))).unwrap();
        assert_eq!(
            placements(&plan),
            pairs(&[
                ("/ws/src/a.ts", "/ws/tsconfig.build.json"),
                ("/ws/scripts/x.ts", "Outside"),
                ("/ws/packages/b/b.ts", "/ws/tsconfig.json"),
            ])
        );
    }

    #[test]
    fn a_given_tsconfig_that_cannot_be_read_is_an_error() {
        let memory = MemoryFs::new().with_file("/ws/src/a.ts", "");
        let error = plan_with(
            memory,
            &[package("/ws", &["/ws/src/a.ts"])],
            Some(Path::new("missing.json")),
        );
        assert!(
            matches!(&error, Err(TypeError::Config { path, .. }) if path == Path::new("missing.json")),
            "{error:?}"
        );
    }
}
