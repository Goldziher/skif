//! The bindings of a config module whose value code the evaluator does not follow may change.
//!
//! The evaluator copies values: `const a = cfg; a.x = 1` changes its copy of `a`, not `cfg`, and a function it does not
//! call, or calls for its value only, does not change what it captures. An import counts as a binding of the module
//! here, as the evaluator reads it as its module computes it, but for an imported function or class called, whose
//! result holds none of the module's locals. This pass finds, by symbol, the bindings such
//! code may change: one a nested function assigns or changes, and one that may share an object with a binding that is
//! changed (`const a = cfg`, `{ test: t }`, `{ ...base }`, `f(cfg)`). Reading one of them yields a value that says it
//! may hold more than the evaluator saw.
//!
//! A change is tracked to the part of a binding it reaches: a write through `cfg.module.rules` changes what the list
//! there holds, and a binding that holds `t` under `test` changes under `test` when `t` does, so the rest of the
//! binding stays known. A change through one part of a binding also reaches another part of it that may hold the same
//! object (`config.b = config.a`), which the evaluator, changing only the part written, leaves as it was. A spread of a
//! part of a local (`{ ...options }`) copies its own keys, so a change below one of
//! them reaches the other at that key, and a new value of a key of either is not the other's. Where a value holds a
//! binding at a place the pass cannot follow (an argument of an unknown call, an element of an array, a spread of
//! anything else but an object literal), anything at or below that place may change. A value that holds no object
//! shares none: a `const` initialised to a string, a number or a template, and
//! the result of a well-known function that changes nothing it is given and returns a value of its own
//! ([`PURE_FUNCTIONS`], such as `path.join` and `fs.readFileSync`). A function the module declares and only calls by name from outside itself takes each argument
//! of a call as the parameter at its place, and its name stands for what it returns, so `const r = set(cfg)` shares
//! with `cfg` only what `set` returns of its parameter, and a fresh object it returns shares nothing with what it is
//! given. The name of any other function the module declares stands for both what its parameters take and what it
//! returns.
//!
//! A call whose effect is not known (a function the module or a library declares, a `new` of a class other than a
//! builtin one, a list or string method on an object not known to be a list or string) may change every object it is
//! given, in any position and through a spread, and every part of a local an object or array literal it is given holds,
//! at any depth ([`given_parts`]); a spread of a local there (`{ ...options }`) leaves all of the local open, as a
//! change of only what its keys hold is not one the pass can say ([`Effect`]). The pass knows the effect of a
//! well-known function only where its name resolves to the module that declares it, and treats a `define…` helper of
//! a package it does not follow (not one of the workspace) and a merge that copies ([`copies`]) and `Object.assign`,
//! beyond its first argument, as changing nothing; the first two keep nothing they are given in another argument
//! either. A loop name, a callback parameter and a name a destructuring assigns share with what they take, but for the element a callback
//! or a loop over a `const` list or object of primitive values (or its `Object.entries`) takes when no change reaches
//! it, and a write in a class body is one the evaluator does not follow.
//!
//! The bindings of the modules the evaluator follows are linked to each other, as two exports, of one module or of two,
//! may be the same object or hold one another; one that is a part of another (`default` of a namespace import and the
//! default import) is linked at that part. A call of a function of such a module that may change a value of a module
//! ([`Imports::changes_modules`]), and an import of one for its effects only (`import './setup'`), may change every
//! such binding in full, wherever it is read, as what a read before the call gives may be the object it changes. What a
//! module imported for its bindings does when it is loaded is not followed.
//!
//! Code a value runs implicitly is not followed: a getter, a `toString` or `valueOf` a template literal or `String(x)`
//! calls, a `toJSON` of `JSON.stringify`, a `then` of a value awaited. Neither is a change an imported function makes
//! to what it was given by an earlier call (`register(config)`, later `apply()`).
//!
//! A `const` that takes a part of a local of its own function (`const { coverage } = config.test`) is the exception
//! when the evaluator can follow it ([`Unstable::follows`]): it then applies the changes made through the alias to that
//! part of the local itself, so they do not mark the local there. They still mark what may share an object with the
//! local, and another part of it that may hold the same object, as the evaluator's copies there are not changed; a
//! local of the alias's function is marked only where it is read after the alias is declared, outside a loop and a
//! nested function, as what an earlier read copied out is itself marked where it went.

use std::collections::BTreeMap;

use oxc_ast::ast::{
    Argument, ArrayExpression, ArrayExpressionElement, ArrowFunctionExpression, AssignmentExpression,
    AssignmentOperator, AssignmentPattern, AssignmentTarget, AssignmentTargetMaybeDefault, AssignmentTargetPattern,
    AssignmentTargetProperty, AssignmentTargetPropertyIdentifier, BindingIdentifier, BindingPattern, CallExpression,
    ChainExpression, Class, ComputedMemberExpression, Declaration, DoWhileStatement, ExportDeclaration, Expression,
    ExpressionStatement, ForInStatement, ForOfStatement, ForStatement, ForStatementLeft, FormalParameter,
    FormalParameters, Function, IdentifierReference, ImportDeclaration, ImportDeclarationSpecifier, LogicalOperator,
    MemberExpression, NewExpression, ObjectExpression, ObjectPropertyKind, PrivateFieldExpression, Program,
    PropertyKind, ReturnStatement, SimpleAssignmentTarget, StaticMemberExpression, TaggedTemplateExpression,
    UnaryExpression, UnaryOperator, VariableDeclaration, VariableDeclarationKind, VariableDeclarator, WhileStatement,
    WithStatement,
};
use oxc_ast_visit::{Visit, walk};
use oxc_semantic::{ReferenceId, ScopeFlags, ScopeId, Scoping, SemanticBuilder, SymbolId};
use oxc_span::{GetSpan, Span};
use skif_core::{HashMap, HashSet};

use crate::config::defined;
use crate::finder::static_string;

/// An alias a declaration declares, with the keys of the part of its local it takes.
type AliasPart = (SymbolId, Vec<String>);

/// A part of a local that an object which is not a name or a member chain may be (`list.find(…)`, `a ? b : c`): the
/// local's name, the keys of the part and whether the object is that part itself ([`Unstable::parts_of`]).
pub(crate) type ObjectPart = (String, Vec<String>, bool);
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

/// What the evaluator knows of the modules a module imports, by specifier.
pub(crate) trait Imports {
    /// Whether the evaluator follows the module `specifier` names.
    fn follows(&self, specifier: &str) -> bool;

    /// Whether a function of the module `specifier` names, which the evaluator follows, may change a value of a
    /// module when it is called: it changes a binding of its own module or calls such a function of another one
    /// ([`Unstable::changes_modules`]).
    fn changes_modules(&self, specifier: &str) -> bool;
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
    /// The parts of locals each object written through that is not a name or a member chain may be, by span.
    objects: HashMap<Span, Vec<ObjectPart>>,
    /// The calls of one of [`CALLBACK_METHODS`] that may call code the pass does not follow, by span.
    calls_back: HashSet<Span>,
    /// The calls of a local by name that holds a function the pass does not follow, by span.
    called_values: HashSet<Span>,
    /// The calls of a function no code of the project is behind ([`foreign`]), by span.
    foreign_calls: HashSet<Span>,
    /// The names of the module's namespace imports.
    namespaces: HashSet<String>,
    /// Whether code of a module the evaluator follows may run unseen ([`Sharing::runs_modules`]).
    runs_modules: bool,
    /// Whether a function of the module may change a value of the module when it is called
    /// ([`Sharing::writes_modules`]).
    writes_modules: bool,
    /// The references that are the only one of a binding no other module can read and that are read once per binding:
    /// not in a loop, nor in a function other than the binding's own, and not of an alias the evaluator follows. None
    /// when the module reads `arguments`, calls `eval` or has a `with` statement, which reach bindings without a
    /// reference.
    sole: HashSet<ReferenceId>,
}

/// What a call the pass knows does to the objects it is given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Effect {
    /// It changes none of them: one of [`PURE_FUNCTIONS`], a method that changes at most the object it is called on
    /// ([`PURE_METHODS`], [`LIST_ADDERS`], [`OWN_RESULT_METHODS`]) called on an object that is known to have it
    /// ([`builtin`]), a `define*` call of a global or of a package the evaluator does not follow ([`foreign`]), which
    /// returns what it is given, a merge that builds a new object ([`copies`]), or `Object.assign` given its first
    /// argument other than through a spread, which changes only that argument ([`assigns`]).
    Keeps,
    /// It calls a function the module declares, whose changes of what its parameters take reach the arguments through
    /// the links of the call ([`Callables`]).
    Followed,
}

impl Unstable {
    /// The bindings of `program` code the evaluator does not follow may change, where `imports` says what the
    /// evaluator knows of the modules it imports.
    pub(crate) fn of(program: &Program<'_>, imports: &dyn Imports) -> Self {
        let semantic = SemanticBuilder::new().build(program).semantic;
        let scoping = semantic.scoping();
        let mut sharing = Sharing {
            scoping,
            imports,
            known: known(program, scoping, imports),
            pure: HashSet::default(),
            effects: HashMap::default(),
            objects: HashMap::default(),
            calls_back: HashSet::default(),
            called_values: HashSet::default(),
            foreign_calls: HashSet::default(),
            exported: HashSet::default(),
            dynamic: false,
            loops: 0,
            repeated: HashSet::default(),
            functions: vec![scoping.root_scope_id()],
            links: Vec::new(),
            mutated: Vec::new(),
            changes: BTreeMap::new(),
            aliases: Vec::new(),
            primitives: HashSet::default(),
            primitive_lists: HashSet::default(),
            element_links: Vec::new(),
            in_const: false,
            callables: callables(program, scoping),
            alias_parts: HashMap::default(),
            ends: HashMap::default(),
            late: Vec::new(),
            runs_modules: false,
            defining: false,
            function_writes: Vec::new(),
        };
        sharing.visit_program(program);
        let (followed, writes_modules) = sharing.share();
        let sole = if sharing.dynamic {
            HashSet::default()
        } else {
            sole_references(scoping, &sharing.exported, &followed, &sharing.repeated)
        };
        let mut unstable = Self {
            followed,
            pure: std::mem::take(&mut sharing.pure),
            effects: std::mem::take(&mut sharing.effects),
            objects: std::mem::take(&mut sharing.objects),
            calls_back: std::mem::take(&mut sharing.calls_back),
            called_values: std::mem::take(&mut sharing.called_values),
            foreign_calls: std::mem::take(&mut sharing.foreign_calls),
            namespaces: sharing
                .known
                .namespaces
                .iter()
                .map(|&symbol| scoping.symbol_name(symbol).to_owned())
                .collect(),
            sole,
            runs_modules: sharing.runs_modules,
            writes_modules,
            ..Self::default()
        };
        for (symbol, change) in std::mem::take(&mut sharing.changes) {
            let change = sorted(change);
            if scoping.symbol_scope_id(symbol) == scoping.root_scope_id() {
                unstable.top.insert(scoping.symbol_name(symbol).to_owned(), change);
            } else {
                for &reference in scoping.get_resolved_reference_ids(symbol) {
                    unstable.reads.insert(reference, change.clone());
                }
            }
        }
        for (symbol, from, change) in std::mem::take(&mut sharing.late) {
            for &reference in scoping.get_resolved_reference_ids(symbol) {
                if sharing.read_before(reference, from) {
                    continue;
                }
                let merged = match unstable.reads.remove(&reference) {
                    Some(existing) => sorted(existing.with(change.clone())),
                    None => sorted(change.clone()),
                };
                unstable.reads.insert(reference, merged);
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
    /// it to the part of the local it takes, which this pass then leaves unmarked there, marking only what else may
    /// share an object with that part. One is followed when it is the only alias of that local, only its own function
    /// changes it, no other binding it or the local may share an object with is changed, and the local is not changed
    /// directly nor by another function, nor ever assigned again, which may leave the alias an object the local no
    /// longer holds.
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

    /// The parts of locals the object `object` may be, when it is written through and is not a name or a member chain
    /// (`config.module.rules.find(…).loader = …`, `(prod ? config.prod : config.dev).devtool = …`).
    pub(crate) fn parts_of(&self, object: &Expression<'_>) -> &[ObjectPart] {
        self.objects.get(&object.span()).map_or(&[], Vec::as_slice)
    }

    /// Whether `call` is a method of [`CALLBACK_METHODS`] that may call code the pass does not follow with what the
    /// object it is called on holds ([`changed_by_call`]).
    pub(crate) fn calls_back(&self, call: &CallExpression<'_>) -> bool {
        self.calls_back.contains(&call.span)
    }

    /// Whether `call` calls a local by name that holds a function the pass does not follow (`addAlias` after
    /// `const addAlias = aliaser(config)`), which may change what the local holds and was made from.
    pub(crate) fn calls_value(&self, call: &CallExpression<'_>) -> bool {
        self.called_values.contains(&call.span)
    }

    /// Whether `call` calls a function no code of the project is behind: one whose callee (`defineConfig`,
    /// `vitest.defineProject`, `Object.assign`) starts from a global or from a binding of a package the evaluator does
    /// not follow, so that what a `define…` helper or a merge of that name does is what such a library's does.
    pub(crate) fn calls_foreign(&self, call: &CallExpression<'_>) -> bool {
        self.foreign_calls.contains(&call.span)
    }

    /// Whether `object` is the binding of a namespace import, whose methods are taken to change nothing of it.
    pub(crate) fn is_namespace(&self, object: &Expression<'_>) -> bool {
        matches!(object.get_inner_expression(), Expression::Identifier(id) if self.namespaces.contains(id.name.as_str()))
    }

    /// Whether a function of the module may change a value of a module when it is called: code in a function writes
    /// to, or gives a call that may change it, a top-level binding, an import, or an object one of them may hold
    /// ([`Sharing::writes_modules`]), or the module calls a function of a module that may ([`Imports::changes_modules`]),
    /// or imports one for its effects. A function that builds and returns new objects changes none, whatever it does
    /// to them or to what its parameters take, which a call gives it.
    pub(crate) fn changes_modules(&self) -> bool {
        self.runs_modules || self.writes_modules
    }

    /// Whether a change made through the argument `id` of a call may be read after the call: `id` is not the only
    /// reference of its binding ([`Self::sole`]).
    pub(crate) fn seen_after(&self, id: &IdentifierReference<'_>) -> bool {
        !id.reference_id
            .get()
            .is_some_and(|reference| self.sole.contains(&reference))
    }
}

/// `change` with its writes in order.
fn sorted(mut change: Change) -> Change {
    if let Change::Contents(writes) = &mut change {
        writes.sort();
    }
    change
}

/// The references that are the only one of their binding, other than of a binding a module exports by declaring it
/// (`export const config = …`), which another module may read, of an alias the evaluator follows, whose changes are
/// read through the local it takes a part of, and other than those of `repeated`, which may be read more than once.
fn sole_references(
    scoping: &Scoping,
    exported: &HashSet<SymbolId>,
    followed: &HashSet<SymbolId>,
    repeated: &HashSet<ReferenceId>,
) -> HashSet<ReferenceId> {
    scoping
        .symbol_ids()
        .filter(|symbol| !exported.contains(symbol) && !followed.contains(symbol))
        .filter_map(|symbol| match scoping.get_resolved_reference_ids(symbol) {
            [only] if !repeated.contains(only) => Some(*only),
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

/// How two places that may hold the same object hold it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Join {
    /// Each is the other (`cfg.test` is `t`).
    Same,
    /// One is a copy of the own keys of the other (`{ ...options }`): each holds at a key what the other holds there,
    /// so a change below a key reaches the other there, and a new value of a key of one is not the other's.
    Copy,
    /// One may hold an object of the other anywhere at or below it (`cfg.list` after `cfg.list.push(t)`).
    Within,
}

impl Join {
    /// [`Self::Same`] when `exact`, [`Self::Within`] otherwise.
    fn of(exact: bool) -> Self {
        if exact { Self::Same } else { Self::Within }
    }
}

/// Two places that may hold the same object, as `join` says.
struct Link {
    a: Place,
    b: Place,
    join: Join,
    /// Whether code the module runs once, when it is loaded, makes the link, outside the definition of a function
    /// (`export const config = create()`): it joins what that run made, which a later call of a function of the module
    /// does not make again ([`Sharing::writes_modules`]).
    loading: bool,
}

/// A part of a binding that a value may hold or share: `symbol` at `keys` (`cfg.test`), found at `at` in the value.
/// With [`Join::Same`], that part is the value there; with [`Join::Copy`], the value there copies its own keys; with
/// [`Join::Within`], it is somewhere at or below `at`.
#[derive(Clone)]
struct Held {
    symbol: SymbolId,
    keys: Vec<String>,
    at: Vec<String>,
    join: Join,
}

struct Sharing<'s> {
    scoping: &'s Scoping,
    /// What the evaluator knows of the modules the module imports.
    imports: &'s dyn Imports,
    /// What the module binds names to that the pass knows ([`known`]).
    known: Known,
    /// The calls of one of [`PURE_FUNCTIONS`] found, by span.
    pure: HashSet<Span>,
    /// The [`Effect`] of each call found whose effect the pass knows, by span.
    effects: HashMap<Span, Effect>,
    /// The parts of locals each object written through that is not a name or a member chain may be ([`Self::parts`]).
    objects: HashMap<Span, Vec<ObjectPart>>,
    /// The calls found that may call code the pass does not follow with what a list holds ([`Self::calls_back`]).
    calls_back: HashSet<Span>,
    /// The calls found of a local by name that holds a function the pass does not follow ([`Unstable::calls_value`]).
    called_values: HashSet<Span>,
    /// The calls found of a function no code of the project is behind ([`Unstable::calls_foreign`]).
    foreign_calls: HashSet<Span>,
    /// The bindings the module exports by declaring them.
    exported: HashSet<SymbolId>,
    /// Whether the module reads `arguments`, calls `eval` or has a `with` statement, which reach bindings without a
    /// reference.
    dynamic: bool,
    /// How many loops the code being visited is in.
    loops: usize,
    /// The references that may be read more than once for one binding: in a loop, or in a function other than the
    /// binding's own.
    repeated: HashSet<ReferenceId>,
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
    /// The `const` bindings of a list or an object of primitive values ([`is_primitive_list`],
    /// [`is_primitive_record`]), whose elements and values are objects only once code changes it.
    primitive_lists: HashSet<SymbolId>,
    /// The links of the first parameter of each callback given to a method of one of [`Self::primitive_lists`] to
    /// that list, made only when a change may reach the list ([`Self::link_elements`]).
    element_links: Vec<Link>,
    /// Whether the declarations being visited are `const`.
    in_const: bool,
    /// The functions the module declares ([`callables`]).
    callables: Callables,
    /// The part of the local each alias takes ([`alias_source`]), and where its declaration ends.
    alias_parts: HashMap<SymbolId, (Vec<String>, u32)>,
    /// Where each reference visited ends.
    ends: HashMap<ReferenceId, u32>,
    /// The changes of a local made through an alias the evaluator follows that a read of it sees only after the
    /// declaration of the alias, which ends at the offset given ([`Self::read_before`]).
    late: Vec<(SymbolId, u32, Change)>,
    /// Whether code of a module the evaluator follows may run unseen and change what such a module exports
    /// ([`Self::runs_followed`]).
    runs_modules: bool,
    /// Whether the links being made are those of the definition of a function: what it returns and what its
    /// parameters take, which hold for every call of it.
    defining: bool,
    /// The changes code in a function makes of a binding, each with the scope of that function.
    function_writes: Vec<(SymbolId, ScopeId, Change)>,
}

/// An alias the evaluator follows ([`Unstable::follows`]): the local it takes a part of, the keys of that part and where
/// the declaration of the alias ends, before which no change is made through it.
struct AppliedAlias {
    alias: SymbolId,
    root: SymbolId,
    keys: Vec<String>,
    from: u32,
}

impl AppliedAlias {
    /// The change of its local the evaluator applies for the change `write` made through the alias.
    fn applied(&self, write: &Write) -> Write {
        let mut keys = self.keys.clone();
        keys.extend_from_slice(&write.keys);
        Write {
            keys,
            key: write.key.clone(),
        }
    }
}

impl Sharing<'_> {
    /// The binding a reference reads or writes, an import included: a change of what an import holds is read where the
    /// import is ([`Unstable::of_top`]).
    fn symbol(&self, id: &IdentifierReference<'_>) -> Option<SymbolId> {
        resolved(self.scoping, id)
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

    /// The parts of locals the object `expr` may be, each with whether it is that part exactly: the part a name or a
    /// member chain is ([`Self::place`]), or each part what any other expression evaluates to may hold ([`Refs`]), as
    /// the object a method returns (`list.find(…)`) or a condition chooses; those are recorded for the evaluator
    /// ([`Unstable::parts_of`]).
    fn parts(&mut self, expr: &Expression<'_>) -> Vec<(Place, bool)> {
        if reach(expr).is_some() {
            return self.place(expr).into_iter().collect();
        }
        let parts: Vec<(Place, bool)> = self
            .refs(Part::Value, |refs| refs.visit_expression(expr))
            .into_iter()
            .map(|held| {
                let exact = held.join == Join::Same && held.at.is_empty();
                let place = Place {
                    symbol: held.symbol,
                    keys: held.keys,
                };
                (place, exact)
            })
            .collect();
        let named = parts
            .iter()
            .map(|(place, exact)| {
                let name = self.scoping.symbol_name(place.symbol).to_owned();
                (name, place.keys.clone(), *exact)
            })
            .collect();
        self.objects.insert(expr.span(), named);
        parts
    }

    /// Records the write of the member target of an assignment: a new value of its key in each object it may be
    /// written in ([`Self::parts`]), or a change of what that object holds when the key is not static or the object
    /// is not exactly a part of a local.
    fn set(&mut self, member: &MemberExpression<'_>) {
        let key = member.static_property_name().map(crate::js_string);
        for (place, exact) in self.parts(member.object()) {
            let write = match &key {
                Some(key) if exact => Write {
                    keys: place.keys,
                    key: Some(key.clone()),
                },
                _ => Write::contents(place.keys),
            };
            self.changed(place.symbol, write);
        }
    }

    /// The parts of locals the member target of an assignment writes to and whether each is the target exactly: the
    /// target of a member chain ([`Self::member_place`]), or somewhere at or below each part its object may be.
    fn member_places(&mut self, member: &MemberExpression<'_>) -> Vec<(Place, bool)> {
        if reach(member.object()).is_some() {
            return self.member_place(member).into_iter().collect();
        }
        self.parts(member.object())
            .into_iter()
            .map(|(place, _)| (place, false))
            .collect()
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
        let root = self.scoping.root_scope_id();
        let function = self.functions.last().copied().unwrap_or(root);
        if function != root {
            self.function_writes.push((symbol, function, change.clone()));
        }
        if function != self.function_of(symbol) {
            self.change(symbol, change);
        }
    }

    /// Whether a link made now is made by code the module runs when it is loaded ([`Link::loading`]).
    fn loading(&self) -> bool {
        !self.defining && self.functions.last() == Some(&self.scoping.root_scope_id())
    }

    /// Makes the links `visit` makes as those of the definition of a function when `defining` ([`Self::defining`]).
    fn in_definition(&mut self, defining: bool, visit: impl FnOnce(&mut Self)) {
        let saved = std::mem::replace(&mut self.defining, defining);
        visit(self);
        self.defining = saved;
    }

    /// Records a change of what the object `expr` holds, in each part of a local it may be ([`Self::parts`]).
    fn mutate(&mut self, expr: &Expression<'_>) {
        for (place, _) in self.parts(expr) {
            self.changed(place.symbol, Write::contents(place.keys));
        }
    }

    /// Records the change `call` makes of the object `target` it is given or called on ([`changed_by_call`]): a new
    /// value of the key it defines on a part of a local ([`property_defined`]), or a change of what that object holds.
    fn change_target(&mut self, call: &CallExpression<'_>, target: &Expression<'_>) {
        if is_namespace(self.names(), target) {
            return;
        }
        match (property_defined(call), self.place(target)) {
            (Some(key), Some((place, true))) => self.changed(
                place.symbol,
                Write {
                    keys: place.keys,
                    key: Some(key),
                },
            ),
            _ => self.mutate(target),
        }
    }

    /// Visits what `visit` visits as code in a loop.
    fn in_loop(&mut self, visit: impl FnOnce(&mut Self)) {
        self.loops += 1;
        visit(self);
        self.loops -= 1;
    }

    /// Records a change of each object a call is given, of what a spread argument holds, and of each part of a local
    /// a literal argument holds ([`given_parts`]).
    fn mutate_arguments(&mut self, arguments: &[Argument<'_>]) {
        for argument in arguments.iter().filter_map(given) {
            for (id, keys) in given_parts(argument) {
                if let Some(symbol) = self.symbol(id) {
                    self.changed(symbol, Write::contents(keys));
                }
            }
        }
    }

    /// Links the parameters of each function given inline to a call (`list.forEach((item) => …)`) to what the object
    /// the call is made on and its arguments hold, which the function may be called with. Over one of
    /// [`Self::primitive_lists`], the first parameter takes an element, linked only once a change may reach the list,
    /// and those after it and a rest parameter (the index, and the list itself for `forEach`, `map` or `reduce`) may
    /// take the list.
    fn callbacks(&mut self, callee: &Expression<'_>, arguments: &[Argument<'_>]) {
        let functions: Vec<Parameters> = arguments
            .iter()
            .filter_map(Argument::as_expression)
            .filter_map(|argument| match argument.get_inner_expression() {
                Expression::ArrowFunctionExpression(f) => Some(Parameters::of(&f.params)),
                Expression::FunctionExpression(f) => Some(Parameters::of(&f.params)),
                _ => None,
            })
            .collect();
        if functions.iter().all(|parameters| parameters.names().next().is_none()) {
            return;
        }
        let receiver = callee.as_member_expression().map(MemberExpression::object);
        // The elements of a list of primitive values share no object unless code changes the list.
        let list = receiver.and_then(|receiver| self.primitive_source(receiver));
        let of_receiver = receiver.map_or_else(Vec::new, |receiver| self.receiver_refs(receiver));
        let mut held = if list.is_some() {
            Vec::new()
        } else {
            of_receiver.clone()
        };
        for argument in arguments {
            held.extend(self.refs(Part::Value, |refs| refs.visit_argument(argument)));
        }
        for parameters in functions {
            let positions = parameters
                .items
                .iter()
                .enumerate()
                .flat_map(|(position, (names, _))| names.iter().map(move |&name| (name, position == 0)))
                .chain(parameters.rest.iter().map(|&name| (name, false)));
            let names: Vec<(SymbolId, bool)> = positions.collect();
            for (name, first) in names {
                if let Some(list) = list {
                    if first {
                        let loading = self.loading();
                        self.element_links.push(Link {
                            a: Place::of(name),
                            b: Place::of(list),
                            join: Join::Within,
                            loading,
                        });
                    } else {
                        self.hold(&Place::of(name), false, of_receiver.clone());
                    }
                }
                self.hold(&Place::of(name), false, held.clone());
            }
        }
    }

    /// The parts of locals the object a method is called on may hold, which the method may keep what it is given in or
    /// return: none for the binding of a package the module only calls methods on ([`Known::inert`]: `_.merge(…)`,
    /// `vitest.defineConfig(…)`), whose methods are taken to keep nothing of what they are given in the package, so
    /// that calls of them share nothing through it.
    fn receiver_refs(&self, receiver: &Expression<'_>) -> Vec<Held> {
        if is_inert_package(self.scoping, &self.known, receiver) {
            return Vec::new();
        }
        self.refs(Part::Value, |refs| refs.visit_expression(receiver))
    }

    /// The one of [`Self::primitive_lists`] whose elements or values the elements of `expr` are: the list or object
    /// itself, or what `Object.entries`, `Object.values` or `Object.keys` makes of it, whose elements are its keys and
    /// values or pairs of them.
    fn primitive_source(&self, expr: &Expression<'_>) -> Option<SymbolId> {
        let source = match expr.get_inner_expression() {
            Expression::CallExpression(call) if lists_entries(self.names(), call) => {
                call.arguments.first()?.as_expression()?
            }
            other => other,
        };
        match source.get_inner_expression() {
            Expression::Identifier(id) => self.symbol(id).filter(|symbol| self.primitive_lists.contains(symbol)),
            _ => None,
        }
    }

    /// The parts of locals an assignment target writes (`x`, `{ a: cfg.b }`, `[y = d]`), and the defaults it may
    /// write instead of what it is given.
    fn target_parts<'b, 'a>(
        &self,
        target: &'b AssignmentTarget<'a>,
        places: &mut Vec<Place>,
        defaults: &mut Vec<&'b Expression<'a>>,
    ) {
        if let Some(simple) = target.as_simple_assignment_target() {
            let place = match simple {
                SimpleAssignmentTarget::AssignmentTargetIdentifier(id) => self.symbol(id).map(Place::of),
                other => other
                    .as_member_expression()
                    .and_then(|member| self.member_place(member))
                    .map(|(place, _)| place),
            };
            places.extend(place);
            return;
        }
        match target.as_assignment_target_pattern() {
            Some(AssignmentTargetPattern::ArrayAssignmentTarget(array)) => {
                for element in array.elements.iter().flatten() {
                    self.default_target_parts(element, places, defaults);
                }
                if let Some(rest) = &array.rest {
                    self.target_parts(&rest.target, places, defaults);
                }
            }
            Some(AssignmentTargetPattern::ObjectAssignmentTarget(object)) => {
                for property in &object.properties {
                    match property {
                        AssignmentTargetProperty::AssignmentTargetPropertyIdentifier(p) => {
                            places.extend(self.symbol(&p.binding).map(Place::of));
                            defaults.extend(&p.init);
                        }
                        AssignmentTargetProperty::AssignmentTargetPropertyProperty(p) => {
                            self.default_target_parts(&p.binding, places, defaults);
                        }
                    }
                }
                if let Some(rest) = &object.rest {
                    self.target_parts(&rest.target, places, defaults);
                }
            }
            None => {}
        }
    }

    /// [`Self::target_parts`] of a target that may have a default.
    fn default_target_parts<'b, 'a>(
        &self,
        target: &'b AssignmentTargetMaybeDefault<'a>,
        places: &mut Vec<Place>,
        defaults: &mut Vec<&'b Expression<'a>>,
    ) {
        match target {
            AssignmentTargetMaybeDefault::AssignmentTargetWithDefault(with) => {
                defaults.push(&with.init);
                self.target_parts(&with.binding, places, defaults);
            }
            other => {
                if let Some(target) = other.as_assignment_target() {
                    self.target_parts(target, places, defaults);
                }
            }
        }
    }

    /// Links each part a destructuring target writes to what `held` holds, or its defaults hold, somewhere at or below
    /// it, as the part it takes is not tracked; with no part, what `held` holds is linked among itself.
    fn destructure(&mut self, target: &AssignmentTarget<'_>, mut held: Vec<Held>) {
        let mut places = Vec::new();
        let mut defaults = Vec::new();
        self.target_parts(target, &mut places, &mut defaults);
        if places.is_empty() {
            return self.link_among(&held);
        }
        for default in defaults {
            held.extend(self.refs(Part::Value, |refs| refs.visit_expression(default)));
        }
        for place in places {
            self.hold(&place, false, held.clone());
        }
    }

    /// What `call` does to the objects it is given, when the pass knows ([`Effect`]); `None` when it may change them.
    fn effect(&self, call: &CallExpression<'_>) -> Option<Effect> {
        if calls_callable(self.scoping, &self.callables, call) {
            return Some(Effect::Followed);
        }
        let names = self.names();
        let keeps = pure_function(names, call).is_some()
            || self.keeps_apart(call)
            || assigns(names, call)
            || (!self.calls_back_unknown(call)
                && matches!(
                    call.callee.get_inner_expression(),
                    Expression::StaticMemberExpression(m)
                        if (PURE_METHODS.contains(&m.property.name.as_str())
                            || LIST_ADDERS.contains(&m.property.name.as_str())
                            || OWN_RESULT_METHODS.contains(&m.property.name.as_str()))
                            && builtin(names, &m.object)
                ));
        keeps.then_some(Effect::Keeps)
    }

    /// Whether `call` is a method of [`CALLBACK_METHODS`] that may call code the pass does not follow with what the
    /// object it is called on holds, or with an object it is given as `this`: its callback is neither a function
    /// written in place nor one the module declares, whose changes of its parameters are followed, or an argument
    /// after the callback is the callback's `this`.
    fn calls_back_unknown(&self, call: &CallExpression<'_>) -> bool {
        let Expression::StaticMemberExpression(m) = call.callee.get_inner_expression() else {
            return false;
        };
        let method = m.property.name.as_str();
        if !CALLBACK_METHODS.contains(&method) {
            return false;
        }
        let followed = call.arguments.first().is_none_or(|argument| {
            is_inline_function(argument)
                || matches!(
                    argument,
                    Argument::Identifier(id)
                        if local_symbol(self.scoping, id).is_some_and(|symbol| self.callables.contains(symbol))
                )
        });
        let with_this = !matches!(method, "reduce" | "reduceRight" | "toSorted") && call.arguments.len() > 1;
        !followed || with_this
    }

    /// Whether `call` changes none of the objects it is given and keeps none of them in another or in what it is called
    /// on: a `define…` helper of a package the evaluator does not follow ([`foreign`]), which returns what it is given,
    /// or a merge that builds a new object ([`copies`]).
    fn keeps_apart(&self, call: &CallExpression<'_>) -> bool {
        let names = self.names();
        (defined(call).is_some() && foreign(names, &call.callee)) || copies(names, call)
    }

    /// Whether `expr` is a name or a member chain that starts from a binding of a module the evaluator follows whose
    /// functions may change a value of a module ([`Imports::changes_modules`]): `tweak`, `shared.tweak`,
    /// `base.plugins`.
    fn of_changing_module(&self, expr: &Expression<'_>) -> bool {
        reach(expr)
            .and_then(|(id, _, _)| self.symbol(id))
            .filter(|symbol| self.known.followed.contains(symbol))
            .and_then(|symbol| self.known.modules.get(&symbol))
            .is_some_and(|(module, _)| self.imports.changes_modules(module))
    }

    /// Whether `call` may run code of a module the evaluator follows that changes a value of a module: it calls a
    /// function of one (`tweak()`, `shared.tweak()`, a method of an imported value), or its effect is not known and
    /// it is given a binding of one, which may be a function it calls.
    fn runs_followed(&self, call: &CallExpression<'_>) -> bool {
        let given_function = || {
            self.effect(call).is_none()
                && call.arguments.iter().filter_map(given).any(|argument| {
                    matches!(argument.get_inner_expression(), Expression::Identifier(_))
                        && self.of_changing_module(argument)
                })
        };
        self.of_changing_module(&call.callee) || given_function()
    }

    /// Records the change `write` of the object `symbol` holds.
    fn changed(&mut self, symbol: SymbolId, write: Write) {
        self.written(symbol, Change::Contents(vec![write.clone()]));
        self.mutated.push((symbol, write));
    }

    /// Links `hub` to each part a value it holds may hold: at the place in the value it is found, as it is found
    /// there, when `exact`, and anywhere at or below the hub otherwise.
    fn hold(&mut self, hub: &Place, exact: bool, held: Vec<Held>) {
        let loading = self.loading();
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
                join: if exact { item.join } else { Join::Within },
                loading,
            });
        }
    }

    /// Links the parts that values with no binding to hold them may share among themselves (`f(a, b)`).
    fn link_among(&mut self, held: &[Held]) {
        let loading = self.loading();
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
                join: Join::Within,
                loading,
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

    /// The aliases a `const` declaration of `pattern` with `init` declares, each with the keys of the part it takes,
    /// and the local they take a part of: `None` unless the local is one of the function being visited
    /// ([`alias_source`], [`alias_names`]).
    fn alias(&self, pattern: &BindingPattern<'_>, init: &Expression<'_>) -> Option<(Vec<AliasPart>, SymbolId)> {
        if !self.in_const {
            return None;
        }
        let (source, keys) = alias_source(init)?;
        let root = self.symbol(source)?;
        let function = *self.functions.last()?;
        if function == self.scoping.root_scope_id() || self.function_of(root) != function {
            return None;
        }
        let names = alias_names(pattern)?
            .into_iter()
            .map(|(id, key)| {
                let mut part = keys.clone();
                part.extend(key);
                id.symbol_id.get().map(|symbol| (symbol, part))
            })
            .collect::<Option<Vec<_>>>()?;
        Some((names, root))
    }

    /// Whether `reference` is read before the offset `from` each time its binding's function runs, and not again
    /// after: it ends before `from`, outside a loop and a nested function, in a module that reaches no binding
    /// without a reference.
    fn read_before(&self, reference: ReferenceId, from: u32) -> bool {
        !self.dynamic
            && !self.repeated.contains(&reference)
            && self.ends.get(&reference).is_some_and(|&end| end <= from)
    }

    /// The pairs of bindings the links join, whatever the parts.
    fn pairs(&self) -> Vec<(SymbolId, SymbolId)> {
        self.links.iter().map(|link| (link.a.symbol, link.b.symbol)).collect()
    }

    /// Whether the evaluator follows the alias at `index` of [`Self::aliases`], given the union-find forest of the links
    /// made so far; see [`Unstable::follows`].
    fn follows(&self, index: usize, parent: &mut HashMap<SymbolId, SymbolId>) -> bool {
        let (alias, root) = self.aliases[index];
        self.aliases.iter().filter(|(_, other)| *other == root).count() == 1
            && !self.scoping.symbol_is_mutated(root)
            && !self.changes.contains_key(&alias)
            && !self.changes.contains_key(&root)
            && !self.mutated.iter().any(|(symbol, _)| *symbol == root)
            && !self.shares_with_changed(parent, alias, &[alias])
            && !self.shares_with_changed(parent, root, &[alias, root])
    }

    /// Whether `symbol` may share an object, through the links of the union-find forest `parent`, with a binding other
    /// than those of `except` whose object or array code changes.
    fn shares_with_changed(
        &self,
        parent: &mut HashMap<SymbolId, SymbolId>,
        symbol: SymbolId,
        except: &[SymbolId],
    ) -> bool {
        let class = find(parent, symbol);
        self.mutated
            .iter()
            .any(|&(other, _)| !except.contains(&other) && find(parent, other) == class)
    }

    /// Links every alias the evaluator does not follow to the local it takes a part of, until no more is linked (a
    /// link may make another alias one that shares an object), and returns the followed ones. Such a link stands for
    /// the whole local, as it is not tracked which part the alias takes.
    fn link_aliases(&mut self) -> HashSet<SymbolId> {
        let mut linked = vec![false; self.aliases.len()];
        loop {
            let mut parent = forest(&self.pairs());
            let unfollowed: Vec<usize> = (0..self.aliases.len())
                .filter(|&index| !linked[index] && !self.follows(index, &mut parent))
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
                    join: Join::Within,
                    loading: false,
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

    /// Links the parameters of the callbacks given to a method of one of [`Self::primitive_lists`] to that list when a
    /// change may reach it, until no more is linked: its elements may then be objects that a change of a parameter
    /// changes. Each alias counts as linked to the local it takes a part of, as which the evaluator follows is not
    /// known yet. In a module that calls `eval`, reads `arguments` or has a `with` statement, every such link is made.
    fn link_elements(&mut self) {
        let mut pending = std::mem::take(&mut self.element_links);
        if self.dynamic {
            return self.links.append(&mut pending);
        }
        while !pending.is_empty() {
            let base = self.links.len();
            self.links.extend(self.aliases.iter().map(|&(alias, root)| Link {
                a: Place::of(alias),
                b: Place::of(root),
                join: Join::Within,
                loading: false,
            }));
            let links_of = self.links_of();
            let reached: HashSet<SymbolId> = self
                .mutated
                .iter()
                .flat_map(|(symbol, write)| self.reached(&links_of, *symbol, write))
                .map(|(symbol, _)| symbol)
                .collect();
            self.links.truncate(base);
            let (linked, rest): (Vec<Link>, Vec<Link>) =
                pending.into_iter().partition(|link| reached.contains(&link.b.symbol));
            pending = rest;
            if linked.is_empty() {
                break;
            }
            self.links.extend(linked);
        }
    }

    /// Drops the links and changes of bindings that hold a primitive value, which share no object.
    fn drop_primitives(&mut self) {
        let primitives = &self.primitives;
        self.links
            .retain(|link| !primitives.contains(&link.a.symbol) && !primitives.contains(&link.b.symbol));
        self.mutated.retain(|(symbol, _)| !primitives.contains(symbol));
        self.changes.retain(|symbol, _| !primitives.contains(symbol));
        self.function_writes
            .retain(|(symbol, _, _)| !primitives.contains(symbol));
    }

    /// The indices of the links of each binding.
    fn links_of(&self) -> HashMap<SymbolId, Vec<usize>> {
        self.links_of_where(|_| true)
    }

    /// The indices of the links `keep` keeps of each binding.
    fn links_of_where(&self, keep: impl Fn(&Link) -> bool) -> HashMap<SymbolId, Vec<usize>> {
        let mut out: HashMap<SymbolId, Vec<usize>> = HashMap::default();
        for (index, link) in self.links.iter().enumerate().filter(|(_, link)| keep(*link)) {
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
                    let Some(mut next) = through(from, to, link.join, &write) else {
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

    /// Marks every part of a binding that may share an object with a changed one, other than the part of the binding
    /// the change is made through, which the evaluator changes itself, and returns the aliases the evaluator follows.
    /// A change through one of those reaches the part of the local it takes, which the evaluator changes too and which
    /// is not marked, and through it what else may share an object with that part; a local of the alias's function is
    /// marked from the alias's declaration on ([`Self::late`]). Returns with them whether a function of the module may
    /// change a value of the module ([`Self::writes_modules`]).
    fn share(&mut self) -> (HashSet<SymbolId>, bool) {
        self.link_imports();
        self.drop_primitives();
        self.link_elements();
        let followed = self.link_aliases();
        let applied = self.applied_aliases(&followed);
        for alias in &applied {
            self.links.push(Link {
                a: Place::of(alias.alias),
                b: Place {
                    symbol: alias.root,
                    keys: alias.keys.clone(),
                },
                join: Join::Same,
                loading: false,
            });
        }
        let mut origins: Vec<(SymbolId, Write)> = Vec::new();
        for origin in &self.mutated {
            if !origins.contains(origin) {
                origins.push(origin.clone());
            }
        }
        let links_of = self.links_of();
        for (symbol, origin) in origins {
            let through_alias = applied.iter().find(|alias| alias.alias == symbol);
            for (other, write) in self.reached(&links_of, symbol, &origin) {
                // The evaluator applies the change to the part of the binding it is made through, not to another part
                // of it that may hold the same object (`config.b = config.a`).
                if other == symbol && origin.covers(&write) {
                    continue;
                }
                let change = Change::Contents(vec![write.clone()]);
                match through_alias {
                    Some(alias) if other == alias.root && alias.applied(&origin).covers(&write) => {}
                    Some(alias) if self.function_of(other) == self.function_of(symbol) => {
                        self.late.push((other, alias.from, change));
                    }
                    None | Some(_) => self.change(other, change),
                }
            }
        }
        let writes_modules = self.writes_modules(&links_of);
        if self.runs_modules {
            self.affect_imports(&links_of);
        }
        (followed, writes_modules)
    }

    /// Whether code in a function of the module, run by a call of it, may change a value of the module: a top-level
    /// binding or an import, or an object one of them may hold. A change counts when it is made in a function
    /// ([`Self::function_writes`]) and is one of such a binding, or reaches one through the links (see
    /// [`Self::reached`]). A change of a binding of the function it is made in (a local, a parameter) or of what a
    /// function of the module returns is one of an object made or given anew by each call, so it is followed only
    /// through the links that hold for every call ([`Self::fresh_reach_modules`]), not those code run when the module
    /// is loaded made of what one call made (`export const config = create()`): a factory that builds and returns new
    /// objects changes none, and a change of what a parameter takes is one of what a call gives it, which the call
    /// accounts for. A change of a binding of an enclosing function, which a function made by an earlier call may make
    /// later, is followed through every link ([`Self::closure_reaches_modules`]). In a module that calls `eval`, reads
    /// `arguments` or has a `with` statement, any change in a function counts.
    fn writes_modules(&self, links_of: &HashMap<SymbolId, Vec<usize>>) -> bool {
        if self.function_writes.is_empty() {
            return false;
        }
        if self.dynamic {
            return true;
        }
        let fresh_links_of = self.links_of_where(|link| !link.loading);
        let root = self.scoping.root_scope_id();
        self.function_writes.iter().any(|(symbol, function, change)| {
            let home = self.function_of(*symbol);
            let fresh = home == *function || (home == root && self.callables.contains(*symbol));
            match change {
                Change::Any => home != *function,
                Change::Contents(writes) => writes.iter().any(|write| {
                    if fresh {
                        self.fresh_reach_modules(links_of, &fresh_links_of, *symbol, *function, write)
                    } else {
                        self.closure_reaches_modules(links_of, *symbol, write)
                    }
                }),
            }
        })
    }

    /// Whether the change `write` of `symbol`, an object a call of the function of the scope `function` makes or is
    /// given anew, reaches through the links that hold for every call (`fresh_links_of`) a value of the module
    /// ([`Self::is_module_value`]), or a binding of a function enclosing that one whose change does
    /// ([`Self::closure_reaches_modules`], through every link, `links_of`).
    fn fresh_reach_modules(
        &self,
        links_of: &HashMap<SymbolId, Vec<usize>>,
        fresh_links_of: &HashMap<SymbolId, Vec<usize>>,
        symbol: SymbolId,
        function: ScopeId,
        write: &Write,
    ) -> bool {
        self.reached(fresh_links_of, symbol, write)
            .iter()
            .any(|(other, change)| {
                self.is_module_value(*other)
                    || (self.encloses(*other, function) && self.closure_reaches_modules(links_of, *other, change))
            })
    }

    /// Whether the change `write` of `symbol` reaches through every link (`links_of`) a value of the module
    /// ([`Self::is_module_value`]).
    fn closure_reaches_modules(
        &self,
        links_of: &HashMap<SymbolId, Vec<usize>>,
        symbol: SymbolId,
        write: &Write,
    ) -> bool {
        self.reached(links_of, symbol, write)
            .iter()
            .any(|(other, _)| self.is_module_value(*other))
    }

    /// Whether `symbol` is a value of the module a change may reach from a call: a top-level binding or an import, but
    /// a function the module declares, which stands for what a call of it returns and what its parameters take.
    fn is_module_value(&self, symbol: SymbolId) -> bool {
        self.function_of(symbol) == self.scoping.root_scope_id() && !self.callables.contains(symbol)
    }

    /// Whether `symbol` is a binding of a function that encloses the function of the scope `function`.
    fn encloses(&self, symbol: SymbolId, function: ScopeId) -> bool {
        let home = self.function_of(symbol);
        home != function
            && home != self.scoping.root_scope_id()
            && self.scoping.scope_ancestors(function).any(|scope| scope == home)
    }

    /// Links every binding of a module the evaluator follows to the others, as two exports, of one module or of two,
    /// may be the same object or hold one another: one that is a part of another (the default import and `default` of
    /// a namespace import of the same module, two imports of one export) at that part, any other anywhere at or below
    /// it.
    fn link_imports(&mut self) {
        let mut imports: Vec<SymbolId> = self.known.followed.iter().copied().collect();
        imports.sort_unstable();
        for pair in imports.windows(2) {
            self.links.push(Link {
                a: Place::of(pair[0]),
                b: Place::of(pair[1]),
                join: Join::Within,
                loading: false,
            });
        }
        for (index, &a) in imports.iter().enumerate() {
            for &b in &imports[index + 1..] {
                if let Some((a, b)) = self.export_part(a, b).or_else(|| self.export_part(b, a)) {
                    self.links.push(Link {
                        a,
                        b,
                        join: Join::Same,
                        loading: false,
                    });
                }
            }
        }
    }

    /// The place in the import `whole` that the import `part` of the same module is, with `part` itself, when the
    /// export `part` takes is `whole`'s or below it (`default` of a namespace import is the default import).
    fn export_part(&self, whole: SymbolId, part: SymbolId) -> Option<(Place, Place)> {
        let (whole_module, _) = self.known.modules.get(&whole)?;
        let (part_module, _) = self.known.modules.get(&part)?;
        let whole_path = self.known.import_paths.get(&whole)?;
        let below = self
            .known
            .import_paths
            .get(&part)?
            .strip_prefix(whole_path.as_slice())?;
        (whole_module == part_module).then(|| {
            (
                Place {
                    symbol: whole,
                    keys: below.to_vec(),
                },
                Place::of(part),
            )
        })
    }

    /// Marks every binding of a module the evaluator follows, and what may share an object with one, as changed in
    /// full, as code of such a module may run unseen. Where it runs is not taken to bound what it changes: what a read
    /// before it gives the config may be the object it changes.
    fn affect_imports(&mut self, links_of: &HashMap<SymbolId, Vec<usize>>) {
        let mut imports: Vec<SymbolId> = self.known.followed.iter().copied().collect();
        imports.sort_unstable();
        for import in imports {
            for (other, write) in self.reached(links_of, import, &Write::default()) {
                self.change(other, Change::Contents(vec![write]));
            }
        }
    }

    /// The aliases of `followed`, in the order they are declared.
    fn applied_aliases(&self, followed: &HashSet<SymbolId>) -> Vec<AppliedAlias> {
        self.aliases
            .iter()
            .filter(|(alias, _)| followed.contains(alias))
            .filter_map(|&(alias, root)| {
                let (keys, from) = self.alias_parts.get(&alias)?;
                Some(AppliedAlias {
                    alias,
                    root,
                    keys: keys.clone(),
                    from: *from,
                })
            })
            .collect()
    }
}

/// The change of `to` that the change `write` of the binding of `from` makes through a link of `from` to `to`, joined
/// as `join` says: as far below `to` as `write` is below `from`, when `write` is at or below `from` and the link is
/// [`Join::Same`] or [`Join::Copy`], but for a new value of a key of the object at `from` itself through a copy, which
/// the other does not see; of all of `to` when the link is [`Join::Within`] and `write` is at or below `from`, or when
/// `write` changes what an object above `from` holds; none when they are apart, or when `from` is below a key `write`
/// gives a new value, which leaves the value it held as it was.
fn through(from: &Place, to: &Place, join: Join, write: &Write) -> Option<Write> {
    if write.keys.starts_with(&from.keys) {
        let below = &write.keys[from.keys.len()..];
        return match join {
            Join::Within => Some(Write::contents(to.keys.clone())),
            Join::Copy if below.is_empty() && write.key.is_some() => None,
            Join::Same | Join::Copy => {
                let mut keys = to.keys.clone();
                keys.extend_from_slice(below);
                Some(Write {
                    keys,
                    key: write.key.clone(),
                })
            }
        };
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
            self.in_definition(true, |sharing| sharing.hold(&Place::of(symbol), true, held));
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

    fn visit_class(&mut self, it: &Class<'a>) {
        // A static block or a field initialiser runs when the class is defined or an object of it is made, which the
        // evaluator does not follow: a write there is one of no function the binding belongs to.
        let Some(scope) = it.scope_id.get() else {
            return walk::walk_class(self, it);
        };
        self.functions.push(scope);
        walk::walk_class(self, it);
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
            for (name, keys) in names {
                self.aliases.push((name, root));
                self.alias_parts.insert(name, (keys, it.span.end));
            }
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
            if whole
                && self.in_const
                && (is_primitive_list(self.names(), init) || is_primitive_record(self.names(), init))
            {
                self.primitive_lists.extend(names.iter().copied());
            }
            self.in_definition(function, |sharing| {
                if names.is_empty() {
                    sharing.link_among(&held);
                }
                for name in names {
                    sharing.hold(&Place::of(name), whole, held.clone());
                }
            });
        }
        walk::walk_variable_declarator(self, it);
    }

    fn visit_assignment_expression(&mut self, it: &AssignmentExpression<'a>) {
        // A function assigned stands for what its parameters take as well as what it returns, as one declared does.
        let part = if matches!(
            it.right.get_inner_expression(),
            Expression::ArrowFunctionExpression(_) | Expression::FunctionExpression(_)
        ) {
            Part::Function { parameters: true }
        } else {
            Part::Value
        };
        let held = self.refs(part, |refs| refs.visit_expression(&it.right));
        // What the target is written with may be shared with it; the target itself is visited by the walk. A value
        // stored where no local holds it (`module.exports = [a(), b()]`, a global) is not changed by being stored, and
        // no change through that place is followed, so its parts share nothing among themselves there.
        match it.left.as_simple_assignment_target() {
            Some(target) => {
                let hubs: Vec<(Place, bool)> = match target {
                    SimpleAssignmentTarget::AssignmentTargetIdentifier(id) => self
                        .symbol(id)
                        .map(|symbol| (Place::of(symbol), true))
                        .into_iter()
                        .collect(),
                    other => other
                        .as_member_expression()
                        .map(|member| self.member_places(member))
                        .unwrap_or_default(),
                };
                let function = matches!(part, Part::Function { .. });
                self.in_definition(function, |sharing| {
                    for (place, exact) in hubs {
                        sharing.hold(&place, exact, held.clone());
                    }
                });
            }
            None => self.destructure(&it.left, held),
        }
        walk::walk_assignment_expression(self, it);
    }

    fn visit_for_of_statement(&mut self, it: &ForOfStatement<'a>) {
        // The loop's names take the elements of what it iterates, somewhere at or below it; over a list or object of
        // primitive values, only once a change may reach it ([`Self::link_elements`]).
        let held = self.refs(Part::Value, |refs| refs.visit_expression(&it.right));
        let source = self.primitive_source(&it.right);
        match &it.left {
            ForStatementLeft::VariableDeclaration(declaration) => {
                for declarator in &declaration.declarations {
                    for name in bound_symbols(&declarator.id) {
                        match source {
                            Some(list) => {
                                let loading = self.loading();
                                self.element_links.push(Link {
                                    a: Place::of(name),
                                    b: Place::of(list),
                                    join: Join::Within,
                                    loading,
                                });
                            }
                            None => self.hold(&Place::of(name), false, held.clone()),
                        }
                    }
                }
            }
            left => {
                if let Some(target) = left.as_assignment_target() {
                    self.destructure(target, held);
                }
            }
        }
        self.in_loop(|sharing| walk::walk_for_of_statement(sharing, it));
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
        if foreign(self.names(), &it.callee) {
            self.foreign_calls.insert(it.span);
        }
        self.runs_modules |= self.runs_followed(it);
        let calls_back = self.calls_back_unknown(it);
        if calls_back {
            self.calls_back.insert(it.span);
        }
        // A call whose effect the pass does not know may change what it is given, wherever it is made.
        match self.effect(it) {
            Some(effect) => {
                self.effects.insert(it.span, effect);
            }
            None => self.mutate_arguments(&it.arguments),
        }
        self.callbacks(it.callee.get_inner_expression(), &it.arguments);
        let receiver = it
            .callee
            .get_inner_expression()
            .as_member_expression()
            .map(MemberExpression::object);
        if pure_function(self.names(), it).is_some() {
            self.pure.insert(it.span);
            return walk::walk_call_expression(self, it);
        }
        // A call that keeps what it is given apart shares none of it with its callee, nor its arguments among
        // themselves: what it returns holds them only as the value holding the call says.
        if self.keeps_apart(it) {
            return walk::walk_call_expression(self, it);
        }
        // A function the module declares passes each argument to the parameter that takes it when they are kept apart,
        // and stands for its parameters otherwise; any other function, and a method in its object, may keep what it
        // is given anywhere.
        let hub = match it.callee.get_inner_expression() {
            Expression::Identifier(id) => {
                let symbol = local_symbol(self.scoping, id);
                if let Some(parameters) = symbol.and_then(|symbol| self.callables.apart.get(&symbol)).cloned() {
                    self.pass(&parameters, it);
                    return walk::walk_call_expression(self, it);
                }
                let exact = calls_callable(self.scoping, &self.callables, it);
                // A function value the pass does not follow (one a call made, `aliaser(config)`) may change what it
                // holds or captures, which it shares with what it was made from. One a module exports is taken to
                // change none of the module's locals.
                if !exact && let Some(symbol) = symbol.filter(|symbol| !self.known.modules.contains_key(symbol)) {
                    self.called_values.insert(it.span);
                    self.changed(symbol, Write::default());
                }
                symbol.map(|symbol| (Place::of(symbol), exact))
            }
            callee => {
                if let Some(target) = changed_by_call(it, false, calls_back) {
                    self.change_target(it, target);
                }
                callee
                    .as_member_expression()
                    .filter(|member| !is_inert_package(self.scoping, &self.known, member.object()))
                    .and_then(|member| self.place(member.object()))
                    .map(|(place, _)| (place, false))
            }
        };
        // A method that keeps nothing it is given returns what a callback returns, and calls it with what the object
        // holds ([`Self::callbacks`]).
        let keeps_callbacks = pure_method(self.names(), it);
        let mut held: Vec<Held> = it
            .arguments
            .iter()
            .filter(|argument| !(keeps_callbacks && is_inline_function(argument)))
            .flat_map(|argument| self.refs(Part::Value, |refs| refs.visit_argument(argument)))
            .collect();
        // A method of an object that is not a local's part may keep what it is given in what that object holds.
        if hub.is_none()
            && let Some(receiver) = receiver
        {
            held.extend(self.receiver_refs(receiver));
        }
        self.hold_or_link(hub, held);
        walk::walk_call_expression(self, it);
    }

    fn visit_new_expression(&mut self, it: &NewExpression<'a>) {
        self.runs_modules |= self.of_changing_module(&it.callee);
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
        // A module the evaluator follows required for its effects (`require('./setup')`) runs its code there.
        if let Some(call) = statement_call(&it.expression)
            && let Expression::Identifier(callee) = call.callee.get_inner_expression()
            && callee.name.as_str() == "require"
            && resolved(self.scoping, callee).is_none()
            && call
                .arguments
                .first()
                .and_then(Argument::as_expression)
                .and_then(static_string)
                .is_some_and(|specifier| self.imports.follows(&specifier))
        {
            self.runs_modules = true;
        }
        walk::walk_expression_statement(self, it);
    }

    fn visit_tagged_template_expression(&mut self, it: &TaggedTemplateExpression<'a>) {
        self.runs_modules |= self.of_changing_module(&it.tag);
        walk::walk_tagged_template_expression(self, it);
    }

    fn visit_import_declaration(&mut self, it: &ImportDeclaration<'a>) {
        // A module imported for its effects only (`import './setup'`) runs before the module that imports it.
        let effects_only = it.specifiers.as_ref().is_none_or(|specifiers| specifiers.is_empty());
        if effects_only && !it.import_kind.is_type() && self.imports.follows(&crate::js_string(it.source.value)) {
            self.runs_modules = true;
        }
        walk::walk_import_declaration(self, it);
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
        if let Some(reference) = it.reference_id.get() {
            self.ends.insert(reference, it.span.end);
        }
        let elsewhere = self
            .symbol(it)
            .is_some_and(|symbol| self.functions.last() != Some(&self.function_of(symbol)));
        if (self.loops > 0 || elsewhere)
            && let Some(reference) = it.reference_id.get()
        {
            self.repeated.insert(reference);
        }
    }

    fn visit_for_statement(&mut self, it: &ForStatement<'a>) {
        self.in_loop(|sharing| walk::walk_for_statement(sharing, it));
    }

    fn visit_with_statement(&mut self, it: &WithStatement<'a>) {
        // A name the body reads or writes may be a key of the object.
        self.dynamic = true;
        self.mutate(&it.object);
        walk::walk_with_statement(self, it);
    }

    fn visit_for_in_statement(&mut self, it: &ForInStatement<'a>) {
        self.in_loop(|sharing| walk::walk_for_in_statement(sharing, it));
    }

    fn visit_while_statement(&mut self, it: &WhileStatement<'a>) {
        self.in_loop(|sharing| walk::walk_while_statement(sharing, it));
    }

    fn visit_do_while_statement(&mut self, it: &DoWhileStatement<'a>) {
        self.in_loop(|sharing| walk::walk_do_while_statement(sharing, it));
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
                join: Join::of(exact),
            }));
    }

    /// Records the part of the local `id` at `keys`, which is the value here when `exact`.
    fn hold(&mut self, id: &IdentifierReference<'_>, keys: Vec<String>, exact: bool) {
        self.hold_as(id, keys, Join::of(self.exact && exact));
    }

    /// Records the part of the local `id` at `keys`, held here as `join` says.
    fn hold_as(&mut self, id: &IdentifierReference<'_>, keys: Vec<String>, join: Join) {
        if self.part == Part::Body {
            return;
        }
        if let Some(symbol) = resolved(self.scoping, id) {
            self.out.push(Held {
                symbol,
                keys,
                at: self.at.clone(),
                join,
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

    /// A spread in an object (`...value`): an object literal adds its keys where the spread is, as does each value a
    /// condition or `&&` may be (the left of `&&` is then a falsy value, which adds none); a part of a local
    /// (`...options`, `...cfg.test`) copies its own keys there ([`Join::Copy`]); anything else may add any key.
    fn spread<'a>(&mut self, value: &Expression<'a>)
    where
        Self: Visit<'a>,
    {
        match value.get_inner_expression() {
            Expression::ObjectExpression(object) => self.visit_object_expression(object),
            Expression::ConditionalExpression(c) => {
                self.spread(&c.consequent);
                self.spread(&c.alternate);
            }
            Expression::LogicalExpression(l) if l.operator == LogicalOperator::And => self.spread(&l.right),
            other => match reach(other) {
                Some((id, keys, true)) if self.exact && !self.frozen => self.hold_as(id, keys, Join::Copy),
                _ => self.opaque(|refs| refs.visit_expression(other)),
            },
        }
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
                    // `__proto__: base` makes `base` the object's prototype, so a change of a key the object inherits
                    // changes `base`, wherever the key is.
                    let key = p
                        .key
                        .static_name()
                        .filter(|_| !p.method && p.kind == PropertyKind::Init)
                        .map(|key| crate::js_string(key.as_js_str()))
                        .filter(|key| key != "__proto__");
                    match key {
                        Some(key) => self.at_key(key, |refs| refs.visit_expression(&p.value)),
                        None => self.opaque(|refs| {
                            refs.visit_property_key(&p.key);
                            refs.visit_expression(&p.value);
                        }),
                    }
                }
                ObjectPropertyKind::SpreadProperty(s) => self.spread(&s.argument),
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
            // Any other local function called by name stands for what it returns, which may hold what it is given; an
            // imported one holds none of the module's locals.
            Expression::Identifier(id) => {
                if local_symbol(self.scoping, id).is_some() {
                    self.hold(id, Vec::new(), true);
                }
            }
            // A method may return a part of the object it is called on (`list.find(…)`, `list.slice()`, `list.sort()`)
            // unless it returns a value of its own.
            Expression::StaticMemberExpression(member)
                if OWN_RESULT_METHODS.contains(&member.property.name.as_str()) && builtin(names, &member.object) => {}
            callee if callee.is_member_expression() => {
                if let Some(member) = callee.as_member_expression()
                    && !is_inert_package(self.scoping, self.known, member.object())
                {
                    let object = member.object();
                    self.opaque(|refs| refs.visit_expression(object));
                }
            }
            _ => self.opaque(|refs| refs.visit_expression(&it.callee)),
        }
        // A `define*` call of a package is the value it is given.
        let defined = defined(it).is_some() && foreign(names, &it.callee);
        for (index, argument) in it.arguments.iter().enumerate() {
            if defined && index == 0 {
                self.visit_argument(argument);
            } else {
                self.opaque(|refs| refs.visit_argument(argument));
            }
        }
    }

    fn visit_new_expression(&mut self, it: &NewExpression<'a>) {
        // An object of a class holds what it is given and what a local class holds; an imported class holds none of
        // the module's locals.
        self.opaque(|refs| {
            match it.callee.get_inner_expression() {
                Expression::Identifier(id) => {
                    if local_symbol(refs.scoping, id).is_some() {
                        refs.hold(id, Vec::new(), true);
                    }
                }
                callee => refs.visit_expression(callee),
            }
            for argument in &it.arguments {
                refs.visit_argument(argument);
            }
        });
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

/// What an argument gives a call: its value, or what a spread argument spreads (`args` in `f(...args)`), whose
/// elements the call is given.
pub(crate) fn given<'b, 'a>(argument: &'b Argument<'a>) -> Option<&'b Expression<'a>> {
    match argument {
        Argument::SpreadElement(spread) => Some(&spread.argument),
        other => other.as_expression(),
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

/// Whether `expr` is a list literal of values that hold no object ([`is_primitive`]), with no spread.
fn is_primitive_list(names: Names<'_>, expr: &Expression<'_>) -> bool {
    let Expression::ArrayExpression(array) = expr.get_inner_expression() else {
        return false;
    };
    array.elements.iter().all(|element| match element {
        ArrayExpressionElement::SpreadElement(_) => false,
        ArrayExpressionElement::Elision(_) => true,
        other => other.as_expression().is_some_and(|value| is_primitive(names, value)),
    })
}

/// Whether `expr` is an object literal of values that hold no object ([`is_primitive`]), with static keys other than
/// `__proto__` and no spread, method or accessor.
fn is_primitive_record(names: Names<'_>, expr: &Expression<'_>) -> bool {
    let Expression::ObjectExpression(object) = expr.get_inner_expression() else {
        return false;
    };
    object.properties.iter().all(|property| match property {
        ObjectPropertyKind::ObjectProperty(p) => {
            !p.method
                && p.kind == PropertyKind::Init
                && p.key
                    .static_name()
                    .is_some_and(|key| crate::js_string(key.as_js_str()) != "__proto__")
                && is_primitive(names, &p.value)
        }
        ObjectPropertyKind::SpreadProperty(_) => false,
    })
}

/// Whether `call` is `Object.entries`, `Object.values` or `Object.keys` of the global `Object`, whose result lists the
/// keys and values of the object it is given.
fn lists_entries(names: Names<'_>, call: &CallExpression<'_>) -> bool {
    matches!(
        call.callee.get_inner_expression(),
        Expression::StaticMemberExpression(m)
            if matches!(m.property.name.as_str(), "entries" | "values" | "keys")
                && matches!(
                    m.object.get_inner_expression(),
                    Expression::Identifier(object)
                        if object.name.as_str() == "Object" && resolved(names.scoping, object).is_none()
                )
    )
}

/// Whether `argument` is a function written in place (`(item) => …`, `function (item) { … }`).
fn is_inline_function(argument: &Argument<'_>) -> bool {
    argument.as_expression().is_some_and(|argument| {
        matches!(
            argument.get_inner_expression(),
            Expression::ArrowFunctionExpression(_) | Expression::FunctionExpression(_)
        )
    })
}

/// Whether `call` calls one of [`PURE_METHODS`] on an object known to have it ([`builtin`]), which keeps nothing it is
/// given.
fn pure_method(names: Names<'_>, call: &CallExpression<'_>) -> bool {
    matches!(
        call.callee.get_inner_expression(),
        Expression::StaticMemberExpression(m)
            if PURE_METHODS.contains(&m.property.name.as_str()) && builtin(names, &m.object)
    )
}

/// The functions of packages that build a new object and change none of the objects they are given, by package: `""`
/// for the module itself or its default export. Their other exports may change what they are given (`deepmergeInto`)
/// or call functions it holds (`defuFn`, `mergeWithCustomize`).
const COPYING_EXPORTS: &[(&str, &[&str])] = &[
    ("deepmerge", &[""]),
    ("deepmerge-ts", &["deepmerge"]),
    ("defu", &["", "defu"]),
    ("ts-deepmerge", &["", "merge"]),
    ("webpack-merge", &["", "merge"]),
];

/// Modules whose `mergeConfig` builds a new config and changes none of those it is given.
const MERGE_CONFIG_MODULES: &[&str] = &["vite", "vitest", "vitest/config"];

/// Globals the module does not declare that build a new object and change none of those they are given, by name.
const COPYING_GLOBALS: &[&str] = &["defu", "mergeConfig"];

/// Whether `call` calls a merge that builds a new object and changes none of the objects it is given: a function of
/// [`COPYING_EXPORTS`], the `mergeConfig` of Vite or Vitest, or one of [`COPYING_GLOBALS`], imported from a package
/// the evaluator does not follow ([`Known::foreign`]) or a global. The options `deepmerge`
/// takes after the objects it merges may hold functions it calls with them (`arrayMerge`, `customMerge`), so a call
/// given options counts only when they are an object literal of values that hold no function ([`function_free`]).
fn copies(names: Names<'_>, call: &CallExpression<'_>) -> bool {
    let Expression::Identifier(id) = call.callee.get_inner_expression() else {
        return false;
    };
    match resolved(names.scoping, id) {
        None => COPYING_GLOBALS.contains(&id.name.as_str()),
        Some(symbol) if !names.known.foreign.contains(&symbol) => false,
        Some(symbol) => names.known.modules.get(&symbol).is_some_and(|(module, imported)| {
            let export = imported.as_deref().unwrap_or_default();
            let copying = COPYING_EXPORTS
                .iter()
                .any(|(package, exports)| *package == module.as_str() && exports.contains(&export));
            let options_free = module != "deepmerge"
                || call.arguments.iter().skip(2).all(|argument| {
                    argument
                        .as_expression()
                        .is_some_and(|options| function_free(names, options))
                });
            (copying && options_free) || (export == "mergeConfig" && MERGE_CONFIG_MODULES.contains(&module.as_str()))
        }),
    }
}

/// Whether `expr` is a literal that holds no function: a value that holds no object ([`is_primitive`]), or an object
/// or array literal of such values, with no spread, method or accessor.
fn function_free(names: Names<'_>, expr: &Expression<'_>) -> bool {
    match expr.get_inner_expression() {
        Expression::ObjectExpression(object) => object.properties.iter().all(|property| match property {
            ObjectPropertyKind::ObjectProperty(p) => {
                !p.method && p.kind == PropertyKind::Init && function_free(names, &p.value)
            }
            ObjectPropertyKind::SpreadProperty(_) => false,
        }),
        Expression::ArrayExpression(array) => array.elements.iter().all(|element| match element {
            ArrayExpressionElement::SpreadElement(_) => false,
            ArrayExpressionElement::Elision(_) => true,
            other => other.as_expression().is_some_and(|value| function_free(names, value)),
        }),
        other => is_primitive(names, other),
    }
}

/// Methods of the global `Object` or `Reflect` that change their first argument only ([`changed_by_call`]), with the
/// object they are methods of.
const FIRST_ARGUMENT_SETTERS: &[(&str, &str)] = &[
    ("Object", "assign"),
    ("Object", "defineProperties"),
    ("Object", "defineProperty"),
    ("Reflect", "defineProperty"),
];

/// The setter of [`FIRST_ARGUMENT_SETTERS`] `callee` names by the name of the object it is called on, which may be
/// one the module declares.
fn first_argument_setter<'b, 'a>(callee: &'b Expression<'a>) -> Option<&'b IdentifierReference<'a>> {
    let Expression::StaticMemberExpression(m) = callee.get_inner_expression() else {
        return None;
    };
    let Expression::Identifier(object) = m.object.get_inner_expression() else {
        return None;
    };
    FIRST_ARGUMENT_SETTERS
        .contains(&(object.name.as_str(), m.property.name.as_str()))
        .then_some(&**object)
}

/// Whether `call` is one of [`FIRST_ARGUMENT_SETTERS`] of the global object, which changes its first argument only,
/// given a first argument that is not a spread: what a spread gives is not known to be its first argument
/// (`Object.assign(...[config, extra])`), so the call may change any object it spreads.
fn assigns(names: Names<'_>, call: &CallExpression<'_>) -> bool {
    first_argument_setter(&call.callee).is_some_and(|object| resolved(names.scoping, object).is_none())
        && call
            .arguments
            .first()
            .is_some_and(|first| first.as_expression().is_some())
}

/// The key `Object.defineProperty(object, key, …)` or `Reflect.defineProperty(object, key, …)` defines on its first
/// argument, when it is a static string.
pub(crate) fn property_defined(call: &CallExpression<'_>) -> Option<String> {
    let Expression::StaticMemberExpression(m) = call.callee.get_inner_expression() else {
        return None;
    };
    if m.property.name.as_str() != "defineProperty" || first_argument_setter(&call.callee).is_none() {
        return None;
    }
    call.arguments
        .get(1)
        .and_then(Argument::as_expression)
        .and_then(static_string)
}

/// Whether the name a callee (`defineConfig`, `vitest.defineProject`) starts from is not one the project declares a
/// value for: a global, or an import or a binding of a `require` of a package the evaluator does not follow
/// ([`Known::foreign`]), not of a module or a workspace package of the project.
fn foreign(names: Names<'_>, callee: &Expression<'_>) -> bool {
    let root = match callee.get_inner_expression() {
        Expression::StaticMemberExpression(member) => member.object.get_inner_expression(),
        other => other,
    };
    let Expression::Identifier(id) = root else {
        return false;
    };
    resolved(names.scoping, id).is_none_or(|symbol| names.known.foreign.contains(&symbol))
}

/// Whether a module specifier names a package (`vite`, `@vitest/config`, `node:path`) rather than a file of the
/// project: a relative or absolute path, a subpath import (`#tools`) or an alias that is no package name (`@/tools`,
/// `~/tools`).
fn is_package(specifier: &str) -> bool {
    match specifier.strip_prefix('@') {
        Some(scoped) => scoped
            .split_once('/')
            .is_some_and(|(scope, name)| !scope.is_empty() && !name.is_empty()),
        None => specifier
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphanumeric()),
    }
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
    /// The `const` bindings of an object whose methods the pass knows ([`creates_builtin`]), or of what
    /// `createRequire` returns, whose `resolve` changes nothing.
    builtins: HashSet<SymbolId>,
    /// The bindings of a namespace import (`import * as shared from './shared'`).
    namespaces: HashSet<SymbolId>,
    /// Those of [`Self::modules`] of a package ([`is_package`]) whose module the evaluator does not follow, so that
    /// none of the project's code is behind them.
    foreign: HashSet<SymbolId>,
    /// Those of [`Self::modules`] whose module the evaluator follows, which code of such a module may change.
    followed: HashSet<SymbolId>,
    /// The export of its module each import of [`Self::modules`] takes, as keys of the module's namespace: none for a
    /// namespace import, `default` for a default import and the name for a named one. A `require` takes no such part.
    import_paths: HashMap<SymbolId, Vec<String>>,
    /// Those of [`Self::foreign`] that the module only calls methods on (`path.format(…)`, `_.merge(…)`): it does not
    /// write through them, store a value in them, give them to a call or read a member of them otherwise.
    inert: HashSet<SymbolId>,
}

/// The bindings of `program` [`Known`] holds, where `imports` says which modules the evaluator follows.
fn known(program: &Program<'_>, scoping: &Scoping, imports: &dyn Imports) -> Known {
    let mut finder = FindKnown {
        scoping,
        known: Known::default(),
        in_const: false,
    };
    finder.visit_program(program);
    let mut known = finder.known;
    known.followed = known
        .modules
        .iter()
        .filter(|(_, (module, _))| imports.follows(module))
        .map(|(&symbol, _)| symbol)
        .collect();
    known.foreign = known
        .modules
        .iter()
        .filter(|(_, (module, _))| is_package(module) && !imports.follows(module))
        .map(|(&symbol, _)| symbol)
        .collect();
    let mut receivers = MethodReceivers::default();
    receivers.visit_program(program);
    known.inert = known
        .foreign
        .iter()
        .copied()
        .filter(|&symbol| {
            scoping
                .get_resolved_reference_ids(symbol)
                .iter()
                .all(|reference| !scoping.get_reference(*reference).is_value() || receivers.0.contains(reference))
        })
        .collect();
    known
}

/// The references to a name that are the object of a method called on it (`path` in `path.format(…)`).
#[derive(Default)]
struct MethodReceivers(HashSet<ReferenceId>);

impl<'a> Visit<'a> for MethodReceivers {
    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        if let Expression::StaticMemberExpression(member) = it.callee.get_inner_expression()
            && let Expression::Identifier(id) = member.object.get_inner_expression()
        {
            self.0.extend(id.reference_id.get());
        }
        walk::walk_call_expression(self, it);
    }
}

/// Finds the bindings of [`known`].
struct FindKnown<'s> {
    scoping: &'s Scoping,
    known: Known,
    /// Whether the declarations being visited are `const`.
    in_const: bool,
}

impl FindKnown<'_> {
    fn bind(&mut self, symbol: Option<SymbolId>, specifier: &str, imported: Option<String>) {
        if let Some(symbol) = symbol {
            let imported = imported.filter(|name| name != "default");
            self.known.modules.insert(symbol, (specifier.to_owned(), imported));
        }
    }

    /// Records the export the import `symbol` takes ([`Known::import_paths`]).
    fn path(&mut self, symbol: Option<SymbolId>, path: Vec<String>) {
        if let Some(symbol) = symbol {
            self.known.import_paths.insert(symbol, path);
        }
    }

    /// The module a `require('module')` of the global `require` loads.
    fn required(&self, init: &Expression<'_>) -> Option<String> {
        let Expression::CallExpression(call) = init.get_inner_expression() else {
            return None;
        };
        match call.callee.get_inner_expression() {
            Expression::Identifier(id) if id.name.as_str() == "require" && resolved(self.scoping, id).is_none() => call
                .arguments
                .first()
                .and_then(Argument::as_expression)
                .and_then(static_string),
            _ => None,
        }
    }

    /// Whether `init` is a call of `createRequire` of the `module` module.
    fn creates_require(&self, init: &Expression<'_>) -> bool {
        let Expression::CallExpression(call) = init.get_inner_expression() else {
            return false;
        };
        let Expression::Identifier(id) = call.callee.get_inner_expression() else {
            return false;
        };
        resolved(self.scoping, id)
            .and_then(|symbol| self.known.modules.get(&symbol))
            .is_some_and(|(module, imported)| {
                matches!(module.as_str(), "module" | "node:module") && imported.as_deref() == Some("createRequire")
            })
    }
}

impl<'a> Visit<'a> for FindKnown<'_> {
    fn visit_variable_declaration(&mut self, it: &VariableDeclaration<'a>) {
        let saved = std::mem::replace(&mut self.in_const, it.kind == VariableDeclarationKind::Const);
        walk::walk_variable_declaration(self, it);
        self.in_const = saved;
    }

    fn visit_import_declaration(&mut self, it: &ImportDeclaration<'a>) {
        let specifier = crate::js_string(it.source.value);
        for item in it.specifiers.iter().flatten() {
            match item {
                ImportDeclarationSpecifier::ImportSpecifier(s) => {
                    let imported = crate::collect::module_export_name(&s.imported);
                    self.path(s.local.symbol_id.get(), vec![imported.clone()]);
                    self.bind(s.local.symbol_id.get(), &specifier, Some(imported));
                }
                ImportDeclarationSpecifier::ImportDefaultSpecifier(s) => {
                    self.path(s.local.symbol_id.get(), vec!["default".to_owned()]);
                    self.bind(s.local.symbol_id.get(), &specifier, None);
                }
                ImportDeclarationSpecifier::ImportNamespaceSpecifier(s) => {
                    self.path(s.local.symbol_id.get(), Vec::new());
                    self.bind(s.local.symbol_id.get(), &specifier, None);
                    self.known.namespaces.extend(s.local.symbol_id.get());
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
        } else if let (true, BindingPattern::BindingIdentifier(id), Some(init)) = (self.in_const, &it.id, &it.init)
            && (creates_builtin(self.scoping, init) || self.creates_require(init))
            && let Some(symbol) = id.symbol_id.get()
        {
            self.known.builtins.insert(symbol);
        }
        walk::walk_variable_declarator(self, it);
    }
}

/// Globals whose methods the pass knows, by name: those of [`PURE_FUNCTIONS`], `Promise`, `Reflect` and `require`.
const BUILTIN_OBJECTS: &[&str] = &[
    "Array", "JSON", "Math", "Number", "Object", "Promise", "Reflect", "String", "console", "process", "require",
];

/// Whether the object a method is called on is one whose methods the pass knows: one `expr` creates
/// ([`creates_builtin`]), the result of one of [`PURE_FUNCTIONS`] (a string, a list or plain data), a `const` holding
/// one ([`Known::builtins`]), one of [`BUILTIN_OBJECTS`] the module does not declare, or the `path`, `fs`, `url` or
/// `module` module itself.
fn builtin(names: Names<'_>, receiver: &Expression<'_>) -> bool {
    match receiver.get_inner_expression() {
        Expression::Identifier(id) => match resolved(names.scoping, id) {
            None => BUILTIN_OBJECTS.contains(&id.name.as_str()),
            Some(symbol) => {
                names.known.builtins.contains(&symbol)
                    || names
                        .known
                        .modules
                        .get(&symbol)
                        .is_some_and(|(module, imported)| imported.is_none() && known_module(module).is_some())
            }
        },
        Expression::CallExpression(call) => pure_function(names, call).is_some(),
        other => creates_builtin(names.scoping, other),
    }
}

/// Whether `expr` creates a list, a string, a regular expression, a `Map` or another of [`BUILTIN_CLASSES`], or is
/// `import.meta`.
fn creates_builtin(scoping: &Scoping, expr: &Expression<'_>) -> bool {
    match expr.get_inner_expression() {
        Expression::ArrayExpression(_)
        | Expression::StringLiteral(_)
        | Expression::TemplateLiteral(_)
        | Expression::RegExpLiteral(_)
        | Expression::ImportMeta(_) => true,
        Expression::NewExpression(new) => constructs_builtin(scoping, new),
        _ => false,
    }
}

/// Whether `expr` is the binding of a namespace import, an object whose keys no code can change: a method called on
/// it is taken to change nothing of it.
fn is_namespace(names: Names<'_>, expr: &Expression<'_>) -> bool {
    matches!(
        expr.get_inner_expression(),
        Expression::Identifier(id) if resolved(names.scoping, id).is_some_and(|symbol| names.known.namespaces.contains(&symbol))
    )
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

/// Whether `expr` is itself one of [`Known::inert`], not a member of one.
fn is_inert_package(scoping: &Scoping, known: &Known, expr: &Expression<'_>) -> bool {
    let Expression::Identifier(id) = expr.get_inner_expression() else {
        return false;
    };
    resolved(scoping, id).is_some_and(|symbol| known.inert.contains(&symbol))
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

/// The parts of locals an argument gives a call, which the call may change: the name or member chain the argument is
/// ([`reach`]), and those an object or array literal holds at any depth, in its values, elements and spreads, as does
/// each value a condition, a logical operator, a sequence or an assignment may be. What a function or a call there
/// returns is not followed.
pub(crate) fn given_parts<'b, 'a>(expr: &'b Expression<'a>) -> Vec<(&'b IdentifierReference<'a>, Vec<String>)> {
    let mut parts = Vec::new();
    add_given_parts(expr, &mut parts);
    parts
}

/// Adds the parts of locals `expr` gives a call to `parts` ([`given_parts`]).
fn add_given_parts<'b, 'a>(expr: &'b Expression<'a>, parts: &mut Vec<(&'b IdentifierReference<'a>, Vec<String>)>) {
    if let Some((id, keys, _)) = reach(expr) {
        parts.push((id, keys));
        return;
    }
    match expr.get_inner_expression() {
        Expression::ObjectExpression(object) => {
            for property in &object.properties {
                match property {
                    ObjectPropertyKind::ObjectProperty(p) => add_given_parts(&p.value, parts),
                    ObjectPropertyKind::SpreadProperty(s) => add_given_parts(&s.argument, parts),
                }
            }
        }
        Expression::ArrayExpression(array) => {
            for element in &array.elements {
                match element {
                    ArrayExpressionElement::SpreadElement(s) => add_given_parts(&s.argument, parts),
                    other => {
                        if let Some(value) = other.as_expression() {
                            add_given_parts(value, parts);
                        }
                    }
                }
            }
        }
        Expression::ConditionalExpression(c) => {
            add_given_parts(&c.consequent, parts);
            add_given_parts(&c.alternate, parts);
        }
        Expression::LogicalExpression(l) => {
            add_given_parts(&l.left, parts);
            add_given_parts(&l.right, parts);
        }
        Expression::SequenceExpression(sequence) => {
            if let Some(last) = sequence.expressions.last() {
                add_given_parts(last, parts);
            }
        }
        Expression::AssignmentExpression(a) if a.operator == AssignmentOperator::Assign => {
            add_given_parts(&a.right, parts);
        }
        _ => {}
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

/// Methods that never change the object they are called on, nor what they are given when that object is known to have
/// them ([`builtin`]). `resolve` is that of `require`, `path`, `Promise`, `import.meta` and the function
/// `createRequire` returns.
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

/// Methods of [`PURE_METHODS`] that call a function they are given with what the list they are called on holds, and,
/// but for `reduce`, `reduceRight` and `toSorted`, with the argument after it as its `this`.
const CALLBACK_METHODS: &[&str] = &[
    "every",
    "filter",
    "find",
    "findIndex",
    "findLast",
    "findLastIndex",
    "flatMap",
    "forEach",
    "map",
    "reduce",
    "reduceRight",
    "some",
    "toSorted",
];

/// Methods that change the list they are called on but never what they are given, when it is known to be a list
/// ([`builtin`]).
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
/// binds `path`, `fs`, `url` or `module` to that module or imports the function from it, and a global's where the name
/// is not declared in the module ([`pure_function`]).
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
    ("module", &["createRequire"], Returns::Fresh),
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
const GLOBAL_OBJECTS: &[&str] = &[
    "Array", "JSON", "Math", "Number", "Object", "String", "console", "process",
];

/// The functions of [`PURE_FUNCTIONS`] that call a function given after their first argument (a replacer, a reviver),
/// by the name of the object they are called on and their own.
const CALLING_PURE_FUNCTIONS: &[(&str, &str)] = &[("JSON", "parse"), ("JSON", "stringify")];

/// What `call` returns when it calls one of [`PURE_FUNCTIONS`] (`path.posix` and `path.win32` count as `path`); `None`
/// for any other call, such as one of a function the module declares under such a name, and for one of
/// [`CALLING_PURE_FUNCTIONS`] given after its first argument anything but a literal that holds no function or
/// `undefined` (`JSON.stringify(config, replacer)`), as the function it may be given may change what it is called with.
fn pure_function(names: Names<'_>, call: &CallExpression<'_>) -> Option<Returns> {
    let (object, name) = match call.callee.get_inner_expression() {
        Expression::Identifier(id) => match resolved(names.scoping, id) {
            None => ("", id.name.as_str()),
            Some(symbol) => {
                let (module, imported) = names.known.modules.get(&symbol)?;
                (known_module(module)?, imported.as_deref()?)
            }
        },
        Expression::StaticMemberExpression(member) => {
            (module_object(names, &member.object)?, member.property.name.as_str())
        }
        _ => return None,
    };
    let returns = PURE_FUNCTIONS
        .iter()
        .find(|(module, functions, _)| *module == object && (functions.is_empty() || functions.contains(&name)))
        .map(|&(_, _, returns)| returns)?;
    let calls_back = CALLING_PURE_FUNCTIONS.contains(&(object, name));
    let given_no_function = || {
        call.arguments.iter().skip(1).all(|argument| {
            argument
                .as_expression()
                .is_some_and(|value| holds_no_function(names, value))
        })
    };
    (!calls_back || given_no_function()).then_some(returns)
}

/// Whether `expr` is a literal that holds no function ([`function_free`]) or the global `undefined`.
fn holds_no_function(names: Names<'_>, expr: &Expression<'_>) -> bool {
    function_free(names, expr)
        || matches!(
            expr.get_inner_expression(),
            Expression::Identifier(id) if id.name.as_str() == "undefined" && resolved(names.scoping, id).is_none()
        )
}

/// The name in [`PURE_FUNCTIONS`] of the object a function is called on: a global of [`GLOBAL_OBJECTS`] the module
/// does not declare, or a binding of the `path`, `fs`, `url` or `module` module itself.
fn module_object(names: Names<'_>, object: &Expression<'_>) -> Option<&'static str> {
    match object.get_inner_expression() {
        Expression::Identifier(id) => match resolved(names.scoping, id) {
            None => GLOBAL_OBJECTS
                .iter()
                .find(|global| **global == id.name.as_str())
                .copied(),
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
        "module" => Some("module"),
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

/// The object a call may change: the first argument of one of [`FIRST_ARGUMENT_SETTERS`] (`Object.assign`,
/// `Object.defineProperty`), or the object a method is called on, other than one of [`PURE_METHODS`] unless it calls
/// code the pass does not follow with what the object holds (`calls_back`, [`Unstable::calls_back`]), and unless the
/// call is one of [`PURE_FUNCTIONS`] (`pure`).
pub(crate) fn changed_by_call<'p, 'a>(
    call: &'p CallExpression<'a>,
    pure: bool,
    calls_back: bool,
) -> Option<&'p Expression<'a>> {
    if pure {
        return None;
    }
    if first_argument_setter(&call.callee).is_some() {
        return call.arguments.first().and_then(Argument::as_expression);
    }
    match call.callee.get_inner_expression() {
        Expression::StaticMemberExpression(m) if PURE_METHODS.contains(&m.property.name.as_str()) && !calls_back => {
            None
        }
        callee => callee.as_member_expression().map(MemberExpression::object),
    }
}
