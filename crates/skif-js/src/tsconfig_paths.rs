//! `baseUrl`, `paths` and `rootDirs` of the nearest `tsconfig.json` or `jsconfig.json` at or above a file.
//!
//! The resolver applies a `tsconfig.json` only to the files it includes, and never reads a `jsconfig.json`. Editors
//! and bundlers apply the nearest config to everything below it, including directories that are not packages
//! (`packages/tsconfig.json`, `src/legacy/jsconfig.json`), test files a config excludes and JavaScript files.

use std::cmp::Reverse;
use std::collections::HashMap;
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
    /// The directory of the config that set `paths`, and the entries.
    paths: Option<(PathBuf, PathEntries)>,
    /// Directories that are merged into one virtual directory for relative imports.
    root_dirs: Vec<PathBuf>,
}

/// The `baseUrl`, `paths` and `rootDirs` that apply to a directory.
pub(crate) struct TsPaths {
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

/// The `baseUrl` and `paths` configs of a workspace, parsed once per config file.
/// A parsed config's file and the directory `${configDir}` stands for.
type ConfigKey = (PathBuf, PathBuf);

pub(crate) struct TsConfigs {
    fs: Arc<dyn FileSystem>,
    root: PathBuf,
    /// Workspace members by name, with their absolute directory, for `extends` that names a package.
    members: Vec<(String, PathBuf)>,
    /// Parsed configs by file and the directory `${configDir}` stands for.
    loaded: RwLock<HashMap<ConfigKey, Option<Arc<Options>>>>,
}

impl TsConfigs {
    pub(crate) fn new(fs: Arc<dyn FileSystem>, root: PathBuf, members: Vec<(String, PathBuf)>) -> Self {
        Self {
            fs,
            root,
            members,
            loaded: RwLock::new(HashMap::new()),
        }
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
        if depth < MAX_EXTENDS
            && let Some(extends) = config.get("extends")
        {
            let parents: Vec<&str> = match extends {
                ConfigValue::String(parent) => vec![parent.as_str()],
                ConfigValue::Array(items) => items.iter().filter_map(ConfigValue::as_str).collect(),
                _ => Vec::new(),
            };
            // Later entries override earlier ones.
            for parent in parents {
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
                options.paths = Some((dir.to_path_buf(), entries));
            }
        }
        Some(options)
    }

    /// The nearest config at or above `dir`, not above the workspace root, that sets `baseUrl` or `paths`.
    pub(crate) fn nearest(&self, dir: &Path) -> Option<TsPaths> {
        dir.ancestors()
            .take_while(|ancestor| ancestor.starts_with(&self.root))
            .find_map(|ancestor| {
                ["tsconfig.json", "jsconfig.json"].iter().find_map(|name| {
                    let file = ancestor.join(name);
                    if !self.fs.is_file(&file) {
                        return None;
                    }
                    let options = self.load(&file, ancestor, 0)?;
                    if options.base_url.is_none() && options.paths.is_none() && options.root_dirs.is_empty() {
                        return None;
                    }
                    let (paths_dir, paths) = options
                        .paths
                        .clone()
                        .unwrap_or_else(|| (ancestor.to_path_buf(), Vec::new()));
                    Some(TsPaths {
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
fn match_pattern<'a>(pattern: &str, specifier: &'a str) -> Option<(usize, &'a str)> {
    match pattern.split_once('*') {
        None => (pattern == specifier).then_some((usize::MAX, "")),
        Some((prefix, suffix)) => {
            let captured = specifier.strip_prefix(prefix)?.strip_suffix(suffix)?;
            Some((prefix.len(), captured))
        }
    }
}

impl TsPaths {
    /// The files a relative `specifier` written in `directory` may stand for through `rootDirs`: the same relative
    /// path below each of the other root directories, in the order of the config.
    pub(crate) fn root_dir_candidates(&self, directory: &Path, specifier: &str) -> Vec<PathBuf> {
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
            .map(|root| join_normalized(&root.join(relative), specifier))
            .collect()
    }

    /// The paths `specifier` may stand for, in the order TypeScript tries them: `paths` patterns with the longest
    /// prefix first, then `baseUrl`.
    pub(crate) fn candidates(&self, specifier: &str) -> Vec<PathBuf> {
        let base = self.base_url.as_deref().unwrap_or(&self.paths_dir);
        let mut matched: Vec<(usize, Vec<PathBuf>)> = self
            .paths
            .iter()
            .filter_map(|(pattern, targets)| {
                let (prefix, captured) = match_pattern(pattern, specifier)?;
                let paths = targets
                    .iter()
                    .map(|target| join_any(base, &target.replacen('*', captured, 1)))
                    .collect();
                Some((prefix, paths))
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
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use skif_core::MemoryFs;

    use super::*;

    #[test]
    fn wildcard_and_exact_patterns_rank_by_prefix() {
        let paths = TsPaths {
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

    #[test]
    fn root_dirs_merge_directories_for_relative_imports() {
        let fs = MemoryFs::new()
            .with_file(
                "/ws/app/tsconfig.base.json",
                r#"{"compilerOptions":{"rootDirs":["src","${configDir}/bazel-out/bin/src"]}}"#,
            )
            .with_file("/ws/app/tsconfig.json", r#"{"extends":"./tsconfig.base.json"}"#);
        let paths = configs(fs, &[]).nearest(Path::new("/ws/app/src/ui")).unwrap();
        assert_eq!(
            paths.root_dir_candidates(Path::new("/ws/app/src/ui"), "./gen/x"),
            [PathBuf::from("/ws/app/bazel-out/bin/src/ui/gen/x")]
        );
        assert_eq!(
            paths.root_dir_candidates(Path::new("/ws/app/bazel-out/bin/src/ui"), "../shared/y"),
            [PathBuf::from("/ws/app/src/shared/y")]
        );
        assert_eq!(
            paths.root_dir_candidates(Path::new("/ws/app/other"), "./z"),
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

    fn configs(fs: MemoryFs, members: &[(&str, &str)]) -> TsConfigs {
        let members = members
            .iter()
            .map(|(name, dir)| ((*name).to_owned(), PathBuf::from(dir)))
            .collect();
        TsConfigs::new(Arc::new(fs), PathBuf::from("/ws"), members)
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
                "/ws/base/tsconfig.json",
                r#"{"compilerOptions":{"paths":{"@/*":["${configDir}/src/*"]}}}"#,
            )
            .with_file("/ws/app/tsconfig.json", r#"{"extends":"../base/tsconfig.json"}"#);
        let paths = configs(fs, &[]).nearest(Path::new("/ws/app/src")).unwrap();
        assert_eq!(paths.candidates("@/util"), [PathBuf::from("/ws/app/src/util")]);
    }
}
