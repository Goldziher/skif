use skif_core::{Category, Fixability, Location, PathSeg, RuleMeta, Severity, diagnostic};
use skif_lint::Emitter;

use super::JsRule;
use crate::analysis::JsAnalysis;

/// Metadata of `unlisted-binaries`. Warning until the rule reaches corpus parity with knip; `unused-dependencies`, `unlisted-dependencies` and `unresolved-imports` stay errors.
pub static META: RuleMeta = RuleMeta::new(
    "unlisted-binaries",
    Category::Js,
    Some(Severity::Warning),
    Fixability::None,
    "A script, or a command in a workflow, tool config or source file, runs a binary that no listed dependency provides.",
);

/// Reports binaries that no dependency provides: those `package.json` scripts run, at the script, and those the
/// commands of other files run (workflows, hooks and tool configs that plugins read, `node:child_process`, `execa`
/// and `zx` calls in source files) and the `node_modules/.bin` links source files look up
/// (`require.resolve('.bin/x')`), at that file. A binary a command runs after `cd` into another workspace package or a
/// project of its own is judged there, and the message names it.
pub struct UnlistedBinaries;

impl JsRule for UnlistedBinaries {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn check(&self, analysis: &JsAnalysis<'_>, out: &mut Emitter<'_>) {
        for binary in &analysis.dependencies.unlisted_binaries {
            let Some(package) = analysis.workspace.package(binary.package) else {
                continue;
            };
            // A binary run after `cd` in another package or project names where it runs and whose dependencies lack it.
            let runs = match &binary.runs_in {
                Some(runs_in) => format!(
                    "`{}` in `{}`, which no dependency of `{}` provides",
                    binary.name, runs_in.dir, runs_in.name
                ),
                None => format!("`{}`, which no dependency provides", binary.name),
            };
            let (message, location) = if let Some(file) = &binary.file {
                (
                    format!("`{}` runs {runs}", file.to_string_lossy().replace('\\', "/")),
                    Location::new(file.clone(), None),
                )
            } else {
                let span = package
                    .manifest
                    .field_span(&[PathSeg::Key("scripts".to_owned()), PathSeg::Key(binary.script.clone())]);
                (
                    format!("script `{}` runs {runs}", binary.script),
                    Location::new(package.manifest.path.clone(), span),
                )
            };
            out.emit(
                diagnostic(&META, message, location)
                    .package(package.display_name())
                    .dependency(binary.name.clone())
                    .help(
                        "add the package that provides the binary to `devDependencies`, or add it to `ignore-binaries`",
                    )
                    .build(),
            );
        }
    }
}
