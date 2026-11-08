use skif_core::{Category, Fixability, Location, PathSeg, RuleMeta, Severity, diagnostic};
use skif_lint::Emitter;
use skif_workspace::PackageId;

use super::JsRule;
use super::support::module_location;
use crate::analysis::JsAnalysis;
use crate::config_hints::{ConfigHint, HintKind};

/// knip's thresholds for its `top-level-unconfigured` and `workspace-unconfigured` hints: more than this many unused
/// files...
const UNCONFIGURED_MIN_FILES: usize = 20;
/// ...making up more than this share of the files the run looked at.
const UNCONFIGURED_MIN_RATIO: f64 = 0.2;

/// Metadata of `stale-js-config`.
pub static META: RuleMeta = RuleMeta::new(
    "stale-js-config",
    Category::Js,
    Some(Severity::Warning),
    Fixability::None,
    "A `[js]` setting matched nothing or is redundant, a published entry file is missing, or many files are unused.",
);

/// The rules that consult `ignore-dependencies`. An entry can only be called stale when all of them ran.
const DEPENDENCY_RULES: [&str; 5] = [
    "unused-dependencies",
    "unused-dev-dependencies",
    "unlisted-dependencies",
    "unused-optional-peer-dependencies",
    "unresolved-imports",
];

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
/// (`top-level-unconfigured`, `workspace-unconfigured`).
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

impl StaleJsConfig {
    fn report(analysis: &JsAnalysis<'_>, out: &mut Emitter<'_>, key: &str, entries: Vec<&str>) {
        let path = analysis.config_path.clone().or_else(|| {
            analysis
                .workspace
                .package(PackageId::ROOT)
                .map(|package| package.manifest.path.clone())
        });
        let Some(path) = path else { return };
        for entry in entries {
            out.emit(
                diagnostic(
                    &META,
                    format!("`{entry}` in `{key}` matched nothing"),
                    Location::new(path.clone(), None),
                )
                .help("remove the entry, or correct it if it was meant to match")
                .build(),
            );
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
            kind, key, identifier, ..
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
                "remove the extension: skif reads JavaScript, TypeScript, Vue, Svelte, Astro, MDX and TSRX files",
            ),
            HintKind::ExtensionExcluded => (
                format!("`{key}` leaves out the package's `{identifier}` files"),
                "add the extension to the `project` patterns",
            ),
            HintKind::PackageEntry => (
                format!("`{identifier}` in `{key}` of `package.json` names no file"),
                "correct the path, or build the package if it is build output that is not gitignored",
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
    /// are unused and they are more than [`UNCONFIGURED_MIN_RATIO`] of the files looked at, entries are likely missing.
    fn report_unconfigured(analysis: &JsAnalysis<'_>, out: &mut Emitter<'_>) {
        let unused = analysis.unused_files.len();
        let looked_at = analysis.graph.modules().len() + unused;
        #[allow(clippy::cast_precision_loss, reason = "file counts are far below 2^52")]
        let ratio = if looked_at == 0 {
            0.0
        } else {
            unused as f64 / looked_at as f64
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
            let dir = if package.is_root {
                ".".to_owned()
            } else {
                package.dir.to_string_lossy().replace('\\', "/")
            };
            (size > 1).then_some((size, dir))
        })
        .collect();
    sizes.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    sizes
}

impl JsRule for StaleJsConfig {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn runs_last(&self) -> bool {
        true
    }

    fn check(&self, analysis: &JsAnalysis<'_>, out: &mut Emitter<'_>) {
        let setup = &analysis.setup;
        let ran = |ids: &[&str]| {
            analysis
                .enabled_rules
                .as_ref()
                .is_none_or(|enabled| ids.iter().all(|id| enabled.contains(*id)))
        };
        let judged = |id: PackageId| !analysis.is_skipped(id) && !analysis.entries_incomplete(id);
        let dependencies_ran = ran(&DEPENDENCY_RULES);
        // The dependencies of a package whose entry files are unknown were not checked, so an entry matching one of
        // them may be needed.
        let unchecked_dependencies = analysis
            .workspace
            .iter()
            .filter(|(id, _)| !analysis.is_skipped(*id) && analysis.entries_incomplete(*id))
            .flat_map(|(_, package)| package.dependencies.iter().map(|entry| entry.name.as_str()))
            .collect::<Vec<_>>();
        let unresolved_ran = ran(&["unresolved-imports"]);
        let binaries_ran = ran(&["unlisted-binaries"]);

        let mut keyed: Vec<(String, Vec<&str>)> = Vec::new();
        if dependencies_ran {
            keyed.push((
                "js.ignore-dependencies".to_owned(),
                setup
                    .global_ignore_dependencies
                    .unmatched_except(&unchecked_dependencies),
            ));
        }
        if binaries_ran {
            keyed.push((
                "js.ignore-binaries".to_owned(),
                setup.global_ignore_binaries.unmatched(),
            ));
        }
        if unresolved_ran {
            keyed.push((
                "js.ignore-unresolved".to_owned(),
                setup.global_ignore_unresolved.unmatched(),
            ));
        }
        keyed.push(("js.ignore-workspaces".to_owned(), setup.ignore_workspaces.unmatched()));
        for (id, package) in analysis.workspace.iter() {
            let Some(package_setup) = setup.package(id) else {
                continue;
            };
            let dir = if package.is_root {
                ".".to_owned()
            } else {
                package.dir.to_string_lossy().replace('\\', "/")
            };
            let key = |name: &str| format!("js.workspaces.\"{dir}\".{name}");
            if dependencies_ran && judged(id) {
                keyed.push((
                    key("ignore-dependencies"),
                    package_setup.user_ignore_dependencies.unmatched(),
                ));
            }
            if binaries_ran {
                keyed.push((key("ignore-binaries"), package_setup.user_ignore_binaries.unmatched()));
            }
            if unresolved_ran {
                keyed.push((key("ignore-unresolved"), package_setup.ignore_unresolved.unmatched()));
            }
        }
        for (key, entries) in keyed {
            Self::report(analysis, out, &key, entries);
        }
        for key in &setup.unmatched_workspace_keys {
            Self::report(analysis, out, "js.workspaces", vec![key.as_str()]);
        }
        let unused_files_ran = ran(&["unused-files"]);
        let ignore_files = analysis.unused_ignore_files.iter().filter(|_| unused_files_ran);
        for empty in analysis.empty_patterns.iter().chain(ignore_files) {
            Self::report(analysis, out, &empty.key, vec![empty.pattern.as_str()]);
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
