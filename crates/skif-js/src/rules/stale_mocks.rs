use skif_core::{Category, Fixability, RuleMeta, Severity, diagnostic};
use skif_js_syntax::MockCall;
use skif_lint::Emitter;

use super::JsRule;
use super::support::{module_location, unresolved_imports};
use crate::analysis::JsAnalysis;

/// Metadata of `stale-mocks`. A warning: the test still runs, because the mock factory stands in for the module
/// (ADR 0073).
pub static META: RuleMeta = RuleMeta::new(
    "stale-mocks",
    Category::Js,
    Some(Severity::Warning),
    Fixability::None,
    "A test-runner mock names a relative or aliased module that does not exist.",
);

/// Reports `vi.mock`, `vi.doMock`, `vi.unmock`, `vi.importActual`, `jest.mock`, `jest.doMock`, `jest.requireActual`
/// and the other calls of the test runners that name a module, when the relative, aliased or `#` specifier resolves to
/// nothing. The specifier is resolved as an import is, query (`?react`, `?url`, `?raw`) dropped. A bare package name
/// that resolves to nothing is taken for a dependency and is not reported here, nor as unlisted (#16).
pub struct StaleMocks;

impl JsRule for StaleMocks {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn check(&self, analysis: &JsAnalysis<'_>, out: &mut Emitter<'_>) {
        for (module, import) in unresolved_imports(analysis, true) {
            let call = import.call.map_or("mock", MockCall::as_str);
            let mut builder = diagnostic(
                &META,
                format!("`{call}('{}')` mocks a module that does not exist", import.specifier),
                module_location(module, Some(import.span)),
            )
            .help("remove the mock, or point it at the module's new path");
            if let Some(package) = analysis.workspace.package(module.owner) {
                builder = builder.package(package.display_name());
            }
            out.emit(builder.build());
        }
    }
}
