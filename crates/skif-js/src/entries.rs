//! Entry files: configured and default globs, what `package.json` publishes, and what scripts run.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use skif_core::FileSystem;
use skif_workspace::{PackageId, Workspace};

use crate::ancestors::external_manifests;
use crate::discover::{Candidate, relative};
use crate::globs::Globs;
use crate::manifest_paths::{
    bin_stub_sources, bin_targets, crossing_stars, manifest_targets, map_to_source, should_have_source, source_patterns,
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
    /// or below their directory, no entry and no script, whose manifest only lists what code elsewhere installs it
    /// for.
    pub unknown: Vec<PackageId>,
}

/// The entry files of every package.
pub(crate) fn entry_files(
    fs: &dyn FileSystem,
    workspace: &Workspace,
    setup: &Setup,
    candidates: &[Candidate],
) -> EntryFiles {
    let mut entries = BTreeSet::new();
    let mut unknown = Vec::new();
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
        for (candidate, rel) in owned() {
            if package_setup.entry.is_match(&rel) {
                own.insert(candidate.abs.clone());
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
                own.extend(
                    owned()
                        .filter(|(_, rel)| globs.is_match(rel))
                        .map(|(candidate, _)| candidate.abs.clone()),
                );
            } else if let Some(path) = map_to_source(fs, &package_setup.dir, &target)
                && is_code_file(&path)
            {
                mapped_any = true;
                own.insert(path);
            } else if should_have_source(&target) {
                published_without_source = true;
            }
        }
        for target in bin_targets(package.manifest.value()) {
            let sources = bin_stub_sources(fs, &package_setup.dir, &target);
            mapped_any |= !sources.is_empty();
            own.extend(sources);
        }
        if published_without_source && !mapped_any {
            unknown.push(id);
        }
        own.extend(
            package_setup
                .config_files
                .iter()
                .filter(|path| is_code_file(path) && fs.is_file(path))
                .cloned(),
        );
        if !setup.production {
            own.extend(script_files(fs, package_setup));
        }
        let has_entries = !own.is_empty();
        entries.append(&mut own);
        if !has_entries
            && package.manifest.value().get("scripts").is_none_or(is_empty_object)
            && below(candidates, &workspace.root, &package_setup.dir).is_empty()
        {
            unknown.push(id);
        }
    }
    if !setup.production {
        entries.extend(ancestor_script_files(fs, workspace, candidates));
    }
    EntryFiles {
        files: entries.into_iter().collect(),
        unknown,
    }
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
            if candidates
                .binary_search_by(|candidate| candidate.rel.as_str().cmp(rel.as_str()))
                .is_ok()
            {
                files.push(path);
            }
        }
    }
    files
}

fn is_empty_object(value: &serde_json::Value) -> bool {
    value.as_object().is_some_and(serde_json::Map::is_empty)
}

fn script_files(fs: &dyn FileSystem, package_setup: &PackageSetup) -> Vec<PathBuf> {
    package_setup
        .scripts
        .files
        .iter()
        .map(|file| package_setup.dir.join(file.trim_start_matches("./")))
        .filter(|path| is_code_file(path) && fs.is_file(path))
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
