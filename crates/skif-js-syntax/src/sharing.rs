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
//! `fs.readFileSync`). A function the module declares and only calls by name from outside itself takes each argument
//! of a call as the parameter at its place, and its name stands for what it returns, so `const r = set(cfg)` shares
//! with `cfg` only what `set` returns of its parameter, and a fresh object it returns shares nothing with what it is
//! given. The name of any other function the module declares stands for both what its parameters take and what it
//! returns.
//!
//! A `const` that takes a part of a local of its own function (`const { coverage } = config.test`) is the exception
//! when the evaluator can follow it ([`Unstable::follows`]): it then applies the changes made through the alias to the
//! local itself, so they do not mark the local, nor what may share an object with it.

use std::collections::BTreeMap;

use oxc_ast::ast::{
    Argument, ArrayExpression, ArrowFunctionExpression, AssignmentExpression, AssignmentOperator, AssignmentPattern,
    AssignmentTargetPropertyIdentifier, BindingIdentifier, BindingPattern, CallExpression, ChainExpression,
    ComputedMemberExpression, Declaration, ExportDeclaration, Expression, ExpressionStatement, FormalParameter,
    FormalParameters, Function,
    IdentifierReference, ImportDeclaration, ImportDeclarationSpecifier, MemberExpression, NewExpression,
    ObjectExpression, ObjectPropertyKind, PrivateFieldExpression, Program, PropertyKind, ReturnStatement,
    SimpleAssignmentTarget, StaticMemberExpression, TaggedTemplateExpression, UnaryExpression, UnaryOperator,
    VariableDeclaration, VariableDeclarationKind, VariableDeclarator,
};
use oxc_ast_visit::{Visit, walk};
use oxc_semantic::{ReferenceId, ScopeFlags, ScopeId, Scoping, SemanticBuilder, SymbolId};
use oxc_span::Span;
use skif_core::{HashMap, HashSet};

use crate::config::defined;
use crate::finder::static_string;

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
    /// The calls of one of [`PURE_FUNCTIONS`], by span.
    pure: HashSet<Span>,
    /// What the calls the pass knows do to the objects they are given, by span; any other may change them.
    effects: HashMap<Span, Effect>,
    /// The references that are the only one of a binding no other module can read, unless the module reads
    /// `arguments` or calls `eval`, which reach bindings without one.
    sole: HashSet<ReferenceId>,
}

/// What a call the pass knows does to the objects it is given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Effect {
    /// It changes none of them: one of [`PURE_FUNCTIONS`], a method of a list or string that changes at most the
    /// list ([`PURE_METHODS`], [`LIST_ADDERS`]), a `define*` call of a function the module does not declare, which
    /// returns what it is given, or a merge that builds a new object ([`copies`]).
    Keeps,
    /// It calls a function the module declares, whose changes of what its parameters take reach the arguments through
    /// the links of the call ([`Callables`]).
    Followed,
}

impl Unstable {
    pub(crate) fn of(program: &Program<'_>) -> Self {
        let semantic = SemanticBuilder::new().build(program).semantic;
        let scoping = semantic.scoping();
        let mut sharing = Sharing {
            scoping,
            known: known(program, scoping),
            pure: HashSet::default(),
            effects: HashMap::default(),
            exported: HashSet::default(),
            dynamic: false,
            functions: vec![scoping.root_scope_id()],
            links: Vec::new(),
            mutated: Vec::new(),
            changes: BTreeMap::new(),
            aliases: Vec::new(),
            primitives: HashSet::default(),
            in_const: false,
            callables: callables(program, scoping),
        };
        sharing.visit_program(program);
        let followed = sharing.share();
        let sole = if sharing.dynamic {
            HashSet::default()
        } else {
            sole_references(scoping, &sharing.exported)
        };
        let mut unstable = Self {
            followed,
            pure: std::mem::take(&mut sharing.pure),
            effects: std::mem::take(&mut sharing.effects),
            sole,
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

    /// Whether `call` calls one of [`PURE_FUNCTIONS`], its name resolved in the module.
    pub(crate) fn is_pure(&self, call: &CallExpression<'_>) -> bool {
        self.pure.contains(&call.span)
    }

    /// Whether the call at `span` may change the objects it is given: any call but those whose [`Effect`] the pass
    /// knows, and a call of a function the module declares made as a statement (`set(config);`).
    pub(crate) fn changes_arguments(&self, span: Span, statement: bool) -> bool {
        match self.effects.get(&span) {
            Some(Effect::Keeps) => false,
            Some(Effect::Followed) => statement,
            None => true,
        }
    }

    /// Whether a change made through the argument `id` of a call may be read after the call: `id` is not the only
    /// reference of its binding ([`Self::sole`]).
    pub(crate) fn seen_after(&self, id: &IdentifierReference<'_>) -> bool {
        !id.reference_id
            .get()
            .is_some_and(|reference| self.sole.contains(&reference))
    }
}

/// The references that are the only one of their binding, other than of a binding a module exports by declaring it
/// (`export const config = …`), which another module may read.
fn sole_references(scoping: &Scoping, exported: &HashSet<SymbolId>) -> HashSet<ReferenceId> {
    scoping
        .symbol_ids()
        .filter(|symbol| !exported.contains(symbol))
        .filter_map(|symbol| match scoping.get_resolved_reference_ids(symbol) {
            [only] => Some(*only),
            _ => None,
        })
        .collect()
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
    /// What the module binds names to that the pass knows ([`known`]).
    known: Known,
    /// The calls of one of [`PURE_FUNCTIONS`] found, by span.
    pure: HashSet<Span>,
    /// The [`Effect`] of each call found whose effect the pass knows, by span.
    effects: HashMap<Span, Effect>,
    /// The bindings the module exports by declaring them.
    exported: HashSet<SymbolId>,
    /// Whether the module reads `arguments` or calls `eval`.
    dynamic: bool,
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
    /// The functions the module declares ([`callables`]).
    callables: Callables,
}

impl Sharing<'_> {
    fn symbol(&self, id: &IdentifierReference<'_>) -> Option<SymbolId> {
        local_symbol(self.scoping, id)
    }

    fn names(&self) -> Names<'_> {
        Names {
            scoping: self.scoping,
            known: &self.known,
        }
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

    /// Records a change of each object a call is given.
    fn mutate_arguments(&mut self, arguments: &[Argument<'_>]) {
        for argument in arguments.iter().filter_map(Argument::as_expression) {
            self.mutate(argument);
        }
    }

    /// What `call` does to the objects it is given, when the pass knows ([`Effect`]); `None` when it may change them.
    fn effect(&self, call: &CallExpression<'_>) -> Option<Effect> {
        if calls_callable(self.scoping, &self.callables, call) {
            return Some(Effect::Followed);
        }
        let names = self.names();
        let keeps = pure_function(names, call).is_some()
            || (defined(call).is_some() && foreign(names, &call.callee))
            || copies(names, call)
            || matches!(
                call.callee.get_inner_expression(),
                Expression::StaticMemberExpression(m)
                    if PURE_METHODS.contains(&m.property.name.as_str())
                        || LIST_ADDERS.contains(&m.property.name.as_str())
            );
        keeps.then_some(Effect::Keeps)
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

    /// The parts of locals the value `visit` visits may hold, starting as `part` of it.
    fn refs(&self, part: Part, visit: impl FnOnce(&mut Refs<'_>)) -> Vec<Held> {
        let mut refs = Refs {
            scoping: self.scoping,
            known: &self.known,
            callables: &self.callables,
            part,
            at: Vec::new(),
            exact: true,
            frozen: false,
            out: Vec::new(),
        };
        visit(&mut refs);
        refs.out
    }

    /// The part a function bound to `symbol` is of the value that holds it: what it returns, and what its parameters
    /// take unless a call of it passes its arguments to them ([`Callables::apart`]).
    fn function_part(&self, symbol: Option<SymbolId>) -> Part {
        Part::Function {
            parameters: !symbol.is_some_and(|symbol| self.callables.apart.contains_key(&symbol)),
        }
    }

    /// Links each argument of a call of a function kept apart ([`Callables::apart`]) to the parameter that takes it.
    fn pass(&mut self, parameters: &Parameters, call: &CallExpression<'_>) {
        for (index, argument) in call.arguments.iter().enumerate() {
            let held = self.refs(Part::Value, |refs| refs.visit_argument(argument));
            let (names, exact) = parameters.taking(index);
            for &name in names {
                self.hold(&Place::of(name), exact, held.clone());
            }
        }
    }

    /// Links the names `pattern` binds to `value`, the default of a parameter or of a destructured name (`b = a`),
    /// which they may take: a name is the default itself, and a name a destructuring binds is somewhere in it.
    fn default_of(&mut self, pattern: &BindingPattern<'_>, value: &Expression<'_>) {
        let held = self.refs(Part::Value, |refs| refs.visit_expression(value));
        let whole = matches!(pattern, BindingPattern::BindingIdentifier(_));
        for name in bound_symbols(pattern) {
            self.hold(&Place::of(name), whole, held.clone());
        }
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
        // A declared function's name stands for what it returns, and for what its parameters take unless they are kept
        // apart.
        if let Some(symbol) = it.id.as_ref().and_then(|id| id.symbol_id.get()) {
            let part = self.function_part(Some(symbol));
            let held = self.refs(part, |refs| refs.visit_function(it, flags));
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
            let part = if function {
                self.function_part(it.id.get_binding_identifier().and_then(|id| id.symbol_id.get()))
            } else {
                Part::Value
            };
            let held = self.refs(part, |refs| refs.visit_expression(init));
            // A name of a destructuring takes a part of the value that is not tracked.
            let whole = matches!(it.id, BindingPattern::BindingIdentifier(_));
            let names: Vec<SymbolId> = it
                .id
                .get_binding_identifiers()
                .iter()
                .filter_map(|id| id.symbol_id.get())
                .collect();
            if whole && self.in_const && is_primitive(self.names(), init) {
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
        // What the target is written with may be shared with it; the target itself is visited by the walk. A value
        // stored where no local holds it (`module.exports = [a(), b()]`, a global) is not changed by being stored, and
        // no change through that place is followed, so its parts share nothing among themselves there.
        let held = self.refs(Part::Value, |refs| refs.visit_expression(&it.right));
        match it.left.as_simple_assignment_target() {
            Some(target) => {
                let hub = match target {
                    SimpleAssignmentTarget::AssignmentTargetIdentifier(id) => {
                        self.symbol(id).map(|symbol| (Place::of(symbol), true))
                    }
                    other => other
                        .as_member_expression()
                        .and_then(|member| self.member_place(member)),
                };
                if let Some((place, exact)) = hub {
                    self.hold(&place, exact, held);
                }
            }
            None => self.link_among(&held),
        }
        walk::walk_assignment_expression(self, it);
    }

    fn visit_formal_parameter(&mut self, it: &FormalParameter<'a>) {
        if let Some(initializer) = &it.initializer {
            self.default_of(&it.pattern, initializer);
        }
        walk::walk_formal_parameter(self, it);
    }

    fn visit_assignment_pattern(&mut self, it: &AssignmentPattern<'a>) {
        self.default_of(&it.left, &it.right);
        walk::walk_assignment_pattern(self, it);
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
        // A call whose effect the pass does not know may change what it is given, wherever it is made.
        match self.effect(it) {
            Some(effect) => {
                self.effects.insert(it.span, effect);
            }
            None => self.mutate_arguments(&it.arguments),
        }
        if pure_function(self.names(), it).is_some() {
            self.pure.insert(it.span);
            return walk::walk_call_expression(self, it);
        }
        // A function the module declares passes each argument to the parameter that takes it when they are kept apart,
        // and stands for its parameters otherwise; any other function, and a method in its object, may keep what it
        // is given anywhere.
        let hub = match it.callee.get_inner_expression() {
            Expression::Identifier(id) => {
                let symbol = self.symbol(id);
                if let Some(parameters) = symbol.and_then(|symbol| self.callables.apart.get(&symbol)).cloned() {
                    self.pass(&parameters, it);
                    return walk::walk_call_expression(self, it);
                }
                let exact = calls_callable(self.scoping, &self.callables, it);
                symbol.map(|symbol| (Place::of(symbol), exact))
            }
            callee => {
                if let Some(target) = changed_by_call(it, false) {
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
            .flat_map(|argument| self.refs(Part::Value, |refs| refs.visit_argument(argument)))
            .collect();
        self.hold_or_link(hub, held);
        walk::walk_call_expression(self, it);
    }

    fn visit_new_expression(&mut self, it: &NewExpression<'a>) {
        // A class the pass does not know may change what it is given, and keep it in another argument.
        if constructs_builtin(self.scoping, it) {
            self.effects.insert(it.span, Effect::Keeps);
        } else {
            self.mutate_arguments(&it.arguments);
            let held: Vec<Held> = it
                .arguments
                .iter()
                .flat_map(|argument| self.refs(Part::Value, |refs| refs.visit_argument(argument)))
                .collect();
            self.link_among(&held);
        }
        walk::walk_new_expression(self, it);
    }

    fn visit_expression_statement(&mut self, it: &ExpressionStatement<'a>) {
        // A function the module declares, called for what it does, is taken to change what it is given too.
        if let Some(call) = statement_call(&it.expression)
            && self.effect(call) == Some(Effect::Followed)
        {
            self.mutate_arguments(&call.arguments);
        }
        walk::walk_expression_statement(self, it);
    }

    fn visit_export_declaration(&mut self, it: &ExportDeclaration<'a>) {
        match &it.declaration {
            Declaration::VariableDeclaration(declaration) => {
                for declarator in &declaration.declarations {
                    self.exported.extend(bound_symbols(&declarator.id));
                }
            }
            Declaration::FunctionDeclaration(function) => {
                self.exported
                    .extend(function.id.as_ref().and_then(|id| id.symbol_id.get()));
            }
            Declaration::ClassDeclaration(class) => {
                self.exported
                    .extend(class.id.as_ref().and_then(|id| id.symbol_id.get()));
            }
            _ => {}
        }
        walk::walk_export_declaration(self, it);
    }

    fn visit_identifier_reference(&mut self, it: &IdentifierReference<'a>) {
        self.dynamic |= matches!(it.name.as_str(), "arguments" | "eval");
    }
}

/// The parts of locals an expression's value may hold or share: the names and member chains it reads, other than an
/// imported function, what holds no object (a template, an operator's result, the result of one of [`PURE_FUNCTIONS`]
/// or of [`OWN_RESULT_METHODS`]) and what a function in it reads other than what it returns.
struct Refs<'s> {
    scoping: &'s Scoping,
    /// What the module binds names to that the pass knows ([`known`]).
    known: &'s Known,
    /// The functions the module declares ([`callables`]).
    callables: &'s Callables,
    /// What part of the value the code being visited is.
    part: Part,
    /// Where in the value the expression being visited is.
    at: Vec<String>,
    /// Whether the expression being visited is the value at `at`, rather than somewhere at or below it.
    exact: bool,
    /// Inside what the value does not show where it holds (an argument of an unknown call, an array element, a
    /// spread): `at` stays where that is.
    frozen: bool,
    out: Vec<Held>,
}

/// What part of a value [`Refs`] visits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    /// A function not entered yet, which stands for what it returns and, with `parameters`, what its parameters take.
    Function { parameters: bool },
    /// The body of that function, outside a `return`: what is read there is not the function's value.
    Body,
    /// The value itself.
    Value,
}

impl Refs<'_> {
    /// Records what the parameters of the function take ([`Parameters`]).
    fn parameters(&mut self, params: &FormalParameters<'_>) {
        self.out
            .extend(Parameters::of(params).names().map(|(symbol, exact)| Held {
                symbol,
                keys: Vec::new(),
                at: Vec::new(),
                exact,
            }));
    }

    /// Records the part of the local `id` at `keys`, which is the value here when `exact`.
    fn hold(&mut self, id: &IdentifierReference<'_>, keys: Vec<String>, exact: bool) {
        if self.part == Part::Body {
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

    /// Visits a function with `params`, whose body `visit` visits as `body` ([`Part::Body`], or [`Part::Value`] for an
    /// arrow whose body is an expression). One not entered yet stands for what it returns and, as [`Part::Function`]
    /// says, what its parameters take. One in the value (a getter, a method, a callback) stands for what it returns,
    /// somewhere at or below where it is, as code that calls it gets that. One in the body of a function is not the
    /// value.
    fn function(&mut self, params: &FormalParameters<'_>, body: Part, visit: impl FnOnce(&mut Self)) {
        match self.part {
            Part::Body => {}
            Part::Function { parameters } => {
                if parameters {
                    self.parameters(params);
                }
                self.part = body;
                visit(self);
                self.part = Part::Value;
            }
            Part::Value => self.opaque(|refs| {
                refs.part = body;
                visit(refs);
                refs.part = Part::Value;
            }),
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
                    match p
                        .key
                        .static_name()
                        .filter(|_| !p.method && p.kind == PropertyKind::Init)
                    {
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
        let names = Names {
            scoping: self.scoping,
            known: self.known,
        };
        match pure_function(names, it) {
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
            // A function the module declares stands for what it returns, which holds what it is given only as its
            // parameters do, and the call links those to the arguments ([`Callables`]).
            Expression::Identifier(id) if calls_callable(self.scoping, self.callables, it) => {
                return self.hold(id, Vec::new(), true);
            }
            // Any other function called by name stands for what it returns, which may hold what it is given.
            Expression::Identifier(id) => self.hold(id, Vec::new(), true),
            // A method may return a part of the object it is called on (`list.find(…)`, `list.slice()`, `list.sort()`)
            // unless it returns a value of its own.
            Expression::StaticMemberExpression(member)
                if OWN_RESULT_METHODS.contains(&member.property.name.as_str()) => {}
            callee if callee.is_member_expression() => {
                if let Some(member) = callee.as_member_expression() {
                    let object = member.object();
                    self.opaque(|refs| refs.visit_expression(object));
                }
            }
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
        let saved = std::mem::replace(&mut self.part, Part::Value);
        walk::walk_return_statement(self, it);
        self.part = saved;
    }

    fn visit_function(&mut self, it: &Function<'a>, flags: ScopeFlags) {
        self.function(&it.params, Part::Body, |refs| walk::walk_function(refs, it, flags));
    }

    fn visit_arrow_function_expression(&mut self, it: &ArrowFunctionExpression<'a>) {
        let body = if it.get_expression().is_some() {
            Part::Value
        } else {
            Part::Body
        };
        self.function(&it.params, body, |refs| walk::walk_arrow_function_expression(refs, it));
    }
}

/// The bindings a module binds to a function it declares and never assigns again, which does not read `arguments`: the
/// name of a function and a name declared with a function as its initialiser. A call of one is followed through what
/// its parameters take and what it returns ([`Refs`]).
#[derive(Default)]
struct Callables {
    /// Those whose name stands for both what their parameters take and what they return: one whose name is used other
    /// than as what a call with no spread argument calls, or is called inside the function itself.
    joined: HashSet<SymbolId>,
    /// The others, with their parameters: a call of one links each argument to the parameter that takes it, and its
    /// name stands for what it returns only.
    apart: HashMap<SymbolId, Parameters>,
}

impl Callables {
    fn contains(&self, symbol: SymbolId) -> bool {
        self.joined.contains(&symbol) || self.apart.contains_key(&symbol)
    }
}

/// What the parameters of a function take of the arguments of a call: the names each parameter binds, with whether
/// they are the argument at its place (a name, with a default or not) rather than somewhere at or below it (a
/// destructuring), and the names the rest parameter binds, somewhere at or below the arguments after them.
#[derive(Debug, Clone, Default)]
struct Parameters {
    items: Vec<(Vec<SymbolId>, bool)>,
    rest: Vec<SymbolId>,
}

impl Parameters {
    fn of(params: &FormalParameters<'_>) -> Self {
        let items = params
            .items
            .iter()
            .map(|param| {
                let exact = param.pattern.get_binding_identifier().is_some();
                (bound_symbols(&param.pattern), exact)
            })
            .collect();
        let rest = params
            .rest
            .as_ref()
            .map_or_else(Vec::new, |rest| bound_symbols(&rest.rest.argument));
        Self { items, rest }
    }

    /// The names that take the argument at `index`, and whether they are it.
    fn taking(&self, index: usize) -> (&[SymbolId], bool) {
        self.items
            .get(index)
            .map_or((self.rest.as_slice(), false), |(names, exact)| {
                (names.as_slice(), *exact)
            })
    }

    /// Every name the parameters bind, with whether it is an argument.
    fn names(&self) -> impl Iterator<Item = (SymbolId, bool)> + '_ {
        self.items
            .iter()
            .flat_map(|(names, exact)| names.iter().map(move |&name| (name, *exact)))
            .chain(self.rest.iter().map(|&name| (name, false)))
    }
}

/// The symbols of the names `pattern` binds.
fn bound_symbols(pattern: &BindingPattern<'_>) -> Vec<SymbolId> {
    pattern
        .get_binding_identifiers()
        .iter()
        .filter_map(|id| id.symbol_id.get())
        .collect()
}

/// The functions the module declares that a call is followed through ([`Callables`]).
fn callables(program: &Program<'_>, scoping: &Scoping) -> Callables {
    let mut finder = FindCallables {
        scoping,
        found: Vec::new(),
        callees: HashSet::default(),
    };
    finder.visit_program(program);
    let FindCallables { found, callees, .. } = finder;
    let mut out = Callables::default();
    for (symbol, scope, parameters) in found {
        let apart = scoping.get_resolved_reference_ids(symbol).iter().all(|&reference| {
            callees.contains(&reference)
                && !scoping
                    .scope_ancestors(scoping.get_reference(reference).scope_id())
                    .any(|ancestor| ancestor == scope)
        });
        if apart {
            out.apart.insert(symbol, parameters);
        } else {
            out.joined.insert(symbol);
        }
    }
    out
}

/// Finds the functions of [`callables`] and the names a call calls.
struct FindCallables<'s> {
    scoping: &'s Scoping,
    /// The functions found, each with its scope and parameters.
    found: Vec<(SymbolId, ScopeId, Parameters)>,
    /// The names that are what a call with no spread argument calls.
    callees: HashSet<ReferenceId>,
}

impl FindCallables<'_> {
    /// Adds the function of `scope` bound to `symbol` unless the name is assigned again or the function reads
    /// `arguments`.
    fn add(
        &mut self,
        symbol: Option<SymbolId>,
        scope: Option<ScopeId>,
        params: &FormalParameters<'_>,
        reads_arguments: bool,
    ) {
        let (Some(symbol), Some(scope)) = (symbol, scope) else {
            return;
        };
        if !reads_arguments && !self.scoping.symbol_is_mutated(symbol) {
            self.found.push((symbol, scope, Parameters::of(params)));
        }
    }
}

impl<'a> Visit<'a> for FindCallables<'_> {
    fn visit_function(&mut self, it: &Function<'a>, flags: ScopeFlags) {
        if let Some(id) = &it.id {
            let mut reads = ReadsArguments(false);
            reads.visit_function(it, flags);
            self.add(id.symbol_id.get(), it.scope_id.get(), &it.params, reads.0);
        }
        walk::walk_function(self, it, flags);
    }

    fn visit_variable_declarator(&mut self, it: &VariableDeclarator<'a>) {
        if let (BindingPattern::BindingIdentifier(id), Some(init)) = (&it.id, &it.init) {
            let function = match init.get_inner_expression() {
                Expression::ArrowFunctionExpression(f) => Some((f.scope_id.get(), &f.params)),
                Expression::FunctionExpression(f) => Some((f.scope_id.get(), &f.params)),
                _ => None,
            };
            if let Some((scope, params)) = function {
                let mut reads = ReadsArguments(false);
                reads.visit_expression(init);
                self.add(id.symbol_id.get(), scope, params, reads.0);
            }
        }
        walk::walk_variable_declarator(self, it);
    }

    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        if let Expression::Identifier(id) = it.callee.get_inner_expression()
            && !has_spread(it)
            && let Some(reference) = id.reference_id.get()
        {
            self.callees.insert(reference);
        }
        walk::walk_call_expression(self, it);
    }
}

/// Whether the code visited reads `arguments`, which takes the arguments of a call whatever its parameters.
struct ReadsArguments(bool);

impl<'a> Visit<'a> for ReadsArguments {
    fn visit_identifier_reference(&mut self, it: &IdentifierReference<'a>) {
        self.0 |= it.name.as_str() == "arguments";
    }
}

/// Whether a call gives a spread argument, which leaves the place of each argument after it unknown.
fn has_spread(call: &CallExpression<'_>) -> bool {
    call.arguments
        .iter()
        .any(|argument| matches!(argument, Argument::SpreadElement(_)))
}

/// Whether `call` calls one of `callables` by name and gives it no spread argument, so that each parameter takes the
/// argument at its place.
fn calls_callable(scoping: &Scoping, callables: &Callables, call: &CallExpression<'_>) -> bool {
    let Expression::Identifier(id) = call.callee.get_inner_expression() else {
        return false;
    };
    local_symbol(scoping, id).is_some_and(|symbol| callables.contains(symbol)) && !has_spread(call)
}

/// Built-in classes whose constructors change none of the objects they are given.
const BUILTIN_CLASSES: &[&str] = &[
    "Array",
    "Date",
    "Error",
    "Map",
    "Promise",
    "RegExp",
    "Set",
    "URL",
    "URLSearchParams",
    "WeakMap",
    "WeakSet",
];

/// Whether `new` constructs one of [`BUILTIN_CLASSES`], a global the module does not declare.
fn constructs_builtin(scoping: &Scoping, new: &NewExpression<'_>) -> bool {
    matches!(
        new.callee.get_inner_expression(),
        Expression::Identifier(id) if resolved(scoping, id).is_none() && BUILTIN_CLASSES.contains(&id.name.as_str())
    )
}

/// Packages whose functions build a new object and change none of the objects they are given.
const COPYING_PACKAGES: &[&str] = &["deepmerge", "deepmerge-ts", "defu", "ts-deepmerge", "webpack-merge"];

/// Modules whose `mergeConfig` builds a new config and changes none of those it is given.
const MERGE_CONFIG_MODULES: &[&str] = &["vite", "vitest", "vitest/config"];

/// Globals the module does not declare that build a new object and change none of those they are given, by name.
const COPYING_GLOBALS: &[&str] = &["defu", "mergeConfig"];

/// Whether `call` calls a merge that builds a new object and changes none of the objects it is given: a function of
/// one of [`COPYING_PACKAGES`], the `mergeConfig` of Vite or Vitest, or one of [`COPYING_GLOBALS`].
fn copies(names: Names<'_>, call: &CallExpression<'_>) -> bool {
    let Expression::Identifier(id) = call.callee.get_inner_expression() else {
        return false;
    };
    match resolved(names.scoping, id) {
        None => COPYING_GLOBALS.contains(&id.name.as_str()),
        Some(symbol) => names.known.modules.get(&symbol).is_some_and(|(module, imported)| {
            COPYING_PACKAGES.contains(&module.as_str())
                || (imported.as_deref() == Some("mergeConfig") && MERGE_CONFIG_MODULES.contains(&module.as_str()))
        }),
    }
}

/// Whether the name a callee (`defineConfig`, `vitest.defineProject`) starts from is not one the module declares a
/// value for: a global, an import, or a binding of a `require`.
fn foreign(names: Names<'_>, callee: &Expression<'_>) -> bool {
    let root = match callee.get_inner_expression() {
        Expression::StaticMemberExpression(member) => member.object.get_inner_expression(),
        other => other,
    };
    let Expression::Identifier(id) = root else {
        return false;
    };
    resolved(names.scoping, id).is_none_or(|symbol| {
        names.scoping.symbol_flags(symbol).is_import() || names.known.modules.contains_key(&symbol)
    })
}

/// What the pass knows of the names of a module: how they resolve, and what the module binds them to.
#[derive(Clone, Copy)]
struct Names<'s> {
    scoping: &'s Scoping,
    known: &'s Known,
}

/// The bindings of a module the pass knows the values of ([`known`]).
#[derive(Default)]
struct Known {
    /// The bindings of an import or a `require` of a module, with its specifier and the export they take: `None` for
    /// the module itself (a default or namespace import, `const fs = require('fs')`).
    modules: HashMap<SymbolId, (String, Option<String>)>,
}

/// The bindings of `program` [`Known`] holds.
fn known(program: &Program<'_>, scoping: &Scoping) -> Known {
    let mut finder = FindKnown {
        scoping,
        known: Known::default(),
    };
    finder.visit_program(program);
    finder.known
}

/// Finds the bindings of [`known`].
struct FindKnown<'s> {
    scoping: &'s Scoping,
    known: Known,
}

impl FindKnown<'_> {
    fn bind(&mut self, symbol: Option<SymbolId>, specifier: &str, imported: Option<String>) {
        if let Some(symbol) = symbol {
            let imported = imported.filter(|name| name != "default");
            self.known.modules.insert(symbol, (specifier.to_owned(), imported));
        }
    }

    /// The module a `require('module')` of the global `require` loads.
    fn required(&self, init: &Expression<'_>) -> Option<String> {
        let Expression::CallExpression(call) = init.get_inner_expression() else {
            return None;
        };
        match call.callee.get_inner_expression() {
            Expression::Identifier(id) if id.name.as_str() == "require" && resolved(self.scoping, id).is_none() => {
                call.arguments.first().and_then(Argument::as_expression).and_then(static_string)
            }
            _ => None,
        }
    }
}

impl<'a> Visit<'a> for FindKnown<'_> {
    fn visit_import_declaration(&mut self, it: &ImportDeclaration<'a>) {
        let specifier = crate::js_string(it.source.value);
        for item in it.specifiers.iter().flatten() {
            match item {
                ImportDeclarationSpecifier::ImportSpecifier(s) => {
                    let imported = crate::collect::module_export_name(&s.imported);
                    self.bind(s.local.symbol_id.get(), &specifier, Some(imported));
                }
                ImportDeclarationSpecifier::ImportDefaultSpecifier(s) => {
                    self.bind(s.local.symbol_id.get(), &specifier, None);
                }
                ImportDeclarationSpecifier::ImportNamespaceSpecifier(s) => {
                    self.bind(s.local.symbol_id.get(), &specifier, None);
                }
            }
        }
    }

    fn visit_variable_declarator(&mut self, it: &VariableDeclarator<'a>) {
        if let Some(specifier) = it.init.as_ref().and_then(|init| self.required(init)) {
            match &it.id {
                BindingPattern::BindingIdentifier(id) => self.bind(id.symbol_id.get(), &specifier, None),
                BindingPattern::ObjectPattern(pattern) => {
                    for property in &pattern.properties {
                        if let (Some(key), BindingPattern::BindingIdentifier(id)) =
                            (property.key.static_name(), &property.value)
                        {
                            self.bind(id.symbol_id.get(), &specifier, Some(crate::js_string(key.as_js_str())));
                        }
                    }
                }
                _ => {}
            }
        }
        walk::walk_variable_declarator(self, it);
    }
}

/// The symbol a reference resolves to, an import included; `None` for a global the module does not declare.
fn resolved(scoping: &Scoping, id: &IdentifierReference<'_>) -> Option<SymbolId> {
    scoping.get_reference(id.reference_id.get()?).symbol_id()
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

/// Methods whose result holds nothing of the object they are called on: a boolean, a number, a string, `undefined` or a
/// new list of strings (`split`, `match`).
const OWN_RESULT_METHODS: &[&str] = &[
    "charAt",
    "endsWith",
    "every",
    "findIndex",
    "findLastIndex",
    "forEach",
    "has",
    "hasOwnProperty",
    "includes",
    "indexOf",
    "join",
    "lastIndexOf",
    "match",
    "padEnd",
    "padStart",
    "push",
    "replace",
    "replaceAll",
    "resolve",
    "some",
    "split",
    "startsWith",
    "test",
    "toLowerCase",
    "toString",
    "toUpperCase",
    "trim",
    "trimEnd",
    "trimStart",
    "unshift",
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
/// empty for a global function called by name), that change neither that object nor any argument: each object with
/// the functions (all of them when none is listed) and what they return. A module's functions count where the module
/// binds `path`, `fs` or `url` to that module or imports the function from it, and a global's where the name is not
/// declared in the module ([`pure_function`]).
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

/// The globals [`PURE_FUNCTIONS`] lists functions of, by name.
const GLOBAL_OBJECTS: &[&str] = &["Array", "JSON", "Math", "Number", "Object", "String", "console", "process"];

/// What `call` returns when it calls one of [`PURE_FUNCTIONS`] (`path.posix` and `path.win32` count as `path`); `None`
/// for any other call, such as one of a function the module declares under such a name.
fn pure_function(names: Names<'_>, call: &CallExpression<'_>) -> Option<Returns> {
    let (object, name) = match call.callee.get_inner_expression() {
        Expression::Identifier(id) => match resolved(names.scoping, id) {
            None => ("", id.name.as_str()),
            Some(symbol) => {
                let (module, imported) = names.known.modules.get(&symbol)?;
                (known_module(module)?, imported.as_deref()?)
            }
        },
        Expression::StaticMemberExpression(member) => (module_object(names, &member.object)?, member.property.name.as_str()),
        _ => return None,
    };
    PURE_FUNCTIONS
        .iter()
        .find(|(module, functions, _)| *module == object && (functions.is_empty() || functions.contains(&name)))
        .map(|&(_, _, returns)| returns)
}

/// The name in [`PURE_FUNCTIONS`] of the object a function is called on: a global of [`GLOBAL_OBJECTS`] the module
/// does not declare, or a binding of the `path`, `fs` or `url` module itself.
fn module_object(names: Names<'_>, object: &Expression<'_>) -> Option<&'static str> {
    match object.get_inner_expression() {
        Expression::Identifier(id) => match resolved(names.scoping, id) {
            None => GLOBAL_OBJECTS.iter().find(|global| **global == id.name.as_str()).copied(),
            Some(symbol) => match names.known.modules.get(&symbol) {
                Some((module, None)) => known_module(module),
                _ => None,
            },
        },
        Expression::StaticMemberExpression(member)
            if matches!(member.property.name.as_str(), "posix" | "win32")
                && module_object(names, &member.object) == Some("path") =>
        {
            Some("path")
        }
        _ => None,
    }
}

/// The name in [`PURE_FUNCTIONS`] of a module the pass knows the functions of.
fn known_module(specifier: &str) -> Option<&'static str> {
    match specifier.strip_prefix("node:").unwrap_or(specifier) {
        "path" | "path/posix" => Some("path"),
        "fs" => Some("fs"),
        "url" => Some("url"),
        _ => None,
    }
}

/// Whether `expr` is a value that holds no object: a string, number, boolean, `null` or bigint literal, a template, the
/// result of an operator, of one of [`PURE_FUNCTIONS`] that returns one, or a choice between such values.
fn is_primitive(names: Names<'_>, expr: &Expression<'_>) -> bool {
    match expr.get_inner_expression() {
        Expression::StringLiteral(_)
        | Expression::NumericLiteral(_)
        | Expression::BooleanLiteral(_)
        | Expression::NullLiteral(_)
        | Expression::BigIntLiteral(_)
        | Expression::TemplateLiteral(_)
        | Expression::BinaryExpression(_)
        | Expression::UnaryExpression(_) => true,
        Expression::ConditionalExpression(c) => is_primitive(names, &c.consequent) && is_primitive(names, &c.alternate),
        Expression::LogicalExpression(l) => is_primitive(names, &l.left) && is_primitive(names, &l.right),
        Expression::CallExpression(call) => pure_function(names, call) == Some(Returns::Primitive),
        _ => false,
    }
}

/// The object a call may change: the first argument of `Object.assign`, or the object a method other than one of
/// [`PURE_METHODS`] is called on, unless the call is one of [`PURE_FUNCTIONS`] (`pure`).
pub(crate) fn changed_by_call<'p, 'a>(call: &'p CallExpression<'a>, pure: bool) -> Option<&'p Expression<'a>> {
    if pure {
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
