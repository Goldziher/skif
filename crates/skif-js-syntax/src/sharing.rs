//! The bindings of a config module whose value code the evaluator does not follow may change.
//!
//! The evaluator copies values: `const a = cfg; a.x = 1` changes its copy of `a`, not `cfg`, and a function it does not
//! call, or calls for its value only, does not change what it captures. This pass finds, by symbol, the bindings such
//! code may change: one a nested function assigns or changes, and one that may share an object with a binding that is
//! changed (`const a = cfg`, `{ test: t }`, `{ ...base }`, `f(cfg)`). Reading one of them yields a value that says it
//! may hold more than the evaluator saw.

use std::collections::BTreeMap;

use oxc_ast::ast::{
    Argument, ArrowFunctionExpression, AssignmentExpression, AssignmentTargetPropertyIdentifier, BindingPattern,
    CallExpression, Expression, ExpressionStatement, FormalParameters, Function, IdentifierReference, MemberExpression,
    Program, ReturnStatement, SimpleAssignmentTarget, UnaryExpression, UnaryOperator, VariableDeclarator,
};
use oxc_ast_visit::{Visit, walk};
use oxc_semantic::{ReferenceId, ScopeFlags, ScopeId, Scoping, SemanticBuilder, SymbolId};
use skif_core::HashMap;

/// How code the evaluator does not follow may change a binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Change {
    /// What the object or array it holds contains.
    Contents,
    /// The binding itself: it may hold any value.
    Any,
}

/// The bindings of a module that code the evaluator does not follow may change.
#[derive(Default)]
pub(crate) struct Unstable {
    /// Reads of such a binding declared inside a function, by reference.
    reads: HashMap<ReferenceId, Change>,
    /// Such top-level bindings, by name.
    top: HashMap<String, Change>,
}

impl Unstable {
    pub(crate) fn of(program: &Program<'_>) -> Self {
        let semantic = SemanticBuilder::new().build(program).semantic;
        let scoping = semantic.scoping();
        let mut sharing = Sharing {
            scoping,
            functions: vec![scoping.root_scope_id()],
            links: Vec::new(),
            mutated: Vec::new(),
            changes: BTreeMap::new(),
        };
        sharing.visit_program(program);
        sharing.share();
        let mut unstable = Self::default();
        for (symbol, change) in sharing.changes {
            if scoping.symbol_scope_id(symbol) == scoping.root_scope_id() {
                unstable.top.insert(scoping.symbol_name(symbol).to_owned(), change);
            } else {
                for &reference in scoping.get_resolved_reference_ids(symbol) {
                    unstable.reads.insert(reference, change);
                }
            }
        }
        unstable
    }

    /// How the binding a reference to a function's local reads may have changed.
    pub(crate) fn of_reference(&self, id: &IdentifierReference<'_>) -> Option<Change> {
        self.reads.get(&id.reference_id.get()?).copied()
    }

    /// How the top-level binding `name` may have changed.
    pub(crate) fn of_top(&self, name: &str) -> Option<Change> {
        self.top.get(name).copied()
    }
}

struct Sharing<'s> {
    scoping: &'s Scoping,
    /// The scopes of the functions being visited, the module's first.
    functions: Vec<ScopeId>,
    /// Pairs of bindings that may hold the same object.
    links: Vec<(SymbolId, SymbolId)>,
    /// Bindings whose object or array code changes.
    mutated: Vec<SymbolId>,
    changes: BTreeMap<SymbolId, Change>,
}

impl Sharing<'_> {
    fn symbol(&self, id: &IdentifierReference<'_>) -> Option<SymbolId> {
        local_symbol(self.scoping, id)
    }

    /// The function (or module) whose binding `symbol` is.
    fn function_of(&self, symbol: SymbolId) -> ScopeId {
        let scope = self.scoping.symbol_scope_id(symbol);
        self.scoping
            .scope_ancestors(scope)
            .find(|&s| {
                let flags = self.scoping.scope_flags(s);
                flags.is_function() || flags.is_top()
            })
            .unwrap_or(scope)
    }

    fn change(&mut self, symbol: SymbolId, change: Change) {
        let entry = self.changes.entry(symbol).or_insert(change);
        *entry = (*entry).max(change);
    }

    /// Records a write of `symbol`; one from a function other than its own may happen at any time.
    fn written(&mut self, symbol: SymbolId, change: Change) {
        if self.functions.last() != Some(&self.function_of(symbol)) {
            self.change(symbol, change);
        }
    }

    /// Records a change of the object `expr` (a name or a member chain) holds.
    fn mutate(&mut self, expr: &Expression<'_>) {
        if let Some(symbol) = root_reference(expr).and_then(|id| self.symbol(id)) {
            self.mutated.push(symbol);
            self.written(symbol, Change::Contents);
        }
    }

    fn link(&mut self, symbols: &[SymbolId]) {
        for pair in symbols.windows(2) {
            self.links.push((pair[0], pair[1]));
        }
    }

    /// The symbols the value `visit` visits may hold; with `function`, the value is a function, which stands for its
    /// parameters and what it returns.
    fn refs(&self, function: bool, visit: impl FnOnce(&mut Refs<'_>)) -> Vec<SymbolId> {
        let mut refs = Refs {
            scoping: self.scoping,
            function,
            in_body: false,
            out: Vec::new(),
        };
        visit(&mut refs);
        refs.out
    }

    /// Marks every binding that may share an object with a changed one, other than a binding changed on its own.
    fn share(&mut self) {
        let mut parent: HashMap<SymbolId, SymbolId> = HashMap::default();
        for &(a, b) in &self.links {
            let (a, b) = (find(&mut parent, a), find(&mut parent, b));
            if a != b {
                parent.insert(a, b);
            }
        }
        let mut changed: HashMap<SymbolId, Vec<SymbolId>> = HashMap::default();
        for &symbol in &self.mutated {
            let class = changed.entry(find(&mut parent, symbol)).or_default();
            if !class.contains(&symbol) {
                class.push(symbol);
            }
        }
        let symbols: Vec<SymbolId> = parent.keys().copied().collect();
        for symbol in symbols {
            let root = find(&mut parent, symbol);
            if changed
                .get(&root)
                .is_some_and(|class| class.len() > 1 || class[0] != symbol)
            {
                self.change(symbol, Change::Contents);
            }
        }
    }
}

impl<'a> Visit<'a> for Sharing<'_> {
    fn visit_function(&mut self, it: &Function<'a>, flags: ScopeFlags) {
        // A declared function's name stands for what its parameters take and what it returns.
        if let Some(symbol) = it.id.as_ref().and_then(|id| id.symbol_id.get()) {
            let mut held = vec![symbol];
            held.extend(self.refs(true, |refs| refs.visit_function(it, flags)));
            self.link(&held);
        }
        self.functions
            .push(it.scope_id.get().unwrap_or_else(|| self.scoping.root_scope_id()));
        walk::walk_function(self, it, flags);
        self.functions.pop();
    }

    fn visit_arrow_function_expression(&mut self, it: &ArrowFunctionExpression<'a>) {
        self.functions
            .push(it.scope_id.get().unwrap_or_else(|| self.scoping.root_scope_id()));
        walk::walk_arrow_function_expression(self, it);
        self.functions.pop();
    }

    fn visit_variable_declarator(&mut self, it: &VariableDeclarator<'a>) {
        if let Some(init) = &it.init {
            let mut held: Vec<SymbolId> = it
                .id
                .get_binding_identifiers()
                .iter()
                .filter_map(|id| id.symbol_id.get())
                .collect();
            let function = matches!(
                init.get_inner_expression(),
                Expression::ArrowFunctionExpression(_) | Expression::FunctionExpression(_)
            );
            held.extend(self.refs(function, |refs| refs.visit_expression(init)));
            self.link(&held);
        }
        walk::walk_variable_declarator(self, it);
    }

    fn visit_assignment_expression(&mut self, it: &AssignmentExpression<'a>) {
        // What the target is written with may be shared with it; the target itself is visited by the walk.
        let mut held = Vec::new();
        match it.left.as_simple_assignment_target() {
            Some(SimpleAssignmentTarget::AssignmentTargetIdentifier(id)) => held.extend(self.symbol(id)),
            Some(target) => held.extend(
                target
                    .as_member_expression()
                    .and_then(|member| root_reference(member.object()))
                    .and_then(|id| self.symbol(id)),
            ),
            None => {}
        }
        held.extend(self.refs(false, |refs| refs.visit_expression(&it.right)));
        self.link(&held);
        walk::walk_assignment_expression(self, it);
    }

    fn visit_simple_assignment_target(&mut self, it: &SimpleAssignmentTarget<'a>) {
        match it {
            SimpleAssignmentTarget::AssignmentTargetIdentifier(id) => {
                if let Some(symbol) = self.symbol(id) {
                    self.written(symbol, Change::Any);
                }
            }
            other => {
                if let Some(member) = other.as_member_expression() {
                    self.mutate(member.object());
                }
            }
        }
        walk::walk_simple_assignment_target(self, it);
    }

    fn visit_assignment_target_property_identifier(&mut self, it: &AssignmentTargetPropertyIdentifier<'a>) {
        if let Some(symbol) = self.symbol(&it.binding) {
            self.written(symbol, Change::Any);
        }
        walk::walk_assignment_target_property_identifier(self, it);
    }

    fn visit_unary_expression(&mut self, it: &UnaryExpression<'a>) {
        if it.operator == UnaryOperator::Delete
            && let Some(member) = it.argument.get_inner_expression().as_member_expression()
        {
            self.mutate(member.object());
        }
        walk::walk_unary_expression(self, it);
    }

    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        let mut held = Vec::new();
        match it.callee.get_inner_expression() {
            Expression::Identifier(id) => held.extend(self.symbol(id)),
            callee => {
                if let Some(target) = changed_by_call(it) {
                    self.mutate(target);
                }
                if let Some(member) = callee.as_member_expression() {
                    held.extend(root_reference(member.object()).and_then(|id| self.symbol(id)));
                }
            }
        }
        for argument in &it.arguments {
            held.extend(self.refs(false, |refs| refs.visit_argument(argument)));
        }
        self.link(&held);
        walk::walk_call_expression(self, it);
    }

    fn visit_expression_statement(&mut self, it: &ExpressionStatement<'a>) {
        // A call made for what it does may change the objects it is given.
        if let Some(call) = statement_call(&it.expression) {
            for argument in call.arguments.iter().filter_map(Argument::as_expression) {
                self.mutate(argument);
            }
        }
        walk::walk_expression_statement(self, it);
    }
}

/// The local symbols an expression's value may hold or share: the names it reads, other than the object a method is
/// called on, an imported function and what a function in it reads.
struct Refs<'s> {
    scoping: &'s Scoping,
    /// The value is a function: it stands for its parameters and what it returns.
    function: bool,
    /// Inside the body of a function, outside a `return`: what is read there is not the function's value.
    in_body: bool,
    out: Vec<SymbolId>,
}

impl Refs<'_> {
    fn parameters(&mut self, params: &FormalParameters<'_>) {
        let patterns = params
            .items
            .iter()
            .map(|param| &param.pattern)
            .chain(params.rest.iter().map(|rest| &rest.rest.argument));
        for pattern in patterns {
            self.out.extend(
                BindingPattern::get_binding_identifiers(pattern)
                    .iter()
                    .filter_map(|id| id.symbol_id.get()),
            );
        }
    }
}

impl<'a> Visit<'a> for Refs<'_> {
    fn visit_identifier_reference(&mut self, it: &IdentifierReference<'a>) {
        if !self.in_body {
            self.out.extend(local_symbol(self.scoping, it));
        }
    }

    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        if it.callee.get_inner_expression().is_member_expression() {
            for argument in &it.arguments {
                self.visit_argument(argument);
            }
        } else {
            walk::walk_call_expression(self, it);
        }
    }

    fn visit_return_statement(&mut self, it: &ReturnStatement<'a>) {
        let saved = std::mem::replace(&mut self.in_body, false);
        walk::walk_return_statement(self, it);
        self.in_body = saved;
    }

    fn visit_function(&mut self, it: &Function<'a>, flags: ScopeFlags) {
        if !std::mem::take(&mut self.function) {
            return;
        }
        self.parameters(&it.params);
        self.in_body = true;
        walk::walk_function(self, it, flags);
        self.in_body = false;
    }

    fn visit_arrow_function_expression(&mut self, it: &ArrowFunctionExpression<'a>) {
        if !std::mem::take(&mut self.function) {
            return;
        }
        self.parameters(&it.params);
        self.in_body = it.get_expression().is_none();
        walk::walk_arrow_function_expression(self, it);
        self.in_body = false;
    }
}

/// The representative of the set `symbol` is in, in a union-find forest.
fn find(parent: &mut HashMap<SymbolId, SymbolId>, symbol: SymbolId) -> SymbolId {
    let mut root = symbol;
    while let Some(&next) = parent.get(&root).filter(|&&next| next != root) {
        root = next;
    }
    parent.insert(symbol, root);
    root
}

/// The symbol a reference resolves to, unless it is unresolved or an import.
fn local_symbol(scoping: &Scoping, id: &IdentifierReference<'_>) -> Option<SymbolId> {
    let symbol = scoping.get_reference(id.reference_id.get()?).symbol_id()?;
    (!scoping.symbol_flags(symbol).is_import()).then_some(symbol)
}

/// The name at the start of a member chain (`config` in `config.a[b]`), or the name itself.
fn root_reference<'b, 'a>(expr: &'b Expression<'a>) -> Option<&'b IdentifierReference<'a>> {
    match expr.get_inner_expression() {
        Expression::Identifier(id) => Some(id),
        other => other
            .as_member_expression()
            .and_then(|member| root_reference(member.object())),
    }
}

/// The call an expression statement makes, awaited or not.
pub(crate) fn statement_call<'b, 'a>(expr: &'b Expression<'a>) -> Option<&'b CallExpression<'a>> {
    match expr.get_inner_expression() {
        Expression::CallExpression(call) => Some(call),
        Expression::AwaitExpression(a) => statement_call(&a.argument),
        _ => None,
    }
}

/// Methods that never change the object they are called on.
const PURE_METHODS: &[&str] = &[
    "at",
    "charAt",
    "concat",
    "endsWith",
    "entries",
    "every",
    "filter",
    "find",
    "findIndex",
    "findLast",
    "findLastIndex",
    "flat",
    "flatMap",
    "forEach",
    "get",
    "has",
    "hasOwnProperty",
    "includes",
    "indexOf",
    "join",
    "keys",
    "lastIndexOf",
    "map",
    "match",
    "padEnd",
    "padStart",
    "reduce",
    "reduceRight",
    "replace",
    "replaceAll",
    "slice",
    "some",
    "split",
    "startsWith",
    "test",
    "toLowerCase",
    "toReversed",
    "toSorted",
    "toSpliced",
    "toString",
    "toUpperCase",
    "trim",
    "trimEnd",
    "trimStart",
    "values",
    "with",
];

/// The object a call may change: the first argument of `Object.assign`, or the object a method other than one of
/// [`PURE_METHODS`] is called on.
pub(crate) fn changed_by_call<'p, 'a>(call: &'p CallExpression<'a>) -> Option<&'p Expression<'a>> {
    match call.callee.get_inner_expression() {
        Expression::StaticMemberExpression(m)
            if m.property.name.as_str() == "assign"
                && matches!(m.object.get_inner_expression(), Expression::Identifier(o) if o.name.as_str() == "Object") =>
        {
            call.arguments.first().and_then(Argument::as_expression)
        }
        Expression::StaticMemberExpression(m) if PURE_METHODS.contains(&m.property.name.as_str()) => None,
        callee => callee.as_member_expression().map(MemberExpression::object),
    }
}
