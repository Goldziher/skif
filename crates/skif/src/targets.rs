//! Multi-root discovery: every workspace and standalone package below a directory, each with its own configuration.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use rayon::prelude::*;
use skif_config::{Ceiling, Config, ConfigCache, ConfigLoader, ConfigOverrides, ConfigSource, Origins};
use skif_core::FileSystem;
use skif_workspace::{
    DiscoverOptions, RootKind, Workspace, claim_roots, declared_member_dirs, declares_workspace, repository_root,
    scan_candidates,
};

use crate::error::Error;
use crate::root::{Located, config_ceiling, is_enclosed, locate, resolve_config};

/// One root to lint: a workspace or a standalone package, with the configuration that governs it.
#[derive(Debug)]
#[non_exhaustive]
pub struct Target {
    /// Absolute root directory.
    pub root: PathBuf,
    /// Every `skif.toml` or `.skif.toml` from the root up to the ceiling, with what they extend, merged and with the
    /// overrides applied; the defaults when there is none.
    pub config: Config,
    /// Absolute path of the nearest configuration file `config` was read from, when there is one.
    pub config_file: Option<PathBuf>,
    /// The files `config` was merged from, in the order they were applied.
    pub sources: Vec<ConfigSource>,
    /// Where each key of `config` was set, as indices into `sources`.
    pub origins: Origins,
    /// The discovered workspace (a standalone package is a workspace of one package).
    pub workspace: Workspace,
}

impl Target {
    /// The configuration file relative to [`root`](Self::root), for
    /// [`RunOptions::with_config_path`](crate::RunOptions::with_config_path): `skif.toml` when it sits in the root,
    /// `../skif.toml` when it was inherited from a parent directory.
    #[must_use]
    pub fn config_path(&self) -> Option<PathBuf> {
        let file = self.config_file.as_ref()?;
        let dir = file.parent()?;
        let up = self.root.strip_prefix(dir).ok()?.components().count();
        let mut path: PathBuf = std::iter::repeat_n("..", up).collect();
        path.push(file.file_name()?);
        Some(path)
    }
}

/// Loads the configuration for `root` from every config file in `root` or a parent up to the `ceiling`, merged (see
/// [`resolve_config`](crate::resolve_config)), and discovers the workspace at `root`.
///
/// A `skif.toml` in the directory of a workspace member (a package file) is applied as an override scoped to that
/// package, after the files of the chain (see [`ConfigLoader::add_package_files`]); `--no-config` skips them.
///
/// `cache` is shared by the roots of a run so each config file is read once; it may be used from several threads.
///
/// # Errors
/// [`Error::Config`] for an unreadable, ambiguous or invalid configuration, [`Error::Workspace`] when the root cannot
/// be discovered.
pub fn load_target(
    root: &Path,
    ceiling: &Ceiling,
    fs: &dyn FileSystem,
    overrides: &ConfigOverrides,
    cache: &ConfigCache,
) -> Result<Target, Error> {
    let mut resolved = resolve_config(root, ceiling, fs, overrides, cache)?;
    let workspace = Workspace::discover_with_fs(root, &resolved.config, fs)?;
    if !overrides.no_config {
        let members: Vec<PathBuf> = workspace.member_dirs().into_iter().collect();
        ConfigLoader::new(fs, cache).add_package_files(&mut resolved, root, &members)?;
    }
    let file = resolved.nearest_file().map(Path::to_path_buf);
    Ok(Target {
        root: root.to_path_buf(),
        config: resolved.config,
        config_file: file,
        sources: resolved.sources,
        origins: resolved.origins,
        workspace,
    })
}

/// Loads `root` with the configuration its repository gives it: the config files at and above `root` up to the root
/// of the git repository, or `root` alone outside one.
fn load_in_repository(
    root: &Path,
    fs: &dyn FileSystem,
    overrides: &ConfigOverrides,
    cache: &ConfigCache,
) -> Result<Target, Error> {
    load_target(root, &Ceiling::Vcs, fs, overrides, cache)
}

/// Finds the workspace that `start` belongs to and loads it, discovering it once.
///
/// The root is the one [`find_workspace_root`](crate::find_workspace_root) names. Its configuration is every config
/// file at or above it, up to the root of the git repository holding it (the root itself outside one), merged, which
/// is the configuration [`discover_all`] gives the same directory. A standalone package with a project above it in the
/// same repository is marked [`Workspace::is_nested`], as [`discover_targets`] marks it.
///
/// # Errors
/// [`Error::RelativePath`] for a relative `start`, [`Error::NoWorkspace`] when `start` and its ancestors hold no
/// project, otherwise [`load_target`]'s errors for the chosen root. An ancestor workspace that lists the project but
/// cannot be loaded is that error, not a silent fallback to the project alone.
pub fn find_target(start: &Path, fs: &dyn FileSystem, overrides: &ConfigOverrides) -> Result<Target, Error> {
    let Located { project, ancestors } = locate(start, fs)?;
    let cache = ConfigCache::new();
    if declares_workspace(fs, &project) {
        return load_in_repository(&project, fs, overrides, &cache);
    }
    for ancestor in ancestors {
        match load_in_repository(&ancestor, fs, overrides, &cache) {
            Ok(target) if target.workspace.member_dirs().contains(&project) => return Ok(target),
            Err(error) if declared_member_dirs(fs, &ancestor).contains(&project) => return Err(error),
            _ => {}
        }
    }
    let mut target = load_in_repository(&project, fs, overrides, &cache)?;
    if !declares_workspace(fs, &project) {
        target
            .workspace
            .set_nested(is_enclosed(&project, &config_ceiling(&project, fs), fs));
    }
    Ok(target)
}

/// A root that could not be loaded.
#[derive(Debug)]
#[non_exhaustive]
pub struct RootFailure {
    /// The root directory.
    pub root: PathBuf,
    /// Why it could not be loaded.
    pub error: Error,
}

/// The outcome of [`discover_targets`].
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct Discovery {
    /// The roots that loaded, sorted by path.
    pub targets: Vec<Target>,
    /// The roots that did not, sorted by path.
    pub failures: Vec<RootFailure>,
    /// Directories at the depth limit with subdirectories the walk did not enter.
    pub truncated: Vec<PathBuf>,
}

/// Finds every workspace root and standalone package at or below `start` and loads each as a [`Target`], sorted by
/// path (see [`skif_workspace::discover_roots`] for what counts as a root).
///
/// Each workspace is discovered once, with the configuration that governs it, and the directories it lists decide
/// which packages are claimed. A root's configuration is every config file at or above it, up to the root of the git
/// repository holding it (`start` outside one), merged farthest first, so a project file adds to the organisation
/// file above it unless it says `root = true`. The config files are read once for all the roots. A workspace that is listed as a member of an outer workspace
/// is a root of its own and the outer one leaves its package to it, so every package is linted by exactly one root.
///
/// A standalone package below another root is marked [`Workspace::is_nested`], which the rules that only concern a
/// repository's outermost project (`root-package-manager-field`) honour. Directories matching
/// `[workspace] discover-ignore` (by default `__fixtures__`, `fixtures`, `__mocks__` and `__snapshots__`) are not
/// searched for roots; the setting is read from the configuration file that governs `start`. A workspace declared
/// by `pnpm-workspace.yaml` or `rush.json` alone, with no root `package.json`, loads as a workspace without a root
/// manifest.
///
/// A root that cannot be loaded is a [`RootFailure`]; the others still load. A workspace that cannot be loaded
/// still claims the directories its patterns declare.
///
/// # Errors
/// [`Error::RelativePath`] for a relative `start`.
pub fn discover_targets(start: &Path, fs: &dyn FileSystem, overrides: &ConfigOverrides) -> Result<Discovery, Error> {
    if !start.has_root() {
        return Err(Error::RelativePath(start.to_path_buf()));
    }
    let dir = if fs.is_dir(start) {
        start
    } else {
        start.parent().unwrap_or(start)
    };
    let cache = ConfigCache::new();
    let ceiling = Ceiling::VcsOr(dir.to_path_buf());
    let enclosing = |root: &Path| repository_ceiling(root, dir, fs);
    let candidates = scan_candidates(dir, fs, &discover_options(dir, fs, overrides, &cache));
    let mut workspaces: BTreeMap<PathBuf, Result<Target, Error>> = candidates
        .workspaces
        .par_iter()
        .map(|root| (root.clone(), load_target(root, &ceiling, fs, overrides, &cache)))
        .collect();
    let members: BTreeMap<PathBuf, BTreeSet<PathBuf>> = workspaces
        .iter()
        .map(|(root, loaded)| {
            let dirs = match loaded {
                Ok(target) => target.workspace.member_dirs(),
                Err(_) => declared_member_dirs(fs, root),
            };
            (root.clone(), dirs)
        })
        .collect();
    let claimed = claim_roots(fs, &candidates, &members);
    let slots: Vec<(PathBuf, Option<Result<Target, Error>>)> = claimed
        .iter()
        .map(|root| (root.path().to_path_buf(), workspaces.remove(root.path())))
        .collect();
    let loaded: Vec<(PathBuf, Result<Target, Error>)> = slots
        .into_par_iter()
        .map(|(root, found)| {
            let result = found.unwrap_or_else(|| load_target(&root, &ceiling, fs, overrides, &cache));
            (root, result)
        })
        .collect();
    let mut discovery = Discovery {
        truncated: candidates.truncated,
        ..Discovery::default()
    };
    let root_dirs: BTreeSet<&Path> = claimed.iter().map(RootKind::path).collect();
    for (root, result) in loaded {
        match result {
            Ok(mut target) => {
                let nested = !candidates.workspaces.contains(&root)
                    && (root.ancestors().skip(1).any(|ancestor| root_dirs.contains(ancestor))
                        || is_enclosed(&root, &enclosing(&root), fs));
                target.workspace.set_nested(nested);
                target.workspace.ignore_dirs(&candidates.workspaces);
                discovery.targets.push(target);
            }
            Err(error) => discovery.failures.push(RootFailure { root, error }),
        }
    }
    Ok(discovery)
}

/// Why a configuration file below a root was not applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum UnappliedReason {
    /// The file is not in the root's own directory or in the directory of one of its workspace members, the only
    /// places configuration is read from.
    NotAMember,
    /// The file is in a member's directory but was not loaded for this run.
    NotLoaded,
}

impl UnappliedReason {
    /// A short sentence for output.
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::NotAMember => "not in the root or a workspace member directory",
            Self::NotLoaded => "in a workspace member directory but not loaded for this run",
        }
    }
}

/// A `skif.toml` below a root that no configuration of that root was built from.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct UnappliedConfig {
    /// Absolute path of the file.
    pub path: PathBuf,
    pub reason: UnappliedReason,
}

/// The `skif.toml` and `.skif.toml` files below `target.root` that its configuration was not built from, sorted by
/// path.
///
/// Configuration is read from the root, the directories above it, what those files extend, and the directory of each
/// workspace member (a package file). Any other file is reported here with the reason. The walk skips `node_modules`,
/// hidden directories and what git ignores, but enters directories `workspace.discover-ignore` skips (a fixture
/// directory is a likely home for such a file). A file in the directory of a deeper root of the same run
/// (`other_roots`) belongs to that root and is not reported.
#[must_use]
pub fn unapplied_config_files(target: &Target, other_roots: &[&Path], fs: &dyn FileSystem) -> Vec<UnappliedConfig> {
    let candidates = scan_candidates(&target.root, fs, &DiscoverOptions::default().with_ignore(Vec::new()));
    let members = target.workspace.member_dirs();
    candidates
        .config_files
        .into_iter()
        .filter_map(|path| {
            let dir = path.parent()?;
            let owned_elsewhere = other_roots
                .iter()
                .any(|other| *other != target.root && other.starts_with(&target.root) && dir.starts_with(other));
            if dir == target.root || owned_elsewhere || target.sources.iter().any(|source| source.path == path) {
                return None;
            }
            let reason = if members.contains(dir) {
                UnappliedReason::NotLoaded
            } else {
                UnappliedReason::NotAMember
            };
            Some(UnappliedConfig { path, reason })
        })
        .collect()
}

/// The bounds of the walk started at `start`: the defaults, with the skipped directories from the configuration that
/// governs `start` when it loads (a broken file is reported by the root that uses it, not here).
fn discover_options(
    start: &Path,
    fs: &dyn FileSystem,
    overrides: &ConfigOverrides,
    cache: &ConfigCache,
) -> DiscoverOptions {
    match resolve_config(start, &Ceiling::Vcs, fs, overrides, cache) {
        Ok(resolved) => DiscoverOptions::default().with_ignore(resolved.config.workspace.discover_ignore),
        Err(_) => DiscoverOptions::default(),
    }
}

/// The directory the configuration search of `root` stops at in a run started at `start`.
fn repository_ceiling(root: &Path, start: &Path, fs: &dyn FileSystem) -> PathBuf {
    repository_root(fs, root).unwrap_or_else(|| start.to_path_buf())
}

/// Like [`discover_targets`], failing with the first root that cannot be loaded.
///
/// # Errors
/// [`Error::RelativePath`] for a relative `start`, otherwise the first failing root's [`load_target`] error.
///
/// # Examples
/// ```
/// use std::path::Path;
/// use skif::{ConfigOverrides, MemoryFs, discover_all};
///
/// let fs = MemoryFs::new()
///     .with_file("/r/a/package.json", r#"{"name": "a", "version": "1.0.0"}"#)
///     .with_file("/r/b/package.json", r#"{"name": "b", "version": "1.0.0"}"#);
/// let targets = discover_all(Path::new("/r"), &fs, &ConfigOverrides::default())?;
/// assert_eq!(targets.len(), 2);
/// # Ok::<(), skif::Error>(())
/// ```
pub fn discover_all(start: &Path, fs: &dyn FileSystem, overrides: &ConfigOverrides) -> Result<Vec<Target>, Error> {
    let discovery = discover_targets(start, fs, overrides)?;
    match discovery.failures.into_iter().next() {
        Some(failure) => Err(failure.error),
        None => Ok(discovery.targets),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use skif_core::MemoryFs;
    use skif_workspace::PackageManager;

    use super::*;

    fn fs() -> MemoryFs {
        MemoryFs::new()
            .with_file("/r/skif.toml", "[lint]\nfail-on-warnings = true\n")
            .with_file("/r/a/package.json", r#"{"name": "a", "version": "1.0.0"}"#)
            .with_file("/r/b/skif.toml", "")
            .with_file("/r/b/package.json", r#"{"name": "b", "version": "1.0.0"}"#)
    }

    #[test]
    fn every_config_up_to_the_start_directory_contributes() {
        let targets = discover_all(Path::new("/r"), &fs(), &ConfigOverrides::default()).unwrap();
        let roots: Vec<_> = targets.iter().map(|target| target.root.clone()).collect();
        assert_eq!(roots, [PathBuf::from("/r/a"), PathBuf::from("/r/b")]);
        assert!(targets[0].config.lint.fail_on_warnings);
        assert_eq!(targets[0].config_path(), Some(PathBuf::from("../skif.toml")));
        assert_eq!(targets[0].sources.len(), 1);
        assert!(targets[1].config.lint.fail_on_warnings);
        assert_eq!(targets[1].config_path(), Some(PathBuf::from("skif.toml")));
        assert_eq!(targets[1].sources.len(), 2);
    }

    #[test]
    fn config_above_the_start_directory_is_not_inherited() {
        let targets = discover_all(Path::new("/r/a"), &fs(), &ConfigOverrides::default()).unwrap();
        assert_eq!(targets.len(), 1);
        assert!(!targets[0].config.lint.fail_on_warnings);
        assert_eq!(targets[0].config_path(), None);
    }

    #[test]
    fn relative_start_is_rejected() {
        assert!(matches!(
            discover_all(Path::new("r"), &fs(), &ConfigOverrides::default()),
            Err(Error::RelativePath(_))
        ));
    }

    fn names(discovery: &Discovery) -> Vec<String> {
        discovery
            .targets
            .iter()
            .map(|target| target.root.display().to_string().replace('\\', "/"))
            .collect()
    }

    fn discover(fs: &MemoryFs, start: &str) -> Discovery {
        discover_targets(Path::new(start), fs, &ConfigOverrides::default()).unwrap()
    }

    fn packages_of(target: &Target) -> Vec<String> {
        target
            .workspace
            .iter()
            .filter(|(id, _)| !target.workspace.is_ignored(*id))
            .filter_map(|(_, package)| package.name.clone())
            .collect()
    }

    fn divergent_declarations() -> MemoryFs {
        MemoryFs::new()
            .with_file("/r/package.json", r#"{"name": "root", "workspaces": ["npmpkgs/*"]}"#)
            .with_file("/r/pnpm-workspace.yaml", "packages:\n  - 'yarnpkgs/*'\n")
            .with_file("/r/skif.toml", "[workspace]\npackage-manager = \"npm\"\n")
            .with_file("/r/npmpkgs/x/package.json", r#"{"name": "x"}"#)
            .with_file("/r/yarnpkgs/y/package.json", r#"{"name": "y"}"#)
    }

    #[test]
    fn members_are_claimed_with_the_root_s_own_configuration() {
        let discovery = discover(&divergent_declarations(), "/r");
        assert_eq!(names(&discovery), ["/r", "/r/yarnpkgs/y"]);
        assert_eq!(packages_of(&discovery.targets[0]), ["root", "x"]);
        assert_eq!(packages_of(&discovery.targets[1]), ["y"]);
    }

    #[test]
    fn a_nested_workspace_is_a_root_and_its_members_are_linted_once() {
        let fs = MemoryFs::new()
            .with_file("/r/package.json", r#"{"name": "root", "workspaces": ["packages/*"]}"#)
            .with_file(
                "/r/packages/inner/package.json",
                r#"{"name": "inner", "workspaces": ["x/*"]}"#,
            )
            .with_file("/r/packages/inner/x/y/package.json", r#"{"name": "y"}"#)
            .with_file("/r/packages/plain/package.json", r#"{"name": "plain"}"#);
        let discovery = discover(&fs, "/r");
        assert_eq!(names(&discovery), ["/r", "/r/packages/inner"]);
        assert_eq!(packages_of(&discovery.targets[0]), ["root", "plain"]);
        assert_eq!(packages_of(&discovery.targets[1]), ["inner", "y"]);

        let direct = find_target(Path::new("/r/packages/inner"), &fs, &ConfigOverrides::default()).unwrap();
        assert_eq!(direct.root, Path::new("/r/packages/inner"));
        assert_eq!(packages_of(&direct), ["inner", "y"]);
    }

    #[test]
    fn a_root_that_fails_is_reported_and_the_others_still_load() {
        let fs = MemoryFs::new()
            .with_file("/r/bad/package.json", r#"{"name": "#)
            .with_file("/r/good/package.json", r#"{"name": "good", "version": "1.0.0"}"#)
            .with_file("/r/ws/pnpm-workspace.yaml", "packages:\n  - 'p/*'\n")
            .with_file("/r/ws/p/m/package.json", r#"{"name": "m"}"#);
        let discovery = discover(&fs, "/r");
        assert_eq!(names(&discovery), ["/r/good", "/r/ws"]);
        let failed: Vec<_> = discovery.failures.iter().map(|failure| failure.root.clone()).collect();
        assert_eq!(failed, [PathBuf::from("/r/bad")]);
        assert!(discover_all(Path::new("/r"), &fs, &ConfigOverrides::default()).is_err());
    }

    #[test]
    fn the_loaded_workspace_carries_its_own_members_and_configuration() {
        let discovery = discover(&divergent_declarations(), "/r");
        let root = &discovery.targets[0];
        assert_eq!(root.workspace.member_dirs().len(), 1);
        assert_eq!(root.config_path(), Some(PathBuf::from("skif.toml")));
    }

    fn nested_project_in_a_workspace_tree() -> MemoryFs {
        MemoryFs::new()
            .with_file("/r/.git/HEAD", "ref")
            .with_file("/r/I/skif.toml", "[lint]\nfail-on-warnings = true\n")
            .with_file("/r/I/package.json", r#"{"name": "i", "workspaces": ["pkgs/*"]}"#)
            .with_file("/r/I/pkgs/a/package.json", r#"{"name": "a"}"#)
            .with_file("/r/I/examples/e/package.json", r#"{"name": "e"}"#)
    }

    #[test]
    fn a_project_gets_the_same_configuration_whichever_way_it_is_reached() {
        let fs = nested_project_in_a_workspace_tree();
        let overrides = ConfigOverrides::default();
        let discovery = discover(&fs, "/r/I");
        let by_discovery = discovery
            .targets
            .iter()
            .find(|target| target.root == Path::new("/r/I/examples/e"))
            .unwrap();
        let direct = find_target(Path::new("/r/I/examples/e"), &fs, &overrides).unwrap();
        assert!(by_discovery.config.lint.fail_on_warnings);
        assert_eq!(direct.config, by_discovery.config);
        assert_eq!(direct.config_path(), Some(PathBuf::from("../../skif.toml")));
    }

    #[test]
    fn the_configuration_search_climbs_to_the_repository_root_and_no_higher() {
        let fs = nested_project_in_a_workspace_tree()
            .with_file("/skif.toml", "[lint]\nfail-on-warnings = true\n")
            .with_file("/r/skif.toml", "")
            .with_file("/r/solo/package.json", r#"{"name": "solo"}"#);
        let found = find_target(Path::new("/r/solo"), &fs, &ConfigOverrides::default()).unwrap();
        assert!(!found.config.lint.fail_on_warnings);
        assert_eq!(found.config_file, Some(PathBuf::from("/r/skif.toml")));
        assert_eq!(found.config_path(), Some(PathBuf::from("../skif.toml")));
    }

    #[test]
    fn find_target_returns_the_ancestor_that_lists_the_project() {
        let fs = divergent_declarations();
        let target = find_target(Path::new("/r/npmpkgs/x"), &fs, &ConfigOverrides::default()).unwrap();
        assert_eq!(target.root, Path::new("/r"));
        let own = find_target(Path::new("/r/yarnpkgs/y"), &fs, &ConfigOverrides::default()).unwrap();
        assert_eq!(own.root, Path::new("/r/yarnpkgs/y"));
    }

    #[test]
    fn an_ancestor_that_lists_the_project_but_cannot_load_is_an_error() {
        let fs = MemoryFs::new()
            .with_file("/r/package.json", r#"{"workspaces": ["p/*"]"#)
            .with_file("/r/pnpm-workspace.yaml", "packages:\n  - 'p/*'\n")
            .with_file("/r/p/m/package.json", r#"{"name": "m"}"#);
        let error = find_target(Path::new("/r/p/m"), &fs, &ConfigOverrides::default()).unwrap_err();
        assert!(matches!(error, Error::Workspace(_)), "{error}");
    }

    #[test]
    fn only_standalone_packages_below_another_root_are_nested() {
        let fs = MemoryFs::new()
            .with_file("/s/a/package.json", r#"{"name": "a"}"#)
            .with_file("/s/b/package.json", r#"{"name": "b"}"#)
            .with_file("/s/b/inner/package.json", r#"{"name": "inner"}"#)
            .with_file("/s/w/package.json", r#"{"name": "w", "workspaces": ["p/*"]}"#)
            .with_file("/s/w/p/m/package.json", r#"{"name": "m"}"#)
            .with_file("/s/w/examples/e/package.json", r#"{"name": "e"}"#)
            .with_file("/s/w/examples/e/deeper/pnpm-workspace.yaml", "packages:\n  - 'q/*'\n")
            .with_file("/s/w/examples/e/deeper/q/n/package.json", r#"{"name": "n"}"#);
        let discovery = discover(&fs, "/s");
        let nested: Vec<_> = discovery
            .targets
            .iter()
            .map(|target| {
                (
                    target.root.display().to_string().replace('\\', "/"),
                    target.workspace.is_nested(),
                )
            })
            .collect();
        assert_eq!(
            nested,
            [
                ("/s/a".to_owned(), false),
                ("/s/b".to_owned(), false),
                ("/s/b/inner".to_owned(), true),
                ("/s/w".to_owned(), false),
                ("/s/w/examples/e".to_owned(), true),
                ("/s/w/examples/e/deeper".to_owned(), false),
            ]
        );
    }

    #[test]
    fn a_single_root_is_never_nested() {
        let fs = MemoryFs::new()
            .with_file("/r/package.json", r#"{"name": "r"}"#)
            .with_file("/r/inner/package.json", r#"{"name": "inner"}"#);
        let target = find_target(Path::new("/r/inner"), &fs, &ConfigOverrides::default()).unwrap();
        assert!(!target.workspace.is_nested());
    }

    #[test]
    fn a_workspace_file_without_a_root_manifest_is_a_workspace_and_not_a_failure() {
        let fs = MemoryFs::new()
            .with_file("/r/ws/pnpm-workspace.yaml", "packages:\n  - 'p/*'\n")
            .with_file("/r/ws/p/m/package.json", r#"{"name": "m"}"#);
        let discovery = discover(&fs, "/r");
        assert!(discovery.failures.is_empty(), "{:?}", discovery.failures);
        assert_eq!(names(&discovery), ["/r/ws"]);
        assert!(!discovery.targets[0].workspace.has_root_manifest());
        assert_eq!(packages_of(&discovery.targets[0]), ["m"]);

        let direct = find_target(Path::new("/r/ws"), &fs, &ConfigOverrides::default()).unwrap();
        assert_eq!(direct.root, Path::new("/r/ws"));
        assert_eq!(packages_of(&direct), ["m"]);
    }

    #[test]
    fn a_rush_workspace_is_one_root_with_its_projects_as_members() {
        let fs = MemoryFs::new()
            .with_file(
                "/r/rush.json",
                r#"{"npmVersion": "10.0.0", "projects": [{"packageName": "a", "projectFolder": "apps/a"}]}"#,
            )
            .with_file("/r/apps/a/package.json", r#"{"name": "a"}"#)
            .with_file("/r/apps/a/examples/e/package.json", r#"{"name": "e"}"#);
        let discovery = discover(&fs, "/r");
        assert_eq!(names(&discovery), ["/r"]);
        assert_eq!(packages_of(&discovery.targets[0]), ["a"]);
        assert_eq!(
            discovery.targets[0].workspace.package_manager,
            Some(PackageManager::Npm)
        );
    }

    #[test]
    fn fixture_directories_are_skipped_unless_the_configuration_says_otherwise() {
        let tree = MemoryFs::new()
            .with_file("/r/real/package.json", r#"{"name": "real"}"#)
            .with_file("/r/real/fixtures/f/package.json", r#"{"name": "f"}"#)
            .with_file("/r/examples/e/package.json", r#"{"name": "e"}"#);
        assert_eq!(names(&discover(&tree, "/r")), ["/r/examples/e", "/r/real"]);

        let replaced = tree
            .clone()
            .with_file("/r/skif.toml", "[workspace]\ndiscover-ignore = [\"examples\"]\n");
        assert_eq!(names(&discover(&replaced, "/r")), ["/r/real", "/r/real/fixtures/f"]);

        let none = tree.with_file("/r/skif.toml", "[workspace]\ndiscover-ignore = []\n");
        assert_eq!(
            names(&discover(&none, "/r")),
            ["/r/examples/e", "/r/real", "/r/real/fixtures/f"]
        );
    }
}
