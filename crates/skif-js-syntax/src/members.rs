//! Members read through imported bindings (`ns.a`, `Enum.A`, `const { a } = ns`, `ns.Type`).
//!
//! Matching is by name, not by scope: a shadowing local with the same name can only make a binding look more used.

use std::collections::{BTreeSet, HashMap};

use oxc_ast::ast::{
    BindingPattern, ComputedMemberExpression, Expression, IdentifierReference, StaticMemberExpression, TSQualifiedName,
    TSTypeName, VariableDeclarator,
};
use oxc_ast_visit::{Visit, walk};

use crate::model::ModuleInfo;

#[derive(Default)]
struct Slot {
    members: BTreeSet<String>,
    uses: u32,
    member_uses: u32,
}

struct Tracker {
    slots: HashMap<String, Slot>,
}

impl Tracker {
    fn member(&mut self, local: &str, member: &str) {
        if let Some(slot) = self.slots.get_mut(local) {
            slot.members.insert(member.to_string());
            slot.member_uses += 1;
        }
    }
}

impl<'a> Visit<'a> for Tracker {
    fn visit_identifier_reference(&mut self, it: &IdentifierReference<'a>) {
        if let Some(slot) = self.slots.get_mut(it.name.as_str()) {
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
        {
            self.member(id.name.as_str(), s.value.as_str());
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
                .map(|p| p.key.static_name().map(|k| k.to_string()))
                .collect();
            if let Some(keys) = keys {
                if let Some(slot) = self.slots.get_mut(id.name.as_str()) {
                    slot.members.extend(keys);
                    slot.member_uses += 1;
                }
            }
        }
        walk::walk_variable_declarator(self, it);
    }
}

/// Fills `accessed_members` and `escapes` of every bound import name from uses in `program`.
pub(crate) fn track(program: &oxc_ast::ast::Program<'_>, info: &mut ModuleInfo) {
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
    let mut tracker = Tracker { slots };
    tracker.visit_program(program);
    for import in &mut info.imports {
        for n in &mut import.names {
            let Some(slot) = n.local.as_ref().and_then(|l| tracker.slots.get(l)) else {
                continue;
            };
            n.accessed_members = slot.members.iter().cloned().collect();
            n.escapes |= slot.uses > slot.member_uses;
        }
    }
}
