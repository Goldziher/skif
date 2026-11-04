use std::path::Path;

use skif_core::{Category, Fix, FixOp, FixSafety, Fixability, RuleMeta, Severity, diagnostic, slash};
use skif_lint::Emitter;

use super::JsRule;
use super::support::file_location;
use crate::analysis::JsAnalysis;

/// Metadata of `unused-files`. Warning until the rule reaches corpus parity with knip; `unused-dependencies`, `unlisted-dependencies` and `unresolved-imports` stay errors.
pub static META: RuleMeta = RuleMeta::new(
    "unused-files",
    Category::Js,
    Some(Severity::Warning),
    Fixability::Unsafe,
    "A project file is not reachable from any entry file.",
);

/// Reports project files that no entry file imports, directly or through other files.
pub struct UnusedFiles;

/// The message of a file no entry reaches, with `/` separators on every platform so reports and SARIF fingerprints
/// do not depend on the OS that produced them.
fn unreachable_message(path: &Path) -> String {
    format!("`{}` is not reachable from any entry file", slash(path))
}

impl JsRule for UnusedFiles {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn check(&self, analysis: &JsAnalysis<'_>, out: &mut Emitter<'_>) {
        for file in &analysis.unused_files {
            let mut builder = diagnostic(&META, unreachable_message(&file.path), file_location(file.path.clone()))
                .help("delete it, or add it to `entry` when a tool loads it");
            if let Some(package) = analysis.workspace.package(file.owner) {
                builder = builder.package(package.display_name());
            }
            out.emit(
                builder
                    .fix(Fix::new(
                        format!("Delete `{}`", slash(&file.path)),
                        FixSafety::Unsafe,
                        vec![FixOp::delete_file(file.path.clone())],
                    ))
                    .build(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_message_names_the_file_with_forward_slashes() {
        let path = Path::new("packages").join("lib").join("src").join("unused.ts");
        assert_eq!(
            unreachable_message(&path),
            "`packages/lib/src/unused.ts` is not reachable from any entry file"
        );
    }
}
