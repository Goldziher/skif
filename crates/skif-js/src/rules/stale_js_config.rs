use std::path::Path;

use skif_core::{Category, ConfigJudgement, Fixability, Location, PathSeg, RuleMeta, Severity, diagnostic};
use skif_lint::Emitter;
use skif_workspace::PackageId;

use super::JsRule;
use super::support::{module_location, unresolved_imports};
use crate::analysis::JsAnalysis;
use crate::config_hints::{ConfigHint, HintKind};

/// knip's thresholds for its `top-level-unconfigured` and `workspace-unconfigured` hints: more than this many unused
/// files...
const UNCONFIGURED_MIN_FILES: usize = 20;
/// ...that are more than this share of the files the analysis reached (knip's unused files over its processed ones).
const UNCONFIGURED_MIN_RATIO: f64 = 0.2;

/// Metadata of `stale-js-config`.
pub static META: RuleMeta = RuleMeta::new(
    "stale-js-config",
    Category::Js,
    Some(Severity::Warning),
    Fixability::None,
    "A `[js]` setting matched nothing, is redundant or cancels another pattern, a published entry file is missing, or \
     many files are unused.",
);

/// The rules that consult `ignore-dependencies`. An entry can only be called stale when all of them ran.
/// `stale-mocks` consults it too, but is not needed: when it is off, [`StaleJsConfig`] reads the mocks itself.
const DEPENDENCY_RULES: [&str; 5] = [
    "unused-dependencies",
    "unused-dev-dependencies",
    "unlisted-dependencies",
    "unused-optional-peer-dependencies",
    "unresolved-imports",
];

/// The rules that consult `ignore-unresolved`. An entry can only be called stale when all of them ran.
/// `stale-mocks` consults it too, but is not needed: when it is off, [`StaleJsConfig`] reads the mocks itself.
const UNRESOLVED_RULES: [&str; 1] = ["unresolved-imports"];

/// Reports configuration that does nothing: `ignore-dependencies`, `ignore-binaries`, `ignore-unresolved` and
/// `ignore-workspaces` entries that matched nothing, `[js.workspaces]` keys that select no package, and `entry`,
/// `project` and `ignore` patterns that match no file (knip's `entry-empty`, `project-empty` and `ignore` hints), and
/// `ignore-files` patterns that hide no file `unused-files` would report.
///
/// It also reports knip's other configuration hints: an `entry` pattern naming a file a plugin, the manifest or a script
/// already makes an entry (`entry-redundant`), a `project` pattern naming an entry file (`project-redundant`), a
/// top-level `entry` or `project` in a workspace with several packages (`entry-top-level`, `project-top-level`), a
/// `project` extension skif does not analyse or a component extension it leaves out (`project-extension-*`), a path
/// `package.json` publishes that names no file (`package-entry`), and many unused files, which suggest missing entries
/// (`top-level-unconfigured`, `workspace-unconfigured`). Beyond knip, it reports a plain `entry` or `project` pattern
/// that selects what a pattern ending in `!` of its list selects: in production mode the plain one excludes it.
///
/// It also reports knip's tag hints: a tag that exempts by configuration (`@internal` in production mode, an
/// `ignore-tags` name of `unused-exports`) on an export or enum or namespace member that is used anyway.
///
/// It is the counterpart of knip's configuration hints. Raise it with `[rules] stale-js-config = "error"` to fail the
/// run on them. It runs after every other `js` rule, because an entry is stale only when no rule used it, and it is
/// silent about a list whose consumers did not all run (a disabled rule) or could not judge (a package whose entry
/// files are not fully known). The global `ignore-dependencies` is judged over the packages whose dependencies were
/// judged: an entry is stale when it matched nothing there and matches no dependency that a package with unknown entry
/// files lists, since such a dependency was never checked and the entry may be what keeps it quiet.
pub struct StaleJsConfig;

/// A configuration list the rule judged: its key as written in the file, the key its findings name, the directory of
/// the package whose list it is (for a list a `[js.workspaces]` table gives a package), and each entry with whether
/// it matched nothing.
struct JudgedList<'a> {
    written: String,
    shown: String,
    package: Option<&'a Path>,
    verdicts: Vec<(&'a str, bool)>,
}

impl<'a> JudgedList<'a> {
    /// A list of no package whose findings name it as it is written.
    fn new(key: impl Into<String>, verdicts: Vec<(&'a str, bool)>) -> Self {
        let written = key.into();
        Self {
            shown: written.clone(),
            written,
            package: None,
            verdicts,
        }
    }
}

impl StaleJsConfig {
    /// Records the judgement of every entry of `list` and reports the stale ones.
    fn judge(analysis: &JsAnalysis<'_>, out: &mut Emitter<'_>, list: &JudgedList<'_>) {
        let Some(path) = Self::config_path(analysis) else {
            return;
        };
        for &(entry, stale) in &list.verdicts {
            let message = format!("`{entry}` in `{}` matched nothing", list.shown);
            let judgement = ConfigJudgement::new(path.clone(), list.written.as_str(), entry, stale, message.as_str());
            out.report_config_judgement(match list.package {
                Some(dir) => judgement.with_package(dir),
                None => judgement,
            });
            if stale {
                out.emit(
                    diagnostic(&META, message, Location::new(path.clone(), None))
                        .help("remove the entry, or correct it if it was meant to match")
                        .build(),
                );
            }
        }
    }
}

impl StaleJsConfig {
    fn report_tags(analysis: &JsAnalysis<'_>, out: &mut Emitter<'_>) {
        for module in analysis.graph.modules() {
            for hint in analysis.unneeded_tags(module) {
                let mut builder = diagnostic(
                    &META,
                    format!(
                        "`{}` is used, so its `@{}` tag exempts nothing",
                        hint.identifier, hint.tag
                    ),
                    module_location(module, Some(hint.span)),
                )
                .help("remove the tag");
                if let Some(package) = analysis.workspace.package(module.owner) {
                    builder = builder.package(package.display_name());
                }
                out.emit(builder.build());
            }
        }
    }
}

impl StaleJsConfig {
    fn config_path(analysis: &JsAnalysis<'_>) -> Option<std::path::PathBuf> {
        analysis.config_path.clone().or_else(|| {
            analysis
                .workspace
                .package(PackageId::ROOT)
                .map(|package| package.manifest.path.clone())
        })
    }

    fn report_hint(analysis: &JsAnalysis<'_>, out: &mut Emitter<'_>, hint: &ConfigHint) {
        let ConfigHint {
            kind,
            key,
            identifier,
            cancelled,
            ..
        } = hint;
        let (message, help) = match kind {
            HintKind::EntryRedundant => (
                format!("`{identifier}` in `{key}` is already an entry of a plugin, `package.json` or a script"),
                "remove the redundant pattern",
            ),
            HintKind::ProjectRedundant => (
                format!("`{identifier}` in `{key}` is an entry file, which is always part of the project"),
                "remove the redundant pattern",
            ),
            HintKind::TopLevel => (
                format!("`{key}` = {identifier} applies to the root package only, but the workspace has several"),
                "remove it, or move it to the `[js.workspaces]` table of the package it is meant for",
            ),
            HintKind::ExtensionUnregistered => (
                format!("`{identifier}` files selected by `{key}` are not analysed"),
                "remove the extension: skif reads JS, TS, Vue, Svelte, Astro, MDX, TSRX and stylesheet files",
            ),
            HintKind::ExtensionExcluded => (
                format!("`{key}` leaves out the package's `{identifier}` files"),
                "add the extension to the `project` patterns",
            ),
            HintKind::PackageEntry => (
                format!("`{identifier}` in `{key}` of `package.json` names no file"),
                "correct the path, or build the package if it is build output that is not gitignored",
            ),
            HintKind::MarkerCancelled => (
                format!(
                    "`{identifier}` in `{key}` excludes in production mode what `{}` selects",
                    cancelled.as_deref().unwrap_or_default()
                ),
                "remove the plain pattern, since the `!` one applies outside production mode too, or remove the `!` \
                 one if the files are not production code",
            ),
        };
        let package = hint.package.and_then(|id| analysis.workspace.package(id));
        let location = if let Some(package) = package {
            let span = package.manifest.field_span(&[PathSeg::Key(key.clone())]);
            Location::new(package.manifest.path.clone(), span)
        } else {
            let Some(path) = Self::config_path(analysis) else {
                return;
            };
            Location::new(path, None)
        };
        let mut builder = diagnostic(&META, message, location).help(help);
        if let Some(package) = package {
            builder = builder.package(package.display_name());
        }
        out.emit(builder.build());
    }

    /// knip's `top-level-unconfigured` and `workspace-unconfigured`: when more than [`UNCONFIGURED_MIN_FILES`] files
    /// are unused and they are more than [`UNCONFIGURED_MIN_RATIO`] of the files the analysis reached, entries are
    /// likely missing.
    fn report_unconfigured(analysis: &JsAnalysis<'_>, out: &mut Emitter<'_>) {
        let unused = analysis.unused_files.len();
        let reached = analysis.graph.modules().len();
        let looked_at = reached + unused;
        #[allow(clippy::cast_precision_loss, reason = "file counts are far below 2^52")]
        let ratio = if reached == 0 {
            0.0
        } else {
            unused as f64 / reached as f64
        };
        if unused <= UNCONFIGURED_MIN_FILES || ratio <= UNCONFIGURED_MIN_RATIO {
            return;
        }
        let Some(path) = Self::config_path(analysis) else {
            return;
        };
        let analysed: Vec<PackageId> = analysis
            .workspace
            .iter()
            .filter(|(id, _)| analysis.setup.package(*id).is_some())
            .map(|(id, _)| id)
            .collect();
        let help = "add `entry` patterns for the files tools or other code load, or narrow `project`";
        if analysed.len() == 1 {
            out.emit(
                diagnostic(
                    &META,
                    format!("{unused} of {looked_at} files are unused, so `[js]` likely misses entry files"),
                    Location::new(path, None),
                )
                .help(help)
                .build(),
            );
            return;
        }
        for (size, dir) in unused_by_workspace(analysis, &analysed) {
            out.emit(
                diagnostic(
                    &META,
                    format!(
                        "{size} files of workspace `{dir}` are unused, so `[js.workspaces.\"{dir}\"]` likely misses \
                         entry files"
                    ),
                    Location::new(path.clone(), None),
                )
                .help(help)
                .build(),
            );
        }
    }
}

/// The packages with more than one unused file, as `(count, directory)`, largest first.
fn unused_by_workspace(analysis: &JsAnalysis<'_>, analysed: &[PackageId]) -> Vec<(usize, String)> {
    let mut sizes: Vec<(usize, String)> = analysed
        .iter()
        .filter_map(|id| {
            let package = analysis.workspace.package(*id)?;
            let size = analysis.unused_files.iter().filter(|file| file.owner == *id).count();
            let dir = analysis.setup.workspace_keys.key_dir(package);
            (size > 1).then_some((size, dir))
        })
        .collect();
    sizes.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    sizes
}

/// Whether the rules that consult each ignore list all ran, so the list can be judged.
#[derive(Clone, Copy)]
struct ListsRan {
    dependencies: bool,
    binaries: bool,
    unresolved: bool,
}

/// The ignore lists (`ignore-dependencies`, `ignore-binaries`, `ignore-unresolved`, `ignore-workspaces`), each entry
/// with whether it matched nothing: the top-level lists, then those of each package. A list is left out when a rule
/// that consults it did not run, or, for a package's `ignore-dependencies`, when the package's dependencies were not
/// judged.
fn keyed_ignore_lists<'a>(analysis: &'a JsAnalysis<'_>, ran: &dyn Fn(&[&str]) -> bool) -> Vec<JudgedList<'a>> {
    let ran = ListsRan {
        dependencies: ran(&DEPENDENCY_RULES),
        binaries: ran(&["unlisted-binaries"]),
        unresolved: ran(&UNRESOLVED_RULES),
    };
    let mut keyed = top_level_ignore_lists(analysis, ran);
    keyed.extend(package_ignore_lists(analysis, ran));
    keyed
}

/// The top-level ignore lists of [`keyed_ignore_lists`]. The global `ignore-dependencies` is judged over the packages
/// whose dependencies were judged: an entry matching a dependency of a package whose entry files are unknown may be
/// what keeps it quiet, so it is not called stale.
fn top_level_ignore_lists<'a>(analysis: &'a JsAnalysis<'_>, ran: ListsRan) -> Vec<JudgedList<'a>> {
    let setup = &analysis.setup;
    let unchecked_dependencies = analysis
        .workspace
        .iter()
        .filter(|(id, _)| !analysis.is_skipped(*id) && analysis.entries_incomplete(*id))
        .flat_map(|(_, package)| package.dependencies.iter().map(|entry| &*entry.name))
        .collect::<Vec<_>>();
    let mut keyed: Vec<JudgedList<'_>> = Vec::new();
    if ran.dependencies {
        keyed.push(JudgedList::new(
            "js.ignore-dependencies",
            setup
                .global_ignore_dependencies
                .verdicts_except(&unchecked_dependencies),
        ));
    }
    if ran.binaries {
        keyed.push(JudgedList::new(
            "js.ignore-binaries",
            setup.global_ignore_binaries.verdicts(),
        ));
    }
    if ran.unresolved {
        keyed.push(JudgedList::new(
            "js.ignore-unresolved",
            setup.global_ignore_unresolved.verdicts(),
        ));
    }
    keyed.push(JudgedList::new(
        "js.ignore-workspaces",
        setup.ignore_workspaces.verdicts(),
    ));
    keyed
}

/// The ignore lists the `[js.workspaces]` tables give each package of [`keyed_ignore_lists`]: named by the package's
/// directory and written under the key of the table that configures it.
fn package_ignore_lists<'a>(analysis: &'a JsAnalysis<'_>, ran: ListsRan) -> Vec<JudgedList<'a>> {
    let setup = &analysis.setup;
    let mut keyed: Vec<JudgedList<'_>> = Vec::new();
    for (id, package) in analysis.workspace.iter() {
        let Some(package_setup) = setup.package(id) else {
            continue;
        };
        let dir = setup.workspace_keys.key_dir(package);
        let table = package_setup.table_key.as_deref().unwrap_or(dir.as_str());
        let package_dir: &'a Path = package.dir.as_path();
        let list = |name: &str, verdicts: Vec<(&'a str, bool)>| JudgedList {
            written: format!("js.workspaces.\"{table}\".{name}"),
            shown: format!("js.workspaces.\"{dir}\".{name}"),
            package: Some(package_dir),
            verdicts,
        };
        if ran.dependencies && !analysis.is_skipped(id) && !analysis.entries_incomplete(id) {
            keyed.push(list(
                "ignore-dependencies",
                package_setup.user_ignore_dependencies.verdicts(),
            ));
        }
        if ran.binaries {
            keyed.push(list("ignore-binaries", package_setup.user_ignore_binaries.verdicts()));
        }
        if ran.unresolved {
            keyed.push(list("ignore-unresolved", package_setup.ignore_unresolved.verdicts()));
        }
    }
    keyed
}

impl JsRule for StaleJsConfig {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn runs_last(&self) -> bool {
        true
    }

    fn check(&self, analysis: &JsAnalysis<'_>, out: &mut Emitter<'_>) {
        let ran = |ids: &[&str]| {
            analysis
                .enabled_rules
                .as_ref()
                .is_none_or(|enabled| ids.iter().all(|id| enabled.contains(*id)))
        };
        // An ignore entry that only silences a mock is used whether or not `stale-mocks` runs: walking the mocks
        // records the entries they match.
        if !ran(&[super::stale_mocks::META.id]) {
            for _mock in unresolved_imports(analysis, true) {}
        }
        for list in keyed_ignore_lists(analysis, &ran) {
            Self::judge(analysis, out, &list);
        }
        let keys = analysis
            .setup
            .workspace_key_verdicts
            .iter()
            .map(|(key, stale)| (key.as_str(), *stale))
            .collect();
        Self::judge(analysis, out, &JudgedList::new("js.workspaces", keys));
        let unused_files_ran = ran(&["unused-files"]);
        let ignore_files = analysis.judged_ignore_files.iter().filter(|_| unused_files_ran);
        for pattern in analysis.judged_patterns.iter().chain(ignore_files) {
            let list = JudgedList::new(pattern.key.as_str(), vec![(pattern.pattern.as_str(), pattern.empty)]);
            Self::judge(analysis, out, &list);
        }
        if ran(&["unused-exports"]) {
            Self::report_tags(analysis, out);
        }
        for hint in &analysis.config_hints {
            Self::report_hint(analysis, out, hint);
        }
        if unused_files_ran {
            Self::report_unconfigured(analysis, out);
        }
    }
}
