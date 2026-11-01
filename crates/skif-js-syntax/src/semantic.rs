//! In-file references to top-level bindings, via oxc semantic analysis.

use std::collections::HashSet;

use oxc_ast::ast::Program;
use oxc_semantic::SemanticBuilder;
use oxc_span::Span as OxcSpan;

/// Names of top-level bindings in `wanted` that are referenced outside `excluded` spans.
pub(crate) fn referenced_locals(
    program: &Program<'_>,
    wanted: &HashSet<String>,
    excluded: &[OxcSpan],
) -> HashSet<String> {
    let mut referenced = HashSet::new();
    if wanted.is_empty() {
        return referenced;
    }
    let semantic = SemanticBuilder::new().build(program).semantic;
    let scoping = semantic.scoping();
    let root = scoping.root_scope_id();
    for symbol in scoping.symbol_ids() {
        let name = scoping.symbol_name(symbol);
        if scoping.symbol_scope_id(symbol) != root || !wanted.contains(name) {
            continue;
        }
        let used = scoping.get_resolved_references(symbol).any(|reference| {
            let span = semantic.reference_span(reference);
            !excluded.iter().any(|e| e.start <= span.start && span.end <= e.end)
        });
        if used {
            referenced.insert(name.to_string());
        }
    }
    referenced
}
