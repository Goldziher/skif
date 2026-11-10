//! Members read through imported bindings (`ns.a`, `Enum.A`, `const { a } = ns`, `ns.Type`, and the member a test
//! runner spies on, `vi.spyOn(ns, "a")`).
//!
//! The binding of an `import` declaration is matched by scope: a parameter or local of an inner scope with the same
//! name shadows it and reads nothing of it. Other bindings (`const ns = await import("x")`, `const x = require("x")`
//! in a function) are matched by name, as knip does for dynamic imports: a shadowing local can only make them look
//! more used.

use std::collections::{BTreeSet, HashMap, HashSet};

use oxc_ast::ast::{
    Argument, BindingPattern, CallExpression, ComputedMemberExpression, Expression, IdentifierReference,
    StaticMemberExpression, TSQualifiedName, TSTypeName, VariableDeclarator,
};
use oxc_ast_visit::{Visit, walk};
use oxc_semantic::{Scoping, SemanticBuilder};

use crate::model::{ImportKind, ModuleInfo};

/// How one name is used in a program: the members read through it and how many uses there are in all.
#[derive(Default)]
pub(crate) struct Slot {
    pub members: BTreeSet<String>,
    pub uses: u32,
    pub member_uses: u32,
}

impl Slot {
    /// Whether the name is used in a way that cannot be reduced to members.
    pub(crate) const fn escapes(&self) -> bool {
        self.uses > self.member_uses
    }
}

/// Uses of names by name, in one program.
pub(crate) type Uses = HashMap<String, Slot>;

struct Tracker<'e> {
    slots: Uses,
    /// Track every name, not only the ones in `slots`.
    open: bool,
    /// `(start, end)` of identifier references that are not uses (the local side of `export { x }` of an import).
    excluded: &'e HashSet<(u32, u32)>,
    /// The scopes of the program, to tell an import binding from a shadowing one.
    scoping: &'e Scoping,
    /// The names matched by scope; `None` matches every name by scope.
    scoped: Option<HashSet<String>>,
}

impl Tracker<'_> {
    /// Whether `id` may name the tracked binding of its name: always for a name matched by name, otherwise unless it
    /// resolves to a binding of an inner scope that is not an import (a parameter or local that shadows it).
    fn may_be_tracked(&self, id: &IdentifierReference<'_>) -> bool {
        if self
            .scoped
            .as_ref()
            .is_some_and(|scoped| !scoped.contains(id.name.as_str()))
        {
            return true;
        }
        id.reference_id
            .get()
            .and_then(|reference| self.scoping.get_reference(reference).symbol_id())
            .is_none_or(|symbol| {
                self.scoping.symbol_scope_id(symbol) == self.scoping.root_scope_id()
                    || self.scoping.symbol_flags(symbol).is_import()
            })
    }

    fn slot(&mut self, local: &str) -> Option<&mut Slot> {
        if self.open {
            Some(self.slots.entry(local.to_owned()).or_default())
        } else {
            self.slots.get_mut(local)
        }
    }

    fn member(&mut self, local: &str, member: &str) {
        if let Some(slot) = self.slot(local) {
            slot.members.insert(member.to_string());
            slot.member_uses += 1;
        }
    }
}

impl<'a> Visit<'a> for Tracker<'_> {
    fn visit_identifier_reference(&mut self, it: &IdentifierReference<'a>) {
        if self.excluded.contains(&(it.span.start, it.span.end)) || !self.may_be_tracked(it) {
            return;
        }
        if let Some(slot) = self.slot(it.name.as_str()) {
            slot.uses += 1;
        }
    }

    fn visit_static_member_expression(&mut self, it: &StaticMemberExpression<'a>) {
        if let Expression::Identifier(id) = &it.object
            && self.may_be_tracked(id)
        {
            self.member(id.name.as_str(), it.property.name.as_str());
        }
        walk::walk_static_member_expression(self, it);
    }

    fn visit_computed_member_expression(&mut self, it: &ComputedMemberExpression<'a>) {
        if let Expression::Identifier(id) = &it.object
            && self.may_be_tracked(id)
            && let Expression::StringLiteral(s) = &it.expression
            && let Some(member) = s.value.as_str()
        {
            self.member(id.name.as_str(), member);
        }
        walk::walk_computed_member_expression(self, it);
    }

    fn visit_ts_qualified_name(&mut self, it: &TSQualifiedName<'a>) {
        if let TSTypeName::IdentifierReference(id) = &it.left
            && self.may_be_tracked(id)
        {
            self.member(id.name.as_str(), it.right.name.as_str());
        }
        walk::walk_ts_qualified_name(self, it);
    }

    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        if let Some((object, member)) = spied_member(it)
            && self.may_be_tracked(object)
        {
            self.member(object.name.as_str(), &member);
        }
        walk::walk_call_expression(self, it);
    }

    fn visit_variable_declarator(&mut self, it: &VariableDeclarator<'a>) {
        if let Some(Expression::Identifier(id)) = it.init.as_ref().map(Expression::get_inner_expression)
            && self.may_be_tracked(id)
            && let BindingPattern::ObjectPattern(pattern) = &it.id
            && pattern.rest.is_none()
        {
            let keys: Option<Vec<String>> = pattern
                .properties
                .iter()
                .map(|p| p.key.static_name().map(|k| crate::js_string(k.as_js_str())))
                .collect();
            if let Some(keys) = keys
                && let Some(slot) = self.slot(id.name.as_str())
            {
                slot.members.extend(keys);
                slot.member_uses += 1;
            }
        }
        walk::walk_variable_declarator(self, it);
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

/// Fills `accessed_members` and `escapes` of every bound import name from uses in `program`.
///
/// `excluded` holds the `(start, end)` spans of identifiers that mention a binding without using it.
pub(crate) fn track(program: &oxc_ast::ast::Program<'_>, info: &mut ModuleInfo, excluded: &HashSet<(u32, u32)>) {
    let mut slots = HashMap::new();
    let mut scoped = HashSet::new();
    for import in &info.imports {
        for n in &import.names {
            if let Some(local) = &n.local {
                slots.insert(local.clone(), Slot::default());
                if matches!(import.kind, ImportKind::Static | ImportKind::TypeOnly) {
                    scoped.insert(local.clone());
                }
            }
        }
    }
    if slots.is_empty() {
        return;
    }
    let scoping = SemanticBuilder::new().build(program).semantic.into_scoping();
    let mut tracker = Tracker {
        slots,
        open: false,
        excluded,
        scoping: &scoping,
        scoped: Some(scoped),
    };
    tracker.visit_program(program);
    for import in &mut info.imports {
        for n in &mut import.names {
            let Some(slot) = n.local.as_ref().and_then(|l| tracker.slots.get(l)) else {
                continue;
            };
            n.accessed_members = slot.members.iter().cloned().collect();
            n.escapes |= slot.escapes();
        }
    }
}

/// The uses of every name in `program`, imported or not. Component files use it to credit imports of one script
/// block that another block reads.
pub(crate) fn uses(program: &oxc_ast::ast::Program<'_>) -> Uses {
    let excluded = HashSet::new();
    let scoping = SemanticBuilder::new().build(program).semantic.into_scoping();
    let mut tracker = Tracker {
        slots: Uses::new(),
        open: true,
        excluded: &excluded,
        scoping: &scoping,
        scoped: None,
    };
    tracker.visit_program(program);
    tracker.slots
}

/// Adds the uses in `from` to `into`.
pub(crate) fn merge_uses(into: &mut Uses, from: &Uses) {
    for (name, slot) in from {
        let target = into.entry(name.clone()).or_default();
        target.members.extend(slot.members.iter().cloned());
        target.uses += slot.uses;
        target.member_uses += slot.member_uses;
    }
}

/// Adds `uses` to the bound import names of `info`.
pub(crate) fn credit(info: &mut ModuleInfo, uses: &Uses) {
    for import in &mut info.imports {
        for n in &mut import.names {
            let Some(slot) = n.local.as_ref().and_then(|l| uses.get(l)) else {
                continue;
            };
            n.accessed_members.extend(slot.members.iter().cloned());
            n.accessed_members.sort();
            n.accessed_members.dedup();
            n.escapes |= slot.escapes();
        }
    }
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
pub(crate) fn mark_local_reads(program: &oxc_ast::ast::Program<'_>, info: &mut ModuleInfo) {
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
        for (name, members) in crate::semantic::namespace_bare_reads(program, &namespaces) {
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
