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
/// and `zx` calls in source files), at that file.
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
            let (message, location) = match &binary.file {
                Some(file) => (
                    format!(
                        "`{}` runs `{}`, which no dependency provides",
                        file.display(),
                        binary.name
                    ),
                    Location::new(file.clone(), None),
                ),
                None => {
                    let span = package
                        .manifest
                        .field_span(&[PathSeg::Key("scripts".to_owned()), PathSeg::Key(binary.script.clone())]);
                    (
                        format!(
                            "script `{}` runs `{}`, which no dependency provides",
                            binary.script, binary.name
                        ),
                        Location::new(package.manifest.path.clone(), span),
                    )
                }
            };
            out.emit(
                diagnostic(&META, message, location)
                    .package(package.display_name())
                    .dependency(binary.name.clone())
                    .help("add the package that provides the binary to `devDependencies`, or add it to `ignore-binaries`")
                    .build(),
            );
        }
    }
}
