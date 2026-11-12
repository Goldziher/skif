//! Test helpers: in-memory workspaces, rule runs and fix application.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use skif_config::Config;
use skif_core::{Diagnostic, FileSystem, FixOp, MemoryFs, Span};
use skif_edit::FileKind;
use skif_lint::{Registry, RunOptions, run_unsorted};
use skif_workspace::Workspace;

pub type Files<'a> = &'a [(&'a str, &'a str)];

pub fn config(toml: &str) -> Config {
    Config::from_toml_str(toml).unwrap()
}

/// The files under `/ws`.
pub fn memory(files: Files<'_>) -> MemoryFs {
    files.iter().fold(MemoryFs::new(), |fs, (path, content)| {
        fs.with_file(Path::new("/ws").join(path), content)
    })
}

pub fn workspace(files: Files<'_>, config: &Config) -> Workspace {
    Workspace::discover_with_fs(Path::new("/ws"), config, &memory(files)).unwrap()
}

/// Runs every built-in provider, the toolchain rules reading `files` too, and keeps the diagnostics of `rule`.
pub fn lint(rule: &str, files: Files<'_>, config: &Config) -> Vec<Diagnostic> {
    lint_with(rule, files, config, &RunOptions::default())
}

/// As [`lint`], with the given run options; the rules read the lockfile and patch files from the same in-memory files.
pub fn lint_with(rule: &str, files: Files<'_>, config: &Config, options: &RunOptions<'_>) -> Vec<Diagnostic> {
    lint_in(rule, &memory(files), config, options)
}

/// As [`lint_with`], discovering the workspace at `/ws` of `fs`.
pub fn lint_in(rule: &str, fs: &dyn FileSystem, config: &Config, options: &RunOptions<'_>) -> Vec<Diagnostic> {
    let workspace = Workspace::discover_with_fs(Path::new("/ws"), config, fs).unwrap();
    let registry = Registry::new(crate::providers()).unwrap();
    run_unsorted(&registry, &workspace, config, &options.with_fs(fs))
        .unwrap()
        .into_iter()
        .filter(|diagnostic| diagnostic.rule.as_str() == rule)
        .collect()
}

/// The text a span covers in the file at `path`.
pub fn slice<'a>(files: Files<'a>, path: &str, span: Span) -> &'a str {
    let (_, source) = files.iter().find(|(candidate, _)| *candidate == path).unwrap();
    &source[span.start as usize..span.end as usize]
}

/// The text the primary location of `diagnostic` covers.
pub fn located<'a>(files: Files<'a>, diagnostic: &Diagnostic) -> &'a str {
    let path = diagnostic.location.path.to_str().unwrap();
    slice(files, path, diagnostic.location.span.unwrap())
}

/// Applies the preferred fix of every diagnostic and returns the resulting files.
pub fn apply_fixes(files: Files<'_>, diagnostics: &[Diagnostic]) -> Vec<(String, String)> {
    let mut contents: BTreeMap<String, String> = files
        .iter()
        .map(|(path, content)| ((*path).to_owned(), (*content).to_owned()))
        .collect();
    let name = |path: &PathBuf| path.to_string_lossy().replace('\\', "/");
    let mut by_file: BTreeMap<String, Vec<&FixOp>> = BTreeMap::new();
    for op in diagnostics
        .iter()
        .filter_map(|diagnostic| diagnostic.fixes.first())
        .flat_map(|fix| &fix.ops)
    {
        match op {
            FixOp::Json { path, .. } | FixOp::Yaml { path, .. } | FixOp::Text { path, .. } => {
                by_file.entry(name(path)).or_default().push(op);
            }
            FixOp::CreateFile {
                path, contents: text, ..
            } => {
                contents.insert(name(path), text.clone());
            }
            FixOp::DeleteFile { path, .. } => {
                contents.remove(&name(path));
            }
            other => panic!("unexpected fix operation {other:?}"),
        }
    }
    for (path, ops) in by_file {
        let source = contents.get(&path).unwrap();
        let kind = FileKind::from_path(Path::new(&path)).unwrap();
        let edited = skif_edit::apply(source, kind, &ops).unwrap();
        contents.insert(path, edited);
    }
    contents.into_iter().collect()
}

pub fn borrowed(files: &[(String, String)]) -> Vec<(&str, &str)> {
    files
        .iter()
        .map(|(path, text)| (path.as_str(), text.as_str()))
        .collect()
}

/// The content of `path` after applying every fix of `rule`, parsed as JSON.
pub fn fixed_json(rule: &str, files: Files<'_>, config: &Config, path: &str) -> serde_json::Value {
    let diagnostics = lint(rule, files, config);
    let fixed = apply_fixes(files, &diagnostics);
    let (_, text) = fixed.iter().find(|(candidate, _)| candidate == path).unwrap();
    serde_json::from_str(text).unwrap()
}

/// Asserts that `rule` reports something, and nothing once its fixes are applied.
pub fn assert_fixes_clear(rule: &str, files: Files<'_>, config: &Config) {
    let before = lint(rule, files, config);
    assert!(!before.is_empty(), "{rule} reported nothing to fix");
    assert!(
        before.iter().all(Diagnostic::is_fixable),
        "{rule} has an unfixable diagnostic"
    );
    let fixed = apply_fixes(files, &before);
    let after = lint(rule, &borrowed(&fixed), config);
    assert!(after.is_empty(), "{rule} still reports after fixing: {after:#?}");
}

/// `(package, dependency)` pairs of the diagnostics, for table assertions.
pub fn subjects(diagnostics: &[Diagnostic]) -> Vec<(String, Option<String>)> {
    diagnostics
        .iter()
        .map(|diagnostic| {
            (
                diagnostic.package.clone().unwrap_or_default(),
                diagnostic.dependency.clone(),
            )
        })
        .collect()
}

/// A valid root manifest; `extra` is spliced in as further members and must start with a comma.
pub fn root(extra: &str) -> String {
    format!(r#"{{"name":"root","private":true,"packageManager":"pnpm@9.0.0","workspaces":["packages/*"]{extra}}}"#)
}
