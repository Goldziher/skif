//! `baseUrl`, `paths` and `rootDirs` of the nearest `tsconfig.json` or `jsconfig.json` at or above a file.
//!
//! The resolver applies a `tsconfig.json` only to the files it includes, and never reads a `jsconfig.json`. Editors
//! and bundlers apply the nearest config to everything below it, including directories that are not packages
//! (`packages/tsconfig.json`, `src/legacy/jsconfig.json`), test files a config excludes and JavaScript files.

use skif_core::HashMap;
use std::cmp::Reverse;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, PoisonError, RwLock};

use skif_core::FileSystem;
use skif_js_syntax::ConfigValue;

use crate::deps::parse_tsconfig;
use crate::resolve::join_normalized;

/// How many `extends` links are followed.
const MAX_EXTENDS: usize = 8;

/// The placeholder TypeScript 5.5 replaces with the directory of the configuration that is being resolved.
const CONFIG_DIR: &str = "${configDir}";

/// `paths` entries: the pattern and its targets.
type PathEntries = Vec<(String, Vec<String>)>;

/// The module resolution options a config sets directly or through `extends`.
#[derive(Clone, Default)]
struct Options {
    base_url: Option<PathBuf>,
    /// The config file that set `paths` (the config itself or one it extends), and the entries.
    paths: Option<(PathBuf, PathEntries)>,
    /// Directories that are merged into one virtual directory for relative imports.
    root_dirs: Vec<PathBuf>,
}

/// The `baseUrl`, `paths` and `rootDirs` that apply to a directory.
pub(crate) struct TsPaths {
    /// The config file found at or above the directory.
    config: PathBuf,
    /// The config file that declares `paths`: `config` or a config it extends.
    paths_config: PathBuf,
    base_url: Option<PathBuf>,
    paths_dir: PathBuf,
    paths: PathEntries,
    root_dirs: Vec<PathBuf>,
}

fn json_file(path: &str) -> String {
    if Path::new(path)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
    {
        path.to_owned()
    } else {
        format!("{path}.json")
    }
}

/// `relative` below `base`; a `relative` that is absolute (a `${configDir}` target) stands on its own.
fn join_any(base: &Path, relative: &str) -> PathBuf {
    let path = Path::new(relative);
    if !path.is_absolute() {
        return join_normalized(base, relative);
    }
    let mut joined = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => joined.push(component),
            Component::CurDir => {}
            Component::ParentDir => {
                joined.pop();
            }
            Component::Normal(name) => joined.push(name),
        }
    }
    joined
}

fn with_config_dir(value: &str, config_dir: &Path) -> String {
    if value.contains(CONFIG_DIR) {
        let dir = config_dir.to_string_lossy().replace('\\', "/");
        value.replace(CONFIG_DIR, dir.trim_end_matches('/'))
    } else {
        value.to_owned()
    }
}

/// Splits `@scope/name/rest` or `name/rest` into the package name and the subpath (possibly empty).
fn split_package(specifier: &str) -> (&str, &str) {
    let name_end = if specifier.starts_with('@') {
        specifier
            .match_indices('/')
            .nth(1)
            .map_or(specifier.len(), |(index, _)| index)
    } else {
        specifier.find('/').unwrap_or(specifier.len())
    };
    let (name, rest) = specifier.split_at(name_end);
    (name, rest.trim_start_matches('/'))
}

/// The config file an `extends` entry of the config in `dir` names: a relative path, or a package (workspace member or
/// installed) with an optional subpath. `None` when nothing is found.
pub(crate) fn resolve_extends(
    fs: &dyn FileSystem,
    members: &[(String, PathBuf)],
    dir: &Path,
    parent: &str,
) -> Option<PathBuf> {
    if parent.starts_with('.') || Path::new(parent).is_absolute() {
        return Some(join_any(dir, &json_file(parent)));
    }
    let (name, subpath) = split_package(parent);
    let in_package = |package_dir: &Path| -> Option<PathBuf> {
        let candidates = if subpath.is_empty() {
            vec![package_dir.join("tsconfig.json")]
        } else {
            vec![
                join_normalized(package_dir, &json_file(subpath)),
                join_normalized(package_dir, &format!("{subpath}/tsconfig.json")),
            ]
        };
        candidates.into_iter().find(|candidate| fs.is_file(candidate))
    };
    members
        .iter()
        .filter(|(member, _)| member == name)
        .find_map(|(_, member_dir)| in_package(member_dir))
        .or_else(|| {
            dir.ancestors()
                .find_map(|ancestor| in_package(&ancestor.join("node_modules").join(name)))
        })
}

/// The entries of the `extends` of `config`: one string or an array of them.
fn extends_entries(config: &ConfigValue) -> Vec<&str> {
    match config.get("extends") {
        Some(ConfigValue::String(parent)) => vec![parent.as_str()],
        Some(ConfigValue::Array(items)) => items.iter().filter_map(ConfigValue::as_str).collect(),
        _ => Vec::new(),
    }
}

/// The `baseUrl` and `paths` configs of a workspace, parsed once per config file.
/// A parsed config's file and the directory `${configDir}` stands for.
type ConfigKey = (PathBuf, PathBuf);

pub(crate) struct TsConfigs {
    fs: Arc<dyn FileSystem>,
    root: PathBuf,
    /// Workspace members by name, with their absolute directory, for `extends` that names a package.
    members: Vec<(String, PathBuf)>,
    /// `[js] tsconfig`: read at the workspace root in place of `tsconfig.json`.
    root_config: Option<PathBuf>,
    /// Parsed configs by file and the directory `${configDir}` stands for.
    loaded: RwLock<HashMap<ConfigKey, Option<Arc<Options>>>>,
}

impl TsConfigs {
    pub(crate) fn new(fs: Arc<dyn FileSystem>, root: PathBuf, members: Vec<(String, PathBuf)>) -> Self {
        Self {
            fs,
            root,
            members,
            root_config: None,
            loaded: RwLock::new(HashMap::default()),
        }
    }

    /// Reads `config` at the workspace root in place of its `tsconfig.json`.
    pub(crate) fn with_root_config(mut self, config: Option<PathBuf>) -> Self {
        self.root_config = config;
        self
    }

    /// The configs looked for in `dir`, in order of preference.
    fn candidates(&self, dir: &Path) -> Vec<PathBuf> {
        let tsconfig = match &self.root_config {
            Some(config) if dir == self.root => config.clone(),
            _ => dir.join("tsconfig.json"),
        };
        vec![tsconfig, dir.join("jsconfig.json")]
    }

    fn load(&self, file: &Path, config_dir: &Path, depth: usize) -> Option<Arc<Options>> {
        let key = (file.to_path_buf(), config_dir.to_path_buf());
        if let Some(known) = self.loaded.read().unwrap_or_else(PoisonError::into_inner).get(&key) {
            return known.clone();
        }
        let options = self.parse(file, config_dir, depth).map(Arc::new);
        self.loaded
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key, options.clone());
        options
    }

    fn parse(&self, file: &Path, config_dir: &Path, depth: usize) -> Option<Options> {
        let config = parse_tsconfig(self.fs.as_ref(), file)?;
        let dir = file.parent()?;
        let mut options = Options::default();
        if depth < MAX_EXTENDS {
            // Later entries override earlier ones.
            for parent in extends_entries(&config) {
                if let Some(path) = resolve_extends(self.fs.as_ref(), &self.members, dir, parent)
                    && let Some(inherited) = self.load(&path, config_dir, depth + 1)
                {
                    options.base_url = inherited.base_url.clone().or(options.base_url);
                    options.paths = inherited.paths.clone().or(options.paths);
                    if !inherited.root_dirs.is_empty() {
                        options.root_dirs.clone_from(&inherited.root_dirs);
                    }
                }
            }
        }
        if let Some(compiler) = config.get("compilerOptions") {
            if let Some(base_url) = compiler.get("baseUrl").and_then(ConfigValue::as_str) {
                options.base_url = Some(join_any(dir, &with_config_dir(base_url, config_dir)));
            }
            if let Some(root_dirs) = compiler.get("rootDirs").and_then(ConfigValue::as_array) {
                options.root_dirs = root_dirs
                    .iter()
                    .filter_map(ConfigValue::as_str)
                    .map(|root| join_any(dir, &with_config_dir(root, config_dir)))
                    .collect();
            }
            if let Some(paths) = compiler.get("paths").and_then(ConfigValue::as_object) {
                let entries = paths
                    .iter()
                    .map(|(pattern, targets)| {
                        let targets = match targets {
                            ConfigValue::String(target) => vec![with_config_dir(target, config_dir)],
                            ConfigValue::Array(items) => items
                                .iter()
                                .filter_map(ConfigValue::as_str)
                                .map(|target| with_config_dir(target, config_dir))
                                .collect(),
                            _ => Vec::new(),
                        };
                        (pattern.clone(), targets)
                    })
                    .collect();
                options.paths = Some((file.to_path_buf(), entries));
            }
        }
        Some(options)
    }

    /// The files that path `extends` entries of the nearest config at or above `dir`, not above the workspace root,
    /// or of a config it extends, name and that do not exist: configs a tool generates before the project is
    /// type-checked (`./.svelte-kit/tsconfig.json`, also when a nested config extends the root one that names it).
    pub(crate) fn absent_extends(&self, dir: &Path) -> Vec<PathBuf> {
        let mut pending: Vec<PathBuf> = dir
            .ancestors()
            .take_while(|ancestor| ancestor.starts_with(&self.root))
            .flat_map(|ancestor| self.candidates(ancestor))
            .find(|file| self.fs.is_file(file))
            .into_iter()
            .collect();
        let mut absent = Vec::new();
        for _ in 0..MAX_EXTENDS {
            let mut extended = Vec::new();
            for file in &pending {
                let (Some(config), Some(config_dir)) = (parse_tsconfig(self.fs.as_ref(), file), file.parent()) else {
                    continue;
                };
                for parent in extends_entries(&config) {
                    match resolve_extends(self.fs.as_ref(), &self.members, config_dir, parent) {
                        Some(path) if self.fs.is_file(&path) => extended.push(path),
                        Some(path) => absent.push(path),
                        None => {}
                    }
                }
            }
            if extended.is_empty() {
                break;
            }
            pending = extended;
        }
        absent
    }

    /// The nearest config at or above `dir`, not above the workspace root, that sets `baseUrl` or `paths`.
    pub(crate) fn nearest(&self, dir: &Path) -> Option<TsPaths> {
        dir.ancestors()
            .take_while(|ancestor| ancestor.starts_with(&self.root))
            .find_map(|ancestor| {
                self.candidates(ancestor).into_iter().find_map(|file| {
                    if !self.fs.is_file(&file) {
                        return None;
                    }
                    let options = self.load(&file, file.parent()?, 0)?;
                    if options.base_url.is_none() && options.paths.is_none() && options.root_dirs.is_empty() {
                        return None;
                    }
                    let (paths_config, paths) = options.paths.clone().unwrap_or_else(|| (file.clone(), Vec::new()));
                    let paths_dir = paths_config.parent().unwrap_or(ancestor).to_path_buf();
                    Some(TsPaths {
                        config: file.clone(),
                        paths_config,
                        base_url: options.base_url.clone(),
                        paths_dir,
                        paths,
                        root_dirs: options.root_dirs.clone(),
                    })
                })
            })
    }
}

/// The part of `specifier` a `paths` pattern captures, with the length of the fixed prefix (an exact pattern ranks
/// above every wildcard).
pub(crate) fn match_pattern<'a>(pattern: &str, specifier: &'a str) -> Option<(usize, &'a str)> {
    match pattern.split_once('*') {
        None => (pattern == specifier).then_some((usize::MAX, "")),
        Some((prefix, suffix)) => {
            let captured = specifier.strip_prefix(prefix)?.strip_suffix(suffix)?;
            Some((prefix.len(), captured))
        }
    }
}

impl TsPaths {
    /// The `rootDirs`, absolute, in the order of the config.
    pub(crate) fn root_dirs(&self) -> &[PathBuf] {
        &self.root_dirs
    }

    /// The files a relative `specifier` written in `directory` may stand for through `rootDirs`: the same relative
    /// path below each of the other root directories, in the order of the config.
    pub(crate) fn root_dir_candidates(&self, directory: &Path, specifier: &str) -> Vec<PathBuf> {
        self.root_dir_targets(directory, specifier)
            .into_iter()
            .map(|(_, path)| path)
            .collect()
    }

    /// [`Self::root_dir_candidates`], each with the root directory it lies below.
    pub(crate) fn root_dir_targets(&self, directory: &Path, specifier: &str) -> Vec<(&Path, PathBuf)> {
        let Some(inside) = self
            .root_dirs
            .iter()
            .filter(|root| directory.starts_with(root))
            .max_by_key(|root| root.components().count())
        else {
            return Vec::new();
        };
        let Ok(relative) = directory.strip_prefix(inside) else {
            return Vec::new();
        };
        self.root_dirs
            .iter()
            .filter(|root| *root != inside)
            .map(|root| (root.as_path(), join_normalized(&root.join(relative), specifier)))
            .collect()
    }

    /// The paths `specifier` may stand for, in the order TypeScript tries them: `paths` patterns with the longest
    /// prefix first, then `baseUrl`.
    pub(crate) fn candidates(&self, specifier: &str) -> Vec<PathBuf> {
        let mut matched: Vec<(usize, Vec<PathBuf>)> = self
            .paths
            .iter()
            .filter_map(|(pattern, targets)| {
                let (prefix, captured) = match_pattern(pattern, specifier)?;
                Some((prefix, self.expand(targets, captured)))
            })
            .collect();
        matched.sort_by_key(|(prefix, _)| Reverse(*prefix));
        let mut candidates: Vec<PathBuf> = matched.into_iter().flat_map(|(_, paths)| paths).collect();
        candidates.extend(
            self.base_url
                .as_ref()
                .map(|base_url| join_normalized(base_url, specifier)),
        );
        candidates
    }

    /// The paths the `paths` pattern `pattern` maps `specifier` to, in the order of its targets; empty when the pattern
    /// is not declared or does not match. `baseUrl` does not count.
    pub(crate) fn pattern_candidates(&self, pattern: &str, specifier: &str) -> Vec<PathBuf> {
        self.paths
            .iter()
            .filter(|(declared, _)| declared == pattern)
            .filter_map(|(_, targets)| {
                let (_, captured) = match_pattern(pattern, specifier)?;
                Some(self.expand(targets, captured))
            })
            .flatten()
            .collect()
    }

    /// The paths `targets` stand for with `captured` in place of their `*`.
    fn expand(&self, targets: &[String], captured: &str) -> Vec<PathBuf> {
        let base = self.base_url.as_deref().unwrap_or(&self.paths_dir);
        targets
            .iter()
            .map(|target| join_any(base, &target.replacen('*', captured, 1)))
            .collect()
    }

    /// Whether a `paths` pattern matches `specifier`; `baseUrl`, which every bare specifier falls back to, does not
    /// count.
    pub(crate) fn maps(&self, specifier: &str) -> bool {
        self.paths
            .iter()
            .any(|(pattern, _)| match_pattern(pattern, specifier).is_some())
    }
}

/// The `paths` patterns of the configs that apply to the workspace packages, indexed by their literal part, to name
/// the config of another package that maps an import its own config does not.
///
/// A lookup costs one map probe per prefix of the specifier, however many packages and patterns there are. A
/// wildcard pattern with no literal prefix (`*`, `*.svg`) matches every specifier and says nothing about where one
/// resolves, so it is left out.
pub(crate) struct ForeignPaths {
    /// The configs with `paths`, sorted and deduplicated by the config file found for a directory, each with the file
    /// that declares its `paths` (itself or a config it extends), relative to the workspace root with `/` separators.
    configs: Vec<(String, Arc<TsPaths>)>,
    /// Exact patterns: the positions in `configs` of the configs that declare each, ascending.
    exact: HashMap<String, Vec<usize>>,
    /// Wildcard patterns by the text before `*`: the text after it, the pattern and the position of the config, by
    /// config.
    wildcards: HashMap<String, Vec<(String, String, usize)>>,
}

/// `path` relative to `root`, with `/` separators.
fn relative_to(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

impl ForeignPaths {
    /// Indexes the `paths` of `configs`; a config found for several directories counts once.
    pub(crate) fn new(root: &Path, configs: &[Arc<TsPaths>]) -> Self {
        let mut by_file: Vec<(String, &Arc<TsPaths>)> = configs
            .iter()
            .filter(|config| !config.paths.is_empty())
            .map(|config| (relative_to(root, &config.config), config))
            .collect();
        by_file.sort_by(|a, b| a.0.cmp(&b.0));
        by_file.dedup_by(|a, b| a.0 == b.0);
        let mut exact: HashMap<String, Vec<usize>> = HashMap::default();
        let mut wildcards: HashMap<String, Vec<(String, String, usize)>> = HashMap::default();
        for (position, (_, config)) in by_file.iter().enumerate() {
            for (pattern, _) in &config.paths {
                match pattern.split_once('*') {
                    None => exact.entry(pattern.clone()).or_default().push(position),
                    Some(("", _)) => {}
                    Some((prefix, suffix)) => wildcards.entry(prefix.to_owned()).or_default().push((
                        suffix.to_owned(),
                        pattern.clone(),
                        position,
                    )),
                }
            }
        }
        Self {
            configs: by_file
                .into_iter()
                .map(|(_, config)| (relative_to(root, &config.paths_config), Arc::clone(config)))
                .collect(),
            exact,
            wildcards,
        }
    }

    /// The file that declares the pattern, relative to the workspace root, and the pattern that map `specifier`
    /// among the patterns `accept` takes (given the config and the pattern), as TypeScript ranks patterns: an exact
    /// pattern first, then the wildcard with the longest prefix. Among equally specific patterns the first config by
    /// path wins.
    pub(crate) fn lookup(&self, specifier: &str, accept: impl Fn(&TsPaths, &str) -> bool) -> Option<(&str, &str)> {
        let found = |pattern: &str, position: usize| {
            let (file, config) = &self.configs[position];
            accept(config, pattern).then_some(file.as_str())
        };
        if let Some((pattern, positions)) = self.exact.get_key_value(specifier)
            && let Some(file) = positions.iter().find_map(|position| found(pattern.as_str(), *position))
        {
            return Some((file, pattern.as_str()));
        }
        (1..=specifier.len())
            .rev()
            .filter(|end| specifier.is_char_boundary(*end))
            .find_map(|end| {
                let (prefix, rest) = specifier.split_at(end);
                self.wildcards
                    .get(prefix)?
                    .iter()
                    .filter(|(suffix, _, _)| rest.ends_with(suffix.as_str()))
                    .filter_map(|(_, pattern, position)| {
                        Some((*position, found(pattern.as_str(), *position)?, pattern))
                    })
                    .min_by_key(|(position, _, _)| *position)
                    .map(|(_, file, pattern)| (file, pattern.as_str()))
            })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use skif_core::MemoryFs;

    use super::*;

    #[test]
    fn wildcard_and_exact_patterns_rank_by_prefix() {
        let paths = TsPaths {
            config: PathBuf::from("/ws/packages/tsconfig.json"),
            paths_config: PathBuf::from("/ws/packages/tsconfig.json"),
            base_url: None,
            paths_dir: PathBuf::from("/ws/packages"),
            root_dirs: Vec::new(),
            paths: vec![
                ("@s/*".to_owned(), vec!["./*/src".to_owned()]),
                ("@s/core".to_owned(), vec!["./core/main.ts".to_owned()]),
            ],
        };
        assert_eq!(
            paths.candidates("@s/core"),
            [
                PathBuf::from("/ws/packages/core/main.ts"),
                PathBuf::from("/ws/packages/core/src")
            ]
        );
        assert_eq!(paths.candidates("@s/ui"), [PathBuf::from("/ws/packages/ui/src")]);
        assert_eq!(paths.candidates("other"), Vec::<PathBuf>::new());
    }

    fn declaring(config: &str, patterns: &[&str]) -> Arc<TsPaths> {
        Arc::new(TsPaths {
            config: PathBuf::from(config),
            paths_config: PathBuf::from(config),
            base_url: None,
            paths_dir: PathBuf::from("/ws"),
            root_dirs: Vec::new(),
            paths: patterns
                .iter()
                .map(|pattern| ((*pattern).to_owned(), vec!["./x/*".to_owned()]))
                .collect(),
        })
    }

    #[test]
    fn foreign_paths_rank_exact_then_longest_prefix_then_config_path() {
        let configs = [
            declaring("/ws/packages/zeta/tsconfig.json", &["shared/*", "*", "*.svg"]),
            declaring("/ws/packages/app/tsconfig.json", &["shared/*", "@ui/*/index"]),
            declaring("/ws/packages/web/tsconfig.json", &["shared/deep/*", "exact"]),
            // The same config found for another directory counts once.
            declaring("/ws/packages/web/tsconfig.json", &["shared/deep/*", "exact"]),
            declaring("/ws/packages/none/tsconfig.json", &[]),
        ];
        let foreign = ForeignPaths::new(Path::new("/ws"), &configs);
        let any = |specifier: &str| foreign.lookup(specifier, |_, _| true);
        assert_eq!(foreign.configs.len(), 3);
        assert_eq!(any("shared/value"), Some(("packages/app/tsconfig.json", "shared/*")));
        assert_eq!(
            any("shared/deep/x"),
            Some(("packages/web/tsconfig.json", "shared/deep/*"))
        );
        assert_eq!(any("exact"), Some(("packages/web/tsconfig.json", "exact")));
        assert_eq!(
            any("@ui/button/index"),
            Some(("packages/app/tsconfig.json", "@ui/*/index"))
        );
        assert_eq!(any("@ui/button"), None);
        // Catch-all patterns match everything and are left out.
        assert_eq!(any("left-pad"), None);
        assert_eq!(any("icon.svg"), None);
    }

    #[test]
    fn foreign_paths_skip_the_patterns_the_caller_rejects() {
        let configs = [
            declaring("/ws/packages/app/tsconfig.json", &["shared/*", "exact"]),
            declaring(
                "/ws/packages/zeta/tsconfig.json",
                &["shared/*", "exact", "shared/deep/*"],
            ),
        ];
        let foreign = ForeignPaths::new(Path::new("/ws"), &configs);
        let not_app = |config: &TsPaths, _: &str| !config.config.starts_with("/ws/packages/app");
        assert_eq!(
            foreign.lookup("shared/value", not_app),
            Some(("packages/zeta/tsconfig.json", "shared/*"))
        );
        assert_eq!(
            foreign.lookup("exact", not_app),
            Some(("packages/zeta/tsconfig.json", "exact"))
        );
        // A rejected longer prefix falls back to a shorter one.
        assert_eq!(
            foreign.lookup("shared/deep/x", |_, pattern| pattern != "shared/deep/*"),
            Some(("packages/app/tsconfig.json", "shared/*"))
        );
        assert_eq!(foreign.lookup("shared/value", |_, _| false), None);
    }

    #[test]
    fn foreign_paths_name_the_config_that_declares_inherited_paths() {
        let fs = MemoryFs::new()
            .with_file(
                "/ws/tsconfig.base.json",
                r#"{"compilerOptions":{"paths":{"shared/*":["./packages/lib/src/*"]}}}"#,
            )
            .with_file(
                "/ws/packages/app/tsconfig.json",
                r#"{"extends":"../../tsconfig.base.json"}"#,
            );
        let app = configs(fs, &[]).nearest(Path::new("/ws/packages/app/src")).unwrap();
        let foreign = ForeignPaths::new(Path::new("/ws"), &[Arc::new(app)]);
        assert_eq!(
            foreign.lookup("shared/value", |_, _| true),
            Some(("tsconfig.base.json", "shared/*"))
        );
    }

    #[test]
    fn pattern_candidates_expand_only_the_named_pattern() {
        let paths = TsPaths {
            config: PathBuf::from("/ws/app/tsconfig.json"),
            paths_config: PathBuf::from("/ws/app/tsconfig.json"),
            base_url: Some(PathBuf::from("/ws/app")),
            paths_dir: PathBuf::from("/ws/app"),
            root_dirs: Vec::new(),
            paths: vec![
                (
                    "lodash/*".to_owned(),
                    vec!["./shims/lodash/*".to_owned(), "./vendor/*".to_owned()],
                ),
                ("lodash/merge".to_owned(), vec!["./merge.ts".to_owned()]),
            ],
        };
        assert_eq!(
            paths.pattern_candidates("lodash/*", "lodash/merge"),
            [
                PathBuf::from("/ws/app/shims/lodash/merge"),
                PathBuf::from("/ws/app/vendor/merge")
            ]
        );
        assert_eq!(paths.pattern_candidates("lodash/*", "react"), Vec::<PathBuf>::new());
        assert_eq!(
            paths.pattern_candidates("unknown/*", "unknown/x"),
            Vec::<PathBuf>::new()
        );
    }

    #[test]
    fn root_dirs_merge_directories_for_relative_imports() {
        let fs = MemoryFs::new()
            .with_file(
                absolute("/ws/app/tsconfig.base.json"),
                r#"{"compilerOptions":{"rootDirs":["src","${configDir}/bazel-out/bin/src"]}}"#,
            )
            .with_file(
                absolute("/ws/app/tsconfig.json"),
                r#"{"extends":"./tsconfig.base.json"}"#,
            );
        let root = absolute("/ws");
        let src = absolute("/ws/app/src/ui");
        let paths = configs_at(fs, &root, &[]).nearest(Path::new(&src)).unwrap();
        assert_eq!(
            paths.root_dir_candidates(Path::new(&src), "./gen/x"),
            [PathBuf::from(absolute("/ws/app/bazel-out/bin/src/ui/gen/x"))]
        );
        assert_eq!(
            paths.root_dir_targets(Path::new(&src), "./gen/x"),
            [(
                Path::new(&absolute("/ws/app/bazel-out/bin/src")),
                PathBuf::from(absolute("/ws/app/bazel-out/bin/src/ui/gen/x"))
            )]
        );
        assert_eq!(
            paths.root_dir_candidates(Path::new(&absolute("/ws/app/bazel-out/bin/src/ui")), "../shared/y"),
            [PathBuf::from(absolute("/ws/app/src/shared/y"))]
        );
        assert_eq!(
            paths.root_dir_candidates(Path::new(&absolute("/ws/app/other")), "./z"),
            Vec::<PathBuf>::new()
        );
    }

    #[test]
    fn extends_supplies_paths_relative_to_the_defining_config() {
        let fs = MemoryFs::new()
            .with_file(
                "/ws/base/tsconfig.base.json",
                r#"{"compilerOptions":{"paths":{"@b/*":["./lib/*"]}}}"#,
            )
            .with_file(
                "/ws/app/jsconfig.json",
                r#"{"extends":"../base/tsconfig.base","compilerOptions":{}}"#,
            );
        let paths = configs(fs, &[]).nearest(Path::new("/ws/app/src")).unwrap();
        assert_eq!(paths.candidates("@b/x"), [PathBuf::from("/ws/base/lib/x")]);
    }

    /// `/ws` has no drive on Windows, so it is not absolute there and a `${configDir}` target built from it would
    /// not take the path a real directory takes.
    fn absolute(path: &str) -> String {
        if cfg!(windows) {
            format!("C:{path}")
        } else {
            path.to_owned()
        }
    }

    fn configs(fs: MemoryFs, members: &[(&str, &str)]) -> TsConfigs {
        configs_at(fs, "/ws", members)
    }

    fn configs_at(fs: MemoryFs, root: &str, members: &[(&str, &str)]) -> TsConfigs {
        let members = members
            .iter()
            .map(|(name, dir)| ((*name).to_owned(), PathBuf::from(dir)))
            .collect();
        TsConfigs::new(Arc::new(fs), PathBuf::from(root), members)
    }

    #[test]
    fn extends_follows_workspace_packages_and_installed_packages() {
        let fs = MemoryFs::new()
            .with_file(
                "/ws/packages/tsconfig/base.json",
                r#"{"compilerOptions":{"paths":{"@m/*":["./m/*"]}}}"#,
            )
            .with_file(
                "/ws/node_modules/@acme/config/tsconfig.json",
                r#"{"compilerOptions":{"baseUrl":"./lib"}}"#,
            )
            .with_file("/ws/apps/a/tsconfig.json", r#"{"extends":"@x/tsconfig/base.json"}"#)
            .with_file("/ws/apps/b/tsconfig.json", r#"{"extends":"@acme/config"}"#);
        let configs = configs(fs, &[("@x/tsconfig", "/ws/packages/tsconfig")]);
        let a = configs.nearest(Path::new("/ws/apps/a/src")).unwrap();
        assert_eq!(a.candidates("@m/q"), [PathBuf::from("/ws/packages/tsconfig/m/q")]);
        let b = configs.nearest(Path::new("/ws/apps/b")).unwrap();
        assert_eq!(
            b.candidates("plain"),
            [PathBuf::from("/ws/node_modules/@acme/config/lib/plain")]
        );
    }

    #[test]
    fn config_dir_placeholder_is_the_directory_of_the_resolved_config() {
        let fs = MemoryFs::new()
            .with_file(
                absolute("/ws/base/tsconfig.json"),
                r#"{"compilerOptions":{"paths":{"@/*":["${configDir}/src/*"]}}}"#,
            )
            .with_file(
                absolute("/ws/app/tsconfig.json"),
                r#"{"extends":"../base/tsconfig.json"}"#,
            );
        let root = absolute("/ws");
        let paths = configs_at(fs, &root, &[])
            .nearest(Path::new(&absolute("/ws/app/src")))
            .unwrap();
        assert_eq!(
            paths.candidates("@/util"),
            [PathBuf::from(absolute("/ws/app/src/util"))]
        );
    }
}
