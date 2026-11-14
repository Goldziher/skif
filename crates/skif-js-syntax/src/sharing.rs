//! The bindings of a config module whose value code the evaluator does not follow may change.
//!
//! The evaluator copies values: `const a = cfg; a.x = 1` changes its copy of `a`, not `cfg`, and a function it does not
//! call, or calls for its value only, does not change what it captures. An import counts as a binding of the module
//! here, as the evaluator reads it as its module computes it, but for a function or class imported from a module the
//! evaluator does not follow, whose result holds none of the module's locals; what one of a followed module returns
//! may be any value of such a module. This pass finds, by symbol, the bindings such
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
//! returns, but a change of what a parameter takes does not reach what it returns: a call gives that, and the function
//! does not return it unless it says so (`return options`).
//!
//! A call whose effect is not known (a function the module or a library declares, a `new` of a class other than a
//! builtin one, a list or string method on an object not known to be a list or string) may change every object it is
//! given, in any position and through a spread, and every part of a local an object or array literal it is given holds,
//! at any depth ([`given_parts`]); a spread of a local there (`{ ...options }`) leaves all of the local open, as a
//! change of only what its keys hold is not one the pass can say ([`Effect`]). The pass knows the effect of a
//! well-known function only where its name resolves to the module that declares it, and treats a `define…` helper of
//! an installed package it does not follow ([`Imports::is_installed`]: not one of the workspace nor an alias of a
//! project directory) and a merge that copies ([`copies`]) and `Object.assign`,
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
//! A getter, a setter or an iterator of an object or of an instance of a class (one an object literal or a class
//! declares, or a descriptor given to `Object.defineProperty`, `Object.defineProperties`, `Reflect.defineProperty` or
//! `Object.create`) runs with the object as `this` where code reads or writes its key, destructures the object (in a
//! declaration, an assignment, a parameter or the parameter of a callback), spreads or iterates it (`for…of`, `yield*`,
//! the `next` of an iterator), or gives it to a tagged template, to a call whose code the pass does not visit (a
//! function the module does not declare, `JSON.stringify(x)`) or to a `new` of a class but a builtin one, whose
//! constructor is not followed with what it is given, with no call of it written; a known function that reads no key of
//! what it is given (`path.join`, a `define…` helper of a package) runs none. Where one the module declares changes
//! what `this` holds, also through a binding taking `this` (`const self = this`), a method of its own object it calls
//! on `this` or another one it runs so, such a place changes what the object holds there, as a call of a method of it
//! does, and only a key of it where the code only gives keys of `this` itself a new value (`this.mode = 'production'`;
//! [`ThisEffect`]). Other code a value runs implicitly is not followed: a static getter, a `toString`, `valueOf` or
//! `toJSON` a template literal, `String(x)` or `JSON.stringify` calls, a `then` of a value awaited, a getter or an
//! iterator of an object of another module. Neither is a change an imported function makes to what it was given by an
//! earlier call (`register(config)`, later `apply()`).
//!
//! A `const` that takes a part of a local of its own function (`const { coverage } = config.test`) is the exception
//! when the evaluator can follow it ([`Unstable::follows`]): it then applies the changes made through the alias to that
//! part of the local itself, so they do not mark the local there. They still mark what may share an object with the
//! local, and another part of it that may hold the same object, as the evaluator's copies there are not changed; a
//! local of the alias's function is marked only where it is read after the alias is declared, outside a loop and a
//! nested function, as what an earlier read copied out is itself marked where it went.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use oxc_ast::ast::{
    Argument, ArrayAssignmentTarget, ArrayExpression, ArrayExpressionElement, ArrayPattern, ArrowFunctionExpression,
    AssignmentExpression, AssignmentOperator, AssignmentPattern, AssignmentTarget, AssignmentTargetMaybeDefault,
    AssignmentTargetPattern, AssignmentTargetProperty, AssignmentTargetPropertyIdentifier, BindingIdentifier,
    BindingPattern, CallExpression, ChainExpression, Class, ComputedMemberExpression, Declaration, DoWhileStatement,
    ExportDeclaration, ExportDefaultDeclaration, ExportDefaultDeclarationKind, Expression, ExpressionStatement,
    ForInStatement, ForOfStatement, ForStatement, ForStatementLeft, FormalParameter, FormalParameters, Function,
    IdentifierReference, ImportDeclaration, ImportDeclarationSpecifier, LogicalOperator, MemberExpression,
    MethodDefinition, MethodDefinitionKind, NewExpression, ObjectAssignmentTarget, ObjectExpression, ObjectPattern,
    ObjectProperty, ObjectPropertyKind, PrivateFieldExpression, Program, PropertyDefinition, PropertyKey, PropertyKind,
    ReturnStatement, SimpleAssignmentTarget, StaticBlock, StaticMemberExpression, Super, TaggedTemplateExpression,
    ThisExpression, UnaryExpression, UnaryOperator, VariableDeclaration, VariableDeclarationKind, VariableDeclarator,
    WhileStatement, WithStatement, YieldExpression,
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
/// What a change reaches through the links ([`Sharing::reach_of`]).
enum Reach {
    /// The bindings it reaches, with the part it changes there.
    Places(Vec<(SymbolId, Write)>),
    /// Every binding of a class of linked bindings ([`Sharing::class_of`]), changed in full.
    InFull(Rc<[SymbolId]>),
}

impl Reach {
    /// The bindings reached, with the part changed there.
    fn into_places(self) -> Vec<(SymbolId, Write)> {
        match self {
            Self::Places(places) => places,
            Self::InFull(class) => class.iter().map(|&other| (other, Write::default())).collect(),
        }
    }
}

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

    /// Whether `specifier`, a package the evaluator does not follow ([`is_package`]), is installed, so that no code of
    /// the project is behind it ([`Known::foreign`]): not a workspace package or an alias of a project directory
    /// (`@app/tools`). None is unless the implementation says so.
    fn is_installed(&self, _specifier: &str) -> bool {
        false
    }

    /// The places in what a call of the export `export` (`default` for the default one) of the followed module
    /// `specifier` returns that may hold an object of a value of a module ([`Unstable::result_places`]): none for a
    /// factory that builds and returns new objects, whose result shares nothing with the module's values. `None` when
    /// that is not known, and what a call returns may then be any value of such a module.
    fn result_places(&self, _specifier: &str, _export: &str) -> Option<Vec<Write>> {
        None
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
    /// The modules the evaluator follows that the module hands out a binding of other than by calling it
    /// ([`Sharing::exposed`]), by specifier.
    exposed: Vec<String>,
    /// Whether the module hands out so a binding of a package neither followed nor installed, whose code is not known.
    exposes_unknown: bool,
    /// The places in what a call of each top-level function of the module returns that may hold an object of a value
    /// of the module ([`Sharing::result_places`]), by name.
    results: HashMap<String, Vec<Write>>,
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
            classes: RefCell::new(None),
            mutated: Vec::new(),
            changes: BTreeMap::new(),
            aliases: Vec::new(),
            primitives: HashSet::default(),
            primitive_lists: HashSet::default(),
            element_links: Vec::new(),
            context: Context::default(),
            callables: callables(program, scoping),
            alias_parts: HashMap::default(),
            ends: HashMap::default(),
            late: Vec::new(),
            runs_modules: false,
            callee_refs: HashSet::default(),
            function_writes: Vec::new(),
            suspending: HashSet::default(),
            this_sites: vec![ThisSite::Module],
            method_sites: HashMap::default(),
            this_writes: Vec::new(),
            this_bound: HashMap::default(),
            makers: HashMap::default(),
            named_inits: HashMap::default(),
            default_exports: HashSet::default(),
            iifes: HashSet::default(),
            rebound: Vec::new(),
            rebinds: false,
            implicit: Implicit {
                accessors: accessors(program, scoping),
                ..Implicit::default()
            },
        };
        sharing.visit_program(program);
        let (followed, writes_modules) = sharing.share();
        let results = sharing.result_places();
        let (exposed, exposes_unknown) = sharing.exposed();
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
            exposed,
            exposes_unknown,
            results,
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
    /// to, or gives a call that may change it, a top-level binding, an import, an object one of them may hold or what
    /// `this` or `super` is there where that may be such a value, unlike the instance a constructor makes or the hooks
    /// object a plugin factory returns ([`Sharing::writes_modules`]), or the module calls a function of a module that
    /// may ([`Imports::changes_modules`]) or whose code is not known (a bare import neither followed nor installed), imports one for its effects, or hands
    /// out a binding of a package whose code is not known other than by calling it ([`Sharing::exposed`]). A function
    /// that builds and returns new objects changes none, whatever it does to them or to what its parameters take,
    /// which a call gives it. A binding of a followed module the module hands out so changes modules when that module
    /// does ([`Self::exposed_modules`]).
    pub(crate) fn changes_modules(&self) -> bool {
        self.runs_modules || self.writes_modules || self.exposes_unknown
    }

    /// The places in what a call of each top-level function the module declares returns that may hold an object of a
    /// value of the module (a top-level binding or an import), by name: each place is where such an object may be, at
    /// or below it, through the links that hold for every call. A function missing is one that is not known, and one
    /// with no place is a factory that builds and returns new objects, whose result shares nothing with the module's
    /// values, so a change of it changes none of them. None is known in a module that calls `eval`, reads `arguments`
    /// or has a `with` statement.
    pub(crate) fn result_places(&self) -> &HashMap<String, Vec<Write>> {
        &self.results
    }

    /// The modules the evaluator follows, by specifier, that the module hands out a binding of other than by calling
    /// it: it exports it (`export { tweak }`, `export default tweak`), puts it in a value (`export const tools = {
    /// tweak }`) or gives it to a name, where code of another module may call it. The module changes modules when one
    /// of them does ([`Imports::changes_modules`]).
    pub(crate) fn exposed_modules(&self) -> &[String] {
        &self.exposed
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
    /// does not make again ([`Sharing::writes_modules`]). A link of what the name of a function the module declares
    /// holds (`f.cache = base`, `add.bind(null, base)`) is no such link, as the function keeps it for every call.
    loading: bool,
    ends: Ends,
}

/// What the ends of a [`Link`] stand for of a function the module declares, whose name stands for what a call of it
/// returns and for what its parameters take.
#[derive(Default, Clone, Copy)]
struct Ends {
    /// Whether `a`, and whether `b`, stands there for what a call of the function it names returns: the function in
    /// its definition (`return config`), or what holds a call of it (`const r = create()`).
    returned: [bool; 2],
    /// Whether `b` is a name a parameter of the function `a` binds, which takes what a call gives it.
    parameter: bool,
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
    /// Whether `symbol` is a function the module declares that the value holds what a call of it returns of
    /// (`create()`), not the function itself.
    result: bool,
    /// Whether `symbol` is a name a parameter of the function the value is binds ([`Refs::parameters`]).
    parameter: bool,
}

/// Where the visitor of [`Sharing`] is.
#[derive(Debug, Default, Clone, Copy)]
struct Context {
    /// Whether the declarations being visited are `const`.
    in_const: bool,
    /// Whether the links being made are those of the definition of a function: what it returns and what its
    /// parameters take, which hold for every call of it.
    defining: bool,
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
    /// The bindings each binding is linked to, whatever the parts, sorted, computed for the first `usize` links
    /// ([`Self::class_of`]); cleared when links are taken back.
    classes: RefCell<Option<(usize, Classes)>>,
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
    /// Where the visitor is: the kind of declaration and whether it is in a function definition.
    context: Context,
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
    /// The references that are what a call, a `new` or a tagged template calls: the name itself, or the name the
    /// member chain it calls starts from (`tweak`, `tools` in `tools.tweak()`).
    callee_refs: HashSet<ReferenceId>,
    /// The changes code in a function makes of a binding, each with the scope of that function and whether it is a
    /// change of what a call of a function the module declares returns ([`Held::result`]).
    function_writes: Vec<(SymbolId, ScopeId, Change, bool)>,
    /// The scopes of the generators and `async` functions, whose code runs after the call that made them returns,
    /// with what that call gave them.
    suspending: HashSet<ScopeId>,
    /// What `this` is in the code being visited, innermost last ([`ThisSite`]).
    this_sites: Vec<ThisSite>,
    /// What `this` is in each method found and not visited yet, by the scope of its function.
    method_sites: HashMap<ScopeId, ThisSite>,
    /// The writes through `this` or `super` in a function (`this.plugins.push(p)`), a call given it that may change
    /// it, and a write reaching a binding that takes what it holds ([`Self::this_bound`]), each with what `this` is
    /// there and whether it gives a key of `this` itself a new value (`this.options = o`). What `this` is depends on
    /// how the function is called (`add.bind(base)`, `obj.add(p)`), which the links do not follow
    /// ([`Self::this_changes_modules`]).
    this_writes: Vec<(ThisSite, bool)>,
    /// The bindings of a function that take what `this` or `super` holds (`const list = this.plugins`, `const self =
    /// this`, `const { plugins } = this`), with what `this` is there: a change of what they hold is a write through
    /// `this`.
    this_bound: HashMap<SymbolId, ThisSite>,
    /// The functions and classes found, by scope, as what makes the objects of [`ThisSite::Made`].
    makers: HashMap<ScopeId, Maker>,
    /// The bindings a declaration initialises with a function or a class, by the scope of that function or class.
    named_inits: HashMap<ScopeId, SymbolId>,
    /// The scopes of the functions and classes the module exports as its default.
    default_exports: HashSet<ScopeId>,
    /// The scopes of the functions called where they are written (`(function () { … })()`), whose `this` is no
    /// object.
    iifes: HashSet<ScopeId>,
    /// The bindings a call of `call`, `apply` or `bind` gives a `this` (`add.bind(base)`), which may be a function the
    /// module declares.
    rebound: Vec<SymbolId>,
    /// Whether code may call a method with an object other than the one it is found on as `this`: a `call`, `apply`
    /// or `bind` of a function that is no binding given an object, or a read of a prototype (`Cls.prototype`,
    /// `Object.getPrototypeOf(x)`), whose methods are then called on it.
    rebinds: bool,
    /// What the pass finds of the code a value may run with no call of it written ([`Self::run_implicitly`]).
    implicit: Implicit,
}

/// What `this` is where code writes through it ([`Sharing::this_writes`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThisSite {
    /// The `this` of the module, outside every function but arrow functions (`module.exports` in a `CommonJS` module).
    Module,
    /// The object `new` makes, in a constructor or an instance field initialiser.
    Instance,
    /// The class, in a static field initialiser or a static block, which run when the class is defined.
    Load,
    /// The class, in a static method.
    Class,
    /// What a function that is no method is called on: it is bound to the binding given, if any, and has the scope
    /// given.
    Plain(Option<SymbolId>, ScopeId),
    /// An object code of the function, class or module of the scope given makes: the object literal a method is in,
    /// or an instance of the class whose method it is.
    Made(ScopeId),
    /// What `super` is, a prototype the object a method is in inherits from.
    Super,
}

/// A function or a class the module declares, which makes the objects of [`ThisSite::Made`] when it runs or is
/// constructed.
struct Maker {
    /// The binding it is declared as or initialises, if any.
    binding: Option<SymbolId>,
    /// The scope of the function, class or module it is written in.
    enclosing: ScopeId,
    /// Whether the module exports it as its default.
    default_export: bool,
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

    /// The parts of locals the object `expr` may be, each with whether it is that part exactly and whether it is what a
    /// call of a function the module declares returns ([`Held::result`]): the part a name or a member chain is
    /// ([`Self::place`]), or each part what any other expression evaluates to may hold ([`Refs`]), as the object a
    /// method returns (`list.find(…)`) or a condition chooses; those are recorded for the evaluator
    /// ([`Unstable::parts_of`]). With `keyed`, a key of the object is written (`this.options = o`).
    fn parts(&mut self, expr: &Expression<'_>, keyed: bool) -> Vec<(Place, bool, bool)> {
        self.note_this(expr, keyed);
        let parts = self.object_parts(expr);
        if reach(expr).is_some() {
            return parts;
        }
        let named = parts
            .iter()
            .map(|(place, exact, _)| {
                let name = self.scoping.symbol_name(place.symbol).to_owned();
                (name, place.keys.clone(), *exact)
            })
            .collect();
        self.objects.insert(expr.span(), named);
        parts
    }

    /// The parts of locals the object `expr` may be, as [`Self::parts`] finds them, recording nothing.
    fn object_parts(&self, expr: &Expression<'_>) -> Vec<(Place, bool, bool)> {
        if reach(expr).is_some() {
            return self
                .place(expr)
                .into_iter()
                .map(|(place, exact)| (place, exact, false))
                .collect();
        }
        self.refs(Part::Value, |refs| refs.visit_expression(expr))
            .into_iter()
            .map(|held| {
                let exact = held.join == Join::Same && held.at.is_empty();
                let place = Place {
                    symbol: held.symbol,
                    keys: held.keys,
                };
                (place, exact, held.result)
            })
            .collect()
    }

    /// Records the write of the member target of an assignment: a new value of its key in each object it may be
    /// written in ([`Self::parts`]), or a change of what that object holds when the key is not static or the object
    /// is not exactly a part of a local.
    fn set(&mut self, member: &MemberExpression<'_>) {
        if is_this(member.object()) {
            let effect = member_key(member).map_or_else(ThisEffect::contents, ThisEffect::key);
            self.note_this_effect(&effect);
        }
        let key = member.static_property_name().map(crate::js_string);
        for (place, exact, result) in self.parts(member.object(), true) {
            let write = match &key {
                Some(key) if exact => Write {
                    keys: place.keys,
                    key: Some(key.clone()),
                },
                _ => Write::contents(place.keys),
            };
            self.changed_as(place.symbol, write, result);
        }
    }

    /// The parts of locals the member target of an assignment writes to and whether each is the target exactly: the
    /// target of a member chain ([`Self::member_place`]), or somewhere at or below each part its object may be.
    fn member_places(&mut self, member: &MemberExpression<'_>) -> Vec<(Place, bool)> {
        if reach(member.object()).is_some() {
            return self.member_place(member).into_iter().collect();
        }
        self.parts(member.object(), true)
            .into_iter()
            .map(|(place, _, _)| (place, false))
            .collect()
    }

    /// The function (or module) whose binding `symbol` is.
    fn function_of(&self, symbol: SymbolId) -> ScopeId {
        self.function_at(self.scoping.symbol_scope_id(symbol), true)
    }

    /// The innermost function (or the module) whose code the scope `scope` is in, `scope` itself included; an arrow
    /// function counts only with `arrows`, as it has the `this` of where it is.
    fn function_at(&self, scope: ScopeId, arrows: bool) -> ScopeId {
        self.scoping
            .scope_ancestors(scope)
            .find(|&s| {
                let flags = self.scoping.scope_flags(s);
                flags.is_top() || (flags.is_function() && (arrows || !flags.is_arrow()))
            })
            .unwrap_or(scope)
    }

    /// The function (or module) whose code is being visited, or the class whose body is.
    fn current_function(&self) -> ScopeId {
        self.functions
            .last()
            .copied()
            .unwrap_or_else(|| self.scoping.root_scope_id())
    }

    /// Whether the code being visited is that of the module, outside every function and class.
    fn at_top(&self) -> bool {
        self.current_function() == self.scoping.root_scope_id()
    }

    fn change(&mut self, symbol: SymbolId, change: Change) {
        let merged = match self.changes.remove(&symbol) {
            Some(existing) => existing.with(change),
            None => change,
        };
        self.changes.insert(symbol, merged);
    }

    /// Records a write of `symbol`, of what a call of it returns when `result` ([`Held::result`]); one from a function
    /// other than its own may happen at any time.
    fn written(&mut self, symbol: SymbolId, change: Change, result: bool) {
        self.written_in(self.current_function(), symbol, change, result);
    }

    /// [`Self::written`] by code of the function (or module) of the scope `function`.
    fn written_in(&mut self, function: ScopeId, symbol: SymbolId, change: Change, result: bool) {
        if function != self.scoping.root_scope_id() {
            self.function_writes.push((symbol, function, change.clone(), result));
        }
        if function != self.function_of(symbol) {
            self.change(symbol, change);
        }
    }

    /// Whether a link made now is made by code the module runs when it is loaded ([`Link::loading`]).
    fn loading(&self) -> bool {
        !self.context.defining && self.functions.last() == Some(&self.scoping.root_scope_id())
    }

    /// Makes the links `visit` makes as those of the definition of a function when `defining` ([`Context::defining`]).
    fn in_definition(&mut self, defining: bool, visit: impl FnOnce(&mut Self)) {
        let saved = std::mem::replace(&mut self.context.defining, defining);
        visit(self);
        self.context.defining = saved;
    }

    /// Records a change of what the object `expr` holds, in each part of a local it may be ([`Self::parts`]).
    fn mutate(&mut self, expr: &Expression<'_>) {
        for (place, _, result) in self.parts(expr, false) {
            self.changed_as(place.symbol, Write::contents(place.keys), result);
        }
    }

    /// Records the change `call` makes of the object `target` it is given or called on ([`changed_by_call`]): a new
    /// value of the key it defines on a part of a local ([`property_defined`]), or a change of what that object holds.
    fn change_target(&mut self, call: &CallExpression<'_>, target: &Expression<'_>) {
        if is_namespace(self.names(), target) {
            return;
        }
        if let Some(method) = this_member(&call.callee) {
            return self.note_this_call(member_key(method));
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

    /// Records a write through `this` or `super` when the object `expr`, which code in a function changes, may be what
    /// one of them holds ([`Self::this_writes`]); with `keyed`, a key of the object is written, which is one of `this`
    /// itself when `expr` is `this` ([`Self::set`] records which).
    fn note_this(&mut self, expr: &Expression<'_>, keyed: bool) {
        let (this, sup) = self.this_mentioned(expr);
        if let Some(site) = this {
            let keyed = keyed && is_this(expr);
            self.wrote_this(site, keyed);
            if !keyed {
                self.note_this_effect(&ThisEffect::contents());
            }
        }
        if let Some(site) = sup {
            self.wrote_this(site, false);
            self.note_this_effect(&ThisEffect::contents());
        }
    }

    /// What `this` and `super` are where `expr` is visited, each when `expr` mentions it outside the functions and
    /// classes it declares ([`MentionsThis`]); neither at the top level of the module.
    fn this_mentioned(&self, expr: &Expression<'_>) -> (Option<ThisSite>, Option<ThisSite>) {
        if self.at_top() {
            return (None, None);
        }
        let mut finder = MentionsThis::default();
        finder.visit_expression(expr);
        (
            finder.this.then(|| self.this_site()),
            finder.sup.then_some(ThisSite::Super),
        )
    }

    /// Records `effect` as a change the getter, setter, iterator or method whose `this` the code being visited has, if
    /// any, makes of what `this` holds ([`Implicit::effects`]).
    fn note_this_effect(&mut self, effect: &ThisEffect) {
        if let Some(function) = self.owned_function() {
            self.implicit.effects.entry(function).or_default().add(effect);
        }
    }

    /// Records a call of the method `method` on `this` (`None` when its key is not static): a write through `this`,
    /// which in a getter, a setter, an iterator or a method of an object is one only when the methods it may call
    /// change what `this` holds, and only as they do ([`Self::call_effect`], [`Self::note_this_call_writes`]).
    fn note_this_call(&mut self, method: Option<String>) {
        if self.at_top() {
            return;
        }
        let site = self.this_site();
        match self.owned_function() {
            Some(caller) => self.implicit.calls.push(ThisCall { caller, method, site }),
            None => self.wrote_this(site, false),
        }
    }

    /// Records that code gives the key `key` (`None` when it is not static) of an object `value`, which may be a
    /// function other than a method the module declares there ([`Implicit::replace`]), unless it holds no object or is
    /// an object or array literal, which no call calls.
    fn note_key_value(&mut self, key: Option<String>, value: &Expression<'_>) {
        let literal = matches!(
            value.get_inner_expression(),
            Expression::ObjectExpression(_) | Expression::ArrayExpression(_)
        );
        if !literal && !is_primitive(self.names(), value) {
            self.implicit.replace(key);
        }
    }

    /// The getter, setter, iterator or method of an object whose `this` the code being visited has
    /// ([`Self::owned_at`]).
    fn owned_function(&self) -> Option<ScopeId> {
        self.owned_at(self.current_function())
    }

    /// The getter, setter, iterator or method of an object whose `this` the code that declares `symbol` has
    /// ([`Self::owned_at`]).
    fn owned_of(&self, symbol: SymbolId) -> Option<ScopeId> {
        self.owned_at(self.scoping.symbol_scope_id(symbol))
    }

    /// The innermost function that is no arrow function holding the scope `scope`, when it is a getter, a setter, an
    /// iterator or a method of an object ([`Implicit::is_owned`]).
    fn owned_at(&self, scope: ScopeId) -> Option<ScopeId> {
        let function = self.function_at(scope, false);
        self.implicit.is_owned(function).then_some(function)
    }

    /// What `this` or `super` is where `expr`, an object code may run functions of, is visited, when the object may be
    /// what one of them holds, each with whether the object is what it holds itself rather than a value below it
    /// (`this.list`). A name or a member chain is a part of a local ([`Self::object_parts`]), also where the local
    /// takes what `this` holds ([`Self::this_bound`]).
    fn this_objects(&self, expr: &Expression<'_>) -> Vec<(ThisSite, bool)> {
        if reach(expr).is_some() {
            return Vec::new();
        }
        let exact = matches!(
            expr.get_inner_expression(),
            Expression::ThisExpression(_) | Expression::Super(_)
        );
        let (this, sup) = self.this_mentioned(expr);
        this.into_iter().chain(sup).map(|site| (site, exact)).collect()
    }

    /// Records the object `expr` as one code may run a getter, a setter or an iterator of as `runs` say with no call of
    /// it written ([`ImplicitSite`]), on the object itself or, `on` [`On::Below`], on a value below it, when the
    /// module declares one of those they may run ([`Accessors::runs`]). A namespace import has none of them.
    fn note_implicit_site(&mut self, expr: &Expression<'_>, runs: Vec<Run>, on: On) {
        let runs: Vec<Run> = runs
            .into_iter()
            .filter(|run| self.implicit.accessors.runs(run))
            .collect();
        if runs.is_empty() || is_namespace(self.names(), expr) {
            return;
        }
        let itself = on == On::Itself;
        let this: Vec<(ThisSite, bool)> = self
            .this_objects(expr)
            .into_iter()
            .map(|(site, exact)| (site, exact && itself))
            .collect();
        let parts: Vec<(Place, bool, bool)> = self
            .object_parts(expr)
            .into_iter()
            .map(|(place, exact, result)| (place, exact && itself, result))
            .collect();
        if parts.is_empty() && this.is_empty() {
            return;
        }
        let site = ImplicitSite {
            runs,
            function: self.current_function(),
            owner: self.owned_function(),
            this,
            parts,
        };
        self.implicit.sites.push(site);
    }

    /// [`Self::note_implicit_site`] of the value `value` a destructuring takes, as `runs` say.
    fn note_destructured(&mut self, value: &Expression<'_>, runs: Destructured) {
        self.note_implicit_site(value, runs.own, On::Itself);
        self.note_implicit_site(value, runs.below, On::Below);
    }

    /// Records the objects code `call` runs may run functions of with no call of them written: each argument as
    /// [`Self::argument_runs`] says and what a spread argument spreads as an iteration, each argument a parameter of a
    /// function the module declares destructures ([`Callables::patterns`]), and the object a method is called on and
    /// the arguments other than a callback, whose elements the parameters of the callback may destructure.
    fn note_call_sites(&mut self, call: &CallExpression<'_>, effect: Option<Effect>) {
        if self.implicit.accessors.is_empty() {
            return;
        }
        let runs = self.argument_runs(call, effect);
        self.note_argument_sites(&call.arguments, runs.as_ref());
        if effect == Some(Effect::Followed) {
            self.note_parameter_sites(call);
        }
        self.note_callback_sites(&call.callee, &call.arguments);
    }

    /// Records each argument as an object code may run functions of as `run` says, if anything does, and what a spread
    /// argument spreads as one code iterates.
    fn note_argument_sites(&mut self, arguments: &[Argument<'_>], run: Option<&Run>) {
        for argument in arguments {
            if let Argument::SpreadElement(spread) = argument {
                self.note_implicit_site(&spread.argument, vec![Run::Iteration], On::Itself);
            } else if let (Some(run), Some(value)) = (run, argument.as_expression()) {
                self.note_implicit_site(value, vec![run.clone()], On::Itself);
            }
        }
    }

    /// How the code `call` runs may run functions of each object it is given with no call of them written ([`Run`]),
    /// where the pass does not visit that code: any for a call whose effect it does not know; none for a function the
    /// module declares, whose code it visits, nor for a known function that reads no key of what it is given
    /// (`path.join`, a `define…` helper of a package, which returns it, a method of a list); a read of any key for
    /// one that reads the own keys of what it is given ([`reads_keys`]).
    fn argument_runs(&self, call: &CallExpression<'_>, effect: Option<Effect>) -> Option<Run> {
        match effect {
            None => Some(Run::Every),
            Some(Effect::Followed) => None,
            Some(Effect::Keeps) => reads_keys(self.names(), call).then_some(Run::Key(None)),
        }
    }

    /// Records each argument of `call`, a call of a function the module declares, that a parameter of it destructures
    /// as an object code may run functions of as the pattern says ([`Callables::patterns`]).
    fn note_parameter_sites(&mut self, call: &CallExpression<'_>) {
        let Expression::Identifier(callee) = call.callee.get_inner_expression() else {
            return;
        };
        let Some(patterns) = local_symbol(self.scoping, callee)
            .and_then(|symbol| self.callables.patterns.get(&symbol))
            .cloned()
        else {
            return;
        };
        for (argument, runs) in call.arguments.iter().zip(patterns) {
            if let Some(value) = argument.as_expression() {
                self.note_destructured(value, runs);
            }
        }
    }

    /// Records the object a method is called on and each argument other than a function written in place as objects
    /// below which code may run functions as the parameters of a callback the call is given destructure what they
    /// take ([`Self::callback_runs`]): `list.map(({ grow }) => grow)`.
    fn note_callback_sites(&mut self, callee: &Expression<'_>, arguments: &[Argument<'_>]) {
        let runs: Vec<Run> = arguments
            .iter()
            .filter_map(Argument::as_expression)
            .flat_map(|argument| self.callback_runs(argument))
            .collect();
        if runs.is_empty() {
            return;
        }
        let receiver = callee
            .get_inner_expression()
            .as_member_expression()
            .map(MemberExpression::object);
        let values = arguments
            .iter()
            .filter(|argument| !is_inline_function(argument))
            .filter_map(given);
        for value in receiver.into_iter().chain(values) {
            self.note_implicit_site(value, runs.clone(), On::Below);
        }
    }

    /// How the parameters of the callback `argument` destructure what they take ([`Destructured`]), a function written
    /// in place or one the module declares ([`Callables::patterns`]).
    fn callback_runs(&self, argument: &Expression<'_>) -> Vec<Run> {
        let patterns = match argument.get_inner_expression() {
            Expression::ArrowFunctionExpression(function) => parameter_patterns(&function.params),
            Expression::FunctionExpression(function) => parameter_patterns(&function.params),
            Expression::Identifier(id) => local_symbol(self.scoping, id)
                .and_then(|symbol| self.callables.patterns.get(&symbol))
                .cloned()
                .unwrap_or_default(),
            _ => return Vec::new(),
        };
        patterns.into_iter().flat_map(Destructured::into_runs).collect()
    }

    /// Applies each place found where code may run a getter, a setter or an iterator with no call of it written
    /// ([`Implicit::sites`]): what those it may run change of what `this` holds ([`ThisEffect`]) changes there what the
    /// object holds, as a call of a method of it does (`make().grow` as `make().grow()`), in each part of a local the
    /// object may be. That part itself changes too, as the evaluator sees no call there, and a write through `this` is
    /// made where the object may be what `this` holds. What each of them changes grows until no more is found: by
    /// what such a place in its code changes of its `this` ([`Self::note_runs_on_this`]), by a change reaching a
    /// binding of it that takes what `this` holds ([`Self::note_bound_writes`]) and by the methods it calls on `this`
    /// ([`Self::note_this_calls`]).
    fn run_implicitly(&mut self) {
        let sites = std::mem::take(&mut self.implicit.sites);
        let mut pending: Vec<Pending> = sites
            .into_iter()
            .filter_map(|site| self.implicit.accessors.pending(site))
            .collect();
        if pending.is_empty() && self.implicit.calls.is_empty() {
            return;
        }
        let mut effects = std::mem::take(&mut self.implicit.effects);
        let owners = self.bound_owners();
        let (base, links_of) = if owners.is_empty() {
            (self.links.len(), HashMap::default())
        } else {
            (self.link_aliases_for_now(), self.links_of())
        };
        let bound = BoundWrites { owners, links_of };
        let mut scan = Scan::default();
        loop {
            let mut grew = self.note_bound_writes(&bound, &mut scan, &mut effects);
            grew |= self.note_this_calls(&mut effects);
            for site in &mut pending {
                grew |= self.apply_site(site, &mut effects);
            }
            if !grew {
                break;
            }
        }
        self.unlink_since(base);
        self.note_this_call_writes(&effects);
    }

    /// Records a write through `this` for each call of a method on `this` in a getter, a setter, an iterator or a
    /// method of an object that may change what `this` holds ([`Self::call_effect`]), of keys of `this` itself alone
    /// when it gives only those a new value.
    fn note_this_call_writes(&mut self, effects: &HashMap<ScopeId, ThisEffect>) {
        let writes: Vec<(ThisSite, bool)> = self
            .implicit
            .calls
            .iter()
            .map(|call| {
                (
                    call.site,
                    self.call_effect(call.caller, call.method.as_deref(), effects),
                )
            })
            .filter(|(_, effect)| *effect != ThisEffect::default())
            .map(|(site, effect)| (site, !effect.contents))
            .collect();
        for (site, keyed) in writes {
            self.wrote_this(site, keyed);
        }
    }

    /// The bindings that take what `this` or `super` holds ([`Self::this_bound`]) in a getter, a setter, an iterator or
    /// a method of an object, each with that function ([`Self::owned_of`]).
    fn bound_owners(&self) -> HashMap<SymbolId, ScopeId> {
        self.this_bound
            .keys()
            .filter_map(|&symbol| Some((symbol, self.owned_of(symbol)?)))
            .collect()
    }

    /// Adds a change of what `this` holds to each function of `bound` with a binding taking what `this` holds that a
    /// change made since the last scan reaches through the links, returning whether one grew. Only a change of a
    /// binding linked to such a binding is followed ([`Self::bound_reached`]), and each change once.
    fn note_bound_writes(
        &self,
        bound: &BoundWrites,
        scan: &mut Scan,
        effects: &mut HashMap<ScopeId, ThisEffect>,
    ) -> bool {
        if bound.owners.is_empty() {
            return false;
        }
        let contents = ThisEffect::contents();
        let mut grew = false;
        for (symbol, write) in &self.mutated[scan.upto..] {
            if !scan.seen.insert((*symbol, write.clone())) {
                continue;
            }
            for owner in self.bound_reached(bound, *symbol, write) {
                grew |= effects.entry(owner).or_default().add(&contents);
            }
        }
        scan.upto = self.mutated.len();
        grew
    }

    /// The functions of `bound` whose binding taking what `this` holds the change `write` of `symbol` reaches.
    fn bound_reached(&self, bound: &BoundWrites, symbol: SymbolId, write: &Write) -> Vec<ScopeId> {
        let Some(class) = self.class_of(symbol) else {
            return bound.owners.get(&symbol).copied().into_iter().collect();
        };
        if !bound.owners.keys().any(|taker| class.binary_search(taker).is_ok()) {
            return Vec::new();
        }
        let reached: Vec<SymbolId> = match self.reach_of(&bound.links_of, symbol, write) {
            Reach::InFull(class) => class.to_vec(),
            Reach::Places(places) => places.into_iter().map(|(other, _)| other).collect(),
        };
        reached
            .iter()
            .filter_map(|other| bound.owners.get(other).copied())
            .collect()
    }

    /// Adds to each function that calls a method on `this` ([`Implicit::calls`]) what the call may change of what
    /// `this` holds ([`Self::call_effect`]), returning whether one grew.
    fn note_this_calls(&self, effects: &mut HashMap<ScopeId, ThisEffect>) -> bool {
        let mut grew = false;
        for call in &self.implicit.calls {
            let effect = self.call_effect(call.caller, call.method.as_deref(), effects);
            grew |= effects.entry(call.caller).or_default().add(&effect);
        }
        grew
    }

    /// What a call of the method `method` on `this` in the function `caller` may change of what `this` holds: what the
    /// methods of that name the module declares change ([`Implicit::own_methods`]), when the object or class `caller`
    /// is in has one of its own; anything otherwise, as the method called is not known.
    fn call_effect(&self, caller: ScopeId, method: Option<&str>, effects: &HashMap<ScopeId, ThisEffect>) -> ThisEffect {
        self.implicit
            .own_methods(caller, method)
            .map_or_else(ThisEffect::contents, |methods| union_of(methods, effects))
    }

    /// Applies what the getters, setters and iterators the place `pending` may run change of what `this` holds, when it
    /// grew since it was last applied, returning whether it did.
    fn apply_site(&mut self, pending: &mut Pending, effects: &mut HashMap<ScopeId, ThisEffect>) -> bool {
        let effect = union_of(&pending.ran, effects);
        if effect == pending.applied {
            return false;
        }
        self.note_runs_on_this(&pending.site, &effect, effects);
        self.apply_effect(&pending.site, &effect);
        pending.applied = effect;
        true
    }

    /// Adds `effect`, what the place `site` changes of the object there, to the functions whose `this` that object may
    /// be: the function whose code the place is in where it is that function's `this`, and a change of all it holds
    /// where it is a value below it (`this.list`) or a binding taking what `this` holds (`const self = this`).
    fn note_runs_on_this(&self, site: &ImplicitSite, effect: &ThisEffect, effects: &mut HashMap<ScopeId, ThisEffect>) {
        let contents = ThisEffect::contents();
        if let Some(owner) = site.owner {
            for &(_, exact) in &site.this {
                effects
                    .entry(owner)
                    .or_default()
                    .add(if exact { effect } else { &contents });
            }
        }
        for (place, _, _) in &site.parts {
            if self.this_bound.contains_key(&place.symbol)
                && let Some(owner) = self.owned_of(place.symbol)
            {
                effects.entry(owner).or_default().add(&contents);
            }
        }
    }

    /// Changes what the object at the place `site` holds as `effect` says, in each part of a local it may be, and
    /// records a write through `this` where it may be what `this` holds.
    fn apply_effect(&mut self, site: &ImplicitSite, effect: &ThisEffect) {
        for &(this, exact) in &site.this {
            self.wrote_this(this, exact && !effect.contents);
        }
        for (place, exact, result) in &site.parts {
            for write in effect.writes(place, *exact) {
                if !result {
                    self.change(place.symbol, Change::Contents(vec![write.clone()]));
                }
                self.changed_in(site.function, place.symbol, write, *result);
            }
        }
    }

    /// What `this` is in the code being visited.
    fn this_site(&self) -> ThisSite {
        self.this_sites.last().copied().unwrap_or(ThisSite::Module)
    }

    /// Records a write through `this` where it is `site`, of a key of `this` itself when `keyed`.
    fn wrote_this(&mut self, site: ThisSite, keyed: bool) {
        if !self.this_writes.contains(&(site, keyed)) {
            self.this_writes.push((site, keyed));
        }
    }

    /// Records that the names `symbols` take what `value` holds when it may be what `this` or `super` holds in a
    /// function ([`Self::this_bound`]).
    fn bind_this(&mut self, symbols: &[SymbolId], value: &Expression<'_>) {
        let site = match self.this_mentioned(value) {
            (_, Some(sup)) => sup,
            (Some(this), None) => this,
            (None, None) => return,
        };
        for &symbol in symbols {
            self.this_bound.insert(symbol, site);
        }
    }

    /// Records what `this` is in the function `function` of the scope `scope`, a method when one was found for it
    /// ([`Self::method_sites`]), and the function as a [`Maker`], before it is visited.
    fn enter_function(&mut self, scope: ScopeId, declared: Option<SymbolId>) -> ThisSite {
        let binding = self.named_inits.get(&scope).copied().or(declared);
        self.add_maker(scope, binding);
        self.method_sites
            .remove(&scope)
            .unwrap_or(ThisSite::Plain(binding, scope))
    }

    /// Records the function or class of the scope `scope`, bound to `binding`, as a [`Maker`].
    fn add_maker(&mut self, scope: ScopeId, binding: Option<SymbolId>) {
        let root = self.scoping.root_scope_id();
        let maker = Maker {
            binding,
            enclosing: self.functions.last().copied().unwrap_or(root),
            default_export: self.default_exports.contains(&scope),
        };
        self.makers.insert(scope, maker);
    }

    /// Records what `call` tells of the `this` a function may be called with ([`Self::rebinds`], [`Self::rebound`]):
    /// a `call`, `apply` or `bind` (or `Reflect.apply`) given an object as `this`, and a read of a prototype through
    /// `Object.getPrototypeOf` or `Reflect.getPrototypeOf`.
    fn note_rebinding(&mut self, call: &CallExpression<'_>) {
        let Expression::StaticMemberExpression(member) = call.callee.get_inner_expression() else {
            return;
        };
        let method = member.property.name.as_str();
        let global = |name: &str| {
            matches!(member.object.get_inner_expression(), Expression::Identifier(id)
                if id.name.as_str() == name && resolved(self.scoping, id).is_none())
        };
        if method == "getPrototypeOf" && (global("Object") || global("Reflect")) {
            self.rebinds = true;
            return;
        }
        let (target, this) = if method == "apply" && global("Reflect") {
            (
                call.arguments.first().and_then(Argument::as_expression),
                call.arguments.get(1),
            )
        } else if matches!(method, "call" | "apply" | "bind") {
            (Some(&member.object), call.arguments.first())
        } else {
            return;
        };
        let Some(this) = this.and_then(given) else {
            return;
        };
        match this.get_inner_expression() {
            Expression::NullLiteral(_) => return,
            Expression::Identifier(id) if id.name.as_str() == "undefined" && resolved(self.scoping, id).is_none() => {
                return;
            }
            _ => {}
        }
        let this_itself = matches!(this.get_inner_expression(), Expression::ThisExpression(_));
        match target.map(Expression::get_inner_expression) {
            // A global or an import is no function of the module.
            Some(Expression::Identifier(id)) => self.rebound.extend(local_symbol(self.scoping, id)),
            // A method of a global (`Object.prototype.hasOwnProperty.call(…)`) is no function of the module either.
            Some(other) if reach(other).is_some_and(|(id, _, _)| resolved(self.scoping, id).is_none()) => {}
            _ => self.rebinds |= !this_itself,
        }
    }

    /// Whether a write through `this` in a function may change a value of the module, as what `this` is there says: the
    /// `this` of the module, of a static method and `super` may be such a value; the instance a constructor or a field
    /// initialiser makes is new, but for a write through a key of it (`this.list.push(x)`) or a call given it; what a
    /// static block or a static field initialiser runs with is the class, when it is defined, not by a call; a function
    /// that is no method has a `this` of a value only when it is handed out or bound to one ([`Self::handed_out`]);
    /// and the object a method is in, or the instance whose method it is, is one only when code the module runs when
    /// it is loaded makes it ([`Self::loaded_makers`]): one a call makes (the hooks of a plugin a factory returns) is
    /// new. Where code may call a method on an object other than its own ([`Self::rebinds`]), every method may change
    /// such a value.
    fn this_changes_modules(&self) -> bool {
        let rebinds = self.rebinds
            || self
                .rebound
                .iter()
                .any(|&symbol| !self.makers.values().any(|maker| maker.binding == Some(symbol)));
        let loaded = self.loaded_makers();
        self.this_writes.iter().any(|&(site, keyed)| match site {
            ThisSite::Module | ThisSite::Class | ThisSite::Super => true,
            ThisSite::Load => false,
            ThisSite::Instance => !keyed,
            ThisSite::Plain(binding, scope) => rebinds || self.handed_out(binding, scope),
            ThisSite::Made(maker) => rebinds || loaded.contains(&maker),
        })
    }

    /// Whether the function of the scope `scope`, bound to `binding`, may be called with a value of a module as `this`:
    /// it is handed out (exported, given to a name or a value, read other than as what a call calls) or bound
    /// (`add.bind(base)`); an anonymous one unless it is called where it is written.
    fn handed_out(&self, binding: Option<SymbolId>, scope: ScopeId) -> bool {
        let Some(symbol) = binding else {
            return !self.iifes.contains(&scope);
        };
        self.exported.contains(&symbol)
            || self.default_exports.contains(&scope)
            || self.rebound.contains(&symbol)
            || self.scoping.get_resolved_reference_ids(symbol).iter().any(|reference| {
                self.scoping.get_reference(*reference).is_value() && !self.callee_refs.contains(reference)
            })
    }

    /// The scopes of the module and of the functions and classes code it runs when it is loaded may run or construct
    /// ([`Maker`]): one whose binding is read in such code, and one with no binding written in such code, but for the
    /// default export, which a module importing it calls.
    fn loaded_makers(&self) -> HashSet<ScopeId> {
        let root = self.scoping.root_scope_id();
        let mut loaded: HashSet<ScopeId> = HashSet::default();
        loaded.insert(root);
        loop {
            let mut grew = false;
            for (&scope, maker) in &self.makers {
                if loaded.contains(&scope) {
                    continue;
                }
                let made = match maker.binding {
                    Some(symbol) => self
                        .scoping
                        .get_resolved_reference_ids(symbol)
                        .iter()
                        .any(|&reference| {
                            loaded.contains(&self.maker_of(self.scoping.get_reference(reference).scope_id()))
                        }),
                    None => !maker.default_export && loaded.contains(&maker.enclosing),
                };
                if made {
                    loaded.insert(scope);
                    grew = true;
                }
            }
            if !grew {
                return loaded;
            }
        }
    }

    /// The scope of the innermost function, class or module that holds the scope `scope` ([`Self::makers`]).
    fn maker_of(&self, scope: ScopeId) -> ScopeId {
        let root = self.scoping.root_scope_id();
        self.scoping
            .scope_ancestors(scope)
            .find(|ancestor| *ancestor == root || self.makers.contains_key(ancestor))
            .unwrap_or(root)
    }

    /// Records a change of each object a call is given, of what a spread argument holds, and of each part of a local
    /// a literal argument holds ([`given_parts`]).
    fn mutate_arguments(&mut self, arguments: &[Argument<'_>]) {
        for argument in arguments.iter().filter_map(given) {
            self.note_this(argument, false);
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
            if let Some(receiver) = receiver {
                let symbols: Vec<SymbolId> = names.iter().map(|&(name, _)| name).collect();
                self.bind_this(&symbols, receiver);
            }
            for (name, first) in names {
                if let Some(list) = list {
                    if first {
                        let loading = self.loading();
                        self.element_links.push(Link {
                            a: Place::of(name),
                            b: Place::of(list),
                            join: Join::Within,
                            loading,
                            ends: Ends::default(),
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

    /// Whether `expr` is a name or a member chain that starts from a binding of a module that may change a value of a
    /// module ([`Self::changing_binding`]): `tweak`, `shared.tweak`, `base.plugins`.
    fn of_changing_module(&self, expr: &Expression<'_>) -> bool {
        reach(expr)
            .and_then(|(id, _, _)| self.symbol(id))
            .is_some_and(|symbol| self.changing_binding(symbol))
    }

    /// Whether `symbol` is a binding of a module whose functions may change a value of a module: one the evaluator
    /// follows that may ([`Imports::changes_modules`]), or a package that is neither followed nor installed
    /// ([`Imports::is_installed`]), an alias of a project directory whose code is not known.
    fn changing_binding(&self, symbol: SymbolId) -> bool {
        let Some((module, _)) = self.known.modules.get(&symbol) else {
            return false;
        };
        if self.known.followed.contains(&symbol) {
            self.imports.changes_modules(module)
        } else {
            is_package(module) && !self.imports.is_installed(module)
        }
    }

    /// Records the name a call, a `new` or a tagged template calls, or the name its member chain starts from
    /// ([`Self::callee_refs`]).
    fn note_callee(&mut self, callee: &Expression<'_>) {
        if let Some(reference) = reach(callee).and_then(|(id, _, _)| id.reference_id.get()) {
            self.callee_refs.insert(reference);
        }
    }

    /// The modules the evaluator follows that the module hands out a binding of other than by calling it, by specifier
    /// ([`Unstable::exposed_modules`]), and whether it hands out so a binding of a package neither followed nor
    /// installed.
    fn exposed(&self) -> (Vec<String>, bool) {
        let mut specifiers: Vec<String> = Vec::new();
        let mut unknown = false;
        for (&symbol, (module, _)) in &self.known.modules {
            let handed_out = self.scoping.get_resolved_reference_ids(symbol).iter().any(|reference| {
                self.scoping.get_reference(*reference).is_value() && !self.callee_refs.contains(reference)
            });
            if !handed_out {
                continue;
            }
            if self.known.followed.contains(&symbol) {
                if !specifiers.contains(module) {
                    specifiers.push(module.clone());
                }
            } else {
                unknown |= is_package(module) && !self.imports.is_installed(module);
            }
        }
        specifiers.sort();
        (specifiers, unknown)
    }

    /// Whether `call` may run code of a module that changes a value of a module ([`Self::changing_binding`]): of a
    /// module the evaluator follows that may, or of a package neither followed nor installed, whose code is not known.
    /// It calls a function of one (`tweak()`, `shared.tweak()`, a method of an imported value), or its effect is not
    /// known and it is given a binding of one, which may be a function it calls.
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

    /// Records the change `write` of the object `symbol` holds, reached through its name.
    fn changed(&mut self, symbol: SymbolId, write: Write) {
        self.changed_as(symbol, write, false);
    }

    /// Records the change `write` of the object `symbol` holds, or of what a call of it returns when `result`
    /// ([`Held::result`]).
    fn changed_as(&mut self, symbol: SymbolId, write: Write, result: bool) {
        self.changed_in(self.current_function(), symbol, write, result);
    }

    /// [`Self::changed_as`] by code of the function (or module) of the scope `function`.
    fn changed_in(&mut self, function: ScopeId, symbol: SymbolId, write: Write, result: bool) {
        self.written_in(function, symbol, Change::Contents(vec![write.clone()]), result);
        self.mutated.push((symbol, write));
    }

    /// Whether `item` is a function the module declares, held as itself rather than as what a call of it returns.
    fn holds_function(&self, item: &Held) -> bool {
        !item.result && self.callables.contains(item.symbol)
    }

    /// Links `hub` to each part a value it holds may hold: at the place in the value it is found, as it is found
    /// there, when `exact`, and anywhere at or below the hub otherwise. What a name of a function the module declares
    /// holds (`f.cache = base`, `add.bind(null, base)`) is held by the function itself, for every call of it, also
    /// when code the module runs when it is loaded links it ([`Link::loading`]).
    fn hold(&mut self, hub: &Place, exact: bool, held: Vec<Held>) {
        let loading = self.loading() && !self.callables.contains(hub.symbol);
        self.link_held(hub, exact, held, loading);
    }

    /// [`Self::hold`], the links made as `loading` says ([`Link::loading`]).
    fn link_held(&mut self, hub: &Place, exact: bool, held: Vec<Held>, loading: bool) {
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
                ends: Ends {
                    returned: [self.context.defining && !item.parameter, item.result],
                    parameter: item.parameter,
                },
            });
        }
    }

    /// Links the parts that values with no binding to hold them may share among themselves (`f(a, b)`); a function
    /// the module declares among them may keep the others for every call of it (`bind(add, base)`).
    fn link_among(&mut self, held: &[Held]) {
        let loading = self.loading() && !held.iter().any(|item| self.holds_function(item));
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
                ends: Ends {
                    returned: [pair[0].result, pair[1].result],
                    parameter: false,
                },
            });
        }
    }

    /// Links `hub`, when there is one, to what `held` holds, and what `held` holds among itself otherwise. A hub that
    /// is the function a call calls (`callee`) stands for what its parameters take of the call, which holds for that
    /// call only.
    fn hold_or_link(&mut self, hub: Option<(Place, bool)>, held: Vec<Held>, callee: bool) {
        match hub {
            Some((place, exact)) if callee => {
                let loading = self.loading();
                self.link_held(&place, exact, held, loading);
            }
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
        if !self.context.in_const {
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
                    ends: Ends::default(),
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
            let targets: HashSet<SymbolId> = pending.iter().map(|link| link.b.symbol).collect();
            let reached = self.reached_by_changes(&targets);
            let (linked, rest): (Vec<Link>, Vec<Link>) =
                pending.into_iter().partition(|link| reached.contains(&link.b.symbol));
            pending = rest;
            if linked.is_empty() {
                break;
            }
            self.links.extend(linked);
        }
    }

    /// The bindings of `targets` a change made so far reaches through the links, each alias counted as linked to the
    /// local it takes a part of, as which the evaluator follows is not known yet. A reach stays within the class of the
    /// changed binding ([`Self::class_of`]), so only the changes of a class holding a target are followed, and none of
    /// a class an earlier change reached in full.
    fn reached_by_changes(&mut self, targets: &HashSet<SymbolId>) -> HashSet<SymbolId> {
        let base = self.link_aliases_for_now();
        let links_of = self.links_of();
        let mut reached: HashSet<SymbolId> = HashSet::default();
        let mut in_full: HashSet<SymbolId> = HashSet::default();
        for (symbol, write) in &self.mutated {
            if reached.len() == targets.len() {
                break;
            }
            if in_full.contains(symbol) {
                continue;
            }
            let Some(class) = self.class_of(*symbol) else {
                if targets.contains(symbol) {
                    reached.insert(*symbol);
                }
                continue;
            };
            if !targets.iter().any(|target| class.binary_search(target).is_ok()) {
                continue;
            }
            let found: Vec<SymbolId> = match self.reach_of(&links_of, *symbol, write) {
                Reach::InFull(class) => {
                    in_full.extend(class.iter().copied());
                    class.to_vec()
                }
                Reach::Places(places) => places.into_iter().map(|(other, _)| other).collect(),
            };
            reached.extend(found.into_iter().filter(|other| targets.contains(other)));
        }
        self.unlink_since(base);
        reached
    }

    /// Links each alias to the local it takes a part of, until [`Self::unlink_since`] is given the number of links
    /// returned, as which aliases the evaluator follows is not known yet.
    fn link_aliases_for_now(&mut self) -> usize {
        let base = self.links.len();
        self.links.extend(self.aliases.iter().map(|&(alias, root)| Link {
            a: Place::of(alias),
            b: Place::of(root),
            join: Join::Within,
            loading: false,
            ends: Ends::default(),
        }));
        base
    }

    /// Takes back the links made after the first `base` ([`Self::link_aliases_for_now`]).
    fn unlink_since(&mut self, base: usize) {
        self.links.truncate(base);
        self.classes.replace(None);
    }

    /// Drops the links and changes of bindings that hold a primitive value, which share no object.
    fn drop_primitives(&mut self) {
        let primitives = &self.primitives;
        self.links
            .retain(|link| !primitives.contains(&link.a.symbol) && !primitives.contains(&link.b.symbol));
        self.mutated.retain(|(symbol, _)| !primitives.contains(symbol));
        self.changes.retain(|symbol, _| !primitives.contains(symbol));
        self.function_writes
            .retain(|(symbol, _, _, _)| !primitives.contains(symbol));
    }

    /// The indices of the links of each binding.
    fn links_of(&self) -> HashMap<SymbolId, Vec<usize>> {
        self.links_of_where(|_| true)
    }

    /// The indices of the links `keep` keeps of each binding.
    fn links_of_where(&self, keep: impl Fn(&Link) -> bool) -> HashMap<SymbolId, Vec<usize>> {
        let mut out: HashMap<SymbolId, Vec<usize>> = HashMap::default();
        for (index, link) in self.links.iter().enumerate().filter(|(_, link)| keep(link)) {
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
        self.reach_of(links_of, symbol, origin).into_places()
    }

    /// [`Self::reached`], past [`MAX_PLACES`] places the class of bindings linked to `symbol` ([`Self::class_of`]). A
    /// change is not followed back through the link it came by: there it changes the objects it came from as they
    /// were changed, which through a [`Join::Within`] link would stand for a change of all they hold (a new key of what
    /// `make()` returns reaching all a function returning `make().grow` returns, and from it all `make` returns). A
    /// change that one found there covers, but for the link that one came by, has the covering one followed back too.
    /// Nor is a change that reaches a function from a name one of its parameters binds followed to what a call of it
    /// returns ([`Ends`]): the name takes what a call gives (`function create({ production })`), not what it returns,
    /// so a change of it reaches the arguments of the calls but not the objects the function makes and returns. Nor
    /// does a change found there so cover one that reaches the function otherwise (`return config` of a `config` that
    /// holds the name), which is followed on its own: the wider one, followed on, would reach those objects in full.
    fn reach_of(&self, links_of: &HashMap<SymbolId, Vec<usize>>, symbol: SymbolId, origin: &Write) -> Reach {
        let mut seen = vec![(symbol, origin.clone())];
        // For each place of `seen`, the link it was reached by and the side of it the change is not followed to.
        let mut back: Vec<Option<(usize, usize)>> = vec![None];
        // For each place of `seen`, whether it was reached from a name a parameter of the function there binds.
        let mut taken = vec![false];
        let mut by_symbol: HashMap<SymbolId, Vec<usize>> = HashMap::default();
        by_symbol.entry(symbol).or_default().push(0);
        let mut queue = vec![0];
        while let Some(place) = queue.pop() {
            let (at, write) = seen[place].clone();
            for &index in links_of.get(&at).map_or(&[][..], Vec::as_slice) {
                let link = &self.links[index];
                for (side, (from, to)) in [(&link.a, &link.b), (&link.b, &link.a)].into_iter().enumerate() {
                    if from.symbol != at
                        || back[place] == Some((index, side))
                        || (taken[place] && link.ends.returned[side])
                    {
                        continue;
                    }
                    let Some(mut next) = through(from, to, link.join, &write) else {
                        continue;
                    };
                    if next.keys.len() > MAX_KEYS {
                        next = Write::contents(next.keys[..MAX_KEYS].to_vec());
                    }
                    let came = Some((index, 1 - side));
                    let from_parameter = side == 1 && link.ends.parameter;
                    let known = by_symbol.entry(to.symbol).or_default();
                    let covering = known
                        .iter()
                        .copied()
                        .filter(|&other| (!taken[other] || from_parameter) && seen[other].1.covers(&next));
                    if covering
                        .clone()
                        .any(|other| back[other].is_none() || back[other] == came)
                    {
                        continue;
                    }
                    if let Some(other) = covering.clone().next() {
                        back[other] = None;
                        queue.push(other);
                        continue;
                    }
                    if seen.len() >= MAX_PLACES {
                        return self
                            .class_of(symbol)
                            .map_or_else(|| Reach::Places(vec![(symbol, Write::default())]), Reach::InFull);
                    }
                    known.push(seen.len());
                    queue.push(seen.len());
                    seen.push((to.symbol, next));
                    back.push(came);
                    taken.push(from_parameter);
                }
            }
        }
        Reach::Places(seen)
    }

    /// The bindings linked to `symbol`, whatever the parts, `symbol` among them, sorted; `None` when it has no link.
    fn class_of(&self, symbol: SymbolId) -> Option<Rc<[SymbolId]>> {
        let mut cached = self.classes.borrow_mut();
        if cached.as_ref().is_none_or(|(count, _)| *count != self.links.len()) {
            *cached = Some((self.links.len(), classes(&self.pairs())));
        }
        cached.as_ref().and_then(|(_, classes)| classes.get(&symbol).cloned())
    }

    /// Marks every part of a binding that may share an object with a changed one, other than the part of the binding
    /// the change is made through, which the evaluator changes itself, and returns the aliases the evaluator follows.
    /// A change through one of those reaches the part of the local it takes, which the evaluator changes too and which
    /// is not marked, and through it what else may share an object with that part; a local of the alias's function is
    /// marked from the alias's declaration on ([`Self::late`]). Returns with them whether a function of the module may
    /// change a value of the module ([`Self::writes_modules`]).
    fn share(&mut self) -> (HashSet<SymbolId>, bool) {
        self.run_implicitly();
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
                ends: Ends::default(),
            });
        }
        let mut origins: Vec<(SymbolId, Write)> = Vec::new();
        for origin in &self.mutated {
            if !origins.contains(origin) {
                origins.push(origin.clone());
            }
        }
        let links_of = self.links_of();
        let mut in_full: HashMap<SymbolId, Option<SymbolId>> = HashMap::default();
        for (symbol, origin) in origins {
            let through_alias = applied.iter().find(|alias| alias.alias == symbol);
            // A reach stays within the class of `symbol`: once every binding of it holds a change in full, a change by
            // an origin that is no alias changes nothing more, so its reach is not walked.
            if through_alias.is_none()
                && self
                    .class_of(symbol)
                    .is_some_and(|class| in_full.get(&class[0]) == Some(&None))
            {
                continue;
            }
            let reach = self.reach_of(&links_of, symbol, &origin);
            if let (None, Reach::InFull(class)) = (through_alias, &reach) {
                let left = origin.covers(&Write::default()).then_some(symbol);
                if self.repeats_in_full(&mut in_full, class, left) {
                    continue;
                }
            }
            for (other, write) in reach.into_places() {
                // A change of what a binding that takes what `this` holds holds is a write through `this`.
                if let Some(&site) = self.this_bound.get(&other) {
                    self.wrote_this(site, false);
                }
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

    /// Whether a change in full of `class` by an origin that is not an alias, which leaves the binding `left` of the
    /// class as it is when there is one, repeats one an earlier origin made, which [`Self::share`] then skips.
    /// `in_full` holds, by its first binding, each class changed so, with the binding the changes so far left as it
    /// is, if any: every other binding of the class holds a change of all it holds, which a later change in full of
    /// the class leaves as it is, so only that binding is changed when this change does not leave it too.
    fn repeats_in_full(
        &mut self,
        in_full: &mut HashMap<SymbolId, Option<SymbolId>>,
        class: &[SymbolId],
        left: Option<SymbolId>,
    ) -> bool {
        let Some(unchanged) = in_full.get_mut(&class[0]) else {
            in_full.insert(class[0], left);
            return false;
        };
        if let Some(member) = *unchanged
            && Some(member) != left
        {
            self.change(member, Change::Contents(vec![Write::default()]));
            *unchanged = None;
        }
        true
    }

    /// Whether code in a function of the module, run by a call of it, may change a value of the module: a top-level
    /// binding or an import, or an object one of them may hold. A change counts when it is made in a function
    /// ([`Self::function_writes`]) and is one of such a binding, or reaches one through the links (see
    /// [`Self::reached`]). A change of a binding of the function it is made in (a local, a parameter) or of what a
    /// call of a function of the module returns is one of an object made or given anew by each call, so it is followed
    /// only through the links that hold for every call ([`Self::fresh_reach_modules`]), not those code run when the
    /// module is loaded made of what one call made (`export const config = create()`): a factory that builds and
    /// returns new objects changes none, and a change of what a parameter takes is one of what a call gives it, which
    /// the call accounts for. That is not so in a generator or an `async` function, whose code runs later with what
    /// the call that made it gave it ([`Self::suspending`]), nor for a change through the name of a function of the
    /// module (`f.cache.plugins.push(p)`), which is one of the function object itself: such a change, and one of a
    /// binding of an enclosing function, which a function made by an earlier call may make later, is followed through
    /// every link ([`Self::closure_reaches_modules`]). A write through `this` or `super` in a function counts where
    /// `this` may be such a value ([`Self::this_changes_modules`]), as what it is depends on how the function is
    /// called. In a module that calls `eval`, reads `arguments` or has a `with` statement, any change in a function
    /// counts.
    fn writes_modules(&self, links_of: &HashMap<SymbolId, Vec<usize>>) -> bool {
        if self.this_changes_modules() {
            return true;
        }
        if self.function_writes.is_empty() {
            return false;
        }
        if self.dynamic {
            return true;
        }
        let fresh_links_of = self.links_of_where(|link| !link.loading);
        let root = self.scoping.root_scope_id();
        self.function_writes.iter().any(|(symbol, function, change, result)| {
            let home = self.function_of(*symbol);
            let fresh = (home == *function && !self.suspending.contains(function))
                || (home == root && *result && self.callables.contains(*symbol));
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

    /// [`Unstable::result_places`]: the change each value of the module may undergo reaches, through the links that
    /// hold for every call, a part of each top-level function the module declares, which then stands for what a call
    /// of it returns; that part may hold an object of the value, as a change of either reaches the other.
    fn result_places(&self) -> HashMap<String, Vec<Write>> {
        if self.dynamic {
            return HashMap::default();
        }
        let root = self.scoping.root_scope_id();
        let mut places: HashMap<SymbolId, Vec<Write>> = self
            .callables
            .joined
            .iter()
            .chain(self.callables.apart.keys())
            .filter(|&&symbol| self.scoping.symbol_scope_id(symbol) == root)
            .map(|&symbol| (symbol, Vec::new()))
            .collect();
        let fresh_links_of = self.links_of_where(|link| !link.loading);
        let mut values: Vec<SymbolId> = fresh_links_of
            .keys()
            .copied()
            .filter(|&symbol| self.is_module_value(symbol))
            .collect();
        values.sort_unstable();
        for value in values {
            for (other, write) in self.reached(&fresh_links_of, value, &Write::default()) {
                if let Some(writes) = places.get_mut(&other) {
                    add_write(writes, write);
                }
            }
        }
        places
            .into_iter()
            .map(|(symbol, mut writes)| {
                writes.sort();
                (self.scoping.symbol_name(symbol).to_owned(), writes)
            })
            .collect()
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
                ends: Ends::default(),
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
                        ends: Ends::default(),
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
        let scope = it.scope_id.get().unwrap_or_else(|| self.scoping.root_scope_id());
        if it.generator || it.r#async {
            self.suspending.insert(scope);
        }
        let site = self.enter_function(scope, it.id.as_ref().and_then(|id| id.symbol_id.get()));
        self.this_sites.push(site);
        self.functions.push(scope);
        walk::walk_function(self, it, flags);
        self.functions.pop();
        self.this_sites.pop();
    }

    fn visit_arrow_function_expression(&mut self, it: &ArrowFunctionExpression<'a>) {
        let scope = it.scope_id.get().unwrap_or_else(|| self.scoping.root_scope_id());
        if it.r#async {
            self.suspending.insert(scope);
        }
        // An arrow function has the `this` of where it is.
        self.add_maker(scope, self.named_inits.get(&scope).copied());
        self.functions.push(scope);
        walk::walk_arrow_function_expression(self, it);
        self.functions.pop();
    }

    fn visit_class(&mut self, it: &Class<'a>) {
        // A static block or a field initialiser runs when the class is defined or an object of it is made, which the
        // evaluator does not follow: a write there is one of no function the binding belongs to.
        // Its methods are those an instance has of its own ([`Implicit::owners`]).
        self.implicit.containers.push(it.span);
        if let Some(scope) = it.scope_id.get() {
            let binding = self
                .named_inits
                .get(&scope)
                .copied()
                .or_else(|| it.id.as_ref().and_then(|id| id.symbol_id.get()));
            self.add_maker(scope, binding);
            self.functions.push(scope);
            walk::walk_class(self, it);
            self.functions.pop();
        } else {
            walk::walk_class(self, it);
        }
        self.implicit.containers.pop();
    }

    fn visit_method_definition(&mut self, it: &MethodDefinition<'a>) {
        if let Some(scope) = it.value.scope_id.get() {
            let site = if matches!(it.kind, MethodDefinitionKind::Constructor) {
                ThisSite::Instance
            } else if it.r#static {
                ThisSite::Class
            } else {
                ThisSite::Made(
                    self.functions
                        .last()
                        .copied()
                        .unwrap_or_else(|| self.scoping.root_scope_id()),
                )
            };
            self.method_sites.insert(scope, site);
            if !it.r#static && it.kind != MethodDefinitionKind::Constructor {
                let method = (it.kind == MethodDefinitionKind::Method).then(|| key_name(&it.key));
                self.implicit.own(scope, method.flatten());
            }
        }
        walk::walk_method_definition(self, it);
    }

    fn visit_property_definition(&mut self, it: &PropertyDefinition<'a>) {
        // An instance field holds a value of its own at its key, whatever method the class has there.
        if !it.r#static
            && let Some(value) = &it.value
        {
            self.note_key_value(key_name(&it.key), value);
        }
        self.this_sites.push(if it.r#static {
            ThisSite::Load
        } else {
            ThisSite::Instance
        });
        walk::walk_property_definition(self, it);
        self.this_sites.pop();
    }

    fn visit_static_block(&mut self, it: &StaticBlock<'a>) {
        self.this_sites.push(ThisSite::Load);
        walk::walk_static_block(self, it);
        self.this_sites.pop();
    }

    fn visit_object_property(&mut self, it: &ObjectProperty<'a>) {
        // A method, a getter, a setter or a function value has the object as `this` when it is called on it.
        if let Expression::FunctionExpression(function) = it.value.get_inner_expression()
            && let Some(scope) = function.scope_id.get()
        {
            let maker = self
                .functions
                .last()
                .copied()
                .unwrap_or_else(|| self.scoping.root_scope_id());
            self.method_sites.insert(scope, ThisSite::Made(maker));
            if !self.implicit.accessors.described.contains(&scope) {
                let method = (it.kind == PropertyKind::Init).then(|| key_name(&it.key));
                self.implicit.own(scope, method.flatten());
            }
        } else if it.kind == PropertyKind::Init {
            self.note_key_value(key_name(&it.key), &it.value);
        }
        walk::walk_object_property(self, it);
    }

    fn visit_export_default_declaration(&mut self, it: &ExportDefaultDeclaration<'a>) {
        let scope = match &it.declaration {
            ExportDefaultDeclarationKind::FunctionDeclaration(function) => function.scope_id.get(),
            ExportDefaultDeclarationKind::ClassDeclaration(class) => class.scope_id.get(),
            other => match other.as_expression().map(Expression::get_inner_expression) {
                Some(Expression::FunctionExpression(function)) => function.scope_id.get(),
                Some(Expression::ArrowFunctionExpression(function)) => function.scope_id.get(),
                Some(Expression::ClassExpression(class)) => class.scope_id.get(),
                _ => None,
            },
        };
        self.default_exports.extend(scope);
        walk::walk_export_default_declaration(self, it);
    }

    fn visit_member_expression(&mut self, it: &MemberExpression<'a>) {
        // A read or a write of a key may run a getter or a setter of the object.
        if !self.implicit.accessors.is_empty() {
            self.note_implicit_site(it.object(), vec![Run::Key(member_key(it))], On::Itself);
        }
        walk::walk_member_expression(self, it);
    }

    fn visit_object_expression(&mut self, it: &ObjectExpression<'a>) {
        // A spread reads the own keys of what it spreads. A key written before one may be given a value of the spread
        // object, whatever method the literal has there.
        let spreads = it
            .properties
            .iter()
            .any(|property| matches!(property, ObjectPropertyKind::SpreadProperty(_)));
        for property in &it.properties {
            match property {
                ObjectPropertyKind::SpreadProperty(spread) => {
                    self.note_implicit_site(&spread.argument, vec![Run::Key(None)], On::Itself);
                }
                ObjectPropertyKind::ObjectProperty(p) if spreads => self.implicit.replace(key_name(&p.key)),
                ObjectPropertyKind::ObjectProperty(_) => {}
            }
        }
        self.implicit.containers.push(it.span);
        walk::walk_object_expression(self, it);
        self.implicit.containers.pop();
    }

    fn visit_array_expression(&mut self, it: &ArrayExpression<'a>) {
        // A spread in a list iterates what it spreads.
        for element in &it.elements {
            if let ArrayExpressionElement::SpreadElement(spread) = element {
                self.note_implicit_site(&spread.argument, vec![Run::Iteration], On::Itself);
            }
        }
        walk::walk_array_expression(self, it);
    }

    fn visit_yield_expression(&mut self, it: &YieldExpression<'a>) {
        if it.delegate
            && let Some(argument) = &it.argument
        {
            self.note_implicit_site(argument, vec![Run::Iteration], On::Itself);
        }
        walk::walk_yield_expression(self, it);
    }

    fn visit_static_member_expression(&mut self, it: &StaticMemberExpression<'a>) {
        // The methods of a prototype of the module's own (`Registry.prototype`) may be called on it.
        let name = it.property.name.as_str();
        if matches!(name, "prototype" | "__proto__")
            && reach(&it.object).is_some_and(|(id, _, _)| resolved(self.scoping, id).is_some())
        {
            self.rebinds = true;
        }
        walk::walk_static_member_expression(self, it);
    }

    fn visit_variable_declaration(&mut self, it: &VariableDeclaration<'a>) {
        let saved = std::mem::replace(&mut self.context.in_const, it.kind == VariableDeclarationKind::Const);
        walk::walk_variable_declaration(self, it);
        self.context.in_const = saved;
    }

    fn visit_variable_declarator(&mut self, it: &VariableDeclarator<'a>) {
        if let (BindingPattern::BindingIdentifier(id), Some(init)) = (&it.id, &it.init) {
            let scope = match init.get_inner_expression() {
                Expression::ArrowFunctionExpression(function) => function.scope_id.get(),
                Expression::FunctionExpression(function) => function.scope_id.get(),
                Expression::ClassExpression(class) => class.scope_id.get(),
                _ => None,
            };
            if let (Some(scope), Some(symbol)) = (scope, id.symbol_id.get()) {
                self.named_inits.insert(scope, symbol);
            }
        }
        if let Some(init) = &it.init {
            self.bind_this(&bound_symbols(&it.id), init);
            self.note_destructured(init, pattern_runs(|pattern| pattern.visit_binding_pattern(&it.id)));
        }
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
            if whole && self.context.in_const && is_primitive(self.names(), init) {
                self.primitives.extend(names.iter().copied());
            }
            if whole
                && self.context.in_const
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
        let mut places = Vec::new();
        let mut defaults = Vec::new();
        self.target_parts(&it.left, &mut places, &mut defaults);
        let targets: Vec<SymbolId> = places.iter().map(|place| place.symbol).collect();
        self.bind_this(&targets, &it.right);
        self.note_destructured(
            &it.right,
            pattern_runs(|pattern| pattern.visit_assignment_target(&it.left)),
        );
        if let Some(member) = it
            .left
            .as_simple_assignment_target()
            .and_then(|target| target.as_member_expression())
        {
            self.note_key_value(member_key(member), &it.right);
        }
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
                        .map_or_default(|member| self.member_places(member)),
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
        // Each element the loop takes is destructured as its pattern says.
        self.note_implicit_site(&it.right, vec![Run::Iteration], On::Itself);
        let elements = pattern_runs(|pattern| pattern.visit_for_statement_left(&it.left));
        self.note_implicit_site(&it.right, elements.into_runs().collect(), On::Below);
        match &it.left {
            ForStatementLeft::VariableDeclaration(declaration) => {
                for declarator in &declaration.declarations {
                    self.bind_this(&bound_symbols(&declarator.id), &it.right);
                    for name in bound_symbols(&declarator.id) {
                        match source {
                            Some(list) => {
                                let loading = self.loading();
                                self.element_links.push(Link {
                                    a: Place::of(name),
                                    b: Place::of(list),
                                    join: Join::Within,
                                    loading,
                                    ends: Ends::default(),
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
            self.bind_this(&bound_symbols(&it.pattern), initializer);
            self.default_of(&it.pattern, initializer);
            self.note_destructured(
                initializer,
                pattern_runs(|pattern| pattern.visit_binding_pattern(&it.pattern)),
            );
        }
        walk::walk_formal_parameter(self, it);
    }

    fn visit_assignment_pattern(&mut self, it: &AssignmentPattern<'a>) {
        self.default_of(&it.left, &it.right);
        self.note_destructured(
            &it.right,
            pattern_runs(|pattern| pattern.visit_binding_pattern(&it.left)),
        );
        walk::walk_assignment_pattern(self, it);
    }

    fn visit_simple_assignment_target(&mut self, it: &SimpleAssignmentTarget<'a>) {
        match it {
            SimpleAssignmentTarget::AssignmentTargetIdentifier(id) => {
                if let Some(symbol) = self.symbol(id) {
                    self.written(symbol, Change::Any, false);
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
            self.written(symbol, Change::Any, false);
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
        self.note_callee(&it.callee);
        self.note_rebinding(it);
        match it.callee.get_inner_expression() {
            Expression::FunctionExpression(function) => self.iifes.extend(function.scope_id.get()),
            Expression::ArrowFunctionExpression(function) => self.iifes.extend(function.scope_id.get()),
            _ => {}
        }
        if foreign(self.names(), &it.callee) {
            self.foreign_calls.insert(it.span);
        }
        self.runs_modules |= self.runs_followed(it);
        let calls_back = self.calls_back_unknown(it);
        if calls_back {
            self.calls_back.insert(it.span);
        }
        // Code the pass does not visit may read the keys of what the call is given or iterate it.
        let effect = self.effect(it);
        self.note_call_sites(it, effect);
        // A call whose effect the pass does not know may change what it is given, wherever it is made.
        match effect {
            Some(effect) => {
                // A function the module declares may change what `this` holds when it is given it.
                if effect == Effect::Followed {
                    for argument in it.arguments.iter().filter_map(given) {
                        self.note_this(argument, false);
                    }
                }
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
        // themselves: what it returns holds them only as the value holding the call says. So does `Object.assign` of
        // an object literal, which copies the keys of the others into it.
        if self.keeps_apart(it) || assigns_fresh(self.names(), it) {
            return walk::walk_call_expression(self, it);
        }
        // A function the module declares passes each argument to the parameter that takes it when they are kept apart,
        // and stands for its parameters otherwise; any other function, and a method in its object, may keep what it
        // is given anywhere.
        let callee_hub = matches!(it.callee.get_inner_expression(), Expression::Identifier(_));
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
        self.hold_or_link(hub, held, callee_hub);
        walk::walk_call_expression(self, it);
    }

    fn visit_new_expression(&mut self, it: &NewExpression<'a>) {
        self.note_callee(&it.callee);
        self.runs_modules |= self.of_changing_module(&it.callee);
        if !self.implicit.accessors.is_empty() {
            self.note_argument_sites(&it.arguments, new_runs(self.scoping, it).as_ref());
        }
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
        self.note_callee(&it.tag);
        self.runs_modules |= self.of_changing_module(&it.tag);
        // The tag is given each value, which the pass does not follow into its parameters, also for a tag the module
        // declares.
        for value in &it.quasi.expressions {
            self.note_implicit_site(value, vec![Run::Every], On::Itself);
        }
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

/// The parts of locals an expression's value may hold or share: the names and member chains it reads, other than a
/// function imported from a module the evaluator does not follow, what holds no object (a template, an operator's result, the result of one of [`PURE_FUNCTIONS`]
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
                result: false,
                parameter: true,
            }));
    }

    /// Records the part of the local `id` at `keys`, which is the value here when `exact`.
    fn hold(&mut self, id: &IdentifierReference<'_>, keys: Vec<String>, exact: bool) {
        self.hold_as(id, keys, Join::of(self.exact && exact));
    }

    /// Records the part of the local `id` at `keys`, held here as `join` says.
    fn hold_as(&mut self, id: &IdentifierReference<'_>, keys: Vec<String>, join: Join) {
        self.push(id, keys, join, false);
    }

    /// Records what a call of the function the module declares that `id` names returns, which is the value here.
    fn hold_result(&mut self, id: &IdentifierReference<'_>) {
        self.push(id, Vec::new(), Join::of(self.exact), true);
    }

    /// Records the part of the local `id` at `keys`, held here as `join` says, as what a call of it returns when
    /// `result` ([`Held::result`]).
    fn push(&mut self, id: &IdentifierReference<'_>, keys: Vec<String>, join: Join, result: bool) {
        if self.part == Part::Body {
            return;
        }
        if let Some(symbol) = resolved(self.scoping, id) {
            self.out.push(Held {
                symbol,
                keys,
                at: self.at.clone(),
                join,
                result,
                parameter: false,
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

    /// Records the bindings the function value `callee` is read from, whole: a name or the name a member chain starts
    /// from, in each value a sequence, a condition or a logical operator may be; any other callee as its value.
    fn callee_roots<'a>(&mut self, callee: &Expression<'a>)
    where
        Self: Visit<'a>,
    {
        match callee.get_inner_expression() {
            Expression::SequenceExpression(sequence) => {
                if let Some(last) = sequence.expressions.last() {
                    self.callee_roots(last);
                }
            }
            Expression::ConditionalExpression(c) => {
                self.callee_roots(&c.consequent);
                self.callee_roots(&c.alternate);
            }
            Expression::LogicalExpression(l) => {
                self.callee_roots(&l.left);
                self.callee_roots(&l.right);
            }
            other => match reach(other) {
                Some((id, _, _)) => self.hold(id, Vec::new(), false),
                None => self.visit_expression(other),
            },
        }
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
        // `Object.assign({ … }, a, b)` is the literal it is given first with the own keys of the others copied in, as
        // `{ …, ...a, ...b }` is.
        if assigns_fresh(names, it) {
            for (index, argument) in it.arguments.iter().enumerate() {
                if let Some(value) = argument.as_expression() {
                    if index == 0 {
                        self.visit_expression(value);
                    } else {
                        self.spread(value);
                    }
                }
            }
            return;
        }
        match it.callee.get_inner_expression() {
            // A function the module declares stands for what it returns, which holds what it is given only as its
            // parameters do, and the call links those to the arguments ([`Callables`]).
            Expression::Identifier(id) if calls_callable(self.scoping, self.callables, it) => {
                return self.hold_result(id);
            }
            // Any other local function called by name stands for what it returns, which may hold what it is given. One
            // imported from a module the evaluator follows may return anything that module holds, such as one of its
            // exports (`getBase()`), which is linked to the other imports of such modules ([`Sharing::link_imports`]),
            // or, when what it returns is known, such a value at the places it says only, none for a factory that
            // builds new objects ([`Known::results`]); one imported from any other module holds none of the module's
            // locals.
            Expression::Identifier(id) => {
                let known = self.known;
                let places = resolved(self.scoping, id).and_then(|symbol| known.results.get(&symbol));
                if let Some(places) = places {
                    for place in places {
                        let depth = self.at.len();
                        if !self.frozen {
                            self.at.extend(place.keys.iter().cloned());
                        }
                        self.opaque(|refs| refs.hold(id, Vec::new(), true));
                        self.at.truncate(depth);
                    }
                } else if is_followed_import(self.scoping, self.known, id) {
                    self.opaque(|refs| refs.hold(id, Vec::new(), true));
                } else if local_symbol(self.scoping, id).is_some() {
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
            // A function read otherwise (`(0, base.self)()`, `(prod ? a.make : b.make)()`) may return anything the
            // bindings it is read from hold, not only what is at its key.
            callee => self.opaque(|refs| refs.callee_roots(callee)),
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
        // An object of a class holds what it is given and what a local class or one of a module the evaluator follows
        // holds; a class imported from any other module holds none of the module's locals.
        self.opaque(|refs| {
            match it.callee.get_inner_expression() {
                Expression::Identifier(id) => {
                    if local_symbol(refs.scoping, id).is_some() || is_followed_import(refs.scoping, refs.known, id) {
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
    /// How the parameters of each of them that destructures an argument do so, by position ([`Destructured`]).
    patterns: HashMap<SymbolId, Vec<Destructured>>,
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
    for (symbol, scope, parameters, patterns) in found {
        if patterns
            .iter()
            .any(|pattern| !pattern.own.is_empty() || !pattern.below.is_empty())
        {
            out.patterns.insert(symbol, patterns);
        }
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
    /// The functions found, each with its scope, its parameters and how they destructure what they take.
    found: Vec<(SymbolId, ScopeId, Parameters, Vec<Destructured>)>,
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
            self.found
                .push((symbol, scope, Parameters::of(params), parameter_patterns(params)));
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

/// Whether the code visited reads `this` (`this`) or `super` (`sup`) of the function it is in, outside the functions
/// and classes it declares other than arrow functions, which have the `this` of where they are.
#[derive(Default)]
struct MentionsThis {
    this: bool,
    sup: bool,
}

impl<'a> Visit<'a> for MentionsThis {
    fn visit_this_expression(&mut self, _: &ThisExpression) {
        self.this = true;
    }

    fn visit_super(&mut self, _: &Super) {
        self.sup = true;
    }

    fn visit_function(&mut self, _: &Function<'a>, _: ScopeFlags) {}

    fn visit_class(&mut self, _: &Class<'a>) {}
}

/// How code may run a function of an object with the object as `this` with no call of it written. For a getter or a
/// setter, a read or a write of its key, any key when it is not static; for an iterator or a method of one (`next`,
/// `return`, `throw`), an iteration. At a place in code, a read or a write of the key given (any key, when it is not
/// static), an iteration (`for…of`, `yield*`, an array destructuring or spread), or either (a call or a tagged
/// template whose code the pass does not visit, given the object).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Run {
    Key(Option<String>),
    Iteration,
    Every,
}

/// Where a place in code runs functions of a value ([`Sharing::note_implicit_site`]): on the value itself, or on a
/// value somewhere below it (an element a loop or a callback takes, a key a nested pattern takes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum On {
    Itself,
    Below,
}

/// What a getter, a setter, an iterator or a method of an object may change of what its `this` holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ThisEffect {
    /// Whether it may change anything the object holds (`this.list.push(x)`), as a call of a method of it may.
    contents: bool,
    /// The keys of the object itself it may give a new value (`this.mode = 'production'`).
    keys: BTreeSet<String>,
}

impl ThisEffect {
    /// A change of anything the object holds.
    fn contents() -> Self {
        Self {
            contents: true,
            keys: BTreeSet::new(),
        }
    }

    /// A new value of the key `key` of the object.
    fn key(key: String) -> Self {
        Self {
            contents: false,
            keys: BTreeSet::from([key]),
        }
    }

    /// Adds what `other` changes, returning whether this one grew.
    fn add(&mut self, other: &Self) -> bool {
        let before = (self.contents, self.keys.len());
        self.contents |= other.contents;
        self.keys.extend(other.keys.iter().cloned());
        before != (self.contents, self.keys.len())
    }

    /// The changes it makes of the part `place` of a local the object may be, which is that object itself when
    /// `exact`: a new value of each key it gives one, or a change of all the part holds where it may change more or
    /// the object is somewhere below the part.
    fn writes(&self, place: &Place, exact: bool) -> Vec<Write> {
        if self.contents || !exact {
            return vec![Write::contents(place.keys.clone())];
        }
        self.keys
            .iter()
            .map(|key| Write {
                keys: place.keys.clone(),
                key: Some(key.clone()),
            })
            .collect()
    }
}

/// What the functions of `functions` change of what `this` holds together, as `effects` says.
fn union_of(functions: &[ScopeId], effects: &HashMap<ScopeId, ThisEffect>) -> ThisEffect {
    let mut all = ThisEffect::default();
    for effect in functions.iter().filter_map(|function| effects.get(function)) {
        all.add(effect);
    }
    all
}

/// The getters, setters and iterators a module declares ([`accessors`]), by how code runs them with no call of them
/// written ([`Run`]).
#[derive(Default)]
struct Accessors {
    /// The getters and setters of each static key.
    keys: BTreeMap<String, Vec<ScopeId>>,
    /// The getters and setters of a key that is not static, which a read or a write of any key may run.
    any_key: Vec<ScopeId>,
    /// The iterators and the methods of an iterator.
    iterators: Vec<ScopeId>,
    /// Those of a descriptor given to `Object.defineProperty` or one of its kin ([`descriptors`]), which run with the
    /// object the call defines them on as `this`, not the descriptor.
    described: HashSet<ScopeId>,
    /// The scopes of the functions of all of them.
    all: HashSet<ScopeId>,
}

impl Accessors {
    /// Adds the function of the scope `scope`, which code runs as `run` says.
    fn add(&mut self, scope: ScopeId, run: Run) {
        if !self.all.insert(scope) {
            return;
        }
        match run {
            Run::Key(Some(key)) => self.keys.entry(key).or_default().push(scope),
            Run::Key(None) => self.any_key.push(scope),
            Run::Iteration | Run::Every => self.iterators.push(scope),
        }
    }

    fn is_empty(&self) -> bool {
        self.all.is_empty()
    }

    /// Whether code at a place that runs functions of an object as `run` says may run one of them.
    fn runs(&self, run: &Run) -> bool {
        match run {
            Run::Key(Some(key)) => !self.any_key.is_empty() || self.keys.contains_key(key),
            Run::Key(None) => !self.any_key.is_empty() || !self.keys.is_empty(),
            Run::Iteration => !self.iterators.is_empty(),
            Run::Every => !self.is_empty(),
        }
    }

    /// Those of them code at a place that runs functions of an object as `runs` say may run, sorted.
    fn ran_by(&self, runs: &[Run]) -> Vec<ScopeId> {
        let mut ran: Vec<ScopeId> = Vec::new();
        for run in runs {
            match run {
                Run::Key(Some(key)) => ran.extend(self.keys.get(key).into_iter().flatten()),
                Run::Key(None) | Run::Every => ran.extend(self.keys.values().flatten()),
                Run::Iteration => {}
            }
            if matches!(run, Run::Key(_) | Run::Every) {
                ran.extend(&self.any_key);
            }
            if matches!(run, Run::Iteration | Run::Every) {
                ran.extend(&self.iterators);
            }
        }
        ran.sort_unstable();
        ran.dedup();
        ran
    }

    /// The place `site` with those of them it may run, when it may run one.
    fn pending(&self, site: ImplicitSite) -> Option<Pending> {
        let ran = self.ran_by(&site.runs);
        (!ran.is_empty()).then(|| Pending {
            site,
            ran,
            applied: ThisEffect::default(),
        })
    }
}

/// What the pass finds of the code a value may run with no call of it written ([`Sharing::run_implicitly`]).
#[derive(Default)]
struct Implicit {
    /// The getters, setters and iterators the module declares.
    accessors: Accessors,
    /// The spans of the object literals and classes being visited, innermost last.
    containers: Vec<Span>,
    /// The object literal or class whose own method, getter, setter or iterator each function of an object is, by the
    /// scope of the function.
    owners: HashMap<ScopeId, Span>,
    /// The methods of objects and classes, which are no getter or setter, by static key.
    methods: HashMap<String, Vec<ScopeId>>,
    /// The keys code gives an object a value that may be a function other than a method the module declares there
    /// (`plugin.apply = apply`), or one before a spread, which may give it any value.
    replaced: HashSet<String>,
    /// Whether code gives such a value to a key of an object that is not static.
    replaces_any: bool,
    /// What each getter, setter, iterator and method of an object changes of what `this` holds by its own code.
    effects: HashMap<ScopeId, ThisEffect>,
    /// The calls of a method on `this` in such a function.
    calls: Vec<ThisCall>,
    /// The places found where code may run a getter, a setter or an iterator with no call of it written.
    sites: Vec<ImplicitSite>,
}

impl Implicit {
    /// Records the function of the scope `scope` as one of the innermost object literal or class being visited, a
    /// method at the key `method` when it is one.
    fn own(&mut self, scope: ScopeId, method: Option<String>) {
        if let Some(&container) = self.containers.last() {
            self.owners.insert(scope, container);
        }
        if let Some(method) = method {
            self.methods.entry(method).or_default().push(scope);
        }
    }

    /// Records that code gives the key `key` of an object (`None` when it is not static) a value other than a method
    /// the module declares there ([`Self::replaced`]).
    fn replace(&mut self, key: Option<String>) {
        match key {
            Some(key) => {
                self.replaced.insert(key);
            }
            None => self.replaces_any = true,
        }
    }

    /// Whether the function of the scope `scope` is a getter, a setter, an iterator or a method of an object, whose
    /// `this` is that object.
    fn is_owned(&self, scope: ScopeId) -> bool {
        self.accessors.all.contains(&scope) || self.owners.contains_key(&scope)
    }

    /// The methods a call of `method` on `this` in the function `caller` may call: every method of that key, wherever
    /// it is, as one may override another, when the object literal or class `caller` is in has one of its own and no
    /// code gives the key another value; `None` when what it calls is not known.
    fn own_methods(&self, caller: ScopeId, method: Option<&str>) -> Option<&[ScopeId]> {
        let method = method.filter(|method| !self.replaces_any && !self.replaced.contains(*method))?;
        let methods = self.methods.get(method)?;
        let container = self.owners.get(&caller)?;
        methods
            .iter()
            .any(|scope| self.owners.get(scope) == Some(container))
            .then_some(methods.as_slice())
    }
}

/// A call of a method on `this` in a getter, a setter, an iterator or a method of an object ([`Implicit::calls`]).
struct ThisCall {
    /// The scope of the function the call is in.
    caller: ScopeId,
    /// The key of the method, `None` when it is not static.
    method: Option<String>,
    /// What `this` is there.
    site: ThisSite,
}

/// A place where code may run a getter, a setter or an iterator of an object with the object as `this` ([`Run`]),
/// found while the module is visited and applied once it is known what they change ([`Sharing::run_implicitly`]).
struct ImplicitSite {
    /// How code there may run them.
    runs: Vec<Run>,
    /// The function (or module) whose code it is.
    function: ScopeId,
    /// The getter, setter, iterator or method of an object whose `this` the code there has, if any
    /// ([`Sharing::owned_function`]).
    owner: Option<ScopeId>,
    /// What `this` and `super` are there, when the object may be what one of them holds, each with whether the object
    /// is what it holds itself.
    this: Vec<(ThisSite, bool)>,
    /// The parts of locals the object may be ([`Sharing::object_parts`]), each with whether the object is that part
    /// itself and whether the part is what a call of a function the module declares returns.
    parts: Vec<(Place, bool, bool)>,
}

/// An [`ImplicitSite`] while [`Sharing::run_implicitly`] applies it: the getters, setters and iterators it may run, and
/// what it has changed so far of the object there.
struct Pending {
    site: ImplicitSite,
    ran: Vec<ScopeId>,
    applied: ThisEffect,
}

/// The bindings that take what `this` holds in a getter, a setter, an iterator or a method of an object, each with
/// that function ([`Sharing::bound_owners`]), and the links a change reaches them through.
struct BoundWrites {
    owners: HashMap<SymbolId, ScopeId>,
    links_of: HashMap<SymbolId, Vec<usize>>,
}

/// How far [`Sharing::note_bound_writes`] has looked through the changes made ([`Sharing::mutated`]), and the changes
/// it has followed.
#[derive(Default)]
struct Scan {
    upto: usize,
    seen: BTreeSet<(SymbolId, Write)>,
}

/// The name of a key: `#name` for a private one; `None` when it is not static.
fn key_name(key: &PropertyKey<'_>) -> Option<String> {
    key.private_name()
        .map(|name| format!("#{}", name.as_str()))
        .or_else(|| key.static_name().map(|name| crate::js_string(name.as_js_str())))
}

/// The key a member expression reads or writes: `#name` for a private one, the digits of an index (`list[0]`);
/// `None` when it is not static.
fn member_key(member: &MemberExpression<'_>) -> Option<String> {
    match member {
        MemberExpression::PrivateFieldExpression(field) => Some(format!("#{}", field.field.name.as_str())),
        MemberExpression::ComputedMemberExpression(computed) => match &computed.expression {
            Expression::NumericLiteral(number) => number
                .raw
                .as_ref()
                .map(|raw| raw.as_str().to_owned())
                .filter(|raw| is_index(raw)),
            _ => computed.static_property_name().map(crate::js_string),
        },
        other @ MemberExpression::StaticMemberExpression(_) => other.static_property_name().map(crate::js_string),
    }
}

/// Whether `digits`, the text of a number, is the key JavaScript names that index by (`0`, `12`, not `012` or `1e3`).
fn is_index(digits: &str) -> bool {
    digits == "0"
        || (!digits.starts_with('0') && !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()))
}

/// Whether `expr` is `this`.
fn is_this(expr: &Expression<'_>) -> bool {
    matches!(expr.get_inner_expression(), Expression::ThisExpression(_))
}

/// The member a callee reads a method from when it is one of `this` (`this.read` in `this.read(key)`).
fn this_member<'b, 'a>(callee: &'b Expression<'a>) -> Option<&'b MemberExpression<'a>> {
    callee
        .get_inner_expression()
        .as_member_expression()
        .filter(|member| is_this(member.object()))
}

/// The keys of the methods of an iterator an iteration calls.
const ITERATOR_METHODS: &[&str] = &["next", "return", "throw"];

/// Whether `key` is `Symbol.iterator` or `Symbol.asyncIterator` of the global `Symbol`, the key of the method an
/// iteration calls.
fn is_iterator_key(scoping: &Scoping, key: &PropertyKey<'_>) -> bool {
    let Some(Expression::StaticMemberExpression(member)) = key.as_expression().map(Expression::get_inner_expression)
    else {
        return false;
    };
    matches!(member.property.name.as_str(), "iterator" | "asyncIterator")
        && matches!(
            member.object.get_inner_expression(),
            Expression::Identifier(id) if id.name.as_str() == "Symbol" && resolved(scoping, id).is_none()
        )
}

/// How code runs the function at `key` of an object with the object as `this` with no call of it written, when
/// anything does: a read or a write of the key for a getter or a setter (`accessor`), and an iteration for an iterator
/// ([`is_iterator_key`]) or a method of one ([`ITERATOR_METHODS`]).
fn implicit_run(scoping: &Scoping, key: &PropertyKey<'_>, accessor: bool) -> Option<Run> {
    let name = key_name(key);
    if accessor {
        return Some(Run::Key(name));
    }
    let iterates =
        is_iterator_key(scoping, key) || name.as_deref().is_some_and(|name| ITERATOR_METHODS.contains(&name));
    iterates.then_some(Run::Iteration)
}

/// The scope of the function an object property holds and [`implicit_run`] of it, when it holds one that is no arrow
/// function and code runs it so.
fn property_run(scoping: &Scoping, property: &ObjectProperty<'_>) -> Option<(ScopeId, Run)> {
    let Expression::FunctionExpression(function) = property.value.get_inner_expression() else {
        return None;
    };
    let run = implicit_run(scoping, &property.key, property.kind != PropertyKind::Init)?;
    Some((function.scope_id.get()?, run))
}

/// [`property_run`] of a method of the instances of a class: neither static nor the constructor.
fn method_run(scoping: &Scoping, method: &MethodDefinition<'_>) -> Option<(ScopeId, Run)> {
    if method.r#static || method.kind == MethodDefinitionKind::Constructor {
        return None;
    }
    let accessor = matches!(method.kind, MethodDefinitionKind::Get | MethodDefinitionKind::Set);
    let run = implicit_run(scoping, &method.key, accessor)?;
    Some((method.value.scope_id.get()?, run))
}

/// The descriptors a call of `Object.defineProperty`, `Reflect.defineProperty`, `Object.defineProperties` or
/// `Object.create` of the global object is given as object literals, each with the key it defines (`None` when it is
/// not static): their `get` and `set` are a getter and a setter of the object the call defines them on.
fn descriptors<'b, 'a>(
    scoping: &Scoping,
    call: &'b CallExpression<'a>,
) -> Vec<(Option<String>, &'b ObjectExpression<'a>)> {
    let Expression::StaticMemberExpression(member) = call.callee.get_inner_expression() else {
        return Vec::new();
    };
    let Expression::Identifier(object) = member.object.get_inner_expression() else {
        return Vec::new();
    };
    if resolved(scoping, object).is_some() {
        return Vec::new();
    }
    let literal = move |index: usize| match call
        .arguments
        .get(index)
        .and_then(Argument::as_expression)
        .map(Expression::get_inner_expression)
    {
        Some(Expression::ObjectExpression(literal)) => Some(&**literal),
        _ => None,
    };
    match (object.name.as_str(), member.property.name.as_str()) {
        ("Object" | "Reflect", "defineProperty") => {
            let key = call
                .arguments
                .get(1)
                .and_then(Argument::as_expression)
                .and_then(static_string);
            literal(2).map(|descriptor| (key, descriptor)).into_iter().collect()
        }
        ("Object", "defineProperties" | "create") => literal(1).map_or_else(Vec::new, described_keys),
        _ => Vec::new(),
    }
}

/// The descriptors an object literal given to `Object.defineProperties` or `Object.create` holds, each with its key.
fn described_keys<'b, 'a>(descriptors: &'b ObjectExpression<'a>) -> Vec<(Option<String>, &'b ObjectExpression<'a>)> {
    descriptors
        .properties
        .iter()
        .filter_map(|property| match property {
            ObjectPropertyKind::ObjectProperty(p) => match p.value.get_inner_expression() {
                Expression::ObjectExpression(descriptor) => Some((key_name(&p.key), &**descriptor)),
                _ => None,
            },
            ObjectPropertyKind::SpreadProperty(_) => None,
        })
        .collect()
}

/// The scopes of the functions a descriptor holds at `get` and `set`, which are no arrow functions.
fn descriptor_accessors(descriptor: &ObjectExpression<'_>) -> Vec<ScopeId> {
    descriptor
        .properties
        .iter()
        .filter_map(|property| match property {
            ObjectPropertyKind::ObjectProperty(p)
                if p.kind == PropertyKind::Init && matches!(key_name(&p.key).as_deref(), Some("get" | "set")) =>
            {
                match p.value.get_inner_expression() {
                    Expression::FunctionExpression(function) => function.scope_id.get(),
                    _ => None,
                }
            }
            _ => None,
        })
        .collect()
}

/// The getters, setters and iterators of the objects and class instances a module makes ([`Accessors`]), which a place
/// in its code may run.
fn accessors(program: &Program<'_>, scoping: &Scoping) -> Accessors {
    let mut finder = FindAccessors {
        scoping,
        found: Accessors::default(),
    };
    finder.visit_program(program);
    finder.found
}

/// Finds the getters, setters and iterators a module declares ([`accessors`]).
struct FindAccessors<'s> {
    scoping: &'s Scoping,
    found: Accessors,
}

impl<'a> Visit<'a> for FindAccessors<'_> {
    fn visit_object_property(&mut self, it: &ObjectProperty<'a>) {
        if let Some((scope, run)) = property_run(self.scoping, it) {
            self.found.add(scope, run);
        }
        walk::walk_object_property(self, it);
    }

    fn visit_method_definition(&mut self, it: &MethodDefinition<'a>) {
        if let Some((scope, run)) = method_run(self.scoping, it) {
            self.found.add(scope, run);
        }
        walk::walk_method_definition(self, it);
    }

    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        for (key, descriptor) in descriptors(self.scoping, it) {
            for scope in descriptor_accessors(descriptor) {
                self.found.add(scope, Run::Key(key.clone()));
                self.found.described.insert(scope);
            }
        }
        walk::walk_call_expression(self, it);
    }
}

/// How a destructuring may run functions of the value it takes ([`Run`]): on that value itself, and on values below it
/// that a nested pattern takes.
#[derive(Debug, Clone, Default)]
struct Destructured {
    own: Vec<Run>,
    below: Vec<Run>,
}

impl Destructured {
    /// All of its runs, wherever they are.
    fn into_runs(self) -> impl Iterator<Item = Run> {
        self.own.into_iter().chain(self.below)
    }
}

/// Finds how a destructuring may run functions of the value it takes ([`Destructured`]): an iteration for an array
/// pattern, a read of each key an object pattern names and of every key for its rest element, each on the value the
/// pattern takes. A default is no part of it.
#[derive(Default)]
struct PatternRuns {
    /// How many patterns the one being visited is nested in.
    depth: usize,
    found: Destructured,
}

impl PatternRuns {
    fn push(&mut self, run: Run) {
        if self.depth == 0 {
            self.found.own.push(run);
        } else {
            self.found.below.push(run);
        }
    }

    /// Visits what `visit` visits as a pattern of a value below the one being taken.
    fn nested(&mut self, visit: impl FnOnce(&mut Self)) {
        self.depth += 1;
        visit(self);
        self.depth -= 1;
    }
}

/// The [`Destructured`] of the pattern `visit` visits.
fn pattern_runs(visit: impl FnOnce(&mut PatternRuns)) -> Destructured {
    let mut runs = PatternRuns::default();
    visit(&mut runs);
    runs.found
}

/// The [`Destructured`] of each parameter of `params`, by position.
fn parameter_patterns(params: &FormalParameters<'_>) -> Vec<Destructured> {
    params
        .items
        .iter()
        .map(|param| pattern_runs(|runs| runs.visit_binding_pattern(&param.pattern)))
        .collect()
}

impl<'a> Visit<'a> for PatternRuns {
    fn visit_expression(&mut self, _: &Expression<'a>) {}

    fn visit_array_pattern(&mut self, it: &ArrayPattern<'a>) {
        self.push(Run::Iteration);
        self.nested(|runs| walk::walk_array_pattern(runs, it));
    }

    fn visit_array_assignment_target(&mut self, it: &ArrayAssignmentTarget<'a>) {
        self.push(Run::Iteration);
        self.nested(|runs| walk::walk_array_assignment_target(runs, it));
    }

    fn visit_object_pattern(&mut self, it: &ObjectPattern<'a>) {
        for property in &it.properties {
            self.push(Run::Key(key_name(&property.key)));
            self.nested(|runs| runs.visit_binding_pattern(&property.value));
        }
        if let Some(rest) = &it.rest {
            self.push(Run::Key(None));
            self.nested(|runs| runs.visit_binding_rest_element(rest));
        }
    }

    fn visit_object_assignment_target(&mut self, it: &ObjectAssignmentTarget<'a>) {
        for property in &it.properties {
            match property {
                AssignmentTargetProperty::AssignmentTargetPropertyIdentifier(p) => {
                    self.push(Run::Key(Some(p.binding.name.as_str().to_owned())));
                }
                AssignmentTargetProperty::AssignmentTargetPropertyProperty(p) => {
                    self.push(Run::Key(key_name(&p.name)));
                    self.nested(|runs| runs.visit_assignment_target_maybe_default(&p.binding));
                }
            }
        }
        if let Some(rest) = &it.rest {
            self.push(Run::Key(None));
            self.nested(|runs| runs.visit_assignment_target_rest(rest));
        }
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

/// Builtin classes whose constructor iterates what it is given (`new Map(entries)`).
const ITERATING_CLASSES: &[&str] = &["Map", "Set", "WeakMap", "WeakSet"];

/// How the constructor `new` runs may run functions of each object it is given with no call of them written ([`Run`]):
/// an iteration for one of [`ITERATING_CLASSES`]; any for `URLSearchParams`, which reads the keys of an object or
/// iterates it, and for a class that is no builtin one, whose constructor the pass does not follow with what it is
/// given; none for any other of [`BUILTIN_CLASSES`].
fn new_runs(scoping: &Scoping, new: &NewExpression<'_>) -> Option<Run> {
    if !constructs_builtin(scoping, new) {
        return Some(Run::Every);
    }
    let Expression::Identifier(class) = new.callee.get_inner_expression() else {
        return None;
    };
    match class.name.as_str() {
        name if ITERATING_CLASSES.contains(&name) => Some(Run::Iteration),
        "URLSearchParams" => Some(Run::Every),
        _ => None,
    }
}

/// Functions of [`PURE_FUNCTIONS`] that read the own keys of an object they are given, which runs its getters, by the
/// name of the object they are called on and their own.
const KEY_READING_FUNCTIONS: &[(&str, &str)] = &[("JSON", "stringify"), ("Object", "entries"), ("Object", "values")];

/// Whether `call`, whose effect the pass knows ([`Effect::Keeps`]), reads the own keys of an object it is given, which
/// runs its getters and setters: one of [`KEY_READING_FUNCTIONS`], `Object.assign`, which sets them on its first
/// argument too, and a merge that copies ([`copies`]). Any other reads no key of what it is given: a `define…` helper
/// of a package returns it, and the rest are given strings and lists.
fn reads_keys(names: Names<'_>, call: &CallExpression<'_>) -> bool {
    if copies(names, call) {
        return true;
    }
    let Expression::StaticMemberExpression(member) = call.callee.get_inner_expression() else {
        return false;
    };
    let name = member.property.name.as_str();
    (name == "assign" && assigns(names, call))
        || (pure_function(names, call).is_some()
            && module_object(names, &member.object)
                .is_some_and(|object| KEY_READING_FUNCTIONS.contains(&(object, name))))
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

/// Whether `call` is `Object.assign` of the global `Object` given an object literal first and no spread argument: it
/// returns that literal with the own keys of the others copied in ([`Join::Copy`]), and changes none of them.
fn assigns_fresh(names: Names<'_>, call: &CallExpression<'_>) -> bool {
    let first = call
        .arguments
        .first()
        .and_then(Argument::as_expression)
        .map(Expression::get_inner_expression);
    matches!(
        call.callee.get_inner_expression(),
        Expression::StaticMemberExpression(m) if m.property.name.as_str() == "assign"
    ) && assigns(names, call)
        && !has_spread(call)
        && matches!(first, Some(Expression::ObjectExpression(_)))
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
/// value for: a global, or an import or a binding of a `require` of an installed package the evaluator does not
/// follow ([`Known::foreign`]), not of a module, a workspace package or an alias of the project.
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
    /// Those of [`Self::modules`] of an installed package ([`is_package`], [`Imports::is_installed`]) whose module the
    /// evaluator does not follow, so that none of the project's code is behind them.
    foreign: HashSet<SymbolId>,
    /// Those of [`Self::modules`] whose module the evaluator follows, which code of such a module may change.
    followed: HashSet<SymbolId>,
    /// Those of [`Self::followed`] the module calls by name, with the places in what a call of the export they take
    /// returns that may hold an object of a value of a module, when that is known ([`Imports::result_places`]).
    results: HashMap<SymbolId, Vec<Write>>,
    /// The export of its module each import of [`Self::modules`] takes, as keys of the module's namespace: none for a
    /// namespace import, `default` for a default import and the name for a named one. A `require` takes no such part.
    import_paths: HashMap<SymbolId, Vec<String>>,
    /// Those of [`Self::foreign`] that the module only calls methods on (`path.format(…)`, `_.merge(…)`), given only
    /// what [`given_parts`] finds in full ([`given_in_full`]): it does not write through them, store a value in them,
    /// give them to a call, read a member of them otherwise, or give a method a function or what a call returns, which
    /// the package may keep and give back (`store.set(() => config)`).
    inert: HashSet<SymbolId>,
}

/// The bindings of `program` [`Known`] holds, where `imports` says which modules the evaluator follows.
fn known(program: &Program<'_>, scoping: &Scoping, imports: &dyn Imports) -> Known {
    let mut finder = FindKnown {
        scoping,
        known: Known::default(),
        in_const: false,
        called: HashSet::default(),
    };
    finder.visit_program(program);
    let called = finder.called;
    let mut known = finder.known;
    known.followed = known
        .modules
        .iter()
        .filter(|(_, (module, _))| imports.follows(module))
        .map(|(&symbol, _)| symbol)
        .collect();
    known.results = known
        .followed
        .iter()
        .filter(|&&symbol| {
            scoping
                .get_resolved_reference_ids(symbol)
                .iter()
                .any(|reference| called.contains(reference))
        })
        .filter_map(|&symbol| {
            let (module, _) = known.modules.get(&symbol)?;
            let [export] = known.import_paths.get(&symbol)?.as_slice() else {
                return None;
            };
            Some((symbol, imports.result_places(module, export)?))
        })
        .collect();
    known.foreign = known
        .modules
        .iter()
        .filter(|(_, (module, _))| is_package(module) && !imports.follows(module) && imports.is_installed(module))
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

/// The references to a name that are the object of a method called on it given only what [`given_parts`] finds in full
/// ([`given_in_full`]): `path` in `path.format(…)`, not `store` in `store.set(() => config)`, whose function the package
/// may keep and give back.
#[derive(Default)]
struct MethodReceivers(HashSet<ReferenceId>);

impl<'a> Visit<'a> for MethodReceivers {
    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        if let Expression::StaticMemberExpression(member) = it.callee.get_inner_expression()
            && let Expression::Identifier(id) = member.object.get_inner_expression()
            && it.arguments.iter().filter_map(given).all(given_in_full)
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
    /// The names that are what a call calls.
    called: HashSet<ReferenceId>,
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
    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        if let Expression::Identifier(id) = it.callee.get_inner_expression() {
            self.called.extend(id.reference_id.get());
        }
        walk::walk_call_expression(self, it);
    }

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

/// The bindings of each set `links` joins, sorted and shared by every binding of the set.
type Classes = HashMap<SymbolId, Rc<[SymbolId]>>;

/// Each binding `links` names, with every binding in its set ([`forest`]), sorted.
fn classes(links: &[(SymbolId, SymbolId)]) -> Classes {
    let mut parent = forest(links);
    let mut members: HashMap<SymbolId, Vec<SymbolId>> = HashMap::default();
    let symbols: Vec<SymbolId> = parent.keys().copied().collect();
    for symbol in symbols {
        let root = find(&mut parent, symbol);
        members.entry(root).or_default().push(symbol);
    }
    let mut out = Classes::default();
    for (_, mut symbols) in members {
        symbols.sort_unstable();
        let shared: Rc<[SymbolId]> = symbols.into();
        for &symbol in shared.iter() {
            out.insert(symbol, Rc::clone(&shared));
        }
    }
    out
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

/// Whether `id` is an import of a module the evaluator follows ([`Known::followed`]).
fn is_followed_import(scoping: &Scoping, known: &Known, id: &IdentifierReference<'_>) -> bool {
    resolved(scoping, id).is_some_and(|symbol| known.followed.contains(&symbol))
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

/// Whether [`given_parts`] finds every part of a local `expr` may hold: it is a name or a member chain, a value that
/// holds no object (a literal, a template, an operator's result), or an object or array literal, a condition, a logical
/// operator, a sequence or an assignment of such values. A function, a call, a `new` or a method there may hold a local
/// where [`given_parts`] does not look.
fn given_in_full(expr: &Expression<'_>) -> bool {
    if reach(expr).is_some() {
        return true;
    }
    match expr.get_inner_expression() {
        Expression::StringLiteral(_)
        | Expression::NumericLiteral(_)
        | Expression::BooleanLiteral(_)
        | Expression::NullLiteral(_)
        | Expression::BigIntLiteral(_)
        | Expression::RegExpLiteral(_)
        | Expression::TemplateLiteral(_)
        | Expression::BinaryExpression(_)
        | Expression::UnaryExpression(_) => true,
        Expression::ObjectExpression(object) => object.properties.iter().all(|property| match property {
            ObjectPropertyKind::ObjectProperty(p) => {
                !p.method && p.kind == PropertyKind::Init && given_in_full(&p.value)
            }
            ObjectPropertyKind::SpreadProperty(s) => given_in_full(&s.argument),
        }),
        Expression::ArrayExpression(array) => array.elements.iter().all(|element| match element {
            ArrayExpressionElement::SpreadElement(s) => given_in_full(&s.argument),
            ArrayExpressionElement::Elision(_) => true,
            other => other.as_expression().is_some_and(given_in_full),
        }),
        Expression::ConditionalExpression(c) => given_in_full(&c.consequent) && given_in_full(&c.alternate),
        Expression::LogicalExpression(l) => given_in_full(&l.left) && given_in_full(&l.right),
        Expression::SequenceExpression(sequence) => sequence.expressions.last().is_some_and(given_in_full),
        Expression::AssignmentExpression(a) => a.operator == AssignmentOperator::Assign && given_in_full(&a.right),
        _ => false,
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
/// `undefined` (`JSON.stringify(config, replacer)`), or given a spread argument (`JSON.stringify(...[config, r])`),
/// which may hold such a function at any place, as the function it may be given may change what it is called with.
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
    (!calls_back || (!has_spread(call) && given_no_function())).then_some(returns)
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

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;
    use std::path::Path;

    use oxc_allocator::Allocator;
    use oxc_parser::Parser;
    use oxc_span::SourceType;

    use super::{Change, Imports, MAX_PLACES, Unstable, Write};

    /// The imports of a module that imports nothing.
    struct NoModules;

    impl Imports for NoModules {
        fn follows(&self, _: &str) -> bool {
            false
        }

        fn changes_modules(&self, _: &str) -> bool {
            false
        }
    }

    /// How many bindings [`chain`] declares: a change of one of them reaches more than [`MAX_PLACES`] places.
    const LINKED: usize = 2 * MAX_PLACES;

    /// A module of [`LINKED`] top-level bindings `b0`, `b1`, … each holding the object the one before it holds, then
    /// the statements `changes`.
    fn chain(changes: &str) -> String {
        let mut source = String::from("const b0 = {};\n");
        for index in 1..LINKED {
            writeln!(source, "const b{index} = b{};", index - 1).unwrap();
        }
        source.push_str(changes);
        source
    }

    /// How code the evaluator does not follow may change each binding of [`chain`] with `changes`.
    fn changes_of(changes: &str) -> Vec<Option<Change>> {
        let source = chain(changes);
        let allocator = Allocator::default();
        let source_type = SourceType::from_path(Path::new("config.js")).unwrap();
        let program = Parser::new(&allocator, &source, source_type).parse().program;
        let unstable = Unstable::of(&program, &NoModules);
        (0..LINKED)
            .map(|index| unstable.of_top(&format!("b{index}")).cloned())
            .collect()
    }

    /// A change of all a binding holds reaches every binding linked to it in full, but for the binding itself, whose
    /// change the evaluator applies.
    #[test]
    fn a_change_in_full_of_a_class_leaves_the_binding_changed_as_it_is() {
        let in_full = Some(Change::Contents(vec![Write::default()]));
        let mut expected = vec![in_full; LINKED];
        expected[0] = None;
        assert_eq!(changes_of("mutate(b0);\n"), expected);
    }

    /// A second change in full of the same class, through another binding or as a new value of a key of the same one,
    /// changes the binding the first left as it is: each binding is then changed in full.
    #[test]
    fn a_second_change_in_full_of_a_class_changes_the_binding_the_first_left() {
        let expected = vec![Some(Change::Contents(vec![Write::default()])); LINKED];
        for changes in ["mutate(b0);\nmutate(b1);\n", "mutate(b0);\nb0.x = 1;\n"] {
            assert_eq!(changes_of(changes), expected, "{changes}");
        }
    }
}
