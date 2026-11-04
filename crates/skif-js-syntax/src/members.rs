//! Members read through imported bindings (`ns.a`, `Enum.A`, `const { a } = ns`, `ns.Type`).
//!
//! Matching is by name, not by scope: a shadowing local with the same name can only make a binding look more used.

use std::collections::{BTreeSet, HashMap, HashSet};

use oxc_ast::ast::{
    BindingPattern, ComputedMemberExpression, Expression, IdentifierReference, StaticMemberExpression, TSQualifiedName,
    TSTypeName, VariableDeclarator,
};
use oxc_ast_visit::{Visit, walk};

use crate::model::ModuleInfo;

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
}

impl Tracker<'_> {
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
        if self.excluded.contains(&(it.span.start, it.span.end)) {
            return;
        }
        if let Some(slot) = self.slot(it.name.as_str()) {
            slot.uses += 1;
        }
    }

    fn visit_static_member_expression(&mut self, it: &StaticMemberExpression<'a>) {
        if let Expression::Identifier(id) = &it.object {
            self.member(id.name.as_str(), it.property.name.as_str());
        }
        walk::walk_static_member_expression(self, it);
    }

    fn visit_computed_member_expression(&mut self, it: &ComputedMemberExpression<'a>) {
        if let Expression::Identifier(id) = &it.object
            && let Expression::StringLiteral(s) = &it.expression
            && let Some(member) = s.value.as_str()
        {
            self.member(id.name.as_str(), member);
        }
        walk::walk_computed_member_expression(self, it);
    }

    fn visit_ts_qualified_name(&mut self, it: &TSQualifiedName<'a>) {
        if let TSTypeName::IdentifierReference(id) = &it.left {
            self.member(id.name.as_str(), it.right.name.as_str());
        }
        walk::walk_ts_qualified_name(self, it);
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
                && let Some(slot) = self.slot(id.name.as_str())
            {
                slot.members.extend(keys);
                slot.member_uses += 1;
            }
        }
        walk::walk_variable_declarator(self, it);
    }
}

/// Fills `accessed_members` and `escapes` of every bound import name from uses in `program`.
///
/// `excluded` holds the `(start, end)` spans of identifiers that mention a binding without using it.
pub(crate) fn track(program: &oxc_ast::ast::Program<'_>, info: &mut ModuleInfo, excluded: &HashSet<(u32, u32)>) {
    let mut slots = HashMap::new();
    for import in &info.imports {
        for n in &import.names {
            if let Some(local) = &n.local {
                slots.insert(local.clone(), Slot::default());
            }
        }
    }
    if slots.is_empty() {
        return;
    }
    let mut tracker = Tracker {
        slots,
        open: false,
        excluded,
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
    let mut tracker = Tracker {
        slots: Uses::new(),
        open: true,
        excluded: &excluded,
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
