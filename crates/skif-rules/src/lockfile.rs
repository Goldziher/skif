//! The `pnpm-lock.yaml` the lockfile checks share, read and parsed once per run by the first rule that asks for it.
//!
//! Only lockfile format 9 of pnpm is read. A check that needs the lockfile and cannot have it (there is none, it is
//! another package manager's, or it does not parse) says so in a diagnostic instead of passing.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use skif_core::{Diagnostic, HashMap, Location, RuleMeta, diagnostic};
use skif_lint::LintContext;
use skif_workspace::{OTHER_LOCKFILES, PNPM_LOCKFILE, Package, PnpmLockfile, Workspace, WorkspaceError, split_key};

/// The outcome of reading the lockfile of the workspace root.
pub(crate) struct Loaded(Result<Option<PnpmLockfile>, WorkspaceError>);

/// The lockfile of the run, read on the first call.
pub(crate) fn load(cx: &LintContext<'_>) -> Arc<Loaded> {
    cx.shared(|| Loaded(PnpmLockfile::read(cx.fs(), &cx.workspace.root)))
}

impl Loaded {
    /// The lockfile, or the diagnostic of `meta` that says nothing is checked for `what`, and why. `fallback` is where
    /// that diagnostic points when there is no lockfile at all: the file that asked for the check.
    pub(crate) fn get(
        &self,
        cx: &LintContext<'_>,
        meta: &'static RuleMeta,
        what: &str,
        fallback: Location,
    ) -> Result<&PnpmLockfile, Box<Diagnostic>> {
        let lockfile = Path::new(PNPM_LOCKFILE);
        let (message, location, help) = match &self.0 {
            Ok(Some(lock)) => return Ok(lock),
            Err(WorkspaceError::InvalidLockfile { message, .. }) => (
                format!("`{PNPM_LOCKFILE}` cannot be read ({message}), so nothing is checked for {what}"),
                Location::new(lockfile, None),
                "fix or regenerate the lockfile with `pnpm install`".to_owned(),
            ),
            Err(error) => (
                format!("`{PNPM_LOCKFILE}` cannot be read ({error}), so nothing is checked for {what}"),
                Location::new(lockfile, None),
                "make the lockfile readable".to_owned(),
            ),
            Ok(None) => match OTHER_LOCKFILES
                .iter()
                .find(|name| cx.fs().is_file(&cx.workspace.root.join(name)))
            {
                Some(other) => (
                    format!("skif reads `{PNPM_LOCKFILE}` only; `{other}` is not checked for {what}"),
                    Location::new(PathBuf::from(other), None),
                    "lockfile checks support pnpm (lockfile format 9) only".to_owned(),
                ),
                None => (
                    format!("there is no `{PNPM_LOCKFILE}`, so nothing is checked for {what}"),
                    fallback,
                    "run `pnpm install` to write the lockfile".to_owned(),
                ),
            },
        };
        Err(Box::new(diagnostic(meta, message, location).help(help).build()))
    }
}

/// The location of the snapshot `key` in the lockfile.
pub(crate) fn snapshot_location(lock: &PnpmLockfile, key: &str) -> Location {
    Location::new(lock.path.clone(), lock.snapshot_span(key))
}

/// `name@version` of a snapshot key, without the peer and patch suffix.
pub(crate) fn display_key(key: &str) -> String {
    split_key(key).map_or_else(|| key.to_owned(), |(name, version, _)| format!("{name}@{version}"))
}

/// The workspace package of each importer directory of the lockfile (`.` is the root), indexed once: the checks look
/// one up per importer, and a lockfile has an importer per package.
pub(crate) struct ImporterPackages<'w> {
    by_dir: HashMap<&'w str, &'w Package>,
}

impl<'w> ImporterPackages<'w> {
    pub(crate) fn index(workspace: &'w Workspace) -> Self {
        let mut by_dir: HashMap<&'w str, &'w Package> = HashMap::default();
        for package in workspace.all_packages() {
            if let Some(dir) = package.dir.to_str() {
                // The first package of a directory, as a search in workspace order finds it.
                by_dir.entry(dir).or_insert(package);
            }
        }
        Self { by_dir }
    }

    /// The package of the importer `importer`.
    pub(crate) fn get(&self, importer: &str) -> Option<&'w Package> {
        let dir = if importer == "." { "" } else { importer };
        self.by_dir.get(dir).copied()
    }
}
