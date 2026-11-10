//! Custom element registrations (ADR 0062): classes the browser instantiates through a tag name.
//!
//! A class registered as a custom element is used even when no module imports it. This pass records how each
//! exported class is registered, on [`Export::registrations`](crate::Export::registrations); which registrations
//! count is the caller's decision, because a decorator registers only when it comes from a custom element library.

use std::collections::{BTreeMap, HashMap, HashSet};

use oxc_ast::ast::{
    Argument, BindingPattern, CallExpression, Class, Declaration, ExportDefaultDeclarationKind, Expression, Program,
    Statement, StaticBlock, VariableDeclarator,
};
use oxc_ast_visit::{Visit, walk};
use oxc_semantic::{Scoping, SymbolId};

use crate::model::{Export, ExportKind, ImportKind, ImportedKind, ModuleInfo, Registration};
use crate::semantic::Analysis;

/// What an imported binding stands for.
enum Binding {
    /// A named import: the module and the imported name.
    Named(String, String),
    /// A namespace import of the module.
    Namespace(String),
}

/// Which export a registration applies to.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Target {
    /// The export whose local binding has this name.
    Local(String),
    /// The `export default class` declaration, named or not.
    Default,
}

/// Registrations found in one file, by the export they apply to.
#[derive(Default)]
struct Found(BTreeMap<Target, Vec<Registration>>);

impl Found {
    fn add(&mut self, target: Target, registration: Registration) {
        let list = self.0.entry(target).or_default();
        if !list.contains(&registration) {
            list.push(registration);
        }
    }
}

/// Records the registrations of the exported classes of `program` on `info.exports`.
pub(crate) fn mark(program: &Program<'_>, analysis: &Analysis<'_>, info: &mut ModuleInfo) {
    let bindings = import_bindings(info);
    let mut found = Found::default();
    let base_classes = top_level_classes(program, &bindings, &mut found);
    let mut calls = Calls::default();
    calls.visit_program(program);
    // A registry variable is matched by name in the first pass. When a `define` goes through one, the calls are
    // collected again with scopes resolved, so a `define` on another binding of the same name does not count. Most
    // files have no registry variable and need no semantic analysis for this pass.
    if calls
        .defines
        .iter()
        .any(|call| calls.is_registry_variable(&call.object))
    {
        calls = Calls {
            scoping: Some(analysis.scoping()),
            ..Calls::default()
        };
        calls.visit_program(program);
    }
    for call in &calls.defines {
        match &call.object {
            // `window.customElements.define('x-a', A)`.
            DefineObject::MemberRegistry => {
                if let Some(class) = &call.class {
                    found.add(Target::Local(class.clone()), Registration::Define);
                }
            }
            // `customElements.define('x-a', A)`, or the same through a variable holding a registry.
            DefineObject::Registry { name, symbol } if calls.is_registry(name, *symbol) => {
                if let Some(class) = &call.class {
                    found.add(Target::Local(class.clone()), Registration::Define);
                }
            }
            // `A.define({ … })`: `A` extends an imported base class.
            DefineObject::Registry { name, .. } if base_classes.contains_key(name) => {
                found.add(Target::Local(name.clone()), base_classes[name].clone());
            }
            // `FASTElement.define(A, { … })`: the static form on the imported base class, with the class first.
            DefineObject::Registry { name, .. } => {
                if let (Some(class), Some(Binding::Named(module, imported))) = (&call.first, bindings.get(name)) {
                    let registration = Registration::BaseClassDefine {
                        module: module.clone(),
                        name: imported.clone(),
                    };
                    found.add(Target::Local(class.clone()), registration);
                }
            }
        }
    }
    for (target, registrations) in found.0 {
        for export in info.exports.iter_mut().filter(|export| applies(&target, export)) {
            for registration in &registrations {
                if !export.registrations.contains(registration) {
                    export.registrations.push(registration.clone());
                }
            }
        }
    }
}

/// Whether a registration of `target` is one of `export`.
fn applies(target: &Target, export: &Export) -> bool {
    match target {
        Target::Local(name) => {
            !matches!(export.kind, ExportKind::ReExport | ExportKind::StarReExport)
                && export.from.is_none()
                && export.local_name.as_deref() == Some(name.as_str())
        }
        Target::Default => export.kind == ExportKind::Default && export.name == "default",
    }
}

/// Adds the decorator registrations of the top-level classes to `found`, and returns the classes that extend an
/// imported base class with the registration a `define` call of theirs would make.
fn top_level_classes(
    program: &Program<'_>,
    bindings: &HashMap<String, Binding>,
    found: &mut Found,
) -> HashMap<String, Registration> {
    let mut base_classes = HashMap::new();
    for (class, is_default) in program.body.iter().filter_map(top_level_class) {
        let name = class.id.as_ref().map(|id| id.name.as_str().to_owned());
        for decorator in &class.decorators {
            let Some(registration) = decorator_registration(&decorator.expression, bindings) else {
                continue;
            };
            if let Some(name) = &name {
                found.add(Target::Local(name.clone()), registration.clone());
            }
            if is_default {
                found.add(Target::Default, registration);
            }
        }
        if let (Some(name), Some(heritage)) = (name, &class.heritage)
            && let Some(registration) = base_class_registration(&heritage.expression, bindings)
        {
            base_classes.insert(name, registration);
        }
    }
    base_classes
}

/// The class a top-level statement declares, and whether it is the default export.
fn top_level_class<'s, 'a>(statement: &'s Statement<'a>) -> Option<(&'s Class<'a>, bool)> {
    match statement {
        Statement::ClassDeclaration(class) => Some((class, false)),
        Statement::ExportDeclaration(export) => match &export.declaration {
            Declaration::ClassDeclaration(class) => Some((class, false)),
            _ => None,
        },
        Statement::ExportDefaultDeclaration(export) => match &export.declaration {
            ExportDefaultDeclarationKind::ClassDeclaration(class) => Some((class, true)),
            _ => None,
        },
        _ => None,
    }
}

/// The local bindings of the static imports, by local name.
fn import_bindings(info: &ModuleInfo) -> HashMap<String, Binding> {
    let mut bindings = HashMap::new();
    for import in info.imports.iter().filter(|import| import.kind == ImportKind::Static) {
        for name in &import.names {
            let Some(local) = &name.local else { continue };
            let binding = match &name.kind {
                ImportedKind::Named(imported) => Binding::Named(import.specifier.clone(), imported.clone()),
                ImportedKind::Namespace => Binding::Namespace(import.specifier.clone()),
                ImportedKind::Default => continue,
            };
            bindings.insert(local.clone(), binding);
        }
    }
    bindings
}

/// The registration a decorator expression goes through, when its binding is imported: `@name`, `@name(…)` or
/// `@ns.name(…)`.
fn decorator_registration(expression: &Expression<'_>, bindings: &HashMap<String, Binding>) -> Option<Registration> {
    let callee = match expression {
        Expression::CallExpression(call) => &call.callee,
        Expression::Identifier(_) => expression,
        _ => return None,
    };
    let (module, name) = match callee {
        Expression::Identifier(id) => match bindings.get(id.name.as_str())? {
            Binding::Named(module, name) => (module.clone(), name.clone()),
            Binding::Namespace(_) => return None,
        },
        Expression::StaticMemberExpression(member) => {
            let Expression::Identifier(object) = &member.object else {
                return None;
            };
            match bindings.get(object.name.as_str())? {
                Binding::Namespace(module) => (module.clone(), member.property.name.as_str().to_owned()),
                Binding::Named(..) => return None,
            }
        }
        _ => return None,
    };
    Some(Registration::Decorator { module, name })
}

/// The registration through an imported base class: `extends Base` or `extends Mixin(Base)`.
fn base_class_registration(expression: &Expression<'_>, bindings: &HashMap<String, Binding>) -> Option<Registration> {
    let named = |name: &str| match bindings.get(name)? {
        Binding::Named(module, imported) => Some(Registration::BaseClassDefine {
            module: module.clone(),
            name: imported.clone(),
        }),
        Binding::Namespace(_) => None,
    };
    match expression {
        Expression::Identifier(id) => named(id.name.as_str()),
        Expression::CallExpression(call) => call.arguments.iter().find_map(|argument| match argument {
            Argument::Identifier(id) => named(id.name.as_str()),
            _ => None,
        }),
        _ => None,
    }
}

/// The object of a `.define(…)` call.
enum DefineObject {
    /// An identifier: `customElements`, a registry variable, or a class (`Element.define()`), with the symbol it
    /// resolves to once scopes are resolved (`None` for a global or before resolution).
    Registry { name: String, symbol: Option<SymbolId> },
    /// `<anything>.customElements`, such as `window.customElements`.
    MemberRegistry,
}

/// One `x.define(…)` or `x.defineAsync(…)` call.
struct DefineCall {
    object: DefineObject,
    /// The class passed as the second argument of a registry's `define`, when it is an identifier or `this` in a
    /// static block.
    class: Option<String>,
    /// The first argument of a `define`, when it is an identifier: the class of `FASTElement.define(A, { … })`.
    first: Option<String>,
}

/// Collects the `define` calls and the variables that hold a custom element registry.
#[derive(Default)]
struct Calls<'s> {
    /// The resolved scopes of the program, in the second pass.
    scoping: Option<&'s Scoping>,
    defines: Vec<DefineCall>,
    /// The names of the variables that hold a registry.
    registries: HashSet<String>,
    /// The symbols of those variables, in the second pass.
    registry_symbols: HashSet<SymbolId>,
    /// Names of the enclosing classes (`None` for an anonymous one).
    classes: Vec<Option<String>>,
    /// The class whose static block encloses the current node, innermost last.
    static_blocks: Vec<Option<String>>,
}

impl Calls<'_> {
    /// Whether `object` names a variable that holds a registry, by name.
    fn is_registry_variable(&self, object: &DefineObject) -> bool {
        matches!(object, DefineObject::Registry { name, .. } if self.registries.contains(name))
    }

    /// Whether the identifier `name`, resolved to `symbol` in the second pass, is a registry: the global
    /// `customElements` or a variable holding one.
    fn is_registry(&self, name: &str, symbol: Option<SymbolId>) -> bool {
        match (self.scoping, symbol) {
            (Some(_), Some(symbol)) => self.registry_symbols.contains(&symbol),
            (Some(_), None) => name == "customElements",
            (None, _) => name == "customElements" || self.registries.contains(name),
        }
    }

    fn define_call(&self, call: &CallExpression<'_>) -> Option<DefineCall> {
        let Expression::StaticMemberExpression(callee) = &call.callee else {
            return None;
        };
        let method = callee.property.name.as_str();
        let object = match &callee.object {
            Expression::Identifier(id) => DefineObject::Registry {
                name: id.name.as_str().to_owned(),
                symbol: self
                    .scoping
                    .zip(id.reference_id.get())
                    .and_then(|(scoping, reference)| scoping.get_reference(reference).symbol_id()),
            },
            Expression::StaticMemberExpression(member) if member.property.name == "customElements" => {
                if method != "define" {
                    return None;
                }
                DefineObject::MemberRegistry
            }
            _ => return None,
        };
        if !matches!(method, "define" | "defineAsync") {
            return None;
        }
        let class = match call.arguments.get(1) {
            Some(Argument::Identifier(id)) if method == "define" => Some(id.name.as_str().to_owned()),
            Some(Argument::ThisExpression(_)) if method == "define" => self.static_blocks.last().cloned().flatten(),
            _ => None,
        };
        let first = match call.arguments.first() {
            Some(Argument::Identifier(id)) if method == "define" => Some(id.name.as_str().to_owned()),
            _ => None,
        };
        Some(DefineCall { object, class, first })
    }
}

impl<'a> Visit<'a> for Calls<'_> {
    fn visit_variable_declarator(&mut self, it: &VariableDeclarator<'a>) {
        if let BindingPattern::BindingIdentifier(id) = &it.id
            && let Some(init) = &it.init
        {
            let is_registry = match init.get_inner_expression() {
                Expression::Identifier(init) => init.name == "customElements",
                Expression::StaticMemberExpression(member) => member.property.name == "customElements",
                Expression::NewExpression(new) => {
                    matches!(&new.callee, Expression::Identifier(callee) if callee.name == "CustomElementRegistry")
                }
                _ => false,
            };
            if is_registry {
                self.registries.insert(id.name.as_str().to_owned());
                self.registry_symbols.extend(id.symbol_id.get());
            }
        }
        walk::walk_variable_declarator(self, it);
    }

    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        if let Some(call) = self.define_call(it) {
            self.defines.push(call);
        }
        walk::walk_call_expression(self, it);
    }

    fn visit_class(&mut self, it: &Class<'a>) {
        self.classes.push(it.id.as_ref().map(|id| id.name.as_str().to_owned()));
        walk::walk_class(self, it);
        self.classes.pop();
    }

    fn visit_static_block(&mut self, it: &StaticBlock<'a>) {
        self.static_blocks.push(self.classes.last().cloned().flatten());
        walk::walk_static_block(self, it);
        self.static_blocks.pop();
    }
}
