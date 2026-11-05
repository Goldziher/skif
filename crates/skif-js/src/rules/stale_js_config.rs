use skif_core::{Category, Fixability, Location, RuleMeta, Severity, diagnostic};
use skif_lint::Emitter;
use skif_workspace::PackageId;

use super::JsRule;
use crate::analysis::JsAnalysis;

/// Metadata of `stale-js-config`.
pub static META: RuleMeta = RuleMeta::new(
    "stale-js-config",
    Category::Js,
    Some(Severity::Warning),
    Fixability::None,
    "A `[js]` ignore entry or workspace key matched nothing in the run.",
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
/// `ignore-workspaces` entries that matched nothing, and `[js.workspaces]` keys that select no package.
///
/// It is the counterpart of knip's configuration hints. Raise it with `[rules] stale-js-config = "error"` to fail the
/// run on them. It runs after every other `js` rule, because an entry is stale only when no rule used it, and it is
/// silent about a list whose consumers did not all run (a disabled rule) or could not judge (a package whose entry
/// files are not fully known).
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
        let dependencies_ran = ran(&DEPENDENCY_RULES)
            && analysis
                .workspace
                .iter()
                .all(|(id, _)| analysis.is_skipped(id) || !analysis.entries_incomplete(id));
        let unresolved_ran = ran(&["unresolved-imports"]);

        let mut keyed: Vec<(String, Vec<&str>)> = Vec::new();
        if dependencies_ran {
            keyed.push(("js.ignore-dependencies".to_owned(), setup.global_ignore_dependencies.unmatched()));
        }
        keyed.push(("js.ignore-binaries".to_owned(), setup.global_ignore_binaries.unmatched()));
        if unresolved_ran {
            keyed.push(("js.ignore-unresolved".to_owned(), setup.global_ignore_unresolved.unmatched()));
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
                keyed.push((key("ignore-dependencies"), package_setup.user_ignore_dependencies.unmatched()));
            }
            keyed.push((key("ignore-binaries"), package_setup.user_ignore_binaries.unmatched()));
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
    }
}
