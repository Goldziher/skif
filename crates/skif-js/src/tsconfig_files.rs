//! The files a `tsconfig.json` selects with `files`, `include` and `exclude`, as project globs of a package
//! (`[js] use-tsconfig-files`, knip's `--use-tsconfig-files`).
//!
//! The keys are inherited through `extends` and stay relative to the config that sets them, as in TypeScript. The
//! configs a solution-style `tsconfig.json` (`"files": []` with `references`) references are followed when they lie
//! below its directory, as knip does. As in TypeScript, `exclude` filters only what `include` selects, never the
//! `files` listed by name. The `include` globs of a package are one set, so the `exclude` of any of these configs
//! applies to what the `include` of all of them selects.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use skif_config::Config;
use skif_core::FileSystem;
use skif_js_syntax::ConfigValue;
use skif_workspace::Workspace;

use crate::config_check::relative_tsconfig;
use crate::deps::{member_dirs, parse_tsconfig};
use crate::discover::relative;
use crate::resolve::join_normalized;
use crate::setup::tsconfig_path;
use crate::tsconfig_paths::resolve_extends;

/// How many `extends` links are followed.
const MAX_EXTENDS: usize = 8;

/// The project of a package as its TypeScript configurations select it, relative to the package directory.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct TsconfigProject {
    /// The `include` globs, then the `exclude` (and `outDir`) globs negated with `!`.
    pub patterns: Vec<String>,
    /// The `files` entries, which no `exclude` filters out.
    pub files: Vec<String>,
}

impl TsconfigProject {
    /// Whether the configurations select nothing in the package.
    pub(crate) fn is_empty(&self) -> bool {
        self.files.is_empty() && self.patterns.iter().all(|pattern| pattern.starts_with('!'))
    }
}

/// A list of patterns and the directory they are relative to.
type Located = (PathBuf, Vec<String>);

/// The file selection a config sets itself or inherits.
#[derive(Clone, Default)]
struct Selection {
    files: Option<Located>,
    include: Option<Located>,
    exclude: Option<Located>,
    out_dir: Option<PathBuf>,
}

fn strings(value: &ConfigValue) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(ConfigValue::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn extends_of(config: &ConfigValue) -> Vec<&str> {
    match config.get("extends") {
        Some(ConfigValue::String(one)) => vec![one.as_str()],
        Some(ConfigValue::Array(many)) => many.iter().filter_map(ConfigValue::as_str).collect(),
        _ => Vec::new(),
    }
}

fn selection(fs: &dyn FileSystem, members: &[(String, PathBuf)], path: &Path, depth: usize) -> Option<Selection> {
    let config = parse_tsconfig(fs, path)?;
    let dir = path.parent()?;
    let mut out = Selection::default();
    if depth < MAX_EXTENDS {
        // A later `extends` entry overrides an earlier one.
        for parent in extends_of(&config) {
            if let Some(inherited) =
                resolve_extends(fs, members, dir, parent).and_then(|target| selection(fs, members, &target, depth + 1))
            {
                out.files = inherited.files.or(out.files);
                out.include = inherited.include.or(out.include);
                out.exclude = inherited.exclude.or(out.exclude);
                out.out_dir = inherited.out_dir.or(out.out_dir);
            }
        }
    }
    let located = |key: &str| config.get(key).map(|value| (dir.to_path_buf(), strings(value)));
    out.files = located("files").or(out.files);
    out.include = located("include").or(out.include);
    out.exclude = located("exclude").or(out.exclude);
    if let Some(out_dir) = config
        .get("compilerOptions")
        .and_then(|options| options.get("outDir"))
        .and_then(ConfigValue::as_str)
    {
        out.out_dir = Some(join_normalized(dir, out_dir));
    }
    Some(out)
}

/// An `include` or `exclude` entry as a glob: a directory (no wildcard, no extension) stands for everything below it.
fn expand(pattern: &str) -> String {
    let trimmed = pattern.trim_end_matches('/');
    let last = trimmed.rsplit('/').next().unwrap_or(trimmed);
    let is_glob = trimmed.contains(['*', '?']);
    let has_extension = last != "." && last != ".." && last.contains('.');
    if is_glob || has_extension {
        trimmed.to_owned()
    } else {
        format!("{trimmed}/**/*")
    }
}

/// `pattern`, relative to `base`, as a glob relative to `package_dir`; `None` when it lies outside the package.
fn package_relative(base: &Path, pattern: &str, package_dir: &Path) -> Option<String> {
    let absolute = if Path::new(pattern).is_absolute() {
        join_normalized(Path::new("/"), pattern)
    } else {
        join_normalized(base, pattern)
    };
    relative(&absolute, package_dir).filter(|rel| !rel.is_empty())
}

/// The config a `references` entry names: the file itself, or the `tsconfig.json` of the directory.
fn reference(fs: &dyn FileSystem, dir: &Path, path: &str) -> Option<PathBuf> {
    let target = join_normalized(dir, path);
    if fs.is_file(&target) {
        return Some(target);
    }
    let in_dir = target.join("tsconfig.json");
    fs.is_file(&in_dir).then_some(in_dir)
}

fn collect(
    fs: &dyn FileSystem,
    members: &[(String, PathBuf)],
    config: &Path,
    package_dir: &Path,
    visited: &mut BTreeSet<PathBuf>,
    project: &mut TsconfigProject,
) -> bool {
    let Some(selected) = selection(fs, members, config, 0) else {
        return false;
    };
    let Some(dir) = config.parent() else {
        return false;
    };
    let mut include: Vec<String> = Vec::new();
    if let Some((base, files)) = &selected.files {
        project.files.extend(
            files
                .iter()
                .filter_map(|file| package_relative(base, file, package_dir)),
        );
    }
    let patterns = selected.include.clone().or_else(|| {
        selected
            .files
            .is_none()
            .then(|| (dir.to_path_buf(), vec!["**/*".to_owned()]))
    });
    if let Some((base, patterns)) = &patterns {
        include.extend(
            patterns
                .iter()
                .filter_map(|pattern| package_relative(base, &expand(pattern), package_dir)),
        );
    }
    let mut exclude: Vec<String> = Vec::new();
    if let Some((base, patterns)) = &selected.exclude {
        exclude.extend(
            patterns
                .iter()
                .filter_map(|pattern| package_relative(base, &expand(pattern), package_dir)),
        );
    }
    if let Some(out_dir) = &selected.out_dir
        && let Some(rel) = relative(out_dir, package_dir).filter(|rel| !rel.is_empty())
    {
        exclude.push(format!("{rel}/**"));
    }
    project.patterns.extend(include);
    project
        .patterns
        .extend(exclude.into_iter().map(|glob| format!("!{glob}")));
    let parsed = parse_tsconfig(fs, config);
    let references: Vec<String> = parsed
        .as_ref()
        .and_then(|value| value.get("references"))
        .and_then(ConfigValue::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("path").and_then(ConfigValue::as_str).map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    for path in references {
        if let Some(target) = reference(fs, dir, &path)
            && target.starts_with(dir)
            && visited.insert(target.clone())
        {
            collect(fs, members, &target, package_dir, visited, project);
        }
    }
    true
}

/// The project, relative to `package_dir`, of the files the config at `config` selects, or `None` when there is no
/// readable config there. Patterns that leave the package directory are dropped: the project globs of a package only
/// judge the files it owns.
pub(crate) fn project_globs(
    fs: &dyn FileSystem,
    members: &[(String, PathBuf)],
    config: &Path,
    package_dir: &Path,
) -> Option<TsconfigProject> {
    let mut project = TsconfigProject::default();
    let mut visited = BTreeSet::from([config.to_path_buf()]);
    collect(fs, members, config, package_dir, &mut visited, &mut project).then_some(project)
}

/// With `[js] use-tsconfig-files`, the TypeScript configurations, relative to the workspace root, whose `files` and
/// `include` select nothing inside their package, so that package has no project files and no file of it can be
/// reported unused. Empty without the setting. A host shows these as warnings; the lint itself goes on.
#[must_use]
pub fn empty_tsconfig_projects(workspace: &Workspace, config: &Config, fs: &dyn FileSystem) -> Vec<PathBuf> {
    if !config.js.enabled || !config.js.use_tsconfig_files {
        return Vec::new();
    }
    let root_tsconfig = config
        .js
        .tsconfig
        .as_deref()
        .and_then(relative_tsconfig)
        .map(|path| join_normalized(&workspace.root, &path));
    let members = member_dirs(workspace);
    workspace
        .iter()
        .filter(|(id, _)| !workspace.is_ignored(*id))
        .filter_map(|(_, package)| {
            let dir = workspace.root.join(&package.dir);
            let config = tsconfig_path(root_tsconfig.as_deref(), &dir, package.is_root);
            let project = project_globs(fs, &members, &config, &dir)?;
            project.is_empty().then(|| {
                config
                    .strip_prefix(&workspace.root)
                    .map_or(config.clone(), Path::to_path_buf)
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use skif_core::MemoryFs;

    use super::{TsconfigProject, project_globs};

    fn project(files: &[(&str, &str)], config: &str, package: &str) -> Option<TsconfigProject> {
        let mut fs = MemoryFs::new();
        for (path, text) in files {
            fs = fs.with_file(*path, *text);
        }
        project_globs(&fs, &[], Path::new(config), Path::new(package))
    }

    /// The `files` entries, then the other patterns.
    fn globs(files: &[(&str, &str)], config: &str, package: &str) -> Option<Vec<String>> {
        project(files, config, package).map(|found| found.files.into_iter().chain(found.patterns).collect())
    }

    #[test]
    fn exclude_filters_only_what_include_selects() {
        let found = project(
            &[(
                "/ws/tsconfig.json",
                r#"{"files":["src/legacy/keep.ts"],"include":["src"],"exclude":["src/legacy"]}"#,
            )],
            "/ws/tsconfig.json",
            "/ws",
        )
        .unwrap();
        assert_eq!(found.files, ["src/legacy/keep.ts"]);
        assert_eq!(found.patterns, ["src/**/*", "!src/legacy/**/*"]);
        assert!(!found.is_empty());
    }

    #[test]
    fn a_config_that_selects_nothing_in_the_package_is_empty() {
        let empty = |text: &str| project(&[("/ws/app/tsconfig.json", text)], "/ws/app/tsconfig.json", "/ws/app");
        assert!(empty(r#"{"files":[]}"#).unwrap().is_empty());
        assert!(empty(r#"{"include":["../shared"]}"#).unwrap().is_empty());
        assert!(!empty("{}").unwrap().is_empty());
    }

    #[test]
    fn include_files_and_exclude_become_package_globs() {
        let found = globs(
            &[(
                "/ws/app/tsconfig.json",
                r#"{"files":["main.ts"],"include":["src","lib/**/*.ts"],"exclude":["src/legacy"],
                    "compilerOptions":{"outDir":"dist"}}"#,
            )],
            "/ws/app/tsconfig.json",
            "/ws/app",
        );
        assert_eq!(
            found.unwrap(),
            ["main.ts", "src/**/*", "lib/**/*.ts", "!src/legacy/**/*", "!dist/**"]
        );
    }

    #[test]
    fn no_include_or_files_selects_everything_and_extends_is_relative_to_its_config() {
        assert_eq!(
            globs(&[("/ws/tsconfig.json", "{}")], "/ws/tsconfig.json", "/ws").unwrap(),
            ["**/*"]
        );
        let found = globs(
            &[
                ("/ws/config/base.json", r#"{"include":["../src"]}"#),
                ("/ws/tsconfig.json", r#"{"extends":"./config/base.json"}"#),
            ],
            "/ws/tsconfig.json",
            "/ws",
        );
        assert_eq!(found.unwrap(), ["src/**/*"]);
    }

    #[test]
    fn solution_configs_follow_their_references_and_a_missing_config_is_none() {
        let found = globs(
            &[
                (
                    "/ws/tsconfig.json",
                    r#"{"files":[],"references":[{"path":"./tsconfig.app.json"},{"path":"./tools"},{"path":"../x"}]}"#,
                ),
                ("/ws/tsconfig.app.json", r#"{"include":["src"]}"#),
                ("/ws/tools/tsconfig.json", r#"{"include":["*.ts"]}"#),
                ("/x/tsconfig.json", r#"{"include":["other"]}"#),
            ],
            "/ws/tsconfig.json",
            "/ws",
        );
        assert_eq!(found.unwrap(), ["src/**/*", "tools/*.ts"]);
        assert_eq!(globs(&[], "/ws/tsconfig.json", "/ws"), None);
    }

    #[test]
    fn patterns_outside_the_package_are_dropped() {
        let found = globs(
            &[("/ws/app/tsconfig.json", r#"{"include":["src","../shared"]}"#)],
            "/ws/app/tsconfig.json",
            "/ws/app",
        );
        assert_eq!(found.unwrap(), ["src/**/*"]);
    }
}
