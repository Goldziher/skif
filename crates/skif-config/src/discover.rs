use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use skif_core::{FileSystem, OsFs};
use toml::Value;

use crate::error::ConfigError;
use crate::layer::ParsedFile;
use crate::merge::{Merged, Merger};
use crate::model::{Config, OverrideScope};
use crate::overrides::ConfigOverrides;
use crate::package_file;
use crate::provenance::{Assignment, ConfigSource, Op, ResolvedConfig, SourceKind};

/// How many files deep `extends` may nest.
const MAX_EXTENDS_DEPTH: usize = 16;

/// Accepted configuration file names, in lookup order.
pub const CONFIG_FILE_NAMES: [&str; 2] = ["skif.toml", ".skif.toml"];

/// Looks for `skif.toml` or `.skif.toml` directly in `root` on the real file system.
///
/// # Errors
/// [`ConfigError::Ambiguous`] when both exist, plus any error from loading the file.
pub fn discover(root: &Path) -> Result<Option<(PathBuf, Config)>, ConfigError> {
    discover_with_fs(root, &OsFs)
}

/// Looks for `skif.toml` or `.skif.toml` directly in `root` through `fs`.
///
/// # Errors
/// [`ConfigError::Ambiguous`] when both exist, plus any error from loading the file.
pub fn discover_with_fs(root: &Path, fs: &dyn FileSystem) -> Result<Option<(PathBuf, Config)>, ConfigError> {
    skif_core::timing::phase(format_args!("config {}", root.display()), || {
        let found: Vec<PathBuf> = CONFIG_FILE_NAMES
            .iter()
            .map(|name| root.join(name))
            .filter(|path| fs.is_file(path))
            .collect();
        match found.as_slice() {
            [] => Ok(None),
            [path] => Ok(Some((path.clone(), Config::from_toml_file_with_fs(path, fs)?))),
            [first, second, ..] => Err(ConfigError::Ambiguous {
                first: first.clone(),
                second: second.clone(),
            }),
        }
    })
}

/// How far above a root the search for configuration files goes.
///
/// A file with `root = true` ends the search at that file whatever the ceiling.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum Ceiling {
    /// The root of the git repository holding the root (the nearest directory with a `.git` entry, a directory or a
    /// file), and the root itself outside a repository.
    #[default]
    Vcs,
    /// Like [`Vcs`](Self::Vcs), with this directory instead of the root when there is no repository: the start
    /// directory of a recursive run.
    VcsOr(PathBuf),
    /// Up to and including this directory.
    Dir(PathBuf),
    /// The root's own directory only.
    RootOnly,
}

#[derive(Clone)]
struct DirInfo {
    /// The configuration files in the directory, in lookup order.
    configs: Vec<PathBuf>,
    has_git: bool,
}

impl DirInfo {
    fn probe(dir: &Path, fs: &dyn FileSystem) -> Self {
        let git = dir.join(".git");
        Self {
            configs: CONFIG_FILE_NAMES
                .iter()
                .map(|name| dir.join(name))
                .filter(|path| fs.is_file(path))
                .collect(),
            has_git: fs.is_dir(&git) || fs.is_file(&git),
        }
    }
}

type FileCell = Arc<OnceLock<Result<Arc<ParsedFile>, ConfigError>>>;
type DirCell = Arc<OnceLock<DirInfo>>;

/// What a run learns about configuration files, shared by every root it loads: which directories hold a file or a
/// `.git`, and each file read and parsed once, whichever roots and `extends` reach it.
///
/// Create one per run (a host with unsaved buffers drops it between runs) and pass it to every
/// [`ConfigLoader`]; it is `Send + Sync`, so roots loaded in parallel share it.
#[derive(Default)]
pub struct ConfigCache {
    files: Mutex<HashMap<PathBuf, FileCell>>,
    dirs: Mutex<HashMap<PathBuf, DirCell>>,
}

impl ConfigCache {
    /// An empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn file(&self, path: &Path, fs: &dyn FileSystem) -> Result<Arc<ParsedFile>, ConfigError> {
        let path = normalize(path);
        let cell = {
            let mut files = self.files.lock().unwrap_or_else(PoisonError::into_inner);
            Arc::clone(files.entry(path.clone()).or_default())
        };
        cell.get_or_init(|| ParsedFile::read(&path, fs).map(Arc::new))
            .as_ref()
            .map(Arc::clone)
            .map_err(ConfigError::duplicate)
    }

    fn dir(&self, dir: &Path, fs: &dyn FileSystem) -> DirInfo {
        let dir = normalize(dir);
        let cell = {
            let mut dirs = self.dirs.lock().unwrap_or_else(PoisonError::into_inner);
            Arc::clone(dirs.entry(dir.clone()).or_default())
        };
        cell.get_or_init(|| DirInfo::probe(&dir, fs)).clone()
    }
}

/// Resolves the configuration of a root: the chain of files that govern it, merged.
///
/// The chain is every `skif.toml` or `.skif.toml` from the root's directory up to the [`Ceiling`], stopping after a
/// file with `root = true`. Each file contributes the files it `extends` (paths relative to it, depth first, each
/// file once), then its own settings; the farthest file applies first and the nearest last. See
/// [`ResolvedConfig`] for the result.
pub struct ConfigLoader<'a> {
    fs: &'a dyn FileSystem,
    cache: &'a ConfigCache,
}

impl<'a> ConfigLoader<'a> {
    /// A loader reading through `fs` and remembering what it reads in `cache`.
    #[must_use]
    pub const fn new(fs: &'a dyn FileSystem, cache: &'a ConfigCache) -> Self {
        Self { fs, cache }
    }

    /// The configuration of `root` from the files of its chain, without CLI overrides.
    ///
    /// `root` must be an absolute path; `.` and `..` components are resolved lexically.
    ///
    /// # Errors
    /// [`ConfigError::Ambiguous`] when a directory of the chain has both file names, [`ConfigError::Io`] when a file
    /// or an extended file cannot be read, [`ConfigError::Parse`] / [`ConfigError::Invalid`] / [`ConfigError::Scope`]
    /// for bad contents (carrying the file, line and column), [`ConfigError::ExtendsCycle`] when files extend each
    /// other, and [`ConfigError::Invalid`] when `extends` nests more than 16 files deep.
    pub fn resolve(&self, root: &Path, ceiling: &Ceiling) -> Result<ResolvedConfig, ConfigError> {
        let root = &checked_root(root)?;
        skif_core::timing::phase(format_args!("config {}", root.display()), || {
            let files = self.discover(root, ceiling)?;
            self.resolve_files(root, files)
        })
    }

    /// Like [`resolve`](Self::resolve), honouring `--config` and `--no-config` and layering `overrides` on top
    /// (defaults < files < CLI).
    ///
    /// With [`ConfigOverrides::config_file`] the chain is that file alone (and what it extends); with
    /// [`ConfigOverrides::no_config`] no file is read.
    ///
    /// # Errors
    /// As [`resolve`](Self::resolve), plus [`ConfigError::Invalid`] for an override that is not a valid selector.
    pub fn load(
        &self,
        root: &Path,
        ceiling: &Ceiling,
        overrides: &ConfigOverrides,
    ) -> Result<ResolvedConfig, ConfigError> {
        let root = &checked_root(root)?;
        let mut resolved = if overrides.no_config {
            ResolvedConfig::default()
        } else if let Some(file) = &overrides.config_file {
            self.resolve_files(root, vec![(file.clone(), SourceKind::Explicit)])?
        } else {
            self.resolve(root, ceiling)?
        };
        resolved.config.apply(overrides)?;
        Ok(resolved)
    }

    /// Adds the package files of the member directories `members` (never the root) to `resolved`, each as an
    /// override scoped to its directory, outermost directory first, after everything `resolved` already holds.
    ///
    /// Call it with the member directories of the root's workspace once they are known. A directory without a
    /// configuration file adds nothing.
    ///
    /// # Errors
    /// [`ConfigError::Ambiguous`] when a member directory has both file names, [`ConfigError::Io`] /
    /// [`ConfigError::Parse`] / [`ConfigError::Invalid`] for an unreadable or invalid file, and
    /// [`ConfigError::Scope`] when the file has a root-scoped key, naming the key and its position.
    pub fn add_package_files(
        &self,
        resolved: &mut ResolvedConfig,
        root: &Path,
        members: &[PathBuf],
    ) -> Result<(), ConfigError> {
        let root = &checked_root(root)?;
        let mut members: Vec<&PathBuf> = members.iter().collect();
        members.sort_by_key(|dir| (dir.components().count(), (*dir).clone()));
        for dir in members {
            let path = match self.cache.dir(dir, self.fs).configs.as_slice() {
                [] => continue,
                [path] => path.clone(),
                [first, second, ..] => {
                    return Err(ConfigError::Ambiguous {
                        first: first.clone(),
                        second: second.clone(),
                    });
                }
            };
            let file = self.cache.file(&path, self.fs)?;
            let mut scoped = package_file::to_override(&file, &path)?;
            let source = resolved.sources.len();
            scoped.scope = Some(OverrideScope::new(&scoped, dir, source, root)?);
            resolved.sources.push(ConfigSource {
                path,
                kind: SourceKind::Package,
            });
            resolved.origins.push(
                format!("overrides[{}]", resolved.config.overrides.len()),
                Assignment {
                    source,
                    line: 1,
                    column: 1,
                    op: Op::Append,
                },
            );
            resolved.config.overrides.push(scoped);
        }
        Ok(())
    }

    /// The nearest directory at or above `start` holding a `.git` entry.
    fn repository_root(&self, start: &Path) -> Option<PathBuf> {
        start
            .ancestors()
            .find(|dir| self.cache.dir(dir, self.fs).has_git)
            .map(Path::to_path_buf)
    }

    /// The configuration files that govern `root`, farthest first.
    fn discover(&self, root: &Path, ceiling: &Ceiling) -> Result<Vec<(PathBuf, SourceKind)>, ConfigError> {
        let limit = match ceiling {
            Ceiling::Vcs => self.repository_root(root).unwrap_or_else(|| root.to_path_buf()),
            Ceiling::VcsOr(fallback) => self.repository_root(root).unwrap_or_else(|| normalize(fallback)),
            Ceiling::Dir(dir) => normalize(dir),
            Ceiling::RootOnly => root.to_path_buf(),
        };
        if !root.starts_with(&limit) {
            return Err(ConfigError::invalid(
                None,
                format!(
                    "the configuration ceiling {} is not the root {} or a directory above it",
                    limit.display(),
                    root.display()
                ),
            ));
        }
        let mut chain = Vec::new();
        for dir in root.ancestors().take_while(|dir| dir.starts_with(&limit)) {
            match self.cache.dir(dir, self.fs).configs.as_slice() {
                [] => {}
                [path] => {
                    let kind = if dir == root {
                        SourceKind::Root
                    } else {
                        SourceKind::Ancestor
                    };
                    chain.push((path.clone(), kind));
                    if self.cache.file(path, self.fs)?.root {
                        break;
                    }
                }
                [first, second, ..] => {
                    return Err(ConfigError::Ambiguous {
                        first: first.clone(),
                        second: second.clone(),
                    });
                }
            }
        }
        chain.reverse();
        Ok(chain)
    }

    fn resolve_files(&self, root: &Path, files: Vec<(PathBuf, SourceKind)>) -> Result<ResolvedConfig, ConfigError> {
        let mut walk = Walk {
            loader: self,
            seen: HashSet::new(),
            sources: Vec::new(),
            files: Vec::new(),
        };
        for (path, kind) in files {
            walk.visit(&path, kind, &mut Vec::new())?;
        }
        walk.merge(root)
    }
}

/// The depth-first expansion of `extends` over the files of a chain.
struct Walk<'l, 'a> {
    loader: &'l ConfigLoader<'a>,
    /// Files already applied: reached again (a diamond, or an extended file that is also an ancestor), they are
    /// skipped.
    seen: HashSet<PathBuf>,
    sources: Vec<ConfigSource>,
    files: Vec<Arc<ParsedFile>>,
}

impl Walk<'_, '_> {
    /// Adds `path` after the files it extends. `open` holds the files being expanded, to detect cycles.
    fn visit(&mut self, path: &Path, kind: SourceKind, open: &mut Vec<PathBuf>) -> Result<(), ConfigError> {
        let key = normalize(path);
        if let Some(at) = open.iter().position(|file| *file == key) {
            let mut chain = open[at..].to_vec();
            chain.push(key);
            return Err(ConfigError::ExtendsCycle { chain });
        }
        if self.seen.contains(&key) {
            return Ok(());
        }
        let file = self.loader.cache.file(&key, self.loader.fs)?;
        if matches!(kind, SourceKind::Extends { .. }) && file.root {
            return Err(ConfigError::scope(
                "root",
                Some(key.as_path()),
                file.position("root"),
                "`root` is only valid in a configuration file found by discovery, not in one that is extended",
            ));
        }
        if open.len() >= MAX_EXTENDS_DEPTH {
            return Err(ConfigError::invalid(
                Some(key.as_path()),
                format!("`extends` is nested more than {MAX_EXTENDS_DEPTH} files deep"),
            ));
        }
        open.push(key.clone());
        for (index, entry) in file.extends.iter().enumerate() {
            let target = normalize(&file.dir.join(entry));
            if !self.loader.fs.is_file(&target) {
                let (line, column) = file.position(&format!("extends[{index}]")).unwrap_or((0, 0));
                return Err(ConfigError::Io {
                    path: target,
                    source: io::Error::new(
                        io::ErrorKind::NotFound,
                        format!("no such file (extended from {}:{line}:{column})", key.display()),
                    ),
                });
            }
            self.visit(&target, SourceKind::Extends { by: key.clone() }, open)?;
        }
        open.pop();
        self.seen.insert(key.clone());
        self.sources.push(ConfigSource { path: key, kind });
        self.files.push(file);
        Ok(())
    }

    /// Merges the visited files, in order, into the configuration of `root`.
    fn merge(self, root: &Path) -> Result<ResolvedConfig, ConfigError> {
        let Self { sources, files, .. } = self;
        let mut merger = Merger::default();
        for (index, file) in files.iter().enumerate() {
            merger.apply(index, &file.table, &file.spans);
        }
        let Merged {
            table,
            origins,
            overrides,
        } = merger.finish();
        let mut config: Config = Value::Table(table)
            .try_into()
            .map_err(|error: toml::de::Error| ConfigError::invalid(None, error.message()))?;
        for (index, scoped) in config.overrides.iter_mut().enumerate() {
            let source = overrides.get(index).copied().unwrap_or_default();
            let declared_in = sources.get(source).and_then(|file| file.path.parent()).unwrap_or(root);
            let scope = OverrideScope::new(scoped, declared_in, source, root)?;
            scoped.scope = Some(scope);
        }
        config.validate()?;
        Ok(ResolvedConfig {
            config,
            sources,
            origins,
        })
    }
}

/// `root` without `.` components and with `..` resolved lexically, which every directory the chain probes is derived
/// from; a relative root is an error, since the ancestors of a relative path never reach the repository.
fn checked_root(root: &Path) -> Result<PathBuf, ConfigError> {
    if root.has_root() {
        Ok(normalize(root))
    } else {
        Err(ConfigError::invalid(
            None,
            format!("the root {} is not an absolute path", root.display()),
        ))
    }
}

/// `path` without `.` components and with `..` resolved lexically; a symlink is not followed, so a cycle through one
/// is only caught by the depth limit.
fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match normalized.components().next_back() {
                Some(Component::Normal(_)) => {
                    normalized.pop();
                }
                Some(Component::RootDir | Component::Prefix(_)) => {}
                _ => normalized.push(".."),
            },
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}
