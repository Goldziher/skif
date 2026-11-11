//! Workspace discovery: root manifest, patterns, members, catalogs and package manager.

use std::collections::BTreeSet;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustc_hash::FxHashMap;
use serde_json::Value;
use skif_config::{Config, PackageManager};
use skif_core::{DirEntry, FileSystem, LineIndex, OsFs, PathSeg, Span};
use skif_specifier::SpecifierCache;

use crate::catalogs::{self, Catalogs};
use crate::detect;
use crate::error::WorkspaceError;
use crate::package::{Package, PackageId};
use crate::parallel::map_ordered;
use crate::pattern::{self, Expander, Pattern};
use crate::pnpm::{self, YamlSource};
use crate::{lerna, roots, rush};

const MANIFEST: &str = "package.json";
const PNPM_WORKSPACE: &str = "pnpm-workspace.yaml";
const YARNRC: &str = ".yarnrc.yml";

/// Where the workspace patterns were declared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum WorkspaceSource {
    /// `workspaces: [...]` in the root `package.json`.
    PackageJsonWorkspaces,
    /// `workspaces: { packages: [...] }` in the root `package.json` (yarn nohoist, bun).
    YarnNohoistWorkspaces,
    /// `packages:` in `pnpm-workspace.yaml`.
    PnpmWorkspaceYaml,
    /// `projects[].projectFolder` in `rush.json`.
    RushJson,
    /// `packages` in `lerna.json`.
    LernaJson,
    /// Nothing declares a workspace: the root package is the only package.
    SinglePackage,
}

/// What one workspace pattern matched.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PatternFact {
    /// Position of the pattern in its declared list: the array index in `package.json`, or the position among the
    /// `packages:` entries of `pnpm-workspace.yaml`. Usable directly as the last segment of a fix path.
    pub index: usize,
    /// The pattern as written, including a leading `!` for negations.
    pub pattern: String,
    /// Whether the pattern is a negation.
    pub negated: bool,
    /// Directories the pattern selected. For a negation, the positively matched directories it removed.
    pub matched: usize,
    /// Whether the pattern cannot be used (a `..` segment, too many brace alternatives), so it selects nothing;
    /// [`Workspace::manifest_errors`] holds why.
    pub invalid: bool,
    /// Where the pattern is written, when known.
    pub span: Option<Span>,
    /// Workspace-relative file declaring the pattern.
    pub file: PathBuf,
}

/// Observations about discovery; judging them is up to rules.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct DiscoveryFacts {
    /// Where the patterns came from.
    pub source: WorkspaceSource,
    /// One entry per declared pattern, in declaration order.
    pub patterns: Vec<PatternFact>,
    /// Matched directories with real content (not only hidden entries, `node_modules` or build output) without a `package.json`, sorted.
    pub dirs_without_manifest: Vec<PathBuf>,
    /// Whether `patterns` are pnpm's defaults rather than written down: a `pnpm-workspace.yaml` without `packages:`
    /// makes pnpm take the root and every package below it (except `test` and `tests` directories).
    pub implicit_patterns: bool,
    /// Whether the package manager is Yarn Classic (1.x): the `packageManager` field says `yarn@1.x`, or, without
    /// one, `yarn.lock` starts with `# yarn lockfile v1`.
    pub yarn_classic: bool,
    /// Lockfiles (see [`LOCKFILES`]) and `pnpm-workspace.yaml` files in the root directory and in every directory the
    /// patterns selected, relative to the root and sorted. Files a `.gitignore`, an `.ignore` or `.git/info/exclude`
    /// excludes are left out; directories below a package that the patterns did not select are not searched.
    pub package_manager_files: Vec<PathBuf>,
}

/// The lockfiles every supported package manager writes, by file name.
pub const LOCKFILES: [&str; 6] = [
    "package-lock.json",
    "npm-shrinkwrap.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "bun.lock",
    "bun.lockb",
];

/// A discovered workspace.
#[derive(Debug)]
pub struct Workspace {
    /// Absolute workspace root.
    pub root: PathBuf,
    /// Config override, then `packageManager`/`devEngines`, then lockfiles, then `pnpm-workspace.yaml`.
    pub package_manager: Option<PackageManager>,
    /// The root package.
    root_package: Package,
    /// Member packages sorted by directory, root excluded.
    packages: Vec<Package>,
    /// Catalogs from all sources.
    pub catalogs: Catalogs,
    /// Pattern and directory observations.
    pub facts: DiscoveryFacts,
    /// `pnpm-workspace.yaml` when present.
    pub pnpm_workspace: Option<YamlSource>,
    /// `.yarnrc.yml` when present and it declares catalogs (Yarn 4.10+).
    pub yarnrc: Option<YamlSource>,
    /// Members whose manifest could not be read or parsed, and workspace patterns that cannot be used (a `..`
    /// segment, too many brace alternatives); the rest of the workspace is still discovered. The
    /// `invalid-workspace-file` rule of `skif-rules` reports them.
    pub manifest_errors: Vec<WorkspaceError>,
    /// Whether the root directory holds a `package.json`. A workspace declared by `pnpm-workspace.yaml` or
    /// `rush.json` alone has none: its root package is then an empty stand-in that is excluded from analysis.
    has_root_manifest: bool,
    /// Member directories (relative to the root) whose `package.json` could not be read or parsed.
    failed_dirs: Vec<PathBuf>,
    /// Package name to id, the root first: with a name declared twice the first package wins.
    by_name: FxHashMap<Box<str>, PackageId>,
    /// Indexed by [`PackageId`]: whether `[workspace] ignore-packages` excludes the package.
    ignored: Vec<bool>,
    /// Whether this is a standalone package found below another root of a recursive run.
    nested: bool,
    /// The root's directory relative to the directory the `[js.workspaces]` keys of its configuration are written
    /// from, when that is not the root (see [`set_workspace_keys_anchor`](Self::set_workspace_keys_anchor)).
    keys_anchor: Option<String>,
}

struct Declared {
    source: WorkspaceSource,
    file: PathBuf,
    /// Whether the patterns are pnpm's defaults and not written in the file.
    implicit: bool,
    patterns: Vec<(usize, String, Option<Span>)>,
}

/// The patterns pnpm applies to a `pnpm-workspace.yaml` that has no `packages:` list.
fn implicit_declared(yaml: &YamlSource) -> Declared {
    Declared {
        source: WorkspaceSource::PnpmWorkspaceYaml,
        file: yaml.path.clone(),
        implicit: true,
        patterns: ["**", "!**/test/**", "!**/tests/**"]
            .into_iter()
            .enumerate()
            .map(|(index, pattern)| (index, pattern.to_owned(), None))
            .collect(),
    }
}

fn io_error(path: &Path, source: std::io::Error) -> WorkspaceError {
    WorkspaceError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// The outcome of loading one selected directory.
enum Member {
    Loaded(Box<Package>),
    /// A directory with real content (not only `node_modules`, build output or hidden entries) and no `package.json`.
    NoManifest(PathBuf),
    /// A directory without real content; those linger after switching git branches and are not worth reporting.
    Empty,
    Failed(PathBuf, WorkspaceError),
}

/// Names that say nothing about a directory being in use: hidden entries, installed dependencies and build output.
const GENERATED: [&str; 3] = ["dist", "build", "coverage"];

/// Whether `entry` is something a person put in the directory, rather than tooling leftovers.
fn has_content(entry: &DirEntry) -> bool {
    entry.path.file_name().is_some_and(|name| {
        let name = name.to_string_lossy();
        !pattern::is_ignored_name(&name) && !GENERATED.contains(&name.as_ref())
    })
}

fn load_member(fs: &dyn FileSystem, root: &Path, dir: &str, cache: &SpecifierCache) -> Member {
    let manifest = root.join(dir).join(MANIFEST);
    if !fs.is_file(&manifest) {
        return if fs
            .read_dir_shared(&root.join(dir))
            .is_ok_and(|entries| entries.iter().any(has_content))
        {
            Member::NoManifest(PathBuf::from(dir))
        } else {
            Member::Empty
        };
    }
    match fs.read_to_string(&manifest) {
        Ok(text) => match Package::parse_with(PathBuf::from(dir), text, false, cache) {
            Ok(package) => Member::Loaded(Box::new(package)),
            Err(error) => Member::Failed(PathBuf::from(dir), error),
        },
        Err(error) => Member::Failed(PathBuf::from(dir), io_error(&manifest, error)),
    }
}

/// The lockfiles and `pnpm-workspace.yaml` files of the root and of `dirs`, relative to the root and sorted, without
/// those an ignore file excludes. A directory is listed once rather than probed per name.
fn package_manager_files(fs: &dyn FileSystem, root: &Path, dirs: &[&String], sequential: bool) -> Vec<PathBuf> {
    let mut scanned: Vec<PathBuf> = vec![root.to_path_buf()];
    scanned.extend(dirs.iter().map(|dir| root.join(dir.as_str())));
    let found: Vec<Vec<PathBuf>> = map_ordered(&scanned, sequential, |dir| {
        let Ok(entries) = fs.read_dir_shared(dir) else {
            return Vec::new();
        };
        entries
            .iter()
            .filter(|entry| entry.is_file)
            .filter(|entry| {
                entry
                    .path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name == PNPM_WORKSPACE || LOCKFILES.contains(&name))
            })
            .filter(|entry| !roots::is_ignored_file(fs, root, &entry.path))
            .filter_map(|entry| entry.path.strip_prefix(root).ok().map(Path::to_path_buf))
            .collect()
    });
    let mut files: Vec<PathBuf> = found.into_iter().flatten().collect();
    files.sort();
    files
}

fn json_patterns(root_package: &Package) -> Option<Declared> {
    let (source, base, items) = match root_package.manifest.value.get("workspaces")? {
        Value::Array(items) => (
            WorkspaceSource::PackageJsonWorkspaces,
            vec![PathSeg::Key("workspaces".into())],
            items,
        ),
        Value::Object(object) => (
            WorkspaceSource::YarnNohoistWorkspaces,
            vec![PathSeg::Key("workspaces".into()), PathSeg::Key("packages".into())],
            object.get("packages")?.as_array()?,
        ),
        _ => return None,
    };
    let patterns = items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let mut pointer = base.clone();
            pointer.push(PathSeg::Index(index));
            Some((
                index,
                item.as_str()?.to_owned(),
                root_package.manifest.field_span(&pointer),
            ))
        })
        .collect();
    Some(Declared {
        source,
        file: root_package.manifest.path.clone(),
        implicit: false,
        patterns,
    })
}

impl Workspace {
    /// Discovers the workspace at `root` on the real file system. A relative `root` is resolved against the
    /// current directory here and nowhere else.
    ///
    /// # Errors
    /// See [`WorkspaceError`]; a broken member manifest is not an error but is listed in `manifest_errors`.
    pub fn discover(root: &Path, config: &Config) -> Result<Self, WorkspaceError> {
        let root = std::path::absolute(root).map_err(|error| io_error(root, error))?;
        Self::discover_with_fs(&root, config, &OsFs)
    }

    /// Discovers the workspace at `root` through `fs`.
    ///
    /// `root` is used exactly as given and never resolved against the current directory, so the same root names the
    /// same tree on every platform; pass an absolute path for a real file system.
    ///
    /// # Errors
    /// See [`WorkspaceError`]; a broken member manifest is not an error but is listed in `manifest_errors`.
    pub fn discover_with_fs(root: &Path, config: &Config, fs: &dyn FileSystem) -> Result<Self, WorkspaceError> {
        Self::discover_inner(root, config, fs, false)
    }

    /// Discovers the workspace at `root` through `fs`, loading members under `threads`.
    ///
    /// `Some(1)` loads members on the calling thread and spawns no threads; `Some(n)` loads them on a dedicated
    /// pool of `n` threads that is gone when this returns; `None` uses the current rayon pool, as
    /// [`discover_with_fs`](Self::discover_with_fs) does. The result does not depend on `threads`.
    ///
    /// # Errors
    /// [`WorkspaceError::ThreadPool`] when the pool cannot be built, otherwise as
    /// [`discover_with_fs`](Self::discover_with_fs).
    pub fn discover_with_threads(
        root: &Path,
        config: &Config,
        fs: &dyn FileSystem,
        threads: Option<NonZeroUsize>,
    ) -> Result<Self, WorkspaceError> {
        match threads {
            None => Self::discover_inner(root, config, fs, false),
            Some(threads) if threads.get() == 1 => Self::discover_inner(root, config, fs, true),
            Some(threads) => {
                let pool = rayon::ThreadPoolBuilder::new()
                    .num_threads(threads.get())
                    .build()
                    .map_err(|error| WorkspaceError::ThreadPool(error.to_string()))?;
                pool.install(|| Self::discover_inner(root, config, fs, false))
            }
        }
    }

    fn discover_inner(
        root: &Path,
        config: &Config,
        fs: &dyn FileSystem,
        sequential: bool,
    ) -> Result<Self, WorkspaceError> {
        skif_core::timing::phase(format_args!("workspace discovery {}", root.display()), || {
            Self::load(root, config, fs, sequential)
        })
    }

    fn load(root: &Path, config: &Config, fs: &dyn FileSystem, sequential: bool) -> Result<Self, WorkspaceError> {
        let root = root.to_path_buf();
        if !fs.is_dir(&root) {
            return Err(WorkspaceError::NotADirectory(root));
        }
        let manifest_path = root.join(MANIFEST);
        let yaml_path = root.join(PNPM_WORKSPACE);
        let rush_path = root.join(rush::FILE);
        let has_root_manifest = fs.is_file(&manifest_path);
        let has_rush = fs.is_file(&rush_path);
        if !has_root_manifest && !has_rush && !fs.is_file(&yaml_path) {
            return Err(WorkspaceError::MissingRootManifest(root));
        }
        let specifiers = SpecifierCache::new();
        let root_package = if has_root_manifest {
            let source = fs
                .read_to_string(&manifest_path)
                .map_err(|error| io_error(&manifest_path, error))?;
            Package::parse_with(PathBuf::new(), source, true, &specifiers)?
        } else {
            Package::parse_with(PathBuf::new(), Arc::from("{}"), true, &specifiers)?
        };
        let rush = if has_rush {
            let text = fs
                .read_to_string(&rush_path)
                .map_err(|error| io_error(&rush_path, error))?;
            Some(rush::parse(text)?)
        } else {
            None
        };

        let (pnpm_workspace, pnpm_config) = if fs.is_file(&yaml_path) {
            let text = fs
                .read_to_string(&yaml_path)
                .map_err(|error| io_error(&yaml_path, error))?;
            let parsed = pnpm::parse(Path::new(PNPM_WORKSPACE), &text)?;
            let line_index = Arc::new(LineIndex::new(&text));
            (
                Some(YamlSource {
                    path: PathBuf::from(PNPM_WORKSPACE),
                    source: text,
                    line_index,
                    overrides: parsed.overrides.clone(),
                    minimum_release_age: parsed.minimum_release_age,
                    node_linker: parsed.node_linker.clone(),
                }),
                Some(parsed),
            )
        } else {
            (None, None)
        };

        let from_json = json_patterns(&root_package);
        // `lerna.json` is the last resort: it is read only when nothing else declares members, and then an unreadable
        // or invalid one declares nothing, as in root discovery, so a stale file never fails a workspace it does not
        // define.
        let others_declare = from_json.is_some()
            || pnpm_config.as_ref().is_some_and(|parsed| parsed.packages.is_some())
            || rush.is_some();
        let lerna_path = root.join(lerna::FILE);
        let lerna = (has_root_manifest && !others_declare)
            .then(|| fs.read_to_string(&lerna_path).ok())
            .flatten()
            .and_then(|text| lerna::parse(lerna_path, text).ok());

        let package_manager = detect::detect(
            fs,
            &root,
            config,
            &root_package.manifest.value,
            rush.as_ref()
                .and_then(|rush| rush.package_manager)
                .or_else(|| lerna.as_ref().and_then(|lerna| lerna.package_manager)),
            pnpm_workspace.is_some(),
        );

        let yarn_classic = package_manager == Some(PackageManager::Yarn)
            && detect::is_yarn_classic(fs, &root, &root_package.manifest.value);

        let from_yaml = |yaml: &YamlSource, packages: &[String]| {
            let spans = pnpm::pattern_spans(&yaml.source, &yaml.line_index);
            Declared {
                source: WorkspaceSource::PnpmWorkspaceYaml,
                file: yaml.path.clone(),
                implicit: false,
                patterns: packages
                    .iter()
                    .enumerate()
                    .map(|(index, pattern)| {
                        let span = spans
                            .get(index)
                            .filter(|(written, _)| written == pattern)
                            .map(|(_, span)| *span);
                        (index, pattern.clone(), span)
                    })
                    .collect(),
            }
        };
        // pnpm reads `['.', '**']` when `packages:` is missing (`@pnpm/find-packages`), whatever `package.json` says.
        let implicit_patterns = package_manager == Some(PackageManager::Pnpm)
            && pnpm_config.as_ref().is_some_and(|parsed| parsed.packages.is_none());
        let yaml_declared = pnpm_workspace
            .as_ref()
            .zip(pnpm_config.as_ref())
            .and_then(|(yaml, parsed)| match parsed.packages.as_deref() {
                Some(packages) => Some(from_yaml(yaml, packages)),
                None => implicit_patterns.then(|| implicit_declared(yaml)),
            });
        let from_rush = rush.map(|rush| Declared {
            source: WorkspaceSource::RushJson,
            file: PathBuf::from(rush::FILE),
            implicit: false,
            patterns: rush.projects,
        });
        let from_lerna = lerna.and_then(|lerna| lerna.packages).map(|packages| Declared {
            source: WorkspaceSource::LernaJson,
            file: PathBuf::from(lerna::FILE),
            implicit: false,
            patterns: packages,
        });
        let chosen = if package_manager == Some(PackageManager::Pnpm) && yaml_declared.is_some() {
            yaml_declared
        } else {
            from_json.or(yaml_declared).or(from_rush).or(from_lerna)
        };
        // A plain package with no workspace declaration is linted on its own.
        let declared = chosen.unwrap_or_else(|| Declared {
            source: WorkspaceSource::SinglePackage,
            file: root_package.manifest.path.clone(),
            implicit: false,
            patterns: Vec::new(),
        });

        let yarnrc_path = root.join(YARNRC);
        let mut manifest_errors = Vec::new();
        // Only the manager that is in use reads its own catalog file: a stale `pnpm-workspace.yaml` in a Yarn
        // repository (or a `.yarnrc.yml` in a pnpm one) declares nothing.
        let reads_pnpm_catalogs = matches!(package_manager, None | Some(PackageManager::Pnpm));
        let reads_yarn_catalogs = matches!(package_manager, None | Some(PackageManager::Yarn));
        let (yarnrc, yarn_catalogs) = if reads_yarn_catalogs && fs.is_file(&yarnrc_path) {
            let text = fs
                .read_to_string(&yarnrc_path)
                .map_err(|error| io_error(&yarnrc_path, error))?;
            let catalogs = catalogs::from_yarnrc(&text, Path::new(YARNRC)).unwrap_or_else(|message| {
                manifest_errors.push(WorkspaceError::InvalidYarnrc {
                    path: PathBuf::from(YARNRC),
                    message,
                });
                Catalogs::default()
            });
            let source = (!catalogs.is_empty()).then(|| YamlSource {
                path: PathBuf::from(YARNRC),
                line_index: Arc::new(LineIndex::new(&text)),
                source: text,
                overrides: Vec::new(),
                minimum_release_age: None,
                node_linker: None,
            });
            (source, catalogs)
        } else {
            (None, Catalogs::default())
        };
        let mut catalogs = pnpm_config
            .filter(|_| reads_pnpm_catalogs)
            .map(|parsed| parsed.catalogs)
            .unwrap_or_default();
        catalogs.merge(catalogs::from_package_json(
            &root_package.manifest.value,
            &root_package.manifest.path,
        ));
        catalogs.merge(yarn_catalogs);

        let expander = Expander { fs, root: &root };
        let mut facts = Vec::new();
        let mut candidates = BTreeSet::new();
        let mut negations = Vec::new();
        // Bun keeps a directory a positive pattern names by its literal path, whatever a negation says.
        let mut literal_dirs = BTreeSet::new();
        for (index, raw, span) in &declared.patterns {
            let negated = raw.trim_start().starts_with('!');
            let errors_before = manifest_errors.len();
            let mut invalid = |message: String| {
                manifest_errors.push(WorkspaceError::InvalidPattern {
                    pattern: raw.clone(),
                    message,
                });
                0
            };
            let matched = if negated {
                match pattern::negation(raw) {
                    Ok(matcher) => {
                        negations.push((facts.len(), matcher));
                        0
                    }
                    Err(message) => invalid(message),
                }
            } else {
                match Pattern::new(raw) {
                    Ok(pattern) => {
                        if package_manager == Some(PackageManager::Bun) {
                            literal_dirs.extend(pattern::literal_dir(raw));
                        }
                        let found = expander.expand(&pattern);
                        let count = found.len();
                        candidates.extend(found);
                        count
                    }
                    Err(message) => invalid(message),
                }
            };
            facts.push(PatternFact {
                index: *index,
                pattern: raw.clone(),
                negated,
                matched,
                invalid: manifest_errors.len() > errors_before,
                span: *span,
                file: declared.file.clone(),
            });
        }
        let mut selected = candidates.clone();
        for (index, matcher) in negations {
            let hits: Vec<&String> = candidates
                .iter()
                .filter(|dir| matcher.is_match(dir.as_str()) && !literal_dirs.contains(dir.as_str()))
                .collect();
            if let Some(fact) = facts.get_mut(index) {
                fact.matched = hits.len();
            }
            for dir in hits {
                selected.remove(dir);
            }
        }

        let dirs: Vec<&String> = selected.iter().collect();
        let members: Vec<Member> = map_ordered(&dirs, sequential, |dir| load_member(fs, &root, dir, &specifiers));
        let package_manager_files = package_manager_files(fs, &root, &dirs, sequential);
        let mut packages = Vec::new();
        let mut dirs_without_manifest = Vec::new();
        let mut failed_dirs = Vec::new();
        for member in members {
            match member {
                Member::Loaded(package) => {
                    let id = PackageId::from_index(packages.len() + 1);
                    packages.push((*package).with_id(id));
                }
                Member::NoManifest(dir) => dirs_without_manifest.push(dir),
                Member::Empty => {}
                Member::Failed(dir, error) => {
                    failed_dirs.push(dir);
                    manifest_errors.push(error);
                }
            }
        }
        let ignore = &config.workspace.ignore_packages;
        let mut ignored: Vec<bool> = std::iter::once(&root_package)
            .chain(&packages)
            .map(|package| package.is_selected_by(ignore))
            .collect();
        if !has_root_manifest && let Some(slot) = ignored.first_mut() {
            *slot = true;
        }

        // A `pnpm-workspace.yaml` that only holds settings selects pnpm's default patterns; when those find no
        // package besides the root, the directory is a single package, not a monorepo.
        let source = if declared.implicit && packages.is_empty() {
            WorkspaceSource::SinglePackage
        } else {
            declared.source
        };

        let mut by_name = FxHashMap::default();
        for package in std::iter::once(&root_package).chain(&packages) {
            if let Some(name) = package.name.as_deref() {
                by_name.entry(Box::<str>::from(name)).or_insert(package.id);
            }
        }

        Ok(Self {
            root,
            package_manager,
            root_package,
            packages,
            catalogs,
            facts: DiscoveryFacts {
                source,
                patterns: facts,
                dirs_without_manifest,
                implicit_patterns: declared.implicit,
                yarn_classic,
                package_manager_files,
            },
            pnpm_workspace,
            yarnrc,
            manifest_errors,
            has_root_manifest,
            failed_dirs,
            by_name,
            ignored,
            nested: false,
            keys_anchor: None,
        })
    }

    /// The root package.
    #[must_use]
    pub const fn root_package(&self) -> &Package {
        &self.root_package
    }

    /// The member packages sorted by directory, the root excluded.
    #[must_use]
    pub fn packages(&self) -> &[Package] {
        &self.packages
    }

    /// Whether the root directory holds a `package.json`. A workspace declared by `pnpm-workspace.yaml` or
    /// `rush.json` alone has none: its root package is then an empty stand-in that is excluded from analysis.
    #[must_use]
    pub const fn has_root_manifest(&self) -> bool {
        self.has_root_manifest
    }

    /// The root first, then the members.
    pub fn all_packages(&self) -> impl Iterator<Item = &Package> {
        std::iter::once(&self.root_package).chain(self.packages.iter())
    }

    /// The ids of every package, the root first.
    pub fn package_ids(&self) -> impl Iterator<Item = PackageId> + use<> {
        (0..=self.packages.len()).map(PackageId::from_index)
    }

    /// Every package with its id, the root first.
    pub fn iter(&self) -> impl Iterator<Item = (PackageId, &Package)> {
        self.all_packages().map(|package| (package.id, package))
    }

    /// The package with `id`, or `None` for an id from another workspace.
    #[must_use]
    pub fn package(&self, id: PackageId) -> Option<&Package> {
        match id.index() {
            0 => Some(&self.root_package),
            index => self.packages.get(index - 1),
        }
    }

    /// Looks a package up by its `name`; with a name declared twice, the first package (the root, then the members in
    /// directory order) wins.
    #[must_use]
    pub fn package_by_name(&self, name: &str) -> Option<&Package> {
        self.by_name.get(name).and_then(|&id| self.package(id))
    }

    /// Whether `[workspace] ignore-packages` excludes the package with `id`.
    #[must_use]
    pub fn is_ignored(&self, id: PackageId) -> bool {
        self.ignored.get(id.index()).copied().unwrap_or(false)
    }

    /// The absolute directories of the members, the root excluded.
    #[must_use]
    pub fn member_dirs(&self) -> BTreeSet<PathBuf> {
        self.packages
            .iter()
            .map(|package| self.root.join(&package.dir))
            .collect()
    }

    /// Whether the package manager of this workspace can install a `workspace:` specifier: false for npm, which has
    /// no such protocol, and for Yarn Classic. True for the managers that implement it and, since nothing says
    /// otherwise, when the manager is unknown.
    #[must_use]
    pub fn supports_workspace_protocol(&self) -> bool {
        match self.package_manager {
            Some(PackageManager::Npm) => false,
            Some(PackageManager::Yarn) => !self.facts.yarn_classic,
            _ => true,
        }
    }

    /// The absolute directories the workspace patterns selected that hold a `package.json`: the members, plus those
    /// whose manifest could not be loaded (see [`manifest_errors`](Self::manifest_errors)). A recursive run claims
    /// these for this workspace, so a broken member is not linted again as a root of its own.
    #[must_use]
    pub fn claimed_dirs(&self) -> BTreeSet<PathBuf> {
        let mut dirs = self.member_dirs();
        dirs.extend(self.failed_dirs.iter().map(|dir| self.root.join(dir)));
        dirs
    }

    /// Whether this is a standalone package found below another root of a recursive run (see
    /// [`set_nested`](Self::set_nested)).
    #[must_use]
    pub const fn is_nested(&self) -> bool {
        self.nested
    }

    /// Marks the workspace as a standalone package nested below another root, which the root-manifest rules that only
    /// make sense for a repository's outermost project (`root-package-manager-field`) skip.
    pub const fn set_nested(&mut self, nested: bool) {
        self.nested = nested;
    }

    /// The root's directory, with `/` separators, relative to the directory the `[js.workspaces]` keys and
    /// `ignore-workspaces` globs of its configuration are written from, when that is not the root (see
    /// [`set_workspace_keys_anchor`](Self::set_workspace_keys_anchor)).
    #[must_use]
    pub fn workspace_keys_anchor(&self) -> Option<&str> {
        self.keys_anchor.as_deref()
    }

    /// Has the `[js.workspaces]` keys and `ignore-workspaces` globs select packages from `anchor`, this root's
    /// directory relative to the directory they are written from: a key or glob then selects a package when it
    /// matches the package directory joined to `anchor`, and `"."` is that directory, not this root. A recursive run
    /// with an explicit configuration file sets it for the roots below its start; `None`, the default, matches them
    /// from the root.
    pub fn set_workspace_keys_anchor(&mut self, anchor: Option<String>) {
        self.keys_anchor = anchor;
    }

    /// Excludes the members at `dirs` (absolute paths) from analysis as `ignore-packages` would, because another
    /// root lints them. The packages stay in the workspace, so dependencies on them still resolve.
    pub fn ignore_dirs(&mut self, dirs: &BTreeSet<PathBuf>) {
        let root = &self.root;
        for (slot, package) in self.ignored.iter_mut().skip(1).zip(&self.packages) {
            if dirs.contains(&root.join(&package.dir)) {
                *slot = true;
            }
        }
    }
}
