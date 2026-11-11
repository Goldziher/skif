//! In-file references to top-level bindings and namespace members, via oxc semantic analysis.

use oxc_ast::ast::{Declaration, IdentifierReference, Program, Statement, TSNamespaceDeclarationBody};
use oxc_ast_visit::Visit;
use oxc_semantic::{ReferenceId, ScopeId, Scoping, SemanticBuilder};
use oxc_span::{GetSpan, Span as OxcSpan};
use skif_core::{HashMap, HashSet};
use std::cell::OnceCell;
use std::collections::BTreeSet;

/// The semantic analysis of one parsed block, built on first use and shared by every pass that needs scopes. It
/// builds no node store: every pass reads scopes, symbols and references only.
pub(crate) struct Analysis<'p> {
    program: &'p Program<'p>,
    scoping: OnceCell<Scoping>,
}

impl<'p> Analysis<'p> {
    pub(crate) const fn new(program: &'p Program<'p>) -> Self {
        Self {
            program,
            scoping: OnceCell::new(),
        }
    }

    pub(crate) fn scoping(&self) -> &Scoping {
        self.scoping
            .get_or_init(|| SemanticBuilder::new().build(self.program).semantic.into_scoping())
    }
}

/// Names of top-level bindings in `wanted` that are referenced outside `excluded` spans, which are the spans of
/// identifiers in top-level statements of `program`.
pub(crate) fn referenced_locals<'s>(
    program: &Program<'_>,
    analysis: &'s Analysis<'_>,
    wanted: &HashSet<&str>,
    excluded: &[OxcSpan],
) -> HashSet<&'s str> {
    let mut referenced = HashSet::default();
    if wanted.is_empty() {
        return referenced;
    }
    let scoping = analysis.scoping();
    let excluded = excluded_references(program, excluded);
    let root = scoping.root_scope_id();
    for symbol in scoping.symbol_ids() {
        let name = scoping.symbol_name(symbol);
        if scoping.symbol_scope_id(symbol) != root || !wanted.contains(name) {
            continue;
        }
        let used = scoping
            .get_resolved_reference_ids(symbol)
            .iter()
            .any(|reference| !excluded.contains(reference));
        if used {
            referenced.insert(name);
        }
    }
    referenced
}

/// The references of `program` whose identifier lies inside one of the `excluded` spans. Only the top-level
/// statements that hold such a span are walked.
fn excluded_references(program: &Program<'_>, excluded: &[OxcSpan]) -> HashSet<ReferenceId> {
    let mut spans = excluded.to_vec();
    spans.sort_unstable_by_key(|span| (span.start, span.end));
    let mut within = Within {
        spans: &spans,
        found: HashSet::default(),
    };
    if spans.is_empty() {
        return within.found;
    }
    for statement in &program.body {
        let span = statement.span();
        let first = spans.partition_point(|e| e.start < span.start);
        if spans.get(first).is_some_and(|e| e.end <= span.end) {
            within.visit_statement(statement);
        }
    }
    within.found
}

/// Collects the references inside sorted, disjoint spans.
struct Within<'e> {
    spans: &'e [OxcSpan],
    found: HashSet<ReferenceId>,
}

impl<'a> Visit<'a> for Within<'_> {
    fn visit_identifier_reference(&mut self, it: &IdentifierReference<'a>) {
        let after = self.spans.partition_point(|e| e.start <= it.span.start);
        let inside = after
            .checked_sub(1)
            .and_then(|index| self.spans.get(index))
            .is_some_and(|e| it.span.end <= e.end);
        if inside && let Some(reference) = it.reference_id.get() {
            self.found.insert(reference);
        }
    }
}

/// The members of the top-level namespaces in `members` (namespace name to the names its declarations export) that
/// the namespace bodies read by their bare name, like TypeScript resolves them.
///
/// A reference inside a body reads a member when it resolves to a binding the body exports, or, when nothing inside
/// the body binds it, when its name is exported by any declaration of the namespace (merged declarations share their
/// exports). A parameter or local that shadows the name, including the exports of a nested namespace, reads nothing.
pub(crate) fn namespace_bare_reads(
    program: &Program<'_>,
    analysis: &Analysis<'_>,
    members: &HashMap<String, BTreeSet<String>>,
) -> HashMap<String, BTreeSet<String>> {
    let mut bodies = BareReads {
        scoping: analysis.scoping(),
        members,
        current: None,
        reads: HashMap::default(),
    };
    for stmt in &program.body {
        let decl = match stmt {
            Statement::ExportDeclaration(e) => Some(&e.declaration),
            other => other.as_declaration(),
        };
        if let Some(Declaration::TSNamespaceDeclaration(ns)) = decl
            && members.contains_key(ns.id.name.as_str())
            && let TSNamespaceDeclarationBody::TSModuleBlock(block) = &ns.body
            && let Some(scope) = ns.scope_id.get()
        {
            let exported = block
                .body
                .iter()
                .filter(|stmt| matches!(stmt, Statement::ExportDeclaration(_)))
                .map(GetSpan::span)
                .collect();
            bodies.current = Some(Body {
                name: ns.id.name.as_str(),
                scope,
                exported,
            });
            bodies.visit_ts_module_block(block);
            bodies.current = None;
        }
    }
    bodies.reads
}

/// One declaration of a namespace being searched for bare reads.
struct Body<'n> {
    name: &'n str,
    scope: ScopeId,
    /// Spans of the `export` statements directly in the body.
    exported: Vec<OxcSpan>,
}

struct BareReads<'s, 'm, 'n> {
    scoping: &'s Scoping,
    members: &'m HashMap<String, BTreeSet<String>>,
    current: Option<Body<'n>>,
    reads: HashMap<String, BTreeSet<String>>,
}

impl<'a> Visit<'a> for BareReads<'_, '_, '_> {
    fn visit_identifier_reference(&mut self, it: &IdentifierReference<'a>) {
        let Some(body) = &self.current else {
            return;
        };
        let name = it.name.as_str();
        let Some(members) = self.members.get(body.name) else {
            return;
        };
        if !members.contains(name) {
            return;
        }
        let symbol = it
            .reference_id
            .get()
            .and_then(|reference| self.scoping.get_reference(reference).symbol_id());
        let reads = match symbol {
            Some(symbol) => {
                let scope = self.scoping.symbol_scope_id(symbol);
                if scope == body.scope {
                    let span = self.scoping.symbol_span(symbol);
                    body.exported.iter().any(|e| e.start <= span.start && span.end <= e.end)
                } else {
                    // Bound outside this body: TypeScript prefers the namespace's own exports.
                    !self.scoping.scope_is_descendant_of(scope, body.scope)
                }
            }
            None => true,
        };
        if reads {
            self.reads
                .entry(body.name.to_owned())
                .or_default()
                .insert(name.to_owned());
        }
    }
}
