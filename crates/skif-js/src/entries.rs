//! Entry files: configured and default globs, what `package.json` publishes, and what scripts run.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use skif_core::FileSystem;
use skif_workspace::{PackageId, Workspace};

use crate::discover::{Candidate, relative};
use crate::globs::Globs;
use crate::manifest_paths::{manifest_targets, map_to_source, source_patterns};
use crate::resolve::is_code_file;
use crate::setup::{PackageSetup, Setup};

/// The entry files of every package, sorted and without duplicates.
pub(crate) fn entry_files(
    fs: &dyn FileSystem,
    workspace: &Workspace,
    setup: &Setup,
    candidates: &[Candidate],
) -> Vec<PathBuf> {
    let mut entries = BTreeSet::new();
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
        for target in manifest_targets(&package.manifest.value) {
            if target.contains('*') {
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
                entries.insert(path);
            }
        }
        if !setup.production {
            entries.extend(script_files(fs, package_setup));
        }
    }
    entries.into_iter().collect()
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
