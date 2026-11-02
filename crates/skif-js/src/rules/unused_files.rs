use skif_core::{Category, Fix, FixOp, FixSafety, Fixability, RuleMeta, Severity, diagnostic};
use skif_lint::Emitter;

use super::JsRule;
use super::support::file_location;
use crate::analysis::JsAnalysis;

/// Metadata of `unused-files`.
pub static META: RuleMeta = RuleMeta {
    id: "unused-files",
    category: Category::Js,
    default: Some(Severity::Error),
    fixable: Fixability::Unsafe,
    summary: "A project file is not reachable from any entry file.",
};

/// Reports project files that no entry file imports, directly or through other files.
pub struct UnusedFiles;

impl JsRule for UnusedFiles {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn check(&self, analysis: &JsAnalysis<'_>, out: &mut Emitter<'_>) {
        for file in &analysis.unused_files {
            let mut builder = diagnostic(
                &META,
                format!("`{}` is not reachable from any entry file", file.path.display()),
                file_location(file.path.clone()),
            )
            .help("delete it, or add it to `entry` when a tool loads it");
            if let Some(package) = analysis.workspace.package(file.owner) {
                builder = builder.package(package.display_name());
            }
            out.emit(
                builder
                    .fix(Fix {
                        title: format!("Delete `{}`", file.path.display()),
                        safety: FixSafety::Unsafe,
                        ops: vec![FixOp::DeleteFile {
                            path: file.path.clone(),
                        }],
                    })
                    .build(),
            );
        }
    }
}
