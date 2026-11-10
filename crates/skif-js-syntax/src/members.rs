//! Members read through imported bindings (`ns.a`, `Enum.A`, `const { a } = ns`, `ns.Type`, and the member a test
//! runner spies on, `vi.spyOn(ns, "a")`), and the members read one level further down (`ns.Enum.A`).
//!
//! A reference counts for the binding semantic analysis resolves it to. The binding of an `import` declaration is
//! matched by scope only: a parameter or local of an inner scope with the same name shadows it and reads nothing of
//! it. The binding a `const ns = await import("x")`, `const x = require("x")`, a `Promise.all` pattern or an
//! `import("x").then` callback declares gets the references that resolve to it, and, as knip matches the bindings of
//! dynamic imports by name, the references of its name that resolve to no other tracked binding (a shadowing local
//! can only make it look more used). A binding whose declaration is none of these (`import x = require("x")`) is
//! matched by name. A reference that resolves to nothing (one in another script block of a component) counts for the
//! binding of its name.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use oxc_ast::ast::{
    Argument, BindingPattern, CallExpression, ComputedMemberExpression, Expression, IdentifierReference,
    ImportDeclaration, StaticMemberExpression, TSQualifiedName, TSTypeName, VariableDeclarator,
};
use oxc_ast_visit::{Visit, walk};
use oxc_semantic::{Scoping, SymbolId};

use crate::model::{ImportKind, ModuleInfo};
use crate::semantic::Analysis;

/// How one name is used in a program: the members read through it and how many uses there are in all.
#[derive(Default)]
pub(crate) struct Slot {
    pub members: BTreeSet<String>,
    pub uses: u32,
    pub member_uses: u32,
    /// The uses of each member read through the name, by member: `ns.E` is a use of member `E`, `ns.E.A` reads `A` of
    /// it. A member read in a way this does not see (destructured, spied on, read in a template) has no member uses.
    pub nested: BTreeMap<String, Slot>,
}

impl Slot {
    /// Whether the name is used in a way that cannot be reduced to members.
    pub(crate) const fn escapes(&self) -> bool {
        self.uses > self.member_uses
    }

    /// The members read through the members of the name, for each member every use of which reads its members.
    pub(crate) fn nested_reads(&self) -> impl Iterator<Item = (&String, &BTreeSet<String>)> {
        self.nested
            .iter()
            .filter(|(_, inner)| !inner.escapes() && !inner.members.is_empty())
            .map(|(member, inner)| (member, &inner.members))
    }

    /// Adds the uses in `other`.
    fn merge(&mut self, other: &Self) {
        self.members.extend(other.members.iter().cloned());
        self.uses += other.uses;
        self.member_uses += other.member_uses;
        for (member, inner) in &other.nested {
            self.nested.entry(member.clone()).or_default().merge(inner);
        }
    }
}

/// Uses of names by name, in one program.
pub(crate) type Uses = HashMap<String, Slot>;

/// The slots of the bindings a [`Tracker`] follows, and how references find them.
#[derive(Default)]
struct Slots {
    slots: Vec<Slot>,
    /// Slots of bindings matched by scope, by symbol.
    by_symbol: HashMap<SymbolId, usize>,
    /// Slots of bindings matched by name.
    by_name: HashMap<String, usize>,
    /// The first slot of each name matched by scope, for a reference that resolves to nothing.
    unresolved: HashMap<String, usize>,
}

impl Slots {
    /// The slot of the binding `symbol` named `name`, matched by scope.
    fn scoped(&mut self, symbol: SymbolId, name: &str) -> usize {
        let next = self.slots.len();
        let index = *self.by_symbol.entry(symbol).or_insert(next);
        if index == next {
            self.slots.push(Slot::default());
        }
        self.unresolved.entry(name.to_owned()).or_insert(index);
        index
    }

    /// The slot of the binding `name`, matched by name. `index` gives it an existing slot.
    fn named(&mut self, name: &str, index: Option<usize>) -> usize {
        if let Some(&found) = self.by_name.get(name) {
            return found;
        }
        let index = index.unwrap_or_else(|| {
            self.slots.push(Slot::default());
            self.slots.len() - 1
        });
        self.by_name.insert(name.to_owned(), index);
        index
    }
}

struct Tracker<'e> {
    slots: Slots,
    /// Track every name by name, not only the bindings in `slots`.
    open: bool,
    /// `(start, end)` of identifier references that are not uses (the local side of `export { x }` of an import).
    excluded: &'e HashSet<(u32, u32)>,
    /// The scopes of the program, to tell an import binding from a shadowing one.
    scoping: &'e Scoping,
}

impl Tracker<'_> {
    /// The slot of the binding `id` refers to, if it is followed.
    fn slot(&mut self, id: &IdentifierReference<'_>) -> Option<&mut Slot> {
        let symbol = id
            .reference_id
            .get()
            .and_then(|reference| self.scoping.get_reference(reference).symbol_id());
        let name = id.name.as_str();
        let index = if self.open {
            // Every name: a reference that resolves to a parameter or local of an inner scope is another binding.
            let shadowed = symbol.is_some_and(|symbol| {
                self.scoping.symbol_scope_id(symbol) != self.scoping.root_scope_id()
                    && !self.scoping.symbol_flags(symbol).is_import()
            });
            if shadowed {
                return None;
            }
            self.slots.named(name, None)
        } else {
            let slots = &self.slots;
            symbol
                .and_then(|symbol| slots.by_symbol.get(&symbol))
                .or_else(|| slots.by_name.get(name))
                .or_else(|| symbol.is_none().then(|| slots.unresolved.get(name)).flatten())
                .copied()?
        };
        self.slots.slots.get_mut(index)
    }

    fn member(&mut self, id: &IdentifierReference<'_>, member: &str) {
        if let Some(slot) = self.slot(id) {
            slot.members.insert(member.to_string());
            slot.member_uses += 1;
            slot.nested.entry(member.to_owned()).or_default().uses += 1;
        }
    }

    /// Records `id.member.inner`. The walk records `id.member` on its own.
    fn nested_member(&mut self, id: &IdentifierReference<'_>, member: &str, inner: &str) {
        if let Some(slot) = self.slot(id) {
            let nested = slot.nested.entry(member.to_owned()).or_default();
            nested.members.insert(inner.to_owned());
            nested.member_uses += 1;
        }
    }

    /// Records `inner` read on `object` when `object` is a member read through an identifier (`ns.E`, `ns['E']`).
    fn read_on_member(&mut self, object: &Expression<'_>, inner: &str) {
        if let Some((id, member)) = identifier_member(object) {
            self.nested_member(id, &member, inner);
        }
    }
}

impl<'a> Visit<'a> for Tracker<'_> {
    fn visit_identifier_reference(&mut self, it: &IdentifierReference<'a>) {
        if self.excluded.contains(&(it.span.start, it.span.end)) {
            return;
        }
        if let Some(slot) = self.slot(it) {
            slot.uses += 1;
        }
    }

    fn visit_static_member_expression(&mut self, it: &StaticMemberExpression<'a>) {
        if let Expression::Identifier(id) = &it.object {
            self.member(id, it.property.name.as_str());
        } else {
            self.read_on_member(&it.object, it.property.name.as_str());
        }
        walk::walk_static_member_expression(self, it);
    }

    fn visit_computed_member_expression(&mut self, it: &ComputedMemberExpression<'a>) {
        if let Expression::StringLiteral(s) = &it.expression
            && let Some(member) = s.value.as_str()
        {
            if let Expression::Identifier(id) = &it.object {
                self.member(id, member);
            } else {
                self.read_on_member(&it.object, member);
            }
        }
        walk::walk_computed_member_expression(self, it);
    }

    fn visit_ts_qualified_name(&mut self, it: &TSQualifiedName<'a>) {
        match &it.left {
            TSTypeName::IdentifierReference(id) => self.member(id, it.right.name.as_str()),
            TSTypeName::QualifiedName(left) => {
                if let TSTypeName::IdentifierReference(id) = &left.left {
                    self.nested_member(id, left.right.name.as_str(), it.right.name.as_str());
                }
            }
            TSTypeName::ThisExpression(_) => {}
        }
        walk::walk_ts_qualified_name(self, it);
    }

    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        if let Some((object, member)) = spied_member(it) {
            self.member(object, &member);
        }
        walk::walk_call_expression(self, it);
    }

    fn visit_variable_declarator(&mut self, it: &VariableDeclarator<'a>) {
        if let Some(Expression::Identifier(id)) = it.init.as_ref().map(Expression::get_inner_expression)
            && let BindingPattern::ObjectPattern(pattern) = &it.id
            && pattern.rest.is_none()
        {
            let keys: Option<Vec<String>> = pattern
                .properties
                .iter()
                .map(|p| p.key.static_name().map(|k| crate::js_string(k.as_js_str())))
                .collect();
            if let Some(keys) = keys
                && let Some(slot) = self.slot(id)
            {
                for key in &keys {
                    slot.nested.entry(key.clone()).or_default().uses += 1;
                }
                slot.members.extend(keys);
                slot.member_uses += 1;
            }
        }
        walk::walk_variable_declarator(self, it);
    }
}

/// The identifier and the member of `id.member` or `id['member']`.
fn identifier_member<'c, 'a>(expression: &'c Expression<'a>) -> Option<(&'c IdentifierReference<'a>, String)> {
    match expression {
        Expression::StaticMemberExpression(member) => match &member.object {
            Expression::Identifier(id) => Some((&**id, member.property.name.as_str().to_owned())),
            _ => None,
        },
        Expression::ComputedMemberExpression(member) => match (&member.object, &member.expression) {
            (Expression::Identifier(id), Expression::StringLiteral(s)) => Some((&**id, s.value.as_str()?.to_owned())),
            _ => None,
        },
        _ => None,
    }
}

/// The object and the member of a test runner's spy on a named member: `vi.spyOn(ns, "a")`, `jest.spyOn(ns, "a")` and
/// Jasmine's `spyOn(ns, "a")` read `a` of `ns` and nothing else of it.
fn spied_member<'c, 'a>(call: &'c CallExpression<'a>) -> Option<(&'c IdentifierReference<'a>, String)> {
    let is_spy = match &call.callee {
        Expression::StaticMemberExpression(callee) => {
            callee.property.name.as_str() == "spyOn"
                && matches!(&callee.object, Expression::Identifier(runner) if matches!(runner.name.as_str(), "vi" | "jest"))
        }
        Expression::Identifier(callee) => callee.name.as_str() == "spyOn",
        _ => false,
    };
    if !is_spy {
        return None;
    }
    let Some(Argument::Identifier(object)) = call.arguments.first() else {
        return None;
    };
    let member = crate::finder::static_string(call.arguments.get(1)?.as_expression()?)?;
    Some((&**object, member))
}

/// The symbols of the bindings imports declare, by the start of the import (the `import` declaration, the `import()`
/// or the `require()` call) and the local name.
#[derive(Default)]
struct ImportSymbols<'a> {
    found: HashMap<(u32, &'a str), SymbolId>,
}

impl<'a> ImportSymbols<'a> {
    fn bind(&mut self, start: u32, pattern: &BindingPattern<'a>) {
        for id in pattern.get_binding_identifiers() {
            if let Some(symbol) = id.symbol_id.get() {
                self.found.insert((start, id.name.as_str()), symbol);
            }
        }
    }
}

impl<'a> Visit<'a> for ImportSymbols<'a> {
    fn visit_import_declaration(&mut self, it: &ImportDeclaration<'a>) {
        for specifier in it.specifiers.iter().flatten() {
            let local = specifier.local();
            if let Some(symbol) = local.symbol_id.get() {
                self.found.insert((it.span.start, local.name.as_str()), symbol);
            }
        }
    }

    fn visit_variable_declarator(&mut self, it: &VariableDeclarator<'a>) {
        if let Some(init) = &it.init {
            if let Some(start) = crate::finder::module_call_start(init) {
                self.bind(start, &it.id);
            } else if let BindingPattern::ArrayPattern(pattern) = &it.id {
                for (start, binding) in crate::finder::promise_all_imports(init, pattern) {
                    self.bind(start, binding);
                }
            }
        }
        walk::walk_variable_declarator(self, it);
    }

    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        if let Some(import) = crate::finder::then_on_import(it) {
            let parameter = match it.arguments.first() {
                Some(Argument::ArrowFunctionExpression(arrow)) => arrow.params.items.first(),
                Some(Argument::FunctionExpression(function)) => function.params.items.first(),
                _ => None,
            };
            if let Some(parameter) = parameter {
                self.bind(import.span.start, &parameter.pattern);
            }
        }
        walk::walk_call_expression(self, it);
    }
}

/// Fills `accessed_members` and `escapes` of every bound import name from uses in `program`.
///
/// `excluded` holds the `(start, end)` spans of identifiers that mention a binding without using it; `base` is the
/// offset of the program in the file, which the spans of `info` include.
pub(crate) fn track(
    program: &oxc_ast::ast::Program<'_>,
    analysis: &Analysis<'_>,
    info: &mut ModuleInfo,
    excluded: &HashSet<(u32, u32)>,
    base: u32,
) {
    let bound = info
        .imports
        .iter()
        .flat_map(|import| &import.names)
        .any(|name| name.local.is_some());
    if !bound {
        return;
    }
    let mut symbols = ImportSymbols::default();
    symbols.visit_program(program);
    let mut slots = Slots::default();
    // The slot of each bound name, by the positions of its import and of the name in it.
    let mut assigned: Vec<(usize, usize, usize)> = Vec::new();
    for (i, import) in info.imports.iter().enumerate() {
        let start = import.span.start.saturating_sub(base);
        for (j, name) in import.names.iter().enumerate() {
            let Some(local) = name.local.as_deref() else {
                continue;
            };
            let index = match symbols.found.get(&(start, local)) {
                Some(&symbol) => {
                    let index = slots.scoped(symbol, local);
                    if !matches!(import.kind, ImportKind::Static | ImportKind::TypeOnly) {
                        slots.named(local, Some(index));
                    }
                    index
                }
                None => slots.named(local, None),
            };
            assigned.push((i, j, index));
        }
    }
    let mut tracker = Tracker {
        slots,
        open: false,
        excluded,
        scoping: analysis.scoping(),
    };
    tracker.visit_program(program);
    for (i, j, index) in assigned {
        let slot = &tracker.slots.slots[index];
        let name = &mut info.imports[i].names[j];
        name.accessed_members = slot.members.iter().cloned().collect();
        name.nested_members = slot
            .nested_reads()
            .map(|(member, inner)| (member.clone(), inner.iter().cloned().collect()))
            .collect();
        name.escapes |= slot.escapes();
    }
}

/// The uses of every name in `program`, imported or not. Component files use it to credit imports of one script
/// block that another block reads.
pub(crate) fn uses(program: &oxc_ast::ast::Program<'_>, analysis: &Analysis<'_>) -> Uses {
    let excluded = HashSet::new();
    let mut tracker = Tracker {
        slots: Slots::default(),
        open: true,
        excluded: &excluded,
        scoping: analysis.scoping(),
    };
    tracker.visit_program(program);
    let Slots { mut slots, by_name, .. } = tracker.slots;
    by_name
        .into_iter()
        .map(|(name, index)| (name, std::mem::take(&mut slots[index])))
        .collect()
}

/// Adds the uses in `from` to `into`.
pub(crate) fn merge_uses(into: &mut Uses, from: &Uses) {
    for (name, slot) in from {
        into.entry(name.clone()).or_default().merge(slot);
    }
}

/// Adds `uses` to the bound import names of `info`.
pub(crate) fn credit(info: &mut ModuleInfo, uses: &Uses) {
    for import in &mut info.imports {
        for n in &mut import.names {
            let Some(slot) = n.local.as_ref().and_then(|l| uses.get(l)) else {
                continue;
            };
            let nested: BTreeMap<&String, &BTreeSet<String>> = slot.nested_reads().collect();
            for member in &slot.members {
                let whole_here = n.accessed_members.contains(member) && !n.nested_members.contains_key(member);
                match nested.get(member) {
                    Some(inner) if !whole_here => {
                        let reads = n.nested_members.entry(member.clone()).or_default();
                        *reads = union(reads, inner);
                    }
                    _ => {
                        n.nested_members.remove(member);
                    }
                }
            }
            n.accessed_members = union(&n.accessed_members, &slot.members);
            n.escapes |= slot.escapes();
        }
    }
}

/// The sorted names in `list` or `more`, each once.
fn union(list: &[String], more: &BTreeSet<String>) -> Vec<String> {
    list.iter().chain(more).cloned().collect::<BTreeSet<_>>().into_iter().collect()
}

/// The members of the enums and namespaces declared in a program that the program itself reads by name.
struct LocalReads<'w> {
    wanted: &'w HashSet<String>,
    reads: HashMap<String, BTreeSet<String>>,
}

impl LocalReads<'_> {
    /// Records `local.member` and reports whether `local` is one of the wanted declarations.
    fn read(&mut self, local: &str, member: &str) -> bool {
        if !self.wanted.contains(local) {
            return false;
        }
        self.reads
            .entry(local.to_owned())
            .or_default()
            .insert(member.to_owned());
        true
    }
}

impl<'a> Visit<'a> for LocalReads<'_> {
    fn visit_static_member_expression(&mut self, it: &StaticMemberExpression<'a>) {
        if let Expression::Identifier(id) = &it.object
            && self.read(id.name.as_str(), it.property.name.as_str())
        {
            return;
        }
        walk::walk_static_member_expression(self, it);
    }

    fn visit_computed_member_expression(&mut self, it: &ComputedMemberExpression<'a>) {
        if let Expression::Identifier(id) = &it.object
            && let Expression::StringLiteral(s) = &it.expression
            && let Some(member) = s.value.as_str()
            && self.read(id.name.as_str(), member)
        {
            return;
        }
        walk::walk_computed_member_expression(self, it);
    }

    // A type annotation (`x: E`) mentions the declaration without reading any member; `E.A` as a type reads `A`.
    fn visit_ts_type_name(&mut self, it: &TSTypeName<'a>) {
        if let TSTypeName::QualifiedName(name) = it {
            self.visit_ts_qualified_name(name);
        }
    }

    fn visit_ts_qualified_name(&mut self, it: &TSQualifiedName<'a>) {
        if let TSTypeName::IdentifierReference(id) = &it.left {
            self.read(id.name.as_str(), it.right.name.as_str());
        } else {
            self.visit_ts_type_name(&it.left);
        }
    }

    fn visit_variable_declarator(&mut self, it: &VariableDeclarator<'a>) {
        if let Some(Expression::Identifier(id)) = it.init.as_ref().map(Expression::get_inner_expression)
            && self.wanted.contains(id.name.as_str())
            && let BindingPattern::ObjectPattern(pattern) = &it.id
            && pattern.rest.is_none()
            && let Some(keys) = pattern
                .properties
                .iter()
                .map(|p| p.key.static_name().map(|k| crate::js_string(k.as_js_str())))
                .collect::<Option<Vec<String>>>()
        {
            for key in keys {
                self.read(id.name.as_str(), &key);
            }
            self.visit_binding_pattern(&it.id);
            return;
        }
        walk::walk_variable_declarator(self, it);
    }
}

/// Fills [`crate::model::Export::members_read_in_file`] of every enum and namespace export from the uses in `program`:
/// `E.A` anywhere in the file, and a namespace member read by its bare name inside the namespace body.
pub(crate) fn mark_local_reads(program: &oxc_ast::ast::Program<'_>, analysis: &Analysis<'_>, info: &mut ModuleInfo) {
    let wanted: HashSet<String> = info
        .exports
        .iter()
        .filter(|export| !export.enum_members.is_empty() || !export.namespace_members.is_empty())
        .filter_map(|export| export.local_name.clone())
        .collect();
    if wanted.is_empty() {
        return;
    }
    let mut tracker = LocalReads {
        wanted: &wanted,
        reads: HashMap::new(),
    };
    tracker.visit_program(program);
    let mut namespaces: HashMap<String, BTreeSet<String>> = HashMap::new();
    for export in info
        .exports
        .iter()
        .filter(|export| !export.namespace_members.is_empty())
    {
        if let Some(local) = &export.local_name {
            namespaces
                .entry(local.clone())
                .or_default()
                .extend(export.namespace_members.iter().map(|member| member.name.clone()));
        }
    }
    if !namespaces.is_empty() {
        for (name, members) in crate::semantic::namespace_bare_reads(program, analysis, &namespaces) {
            tracker.reads.entry(name).or_default().extend(members);
        }
    }
    for export in &mut info.exports {
        let Some(local) = export.local_name.as_deref() else {
            continue;
        };
        let reads = tracker
            .reads
            .get(local)
            .map(|members| members.iter().cloned().collect())
            .unwrap_or_default();
        export.members_read_in_file = reads;
    }
}
