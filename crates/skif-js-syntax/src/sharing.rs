//! The bindings of a config module whose value code the evaluator does not follow may change.
//!
//! The evaluator copies values: `const a = cfg; a.x = 1` changes its copy of `a`, not `cfg`, and a function it does not
//! call, or calls for its value only, does not change what it captures. This pass finds, by symbol, the bindings such
//! code may change: one a nested function assigns or changes, and one that may share an object with a binding that is
//! changed (`const a = cfg`, `{ test: t }`, `{ ...base }`, `f(cfg)`). Reading one of them yields a value that says it
//! may hold more than the evaluator saw.
//!
//! A change is tracked to the part of a binding it reaches: a write through `cfg.module.rules` changes what the list
//! there holds, and a binding that holds `t` under `test` changes under `test` when `t` does, so the rest of the
//! binding stays known. Where a value holds a binding at a place the pass cannot follow (an argument of an unknown
//! call, an element of an array, a spread), anything at or below that place may change. A value that holds no object
//! shares none: a `const` initialised to a string, a number or a template, and the result of a well-known function
//! that changes nothing it is given and returns a value of its own ([`PURE_FUNCTIONS`], such as `path.join` and
//! `fs.readFileSync`).
//!
//! A `const` that takes a part of a local of its own function (`const { coverage } = config.test`) is the exception
//! when the evaluator can follow it ([`Unstable::follows`]): it then applies the changes made through the alias to the
//! local itself, so they do not mark the local, nor what may share an object with it.

use std::collections::BTreeMap;

use oxc_ast::ast::{
    Argument, ArrayExpression, ArrowFunctionExpression, AssignmentExpression, AssignmentOperator,
    AssignmentTargetPropertyIdentifier, BindingIdentifier, BindingPattern, CallExpression, ChainExpression,
    ComputedMemberExpression, Expression, ExpressionStatement, FormalParameters, Function, IdentifierReference,
    MemberExpression, NewExpression, ObjectExpression, ObjectPropertyKind, PrivateFieldExpression, Program,
    PropertyKind, ReturnStatement, SimpleAssignmentTarget, StaticMemberExpression, TaggedTemplateExpression,
    UnaryExpression, UnaryOperator, VariableDeclaration, VariableDeclarationKind, VariableDeclarator,
};
use oxc_ast_visit::{Visit, walk};
use oxc_semantic::{ReferenceId, ScopeFlags, ScopeId, Scoping, SemanticBuilder, SymbolId};
use skif_core::{HashMap, HashSet};

use crate::config::defined;

/// The most keys a path below a binding keeps; a longer one is cut to this many, standing for all that is below it.
const MAX_KEYS: usize = 8;

/// The most places one change is followed to; past it, every binding linked to the changed one counts as changed in
/// full, as when changes were not tracked to a part.
const MAX_PLACES: usize = 256;

/// How code the evaluator does not follow may change a binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Change {
    /// The parts of the objects or arrays it holds that these change, none covering another ([`Write::covers`]).
    Contents(Vec<Write>),
    /// The binding itself: it may hold any value.
    Any,
}

impl Change {
    /// The change that is both this one and `other`.
    fn with(self, other: Self) -> Self {
        match (self, other) {
            (Self::Contents(mut writes), Self::Contents(more)) => {
                for write in more {
                    add_write(&mut writes, write);
                }
                Self::Contents(writes)
            }
            _ => Self::Any,
        }
    }
}

/// Adds `write` to `writes` unless one of them covers it, dropping those it covers.
fn add_write(writes: &mut Vec<Write>, write: Write) {
    if writes.iter().any(|known| known.covers(&write)) {
        return;
    }
    writes.retain(|known| !write.covers(known));
    writes.push(write);
}

/// A change below a binding: of what the object or array at `keys` holds, anything below it included, or with `key`,
/// of the value of that key of the object there (`cfg.output.path = …` is `["output"]` and `path`). The empty path is
/// the object or array the binding holds itself.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Write {
    pub(crate) keys: Vec<String>,
    pub(crate) key: Option<String>,
}

impl Write {
    fn contents(keys: Vec<String>) -> Self {
        Self { keys, key: None }
    }

    /// The path of what the change replaces or changes: `keys`, and `key` after them.
    fn path(&self) -> Vec<String> {
        self.keys.iter().chain(&self.key).cloned().collect()
    }

    /// Whether this change changes all that `other` does: a change of what an object holds covers every change at or
    /// below it, and a new value of a key every change at or below that key.
    fn covers(&self, other: &Self) -> bool {
        match self.key {
            None => other.keys.starts_with(&self.keys),
            Some(_) => other.path().starts_with(&self.path()),
        }
    }
}

/// The bindings of a module that code the evaluator does not follow may change.
#[derive(Default)]
pub(crate) struct Unstable {
    /// Reads of such a binding declared inside a function, by reference.
    reads: HashMap<ReferenceId, Change>,
    /// Such top-level bindings, by name.
    top: HashMap<String, Change>,
    /// The aliases whose changes the evaluator applies to the local they take a part of.
    followed: HashSet<SymbolId>,
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
            aliases: Vec::new(),
            primitives: HashSet::default(),
            in_const: false,
        };
        sharing.visit_program(program);
        let followed = sharing.share();
        let mut unstable = Self {
            followed,
            ..Self::default()
        };
        for (symbol, mut change) in sharing.changes {
            if let Change::Contents(writes) = &mut change {
                writes.sort();
            }
            if scoping.symbol_scope_id(symbol) == scoping.root_scope_id() {
                unstable.top.insert(scoping.symbol_name(symbol).to_owned(), change);
            } else {
                for &reference in scoping.get_resolved_reference_ids(symbol) {
                    unstable.reads.insert(reference, change.clone());
                }
            }
        }
        unstable
    }

    /// How the binding a reference to a function's local reads may have changed.
    pub(crate) fn of_reference(&self, id: &IdentifierReference<'_>) -> Option<&Change> {
        self.reads.get(&id.reference_id.get()?)
    }

    /// How the top-level binding `name` may have changed.
    pub(crate) fn of_top(&self, name: &str) -> Option<&Change> {
        self.top.get(name)
    }

    /// Whether the evaluator follows the alias `id` declares ([`alias_source`]): it applies the changes made through
    /// it to the local it takes a part of, which this pass then leaves unmarked. One is followed when it is the only
    /// alias of that local, only its own function changes it, no other binding it may share an object with is changed,
    /// and the local is not changed directly nor by another function.
    pub(crate) fn follows(&self, id: &BindingIdentifier<'_>) -> bool {
        id.symbol_id.get().is_some_and(|symbol| self.followed.contains(&symbol))
    }
}

/// A part of a binding: what it holds at `keys` below it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Place {
    symbol: SymbolId,
    keys: Vec<String>,
}

impl Place {
    fn of(symbol: SymbolId) -> Self {
        Self {
            symbol,
            keys: Vec::new(),
        }
    }
}

/// Two places that may hold the same object: with `exact`, each is the other (`cfg.test` is `t`); otherwise one may
/// hold an object of the other anywhere at or below it (`cfg.list` after `cfg.list.push(t)`).
struct Link {
    a: Place,
    b: Place,
    exact: bool,
}

/// A part of a binding that a value may hold or share: `symbol` at `keys` (`cfg.test`), found at `at` in the value.
/// With `exact`, that part is the value there; otherwise it is somewhere at or below `at`.
#[derive(Clone)]
struct Held {
    symbol: SymbolId,
    keys: Vec<String>,
    at: Vec<String>,
    exact: bool,
}

struct Sharing<'s> {
    scoping: &'s Scoping,
    /// The scopes of the functions being visited, the module's first.
    functions: Vec<ScopeId>,
    /// Places that may hold the same object.
    links: Vec<Link>,
    /// The bindings whose object or array code changes, with the part it changes.
    mutated: Vec<(SymbolId, Write)>,
    changes: BTreeMap<SymbolId, Change>,
    /// The aliases declared, each with the local it takes a part of; linked only when it is not followed.
    aliases: Vec<(SymbolId, SymbolId)>,
    /// The `const` bindings that hold a primitive value ([`is_primitive`]), which shares no object.
    primitives: HashSet<SymbolId>,
    /// Whether the declarations being visited are `const`.
    in_const: bool,
}

impl Sharing<'_> {
    fn symbol(&self, id: &IdentifierReference<'_>) -> Option<SymbolId> {
        local_symbol(self.scoping, id)
    }

    /// The part of a local that `expr` (a name or a member chain) is, and whether it is that part exactly; see [`reach`].
    fn place(&self, expr: &Expression<'_>) -> Option<(Place, bool)> {
        let (id, keys, exact) = reach(expr)?;
        Some((
            Place {
                symbol: self.symbol(id)?,
                keys,
            },
            exact,
        ))
    }

    /// The part of a local that the member target of an assignment (`cfg.a.b`) is, and whether it is that part
    /// exactly.
    fn member_place(&self, member: &MemberExpression<'_>) -> Option<(Place, bool)> {
        let (mut place, exact) = self.place(member.object())?;
        match member.static_property_name() {
            Some(key) if exact => {
                place.keys.push(crate::js_string(key));
                Some((place, true))
            }
            _ => Some((place, false)),
        }
    }

    /// Records the write of the member target of an assignment: a new value of its key in the object at its path, or
    /// a change of what that object holds when the key is not static.
    fn set(&mut self, member: &MemberExpression<'_>) {
        let Some((place, exact)) = self.place(member.object()) else {
            return;
        };
        let write = match member.static_property_name() {
            Some(key) if exact => Write {
                keys: place.keys,
                key: Some(crate::js_string(key)),
            },
            _ => Write::contents(place.keys),
        };
        self.changed(place.symbol, write);
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
        let merged = match self.changes.remove(&symbol) {
            Some(existing) => existing.with(change),
            None => change,
        };
        self.changes.insert(symbol, merged);
    }

    /// Records a write of `symbol`; one from a function other than its own may happen at any time.
    fn written(&mut self, symbol: SymbolId, change: Change) {
        if self.functions.last() != Some(&self.function_of(symbol)) {
            self.change(symbol, change);
        }
    }

    /// Records a change of the object `expr` (a name or a member chain) holds.
    fn mutate(&mut self, expr: &Expression<'_>) {
        if let Some((place, _)) = self.place(expr) {
            self.changed(place.symbol, Write::contents(place.keys));
        }
    }

    /// Records the change `write` of the object `symbol` holds.
    fn changed(&mut self, symbol: SymbolId, write: Write) {
        self.written(symbol, Change::Contents(vec![write.clone()]));
        self.mutated.push((symbol, write));
    }

    /// Links `hub` to each part a value it holds may hold: at the place in the value it is found when `exact`, and
    /// anywhere at or below the hub otherwise.
    fn hold(&mut self, hub: &Place, exact: bool, held: Vec<Held>) {
        for item in held {
            let mut keys = hub.keys.clone();
            if exact {
                keys.extend(item.at);
            }
            self.links.push(Link {
                a: Place {
                    symbol: hub.symbol,
                    keys,
                },
                b: Place {
                    symbol: item.symbol,
                    keys: item.keys,
                },
                exact: exact && item.exact,
            });
        }
    }

    /// Links the parts that values with no binding to hold them may share among themselves (`f(a, b)`).
    fn link_among(&mut self, held: &[Held]) {
        for pair in held.windows(2) {
            self.links.push(Link {
                a: Place {
                    symbol: pair[0].symbol,
                    keys: pair[0].keys.clone(),
                },
                b: Place {
                    symbol: pair[1].symbol,
                    keys: pair[1].keys.clone(),
                },
                exact: false,
            });
        }
    }

    /// Links `hub`, when there is one, to what `held` holds, and what `held` holds among itself otherwise.
    fn hold_or_link(&mut self, hub: Option<(Place, bool)>, held: Vec<Held>) {
        match hub {
            Some((place, exact)) => self.hold(&place, exact, held),
            None => self.link_among(&held),
        }
    }

    /// The parts of locals the value `visit` visits may hold; with `function`, the value is a function, which stands
    /// for its parameters and what it returns.
    fn refs(&self, function: bool, visit: impl FnOnce(&mut Refs<'_>)) -> Vec<Held> {
        let mut refs = Refs {
            scoping: self.scoping,
            function,
            in_body: false,
            at: Vec::new(),
            exact: true,
            frozen: false,
            out: Vec::new(),
        };
        visit(&mut refs);
        refs.out
    }

    /// The aliases a `const` declaration of `pattern` with `init` declares, with the local they take a part of: `None`
    /// unless the local is one of the function being visited ([`alias_source`], [`alias_names`]).
    fn alias(&self, pattern: &BindingPattern<'_>, init: &Expression<'_>) -> Option<(Vec<SymbolId>, SymbolId)> {
        if !self.in_const {
            return None;
        }
        let root = self.symbol(alias_source(init)?.0)?;
        let function = *self.functions.last()?;
        if function == self.scoping.root_scope_id() || self.function_of(root) != function {
            return None;
        }
        let names = alias_names(pattern)?
            .into_iter()
            .map(|(id, _)| id.symbol_id.get())
            .collect::<Option<Vec<SymbolId>>>()?;
        Some((names, root))
    }

    /// The pairs of bindings the links join, whatever the parts.
    fn pairs(&self) -> Vec<(SymbolId, SymbolId)> {
        self.links.iter().map(|link| (link.a.symbol, link.b.symbol)).collect()
    }

    /// Whether the evaluator follows the alias at `index` of [`Self::aliases`]; see [`Unstable::follows`].
    fn follows(&self, index: usize) -> bool {
        let (alias, root) = self.aliases[index];
        self.aliases.iter().filter(|(_, other)| *other == root).count() == 1
            && !self.changes.contains_key(&alias)
            && !self.changes.contains_key(&root)
            && !self.mutated.iter().any(|(symbol, _)| *symbol == root)
            && !self.shares_with_changed(alias)
    }

    /// Whether `symbol` may share an object, through the links made so far, with another binding whose object or
    /// array code changes.
    fn shares_with_changed(&self, symbol: SymbolId) -> bool {
        let mut parent = forest(&self.pairs());
        let class = find(&mut parent, symbol);
        self.mutated
            .iter()
            .any(|&(other, _)| other != symbol && find(&mut parent, other) == class)
    }

    /// Links every alias the evaluator does not follow to the local it takes a part of, until no more is linked (a
    /// link may make another alias one that shares an object), and returns the followed ones. Such a link stands for
    /// the whole local, as it is not tracked which part the alias takes.
    fn link_aliases(&mut self) -> HashSet<SymbolId> {
        let mut linked = vec![false; self.aliases.len()];
        loop {
            let unfollowed: Vec<usize> = (0..self.aliases.len())
                .filter(|&index| !linked[index] && !self.follows(index))
                .collect();
            if unfollowed.is_empty() {
                break;
            }
            for index in unfollowed {
                linked[index] = true;
                let (alias, root) = self.aliases[index];
                self.links.push(Link {
                    a: Place::of(alias),
                    b: Place::of(root),
                    exact: false,
                });
            }
        }
        self.aliases
            .iter()
            .zip(linked)
            .filter(|(_, linked)| !linked)
            .map(|(&(alias, _), _)| alias)
            .collect()
    }

    /// Drops the links and changes of bindings that hold a primitive value, which share no object.
    fn drop_primitives(&mut self) {
        let primitives = &self.primitives;
        self.links
            .retain(|link| !primitives.contains(&link.a.symbol) && !primitives.contains(&link.b.symbol));
        self.mutated.retain(|(symbol, _)| !primitives.contains(symbol));
        self.changes.retain(|symbol, _| !primitives.contains(symbol));
    }

    /// The indices of the links of each binding.
    fn links_of(&self) -> HashMap<SymbolId, Vec<usize>> {
        let mut out: HashMap<SymbolId, Vec<usize>> = HashMap::default();
        for (index, link) in self.links.iter().enumerate() {
            out.entry(link.a.symbol).or_default().push(index);
            if link.b.symbol != link.a.symbol {
                out.entry(link.b.symbol).or_default().push(index);
            }
        }
        out
    }

    /// The bindings a change `origin` of `symbol` reaches through the links (`links_of`, by binding), with the part it
    /// changes there, `symbol` among them; see [`through`].
    fn reached(
        &self,
        links_of: &HashMap<SymbolId, Vec<usize>>,
        symbol: SymbolId,
        origin: &Write,
    ) -> Vec<(SymbolId, Write)> {
        let mut seen = vec![(symbol, origin.clone())];
        let mut queue = seen.clone();
        while let Some((at, write)) = queue.pop() {
            for &index in links_of.get(&at).map_or(&[][..], Vec::as_slice) {
                let link = &self.links[index];
                for (from, to) in [(&link.a, &link.b), (&link.b, &link.a)] {
                    if from.symbol != at {
                        continue;
                    }
                    let Some(mut next) = through(from, to, link.exact, &write) else {
                        continue;
                    };
                    if next.keys.len() > MAX_KEYS {
                        next = Write::contents(next.keys[..MAX_KEYS].to_vec());
                    }
                    if seen
                        .iter()
                        .any(|(known, change)| *known == to.symbol && change.covers(&next))
                    {
                        continue;
                    }
                    if seen.len() >= MAX_PLACES {
                        return self.linked_in_full(symbol);
                    }
                    seen.push((to.symbol, next.clone()));
                    queue.push((to.symbol, next));
                }
            }
        }
        seen
    }

    /// Every binding linked to `symbol`, whatever the parts, changed in full.
    fn linked_in_full(&self, symbol: SymbolId) -> Vec<(SymbolId, Write)> {
        let mut parent = forest(&self.pairs());
        let class = find(&mut parent, symbol);
        let mut symbols: Vec<SymbolId> = parent.keys().copied().collect();
        symbols.sort_unstable();
        symbols
            .into_iter()
            .filter(|&other| find(&mut parent, other) == class)
            .map(|other| (other, Write::default()))
            .collect()
    }

    /// Marks every part of a binding that may share an object with a changed one, other than of the binding changed,
    /// and returns the aliases the evaluator follows, which mark nothing.
    fn share(&mut self) -> HashSet<SymbolId> {
        self.drop_primitives();
        let followed = self.link_aliases();
        let mut origins: Vec<(SymbolId, Write)> = Vec::new();
        for origin in &self.mutated {
            if !origins.contains(origin) {
                origins.push(origin.clone());
            }
        }
        let links_of = self.links_of();
        for (symbol, origin) in origins {
            for (other, write) in self.reached(&links_of, symbol, &origin) {
                if other != symbol {
                    self.change(other, Change::Contents(vec![write]));
                }
            }
        }
        followed
    }
}

/// The change of `to` that the change `write` of the binding of `from` makes through a link of `from` to `to`: as far
/// below `to` as `write` is below `from`, when the link is exact and `write` is at or below `from`; of all of `to` when
/// the link is not exact and `write` is at or below `from`, or when `write` changes what an object above `from` holds;
/// none when they are apart, or when `from` is below a key `write` gives a new value, which leaves the value it held
/// as it was.
fn through(from: &Place, to: &Place, exact: bool, write: &Write) -> Option<Write> {
    if write.keys.starts_with(&from.keys) {
        if !exact {
            return Some(Write::contents(to.keys.clone()));
        }
        let mut keys = to.keys.clone();
        keys.extend_from_slice(&write.keys[from.keys.len()..]);
        return Some(Write {
            keys,
            key: write.key.clone(),
        });
    }
    (from.keys.starts_with(&write.keys) && write.key.is_none()).then(|| Write::contents(to.keys.clone()))
}

impl<'a> Visit<'a> for Sharing<'_> {
    fn visit_function(&mut self, it: &Function<'a>, flags: ScopeFlags) {
        // A declared function's name stands for what its parameters take and what it returns.
        if let Some(symbol) = it.id.as_ref().and_then(|id| id.symbol_id.get()) {
            let held = self.refs(true, |refs| refs.visit_function(it, flags));
            self.hold(&Place::of(symbol), true, held);
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

    fn visit_variable_declaration(&mut self, it: &VariableDeclaration<'a>) {
        let saved = std::mem::replace(&mut self.in_const, it.kind == VariableDeclarationKind::Const);
        walk::walk_variable_declaration(self, it);
        self.in_const = saved;
    }

    fn visit_variable_declarator(&mut self, it: &VariableDeclarator<'a>) {
        if let Some(init) = &it.init
            && let Some((names, root)) = self.alias(&it.id, init)
        {
            self.aliases.extend(names.into_iter().map(|name| (name, root)));
        } else if let Some(init) = &it.init {
            let function = matches!(
                init.get_inner_expression(),
                Expression::ArrowFunctionExpression(_) | Expression::FunctionExpression(_)
            );
            let held = self.refs(function, |refs| refs.visit_expression(init));
            // A name of a destructuring takes a part of the value that is not tracked.
            let whole = matches!(it.id, BindingPattern::BindingIdentifier(_));
            let names: Vec<SymbolId> = it
                .id
                .get_binding_identifiers()
                .iter()
                .filter_map(|id| id.symbol_id.get())
                .collect();
            if whole && self.in_const && is_primitive(init) {
                self.primitives.extend(names.iter().copied());
            }
            if names.is_empty() {
                self.link_among(&held);
            }
            for name in names {
                self.hold(&Place::of(name), whole, held.clone());
            }
        }
        walk::walk_variable_declarator(self, it);
    }

    fn visit_assignment_expression(&mut self, it: &AssignmentExpression<'a>) {
        // What the target is written with may be shared with it; the target itself is visited by the walk.
        let hub = match it.left.as_simple_assignment_target() {
            Some(SimpleAssignmentTarget::AssignmentTargetIdentifier(id)) => {
                self.symbol(id).map(|symbol| (Place::of(symbol), true))
            }
            Some(target) => target
                .as_member_expression()
                .and_then(|member| self.member_place(member)),
            None => None,
        };
        let held = self.refs(false, |refs| refs.visit_expression(&it.right));
        self.hold_or_link(hub, held);
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
                    self.set(member);
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
        if pure_function(it).is_some() {
            return walk::walk_call_expression(self, it);
        }
        // A function called by name stands for its parameters; a method may keep what it is given anywhere in its
        // object.
        let hub = match it.callee.get_inner_expression() {
            Expression::Identifier(id) => self.symbol(id).map(|symbol| (Place::of(symbol), true)),
            callee => {
                if let Some(target) = changed_by_call(it) {
                    self.mutate(target);
                }
                callee
                    .as_member_expression()
                    .and_then(|member| self.place(member.object()))
                    .map(|(place, _)| (place, false))
            }
        };
        let held: Vec<Held> = it
            .arguments
            .iter()
            .flat_map(|argument| self.refs(false, |refs| refs.visit_argument(argument)))
            .collect();
        self.hold_or_link(hub, held);
        walk::walk_call_expression(self, it);
    }

    fn visit_expression_statement(&mut self, it: &ExpressionStatement<'a>) {
        // A call made for what it does may change the objects it is given.
        if let Some(call) = statement_call(&it.expression)
            && changes_arguments(call)
        {
            for argument in call.arguments.iter().filter_map(Argument::as_expression) {
                self.mutate(argument);
            }
        }
        walk::walk_expression_statement(self, it);
    }
}

/// The parts of locals an expression's value may hold or share: the names and member chains it reads, other than the
/// object a method is called on, an imported function, what a function in it reads and what holds no object (a
/// template, an operator's result, the result of one of [`PURE_FUNCTIONS`]).
struct Refs<'s> {
    scoping: &'s Scoping,
    /// The value is a function: it stands for its parameters and what it returns.
    function: bool,
    /// Inside the body of a function, outside a `return`: what is read there is not the function's value.
    in_body: bool,
    /// Where in the value the expression being visited is.
    at: Vec<String>,
    /// Whether the expression being visited is the value at `at`, rather than somewhere at or below it.
    exact: bool,
    /// Inside what the value does not show where it holds (an argument of an unknown call, an array element, a
    /// spread): `at` stays where that is.
    frozen: bool,
    out: Vec<Held>,
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
                    .filter_map(|id| id.symbol_id.get())
                    .map(|symbol| Held {
                        symbol,
                        keys: Vec::new(),
                        at: Vec::new(),
                        exact: true,
                    }),
            );
        }
    }

    /// Records the part of the local `id` at `keys`, which is the value here when `exact`.
    fn hold(&mut self, id: &IdentifierReference<'_>, keys: Vec<String>, exact: bool) {
        if self.in_body {
            return;
        }
        if let Some(symbol) = local_symbol(self.scoping, id) {
            self.out.push(Held {
                symbol,
                keys,
                at: self.at.clone(),
                exact: self.exact && exact,
            });
        }
    }

    /// Visits what `visit` visits as held somewhere at or below where it is.
    fn opaque(&mut self, visit: impl FnOnce(&mut Self)) {
        let saved = (self.exact, self.frozen);
        self.exact = false;
        self.frozen = true;
        visit(self);
        (self.exact, self.frozen) = saved;
    }

    /// Visits what `visit` visits as the value at `key` of where it is.
    fn at_key(&mut self, key: String, visit: impl FnOnce(&mut Self)) {
        if self.frozen {
            return visit(self);
        }
        self.at.push(key);
        visit(self);
        self.at.pop();
    }

    /// A member read: the part of the local its chain names ([`reach`]), or what an object that is not a local
    /// holds, somewhere below.
    fn member(&mut self, expr: &Expression<'_>) {
        match reach(expr) {
            Some((id, keys, exact)) => self.hold(id, keys, exact),
            None => self.opaque(|refs| walk::walk_expression(refs, expr)),
        }
    }
}

impl<'a> Visit<'a> for Refs<'_> {
    fn visit_expression(&mut self, it: &Expression<'a>) {
        match it {
            Expression::StaticMemberExpression(_)
            | Expression::ComputedMemberExpression(_)
            | Expression::PrivateFieldExpression(_) => self.member(it),
            // These hold no object.
            Expression::TemplateLiteral(_) | Expression::BinaryExpression(_) | Expression::UnaryExpression(_) => {}
            // The test is not the value.
            Expression::ConditionalExpression(c) => {
                self.visit_expression(&c.consequent);
                self.visit_expression(&c.alternate);
            }
            _ => walk::walk_expression(self, it),
        }
    }

    fn visit_identifier_reference(&mut self, it: &IdentifierReference<'a>) {
        self.hold(it, Vec::new(), true);
    }

    fn visit_object_expression(&mut self, it: &ObjectExpression<'a>) {
        for property in &it.properties {
            match property {
                ObjectPropertyKind::ObjectProperty(p) => {
                    match p.key.static_name().filter(|_| !p.method && p.kind == PropertyKind::Init) {
                        Some(key) => {
                            let key = crate::js_string(key.as_js_str());
                            self.at_key(key, |refs| refs.visit_expression(&p.value));
                        }
                        None => self.opaque(|refs| {
                            refs.visit_property_key(&p.key);
                            refs.visit_expression(&p.value);
                        }),
                    }
                }
                ObjectPropertyKind::SpreadProperty(s) => self.opaque(|refs| refs.visit_expression(&s.argument)),
            }
        }
    }

    fn visit_array_expression(&mut self, it: &ArrayExpression<'a>) {
        self.opaque(|refs| walk::walk_array_expression(refs, it));
    }

    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        match pure_function(it) {
            Some(Returns::Shared) => {
                return self.opaque(|refs| {
                    for argument in &it.arguments {
                        refs.visit_argument(argument);
                    }
                });
            }
            Some(_) => return,
            None => {}
        }
        match it.callee.get_inner_expression() {
            // A local function stands for what it returns.
            Expression::Identifier(id) => self.hold(id, Vec::new(), true),
            // The object a method is called on is not what it returns.
            callee if callee.is_member_expression() => {}
            _ => self.opaque(|refs| refs.visit_expression(&it.callee)),
        }
        // A `define*` call is the value it is given.
        let defined = defined(it).is_some();
        for (index, argument) in it.arguments.iter().enumerate() {
            if defined && index == 0 {
                self.visit_argument(argument);
            } else {
                self.opaque(|refs| refs.visit_argument(argument));
            }
        }
    }

    fn visit_new_expression(&mut self, it: &NewExpression<'a>) {
        self.opaque(|refs| walk::walk_new_expression(refs, it));
    }

    fn visit_tagged_template_expression(&mut self, it: &TaggedTemplateExpression<'a>) {
        self.opaque(|refs| walk::walk_tagged_template_expression(refs, it));
    }

    fn visit_chain_expression(&mut self, it: &ChainExpression<'a>) {
        self.opaque(|refs| walk::walk_chain_expression(refs, it));
    }

    fn visit_static_member_expression(&mut self, it: &StaticMemberExpression<'a>) {
        self.opaque(|refs| walk::walk_static_member_expression(refs, it));
    }

    fn visit_computed_member_expression(&mut self, it: &ComputedMemberExpression<'a>) {
        self.opaque(|refs| walk::walk_computed_member_expression(refs, it));
    }

    fn visit_private_field_expression(&mut self, it: &PrivateFieldExpression<'a>) {
        self.opaque(|refs| walk::walk_private_field_expression(refs, it));
    }

    fn visit_assignment_expression(&mut self, it: &AssignmentExpression<'a>) {
        // `x = v` is `v`; a logical assignment may also be what the target held.
        if it.operator == AssignmentOperator::Assign {
            self.visit_expression(&it.right);
        } else {
            self.opaque(|refs| walk::walk_assignment_expression(refs, it));
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

/// The union-find forest of the sets of bindings `links` joins.
fn forest(links: &[(SymbolId, SymbolId)]) -> HashMap<SymbolId, SymbolId> {
    let mut parent: HashMap<SymbolId, SymbolId> = HashMap::default();
    for &(a, b) in links {
        let (a, b) = (find(&mut parent, a), find(&mut parent, b));
        if a != b {
            parent.insert(a, b);
        }
    }
    parent
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

/// The name a member chain starts from, the static keys below it and whether the chain is that part exactly: `config`
/// and `["module", "rules"]` for `config.module.rules`, and the name alone for a name. A computed or private member
/// cuts the keys before it (`config.list[i].x` is somewhere below `config.list`). `None` when the chain does not
/// start from a name.
pub(crate) fn reach<'b, 'a>(expr: &'b Expression<'a>) -> Option<(&'b IdentifierReference<'a>, Vec<String>, bool)> {
    let mut keys = Vec::new();
    let mut exact = true;
    let mut expr = expr.get_inner_expression();
    loop {
        match expr {
            Expression::Identifier(id) => {
                keys.reverse();
                return Some((id, keys, exact));
            }
            Expression::StaticMemberExpression(member) => {
                keys.push(member.property.name.as_str().to_owned());
                expr = member.object.get_inner_expression();
            }
            other => {
                let member = other.as_member_expression()?;
                keys.clear();
                exact = false;
                expr = member.object().get_inner_expression();
            }
        }
    }
}

/// The local an alias declaration takes a part of and the keys of that part: `config` and `["test"]` for `config.test`,
/// and the local alone for `config`. `None` for any other expression, a computed or optional member included.
pub(crate) fn alias_source<'b, 'a>(init: &'b Expression<'a>) -> Option<(&'b IdentifierReference<'a>, Vec<String>)> {
    let mut keys = Vec::new();
    let mut expr = init.get_inner_expression();
    loop {
        match expr {
            Expression::Identifier(id) => {
                keys.reverse();
                return Some((id, keys));
            }
            Expression::StaticMemberExpression(member) => {
                keys.push(member.property.name.as_str().to_owned());
                expr = member.object.get_inner_expression();
            }
            _ => return None,
        }
    }
}

/// The names an alias declaration binds, each with the key of the part it takes: the name alone, or each name of an
/// object pattern with its key (`{ coverage, css: style }`). `None` for any other pattern, and for an object pattern
/// with a default, a nested pattern, a key that is not static or a rest element.
pub(crate) fn alias_names<'b, 'a>(
    pattern: &'b BindingPattern<'a>,
) -> Option<Vec<(&'b BindingIdentifier<'a>, Option<String>)>> {
    match pattern {
        BindingPattern::BindingIdentifier(id) => Some(vec![(&**id, None)]),
        BindingPattern::ObjectPattern(object) if object.rest.is_none() => object
            .properties
            .iter()
            .map(|property| match &property.value {
                BindingPattern::BindingIdentifier(id) => {
                    let key = property.key.static_name()?;
                    Some((&**id, Some(crate::js_string(key.as_js_str()))))
                }
                _ => None,
            })
            .collect(),
        _ => None,
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

/// Methods that never change the object they are called on. `resolve` is that of `require`, `path`, `Promise`,
/// `import.meta` and the function `createRequire` returns.
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
    "resolve",
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

/// Methods that change the list they are called on but never what they are given.
const LIST_ADDERS: &[&str] = &["push", "unshift"];

/// What one of [`PURE_FUNCTIONS`] returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Returns {
    /// A string, a number or a boolean.
    Primitive,
    /// An object or array of its own, sharing nothing with its arguments.
    Fresh,
    /// A new object or array that holds parts of its arguments (`Object.entries`).
    Shared,
}

/// Functions of a well-known module or global, by the name of the object they are called on (`path` in `path.join`;
/// empty for a function called by name), that change neither that object nor any argument: each object with the
/// functions (all of them when none is listed) and what they return. The object is known by name only, as a config
/// names these modules by their own names.
const PURE_FUNCTIONS: &[(&str, &[&str], Returns)] = &[
    (
        "path",
        &[
            "basename",
            "dirname",
            "extname",
            "format",
            "isAbsolute",
            "join",
            "normalize",
            "relative",
            "resolve",
            "toNamespacedPath",
        ],
        Returns::Primitive,
    ),
    ("path", &["parse"], Returns::Fresh),
    ("fs", &["existsSync"], Returns::Primitive),
    (
        "fs",
        &["readFileSync", "readdirSync", "realpathSync", "statSync", "lstatSync"],
        Returns::Fresh,
    ),
    ("url", &["fileURLToPath"], Returns::Primitive),
    ("url", &["pathToFileURL"], Returns::Fresh),
    ("JSON", &["stringify"], Returns::Primitive),
    ("JSON", &["parse"], Returns::Fresh),
    ("Object", &["keys", "getOwnPropertyNames"], Returns::Fresh),
    ("Object", &["entries", "values"], Returns::Shared),
    ("Array", &["isArray"], Returns::Primitive),
    ("Math", &[], Returns::Primitive),
    ("Number", &[], Returns::Primitive),
    ("String", &[], Returns::Primitive),
    ("console", &[], Returns::Primitive),
    ("process", &["cwd"], Returns::Primitive),
    (
        "",
        &[
            "Boolean",
            "Number",
            "String",
            "decodeURIComponent",
            "encodeURIComponent",
            "existsSync",
            "fileURLToPath",
            "parseFloat",
            "parseInt",
        ],
        Returns::Primitive,
    ),
    ("", &["readFileSync", "readdirSync"], Returns::Fresh),
];

/// What `call` returns when it calls one of [`PURE_FUNCTIONS`] (`path.posix` and `path.win32` count as `path`); `None`
/// for any other call.
fn pure_function(call: &CallExpression<'_>) -> Option<Returns> {
    let (object, name) = match call.callee.get_inner_expression() {
        Expression::Identifier(id) => ("", id.name.as_str()),
        Expression::StaticMemberExpression(member) => (module_name(&member.object)?, member.property.name.as_str()),
        _ => return None,
    };
    PURE_FUNCTIONS
        .iter()
        .find(|(module, names, _)| *module == object && (names.is_empty() || names.contains(&name)))
        .map(|&(_, _, returns)| returns)
}

/// The name of the object a function of [`PURE_FUNCTIONS`] is called on.
fn module_name<'b>(object: &'b Expression<'_>) -> Option<&'b str> {
    match object.get_inner_expression() {
        Expression::Identifier(id) => Some(id.name.as_str()),
        Expression::StaticMemberExpression(member)
            if matches!(member.property.name.as_str(), "posix" | "win32")
                && matches!(member.object.get_inner_expression(), Expression::Identifier(id) if id.name.as_str() == "path") =>
        {
            Some("path")
        }
        _ => None,
    }
}

/// Whether a call made for what it does may change the objects it is given: not one of [`PURE_FUNCTIONS`], nor a
/// method that changes at most the object it is called on ([`PURE_METHODS`], `push`, `unshift`).
pub(crate) fn changes_arguments(call: &CallExpression<'_>) -> bool {
    if pure_function(call).is_some() {
        return false;
    }
    !matches!(
        call.callee.get_inner_expression(),
        Expression::StaticMemberExpression(m)
            if PURE_METHODS.contains(&m.property.name.as_str()) || LIST_ADDERS.contains(&m.property.name.as_str())
    )
}

/// Whether `expr` is a value that holds no object: a string, number, boolean, `null` or bigint literal, a template, the
/// result of an operator, of one of [`PURE_FUNCTIONS`] that returns one, or a choice between such values.
fn is_primitive(expr: &Expression<'_>) -> bool {
    match expr.get_inner_expression() {
        Expression::StringLiteral(_)
        | Expression::NumericLiteral(_)
        | Expression::BooleanLiteral(_)
        | Expression::NullLiteral(_)
        | Expression::BigIntLiteral(_)
        | Expression::TemplateLiteral(_)
        | Expression::BinaryExpression(_)
        | Expression::UnaryExpression(_) => true,
        Expression::ConditionalExpression(c) => is_primitive(&c.consequent) && is_primitive(&c.alternate),
        Expression::LogicalExpression(l) => is_primitive(&l.left) && is_primitive(&l.right),
        Expression::CallExpression(call) => pure_function(call) == Some(Returns::Primitive),
        _ => false,
    }
}

/// The object a call may change: the first argument of `Object.assign`, or the object a method other than one of
/// [`PURE_METHODS`] or [`PURE_FUNCTIONS`] is called on.
pub(crate) fn changed_by_call<'p, 'a>(call: &'p CallExpression<'a>) -> Option<&'p Expression<'a>> {
    if pure_function(call).is_some() {
        return None;
    }
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
