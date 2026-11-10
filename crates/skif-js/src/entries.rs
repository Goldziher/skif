//! Entry files: configured and default globs, what `package.json` publishes, and what scripts run.

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use serde_json::Value;
use skif_core::FileSystem;
use skif_workspace::{PackageId, Workspace};

use crate::analysis::on_disk;
use crate::ancestors::external_manifests;
use crate::discover::{Candidate, is_ignored_in_repository, relative};
use crate::globs::Globs;
use crate::manifest_paths::{
    bin_stub_sources, bin_targets, crossing_stars, is_out_dir, manifest_targets, map_to_source, should_have_source,
    source_patterns,
};
use crate::resolve::{is_code_file, join_normalized};
use crate::scripts;
use crate::setup::{PackageSetup, Setup};

/// The entry files of the workspace.
pub(crate) struct EntryFiles {
    /// Sorted and without duplicates.
    pub files: Vec<PathBuf>,
    /// Packages whose real entries are unknown, so their files and dependencies cannot be judged unused: those that
    /// publish build output or bin files none of which maps back to a source file, and those with no source file at
    /// or below their directory other than a package manager's install hook (`.pnpmfile.cjs`), no other entry and no
    /// script, whose manifest only lists what code elsewhere installs it for. A published path that names no file at
    /// all ([`names_no_file`]) is no entry and does not make a package's entries unknown (ADR 0069).
    pub unknown: Vec<PackageId>,
    /// The entry files something other than the configured and default `entry` globs makes entries: plugins, the
    /// manifest, tool configuration files and scripts (with `include-entry-exports`, only plugin entry globs and the
    /// manifest, whose exports are judged like a configured entry's). A configured entry naming one of them is
    /// redundant (knip's `entry-redundant`).
    pub provided: HashSet<PathBuf>,
}

/// Where published paths are looked up to tell build output not built yet from a path into nothing: `fs` lists
/// gitignored files too, and `gitignore` says whether `.gitignore` files apply.
#[derive(Clone, Copy)]
pub(crate) struct Disk<'a> {
    pub fs: &'a dyn FileSystem,
    pub gitignore: bool,
}

/// The entry files of every package. `extra` are files, relative to the workspace root, that other roots of a
/// multi-root run import; those the analysis owns are entries too.
pub(crate) fn entry_files(
    fs: &dyn FileSystem,
    workspace: &Workspace,
    setup: &Setup,
    candidates: &[Candidate],
    extra: &[PathBuf],
    disk: Disk<'_>,
) -> EntryFiles {
    let mut entries = BTreeSet::new();
    let mut unknown = Vec::new();
    let mut provided = HashSet::new();
    let mut by_owner: Vec<Vec<&Candidate>> = vec![Vec::new(); workspace.iter().count()];
    for candidate in candidates {
        if let Some(owned) = by_owner.get_mut(candidate.owner.index()) {
            owned.push(candidate);
        }
    }
    for (id, package) in workspace.iter() {
        let Some(package_setup) = setup.package(id) else {
            continue;
        };
        let mine: &[&Candidate] = by_owner.get(id.index()).map_or(&[], Vec::as_slice);
        let owned = || owned_by(mine, &package_setup.dir);
        // The entries of this package alone: another package may already have found the same files.
        let mut own = BTreeSet::new();
        // What provides an entry besides the configured and default globs.
        let mut from_tools = BTreeSet::new();
        for (candidate, rel) in owned() {
            if package_setup.entry.is_match(&rel) {
                own.insert(candidate.abs.clone());
                if package_setup.plugin_entry.is_match(&rel) {
                    from_tools.insert(candidate.abs.clone());
                }
            }
        }
        own.extend(nested_entries(workspace, setup, candidates, id, package_setup));
        if !package_setup.external_entry.is_empty() {
            own.extend(
                candidates
                    .iter()
                    .filter(|candidate| candidate.owner != id && package_setup.external_entry.is_match(&candidate.rel))
                    .map(|candidate| candidate.abs.clone()),
            );
        }
        let mut published_without_source = false;
        let mut mapped_any = false;
        for manifest_target in manifest_targets(package.manifest.value()) {
            let target = manifest_target.path;
            if target.contains('*') {
                mapped_any = true;
                let patterns = source_patterns(&target)
                    .into_iter()
                    .chain(std::iter::once(target.trim_start_matches("./").to_owned()))
                    .map(|pattern| {
                        if manifest_target.from_exports {
                            crossing_stars(&pattern)
                        } else {
                            pattern
                        }
                    });
                let globs = Globs::new(patterns);
                from_tools.extend(
                    owned()
                        .filter(|(_, rel)| globs.is_match(rel))
                        .map(|(candidate, _)| candidate.abs.clone()),
                );
            } else if let Some(path) = map_published(fs, package_setup, &target)
                && is_code_file(&path)
            {
                mapped_any = true;
                from_tools.insert(path);
            } else if should_have_source(&target)
                && !names_no_file(disk, &workspace.root, package_setup, package.manifest.value(), &target)
            {
                published_without_source = true;
            }
        }
        for target in bin_targets(package.manifest.value()) {
            let sources = bin_stub_sources(fs, &package_setup.dir, &target);
            mapped_any |= !sources.is_empty();
            from_tools.extend(sources);
        }
        own.extend(from_tools.iter().cloned());
        provided.extend(from_tools);
        if published_without_source && !mapped_any {
            unknown.push(id);
        }
        let mut tool_entries: Vec<PathBuf> = package_setup
            .config_files
            .iter()
            .filter(|path| is_code_file(path) && fs.is_file(path))
            .cloned()
            .collect();
        if !setup.production {
            tool_entries.extend(script_files(fs, package_setup, mine));
        }
        if !package_setup.include_entry_exports {
            provided.extend(tool_entries.iter().cloned());
        }
        own.extend(tool_entries);
        // An install hook is an entry, so what it imports is used, but it is no project code: a package with nothing
        // else lists what code elsewhere installs it for.
        let has_entries = own.iter().any(|path| !is_install_hook(path));
        entries.append(&mut own);
        entries.extend(scope_entries(workspace, setup, candidates, package_setup));
        if !has_entries
            && package.manifest.value().get("scripts").is_none_or(is_empty_object)
            && below(candidates, &workspace.root, &package_setup.dir)
                .iter()
                .all(|candidate| is_install_hook(&candidate.abs))
        {
            unknown.push(id);
        }
    }
    if !setup.production {
        entries.extend(ancestor_script_files(fs, workspace, candidates));
    }
    entries.extend(
        extra
            .iter()
            .filter_map(|file| candidate_at(candidates, &file.to_string_lossy().replace('\\', "/")))
            .map(|candidate| candidate.abs.clone()),
    );
    EntryFiles {
        files: entries.into_iter().collect(),
        unknown,
        provided,
    }
}

/// The file names of the hooks a package manager loads while it installs a project: `.pnpmfile.cjs` and
/// `.pnpmfile.mjs`.
const INSTALL_HOOKS: &[&str] = &[".pnpmfile.cjs", ".pnpmfile.mjs"];

/// Whether `path` is a package manager's install hook ([`INSTALL_HOOKS`]): it shapes how dependencies are installed
/// and is not code of the project, so it alone does not give a package source files whose dependencies can be judged.
fn is_install_hook(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| INSTALL_HOOKS.contains(&name))
}

/// Files of workspace packages nested below `package_setup` that its plugin and configuration entry globs match.
/// Packages excluded with `ignore-packages` own no files that count.
fn nested_entries<'a>(
    workspace: &'a Workspace,
    setup: &'a Setup,
    candidates: &'a [Candidate],
    id: PackageId,
    package_setup: &'a PackageSetup,
) -> impl Iterator<Item = PathBuf> + 'a {
    let nested: &[Candidate] = if package_setup.shared_entry.is_empty() {
        &[]
    } else {
        below(candidates, &workspace.root, &package_setup.dir)
    };
    nested
        .iter()
        .filter(move |candidate| candidate.owner != id && setup.package(candidate.owner).is_some())
        .filter(move |candidate| {
            relative(&candidate.abs, &package_setup.dir).is_some_and(|rel| package_setup.shared_entry.is_match(&rel))
        })
        .map(|candidate| candidate.abs.clone())
}

/// The candidates below `dir`. They are one run of `candidates`, which are sorted by their path relative to `root`,
/// so the run is found by binary search instead of testing every candidate.
fn below<'a>(candidates: &'a [Candidate], root: &Path, dir: &Path) -> &'a [Candidate] {
    let Some(relative_dir) = relative(dir, root) else {
        return &[];
    };
    if relative_dir.is_empty() {
        return candidates;
    }
    let prefix = format!("{relative_dir}/");
    let start = candidates.partition_point(|candidate| candidate.rel.as_str() < prefix.as_str());
    let run = candidates[start..]
        .iter()
        .take_while(|candidate| candidate.rel.starts_with(&prefix))
        .count();
    &candidates[start..start + run]
}

/// The files the narrowed scope of an incomplete package matches (ADR 0065), its own and those of the analysed
/// workspace packages nested below it, whatever its `project` globs say: each may be an entry its configs hide (a
/// setup module in `test/`, a nested package's test a root test config runs), so they are entries and what they
/// import is counted (ADR 0081).
fn scope_entries(
    workspace: &Workspace,
    setup: &Setup,
    candidates: &[Candidate],
    package: &PackageSetup,
) -> Vec<PathBuf> {
    if package.incomplete_scope.is_none() {
        return Vec::new();
    }
    below(candidates, &workspace.root, &package.dir)
        .iter()
        .filter(|candidate| setup.package(candidate.owner).is_some())
        .filter(|candidate| relative(&candidate.abs, &package.dir).is_some_and(|rel| package.in_incomplete_scope(&rel)))
        .map(|candidate| candidate.abs.clone())
        .collect()
}

/// The candidates of one package, with their path relative to the package directory `dir`.
fn owned_by<'a>(owned: &'a [&'a Candidate], dir: &'a Path) -> impl Iterator<Item = (&'a Candidate, String)> + 'a {
    owned
        .iter()
        .copied()
        .filter_map(move |candidate| Some((candidate, relative(&candidate.abs, dir)?)))
}

/// The files of this workspace that the scripts of a `package.json` above its root run, up to the repository root:
/// `node build/npm/postinstall.ts` in the outer project makes that file an entry of the nested root at `build`. Only
/// files the analysis owns count, so a file of an ignored package or of another project stays out.
fn ancestor_script_files(fs: &dyn FileSystem, workspace: &Workspace, candidates: &[Candidate]) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for (dir, manifest) in external_manifests(fs, &workspace.root) {
        for file in scripts::analyze_manifest(&manifest).files {
            let path = join_normalized(&dir, &file);
            let Some(rel) = relative(&path, &workspace.root) else {
                continue;
            };
            if candidate_at(candidates, &rel).is_some() {
                files.push(path);
            }
        }
    }
    files
}

/// The candidate at `rel`, relative to the workspace root with `/` separators; candidates are sorted by that path.
fn candidate_at<'a>(candidates: &'a [Candidate], rel: &str) -> Option<&'a Candidate> {
    candidates
        .binary_search_by(|candidate| candidate.rel.as_str().cmp(rel))
        .ok()
        .map(|position| &candidates[position])
}

fn is_empty_object(value: &serde_json::Value) -> bool {
    value.as_object().is_some_and(serde_json::Map::is_empty)
}

/// The files the package's scripts run. A path inside a build output directory (`outDir` of a `tsconfig`) stands for
/// its source below the matching `rootDir`, even when the build output is on disk, and a directory a test runner reads
/// (`mocha build/compiled/test`) for the code files the runner reads in it.
fn script_files(fs: &dyn FileSystem, package_setup: &PackageSetup, mine: &[&Candidate]) -> Vec<PathBuf> {
    let dir = &package_setup.dir;
    let mut found = Vec::new();
    for file in &package_setup.scripts.files {
        let path = join_normalized(dir, file);
        let rel = relative(&path, dir);
        let sources = rel
            .as_deref()
            .map(|rel| source_paths(package_setup, rel))
            .unwrap_or_default();
        let mapped = sources
            .iter()
            .filter_map(|source| map_to_source(fs, dir, source))
            .find(|source| is_code_file(source));
        if let Some(source) = mapped {
            found.push(source);
            continue;
        }
        if is_code_file(&path) && fs.is_file(&path) {
            found.push(path);
            continue;
        }
        let (Some(rel), Some(tree)) = (rel, package_setup.scripts.file_trees.get(file)) else {
            continue;
        };
        let built = !sources.is_empty();
        let extensions = tree_extensions(&tree.extensions, built);
        let directories = if built { sources } else { vec![rel] };
        for directory in directories.iter().filter(|directory| !directory.is_empty()) {
            let prefix = format!("{directory}/");
            found.extend(
                owned_by(mine, dir)
                    .filter(|(candidate, rel)| {
                        rel.strip_prefix(&prefix)
                            .is_some_and(|rest| tree.recursive || !rest.contains('/'))
                            && is_code_file(&candidate.abs)
                            && has_extension(&candidate.abs, &extensions)
                    })
                    .map(|(candidate, _)| candidate.abs.clone()),
            );
        }
    }
    found
}

/// The extensions a test runner reads from a directory. For a build output directory its source directory is read
/// instead, so each output extension also stands for the TypeScript sources that compile to it.
fn tree_extensions(extensions: &[String], built: bool) -> Vec<String> {
    let mut all: Vec<String> = extensions
        .iter()
        .map(|extension| extension.to_ascii_lowercase())
        .collect();
    if built {
        for extension in extensions {
            let sources: &[&str] = match extension.to_ascii_lowercase().as_str() {
                "js" => &["ts", "tsx"],
                "jsx" => &["tsx"],
                "mjs" => &["mts"],
                "cjs" => &["cts"],
                _ => &[],
            };
            all.extend(sources.iter().map(|&source| source.to_owned()));
        }
    }
    all
}

/// Whether `path` ends in one of `extensions` (lowercase, without the dot).
fn has_extension(path: &Path, extensions: &[String]) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extensions.contains(&extension.to_ascii_lowercase()))
}

/// The source file behind a published path: through the package's build directories first, then by the `dist` to
/// `src` convention.
pub(crate) fn map_published(fs: &dyn FileSystem, package_setup: &PackageSetup, target: &str) -> Option<PathBuf> {
    let dir = &package_setup.dir;
    relative(&join_normalized(dir, target), dir)
        .map(|rel| source_paths(package_setup, &rel))
        .unwrap_or_default()
        .iter()
        .find_map(|source| map_to_source(fs, dir, source))
        .or_else(|| map_to_source(fs, dir, target))
}

/// Whether the published path `target` of the package with `manifest` names no file: neither the path nor a source
/// file behind it exists, git does not ignore it and it is not build output not built yet. A path in a build output
/// directory (`dist`, `lib`, ...), a gitignored path (by an ignore file of the repository above or below the analysed
/// root, or its `info/exclude`) and a path in `bin` of a package with a build step ([`may_build_bin`]) are build
/// output. A path outside the package is never judged. Such a path is no entry, so it neither makes the package's
/// entries unknown nor uses anything (ADR 0069); `stale-js-config` reports it (`package-entry`).
pub(crate) fn names_no_file(
    disk: Disk<'_>,
    root: &Path,
    package_setup: &PackageSetup,
    manifest: &Value,
    target: &str,
) -> bool {
    let rel = target.trim_start_matches("./");
    if rel.starts_with("../") || rel.starts_with('/') {
        return false;
    }
    // Build output (`dist/index.js`, `dist/style.css`) is missing until the package is built, and the `.gitignore`
    // that hides it may lie above the analysed root, so it is never called missing.
    let first = rel.split_once('/').map(|(first, _)| first);
    if first.is_some_and(is_out_dir) {
        return false;
    }
    if first == Some("bin") && may_build_bin(disk.fs, package_setup, manifest) {
        return false;
    }
    let dir = &package_setup.dir;
    if disk.gitignore && is_ignored_in_repository(disk.fs, root, &dir.join(rel)) {
        return false;
    }
    if rel.contains('*') {
        return !on_disk(disk.fs, dir, rel)
            && !source_patterns(rel)
                .iter()
                .any(|pattern| on_disk(disk.fs, dir, pattern));
    }
    let path = join_normalized(dir, rel);
    !disk.fs.is_file(&path) && !disk.fs.kind(&path).is_dir && map_published(disk.fs, package_setup, rel).is_none()
}

/// Scripts that build a package before it is run or published: `build`, `build:*`, `prepack` and `prepublishOnly`.
/// `prepare` is left out, since it mostly installs git hooks; one that builds runs a builder or the `build` script.
const BUILD_SCRIPTS: &[&str] = &["build", "prepack", "prepublishOnly"];

/// Binaries that compile or bundle code into files a package publishes.
const BUILDERS: &[&str] = &[
    "babel",
    "esbuild",
    "ncc",
    "rolldown",
    "rollup",
    "rslib",
    "rspack",
    "swc",
    "tsc",
    "tsdown",
    "tsgo",
    "tsup",
    "tsup-node",
    "unbuild",
    "webpack",
    "webpack-cli",
];

/// The file name prefixes of the bundler configurations the plugins read, which can write a package's bin files.
const BUNDLER_CONFIGS: &[&str] = &[
    "build.config.",
    "esbuild.",
    "rolldown.config.",
    "rollup.config.",
    "rslib",
    "rspack.config.",
    "tsdown.config.",
    "tsup.config.",
    "webpack.config.",
];

/// Whether a build step of the package may write its `bin` directory, so that a missing file there is build output
/// not built yet rather than a path into nothing: a build script ([`BUILD_SCRIPTS`]), a script that runs a compiler
/// or bundler ([`BUILDERS`]), a bundler configuration a plugin reads ([`BUNDLER_CONFIGS`]), or a build directory
/// (`tsconfig` `outDir`) at or inside `bin`.
fn may_build_bin(fs: &dyn FileSystem, package_setup: &PackageSetup, manifest: &Value) -> bool {
    let build_script = manifest
        .get("scripts")
        .and_then(Value::as_object)
        .is_some_and(|scripts| {
            scripts
                .keys()
                .any(|name| BUILD_SCRIPTS.contains(&name.as_str()) || name.starts_with("build:"))
        });
    let builder = package_setup
        .scripts
        .binaries
        .iter()
        .any(|binary| BUILDERS.contains(&binary.name.as_str()));
    let bundler_config = package_setup.config_files.iter().any(|path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| BUNDLER_CONFIGS.iter().any(|prefix| name.starts_with(prefix)))
            && fs.is_file(path)
    });
    let bin_build_dir = package_setup
        .build_dirs
        .iter()
        .any(|build| build.out_dir == "bin" || build.out_dir.starts_with("bin/"));
    build_script || builder || bundler_config || bin_build_dir
}

/// The source paths behind `rel`, a package-relative path inside the output directory of one of the package's build
/// directories, in the order the build directories are declared.
fn source_paths(package_setup: &PackageSetup, rel: &str) -> Vec<String> {
    package_setup
        .build_dirs
        .iter()
        .filter(|build| !build.out_dir.is_empty() && !build.root_dir.starts_with(".."))
        .filter_map(|build| {
            let rest = rel.strip_prefix(build.out_dir.as_str())?;
            if !rest.is_empty() && !rest.starts_with('/') {
                return None;
            }
            let rest = rest.trim_start_matches('/');
            Some(match (build.root_dir.as_str(), rest) {
                ("", rest) => rest.to_owned(),
                (root, "") => root.to_owned(),
                (root, rest) => format!("{root}/{rest}"),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(rel: &str) -> Candidate {
        Candidate {
            is_link: false,
            abs: Path::new("/ws").join(rel),
            rel: rel.to_owned(),
            owner: PackageId::ROOT,
        }
    }

    fn rels(candidates: &[Candidate]) -> Vec<&str> {
        candidates.iter().map(|candidate| candidate.rel.as_str()).collect()
    }

    #[test]
    fn the_candidates_below_a_directory_are_one_sorted_run() {
        let all = vec![
            candidate("a-b/x.ts"),
            candidate("a/b.ts"),
            candidate("a/c/d.ts"),
            candidate("ab.ts"),
            candidate("z/y.ts"),
        ];
        assert_eq!(
            rels(below(&all, Path::new("/ws"), Path::new("/ws/a"))),
            ["a/b.ts", "a/c/d.ts"]
        );
        assert_eq!(rels(below(&all, Path::new("/ws"), Path::new("/ws/a/c"))), ["a/c/d.ts"]);
        assert_eq!(below(&all, Path::new("/ws"), Path::new("/ws")).len(), 5);
        assert_eq!(
            rels(below(&all, Path::new("/ws"), Path::new("/ws/none"))),
            Vec::<&str>::new()
        );
        assert_eq!(
            rels(below(&all, Path::new("/ws"), Path::new("/other"))),
            Vec::<&str>::new()
        );
    }
}
