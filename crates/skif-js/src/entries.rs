//! Entry files: configured and default globs, what `package.json` publishes, and what scripts run.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use skif_core::FileSystem;
use skif_workspace::{PackageId, Workspace};

use crate::discover::{Candidate, relative};
use crate::globs::Globs;
use crate::manifest_paths::{
    bin_stub_sources, bin_targets, manifest_targets, map_to_source, should_have_source, source_patterns,
};
use crate::resolve::is_code_file;
use crate::setup::{PackageSetup, Setup};

/// The entry files of the workspace.
pub(crate) struct EntryFiles {
    /// Sorted and without duplicates.
    pub files: Vec<PathBuf>,
    /// Packages that publish build output or bin files none of which maps back to a source file. Their real entries
    /// are unknown, so their files and dependencies cannot be judged unused.
    pub unmapped: Vec<PackageId>,
}

/// The entry files of every package.
pub(crate) fn entry_files(
    fs: &dyn FileSystem,
    workspace: &Workspace,
    setup: &Setup,
    candidates: &[Candidate],
) -> EntryFiles {
    let mut entries = BTreeSet::new();
    let mut unmapped = Vec::new();
    for (id, package) in workspace.iter() {
        let Some(package_setup) = setup.package(id) else {
            continue;
        };
        let owned = || owned_by(candidates, id, &package_setup.dir);
        for (candidate, rel) in owned() {
            if package_setup.entry.is_match(&rel) {
                entries.insert(candidate.abs.clone());
            }
        }
        entries.extend(nested_entries(setup, candidates, id, package_setup));
        entries.extend(
            candidates
                .iter()
                .filter(|candidate| candidate.owner != id && package_setup.external_entry.is_match(&candidate.rel))
                .map(|candidate| candidate.abs.clone()),
        );
        let mut published_without_source = false;
        let mut mapped_any = false;
        for target in manifest_targets(package.manifest.value()) {
            if target.contains('*') {
                mapped_any = true;
                let patterns = source_patterns(&target)
                    .into_iter()
                    .chain(std::iter::once(target.trim_start_matches("./").to_owned()));
                let globs = Globs::new(patterns);
                entries.extend(
                    owned()
                        .filter(|(_, rel)| globs.is_match(rel))
                        .map(|(candidate, _)| candidate.abs.clone()),
                );
            } else if let Some(path) = map_to_source(fs, &package_setup.dir, &target)
                && is_code_file(&path)
            {
                mapped_any = true;
                entries.insert(path);
            } else if should_have_source(&target) {
                published_without_source = true;
            }
        }
        for target in bin_targets(package.manifest.value()) {
            let sources = bin_stub_sources(fs, &package_setup.dir, &target);
            mapped_any |= !sources.is_empty();
            entries.extend(sources);
        }
        if published_without_source && !mapped_any {
            unmapped.push(id);
        }
        entries.extend(
            package_setup
                .config_files
                .iter()
                .filter(|path| is_code_file(path) && fs.is_file(path))
                .cloned(),
        );
        if !setup.production {
            entries.extend(script_files(fs, package_setup));
        }
    }
    EntryFiles {
        files: entries.into_iter().collect(),
        unmapped,
    }
}

/// Files of workspace packages nested below `package_setup` that its plugin and configuration entry globs match.
/// Packages excluded with `ignore-packages` own no files that count.
fn nested_entries<'a>(
    setup: &'a Setup,
    candidates: &'a [Candidate],
    id: PackageId,
    package_setup: &'a PackageSetup,
) -> impl Iterator<Item = PathBuf> + 'a {
    let enabled = !package_setup.shared_entry.is_empty();
    candidates
        .iter()
        .filter(move |_| enabled)
        .filter(move |candidate| candidate.owner != id && setup.package(candidate.owner).is_some())
        .filter(move |candidate| {
            relative(&candidate.abs, &package_setup.dir).is_some_and(|rel| package_setup.shared_entry.is_match(&rel))
        })
        .map(|candidate| candidate.abs.clone())
}

/// The candidates owned by package `id`, with their path relative to the package directory.
fn owned_by<'a>(
    candidates: &'a [Candidate],
    id: PackageId,
    dir: &'a Path,
) -> impl Iterator<Item = (&'a Candidate, String)> + 'a {
    candidates
        .iter()
        .filter(move |candidate| candidate.owner == id)
        .filter_map(move |candidate| Some((candidate, relative(&candidate.abs, dir)?)))
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
