//! In-file references to top-level bindings and namespace members, via oxc semantic analysis.

use std::cell::OnceCell;
use std::collections::{BTreeSet, HashMap, HashSet};

use oxc_ast::ast::{Declaration, IdentifierReference, Program, Statement, TSNamespaceDeclarationBody};
use oxc_ast_visit::Visit;
use oxc_semantic::{ScopeId, Scoping, Semantic, SemanticBuilder};
use oxc_span::{GetSpan, Span as OxcSpan};

/// The semantic analysis of one parsed block, built on first use and shared by every pass that needs scopes.
pub(crate) struct Analysis<'p> {
    program: &'p Program<'p>,
    /// Build the node store, which [`referenced_locals`] reads.
    nodes: bool,
    semantic: OnceCell<Semantic<'p>>,
}

impl<'p> Analysis<'p> {
    pub(crate) const fn new(program: &'p Program<'p>, nodes: bool) -> Self {
        Self {
            program,
            nodes,
            semantic: OnceCell::new(),
        }
    }

    pub(crate) fn semantic(&self) -> &Semantic<'p> {
        self.semantic.get_or_init(|| {
            SemanticBuilder::new()
                .with_build_nodes(self.nodes)
                .build(self.program)
                .semantic
        })
    }

    pub(crate) fn scoping(&self) -> &Scoping {
        self.semantic().scoping()
    }
}

/// Names of top-level bindings in `wanted` that are referenced outside `excluded` spans. `analysis` builds the node
/// store.
pub(crate) fn referenced_locals(
    analysis: &Analysis<'_>,
    wanted: &HashSet<String>,
    excluded: &[OxcSpan],
) -> HashSet<String> {
    let mut referenced = HashSet::new();
    if wanted.is_empty() {
        return referenced;
    }
    let semantic = analysis.semantic();
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
        reads: HashMap::new(),
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
