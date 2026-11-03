//! `baseUrl` and `paths` of the nearest `tsconfig.json` or `jsconfig.json` at or above a file.
//!
//! The resolver applies a `tsconfig.json` only to the files it includes, and never reads a `jsconfig.json`. Editors
//! and bundlers apply the nearest config to everything below it, including directories that are not packages
//! (`packages/tsconfig.json`, `src/legacy/jsconfig.json`), test files a config excludes and JavaScript files.

use std::cmp::Reverse;
use std::path::{Path, PathBuf};

use skif_core::FileSystem;
use skif_js_syntax::ConfigValue;

use crate::deps::parse_tsconfig;
use crate::resolve::join_normalized;

/// How many `extends` links are followed.
const MAX_EXTENDS: usize = 8;

/// `paths` entries: the pattern and its targets.
type PathEntries = Vec<(String, Vec<String>)>;

/// The module resolution options a config sets directly or through relative `extends`.
#[derive(Default)]
struct Options {
    base_url: Option<PathBuf>,
    /// The directory of the config that set `paths`, and the entries.
    paths: Option<(PathBuf, PathEntries)>,
}

/// The `baseUrl` and `paths` that apply to a directory.
pub(crate) struct TsPaths {
    base_url: Option<PathBuf>,
    paths_dir: PathBuf,
    paths: PathEntries,
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

fn load(fs: &dyn FileSystem, file: &Path, depth: usize) -> Option<Options> {
    let config = parse_tsconfig(fs, file)?;
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
        // Later entries override earlier ones; packages are not followed.
        for parent in parents.into_iter().filter(|parent| parent.starts_with('.')) {
            if let Some(inherited) = load(fs, &join_normalized(dir, &json_file(parent)), depth + 1) {
                options.base_url = inherited.base_url.or(options.base_url);
                options.paths = inherited.paths.or(options.paths);
            }
        }
    }
    if let Some(compiler) = config.get("compilerOptions") {
        if let Some(base_url) = compiler.get("baseUrl").and_then(ConfigValue::as_str) {
            options.base_url = Some(join_normalized(dir, base_url));
        }
        if let Some(paths) = compiler.get("paths").and_then(ConfigValue::as_object) {
            let entries = paths
                .iter()
                .map(|(pattern, targets)| {
                    let targets = match targets {
                        ConfigValue::String(target) => vec![target.clone()],
                        ConfigValue::Array(items) => {
                            items.iter().filter_map(ConfigValue::as_str).map(str::to_owned).collect()
                        }
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

/// The nearest config at or above `dir`, not above `root`, that sets `baseUrl` or `paths`.
pub(crate) fn nearest(fs: &dyn FileSystem, root: &Path, dir: &Path) -> Option<TsPaths> {
    dir.ancestors().take_while(|ancestor| ancestor.starts_with(root)).find_map(|ancestor| {
        ["tsconfig.json", "jsconfig.json"].iter().find_map(|name| {
            let file = ancestor.join(name);
            if !fs.is_file(&file) {
                return None;
            }
            let options = load(fs, &file, 0)?;
            if options.base_url.is_none() && options.paths.is_none() {
                return None;
            }
            let (paths_dir, paths) = options.paths.unwrap_or_else(|| (ancestor.to_path_buf(), Vec::new()));
            Some(TsPaths {
                base_url: options.base_url,
                paths_dir,
                paths,
            })
        })
    })
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
                    .map(|target| join_normalized(base, &target.replacen('*', captured, 1)))
                    .collect();
                Some((prefix, paths))
            })
            .collect();
        matched.sort_by_key(|(prefix, _)| Reverse(*prefix));
        let mut candidates: Vec<PathBuf> = matched.into_iter().flat_map(|(_, paths)| paths).collect();
        candidates.extend(self.base_url.as_ref().map(|base_url| join_normalized(base_url, specifier)));
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
        assert!(paths.candidates("other").is_empty());
    }

    #[test]
    fn extends_supplies_paths_relative_to_the_defining_config() {
        let fs = MemoryFs::new()
            .with_file("/ws/base/tsconfig.base.json", r#"{"compilerOptions":{"paths":{"@b/*":["./lib/*"]}}}"#)
            .with_file("/ws/app/jsconfig.json", r#"{"extends":"../base/tsconfig.base","compilerOptions":{}}"#);
        let paths = nearest(&fs, Path::new("/ws"), Path::new("/ws/app/src")).unwrap();
        assert_eq!(paths.candidates("@b/x"), [PathBuf::from("/ws/base/lib/x")]);
    }
}
