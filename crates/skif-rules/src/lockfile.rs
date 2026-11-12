//! The lockfile the lockfile checks share, read and parsed once per run by the first rule that asks for it:
//! `bun.lock` in a Bun workspace, `pnpm-lock.yaml` otherwise.
//!
//! Only lockfile format 9 of pnpm is read, and Bun's text lockfile, not the binary `bun.lockb`. A check that needs the
//! lockfile and cannot have it (there is none, it is another package manager's, or it does not parse) says so in a
//! diagnostic instead of passing.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;
use skif_core::{Diagnostic, HashMap, LineIndex, Location, RuleMeta, Span, diagnostic};
use skif_lint::LintContext;
use skif_workspace::{
    BUN_LOCKFILE, BunLockfile, OTHER_LOCKFILES, PNPM_LOCKFILE, Package, PackageManager, PnpmLockfile, Resolutions,
    Snapshot, Workspace, WorkspaceError,
};

/// Bun's legacy binary lockfile, which skif does not read.
const BUN_BINARY_LOCKFILE: &str = "bun.lockb";

/// A lockfile the checks read.
pub(crate) enum Lock {
    /// `pnpm-lock.yaml`.
    Pnpm(Box<PnpmLockfile>),
    /// `bun.lock`.
    Bun(Box<BunLock>),
}

/// A parsed `bun.lock` with its text, for the spans of its entries.
pub(crate) struct BunLock {
    lock: BunLockfile,
    source: Arc<str>,
    line_index: LineIndex,
}

impl Lock {
    /// The importer directories, `.` for the root, in byte order.
    pub(crate) fn importers(&self) -> Vec<&str> {
        match self {
            Self::Pnpm(lock) => lock.importers.keys().map(String::as_str).collect(),
            Self::Bun(bun) => bun.lock.importers.keys().map(String::as_str).collect(),
        }
    }

    /// What `importer` reaches, each snapshot with a shortest chain.
    pub(crate) fn resolutions(&self, importer: &str) -> Resolutions<'_> {
        match self {
            Self::Pnpm(lock) => lock.resolutions(importer),
            Self::Bun(bun) => bun.lock.resolutions(importer),
        }
    }

    /// The resolved packages by key: pnpm's snapshot keys, or the keys of `bun.lock`'s `packages`.
    pub(crate) fn snapshots(&self) -> &BTreeMap<String, Snapshot> {
        match self {
            Self::Pnpm(lock) => &lock.snapshots,
            Self::Bun(bun) => &bun.lock.snapshots,
        }
    }

    /// `name@version` of the snapshot `key`, without peers and patches; the key itself when it names no snapshot.
    pub(crate) fn release(&self, key: &str) -> String {
        self.snapshots().get(key).map_or_else(
            || key.to_owned(),
            |snapshot| format!("{}@{}", snapshot.name, snapshot.version),
        )
    }

    /// Where the snapshot `key` is written.
    pub(crate) fn location(&self, key: &str) -> Location {
        match self {
            Self::Pnpm(lock) => Location::new(lock.path.clone(), lock.snapshot_span(key)),
            Self::Bun(bun) => Location::new(
                bun.lock.path.clone(),
                bun_package_span(&bun.source, &bun.line_index, key),
            ),
        }
    }

    /// The pnpm lockfile, for the checks that read what only it models.
    pub(crate) fn pnpm(&self) -> Option<&PnpmLockfile> {
        match self {
            Self::Pnpm(lock) => Some(lock.as_ref()),
            Self::Bun(_) => None,
        }
    }
}

/// The span of the key of the `packages` entry `key` in the text of a `bun.lock`, quotes included.
fn bun_package_span(source: &str, line_index: &LineIndex, key: &str) -> Option<Span> {
    let packages = source.find("\"packages\":")?;
    let quoted = Value::from(key).to_string();
    let needle = format!("{quoted}: [");
    let start = packages + source[packages..].find(&needle)?;
    let end = start + quoted.len();
    Some(line_index.span(u32::try_from(start).ok()?, u32::try_from(end).ok()?))
}

/// The outcome of reading the lockfile of the workspace root.
pub(crate) struct Loaded {
    /// The lockfile's name: `bun.lock` in a Bun workspace, `pnpm-lock.yaml` otherwise.
    file: &'static str,
    lock: Result<Option<Lock>, WorkspaceError>,
}

/// The lockfile of the run, read on the first call.
pub(crate) fn load(cx: &LintContext<'_>) -> Arc<Loaded> {
    cx.shared(|| {
        let root = &cx.workspace.root;
        if cx.workspace.package_manager == Some(PackageManager::Bun) {
            Loaded {
                file: BUN_LOCKFILE,
                lock: read_bun(cx, root),
            }
        } else {
            Loaded {
                file: PNPM_LOCKFILE,
                lock: PnpmLockfile::read(cx.fs(), root).map(|lock| lock.map(|lock| Lock::Pnpm(Box::new(lock)))),
            }
        }
    })
}

fn read_bun(cx: &LintContext<'_>, root: &Path) -> Result<Option<Lock>, WorkspaceError> {
    let path = root.join(BUN_LOCKFILE);
    if !cx.fs().is_file(&path) {
        return Ok(None);
    }
    let source = cx
        .fs()
        .read_to_string(&path)
        .map_err(|source| WorkspaceError::Io { path, source })?;
    let lock = BunLockfile::parse(PathBuf::from(BUN_LOCKFILE), &source)?;
    Ok(Some(Lock::Bun(Box::new(BunLock {
        lock,
        line_index: LineIndex::new(&source),
        source,
    }))))
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
    ) -> Result<&Lock, Box<Diagnostic>> {
        let file = self.file;
        let lockfile = Path::new(file);
        let install = if file == BUN_LOCKFILE {
            "bun install"
        } else {
            "pnpm install"
        };
        let (message, location, help) = match &self.lock {
            Ok(Some(lock)) => return Ok(lock),
            Err(WorkspaceError::InvalidLockfile { message, .. }) => (
                format!("`{file}` cannot be read ({message}), so nothing is checked for {what}"),
                Location::new(lockfile, None),
                format!("fix or regenerate the lockfile with `{install}`"),
            ),
            Err(error) => (
                format!("`{file}` cannot be read ({error}), so nothing is checked for {what}"),
                Location::new(lockfile, None),
                "make the lockfile readable".to_owned(),
            ),
            Ok(None) => match self.unread(cx) {
                Some((other, help)) => (
                    format!("skif reads `{file}` only; `{other}` is not checked for {what}"),
                    Location::new(PathBuf::from(other), None),
                    help.to_owned(),
                ),
                None => (
                    format!("there is no `{file}`, so nothing is checked for {what}"),
                    fallback,
                    format!("run `{install}` to write the lockfile"),
                ),
            },
        };
        Err(Box::new(diagnostic(meta, message, location).help(help).build()))
    }

    /// A lockfile at the root that skif does not read in place of the missing one, with the help to give.
    fn unread(&self, cx: &LintContext<'_>) -> Option<(&'static str, &'static str)> {
        let exists = |name: &&'static str| cx.fs().is_file(&cx.workspace.root.join(name));
        if self.file == BUN_LOCKFILE {
            return [BUN_BINARY_LOCKFILE]
                .into_iter()
                .find(exists)
                .map(|name| (name, "write the text lockfile with `bun install --save-text-lockfile`"));
        }
        OTHER_LOCKFILES.into_iter().find(exists).map(|name| {
            (
                name,
                "lockfile checks support pnpm (lockfile format 9) and `bun.lock` only",
            )
        })
    }
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
