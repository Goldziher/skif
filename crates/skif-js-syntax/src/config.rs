//! Static evaluation of the value a tool configuration file exports. Nothing is executed.

use std::collections::HashMap;
use std::path::Path;

use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Argument, ArrayExpressionElement, BindingPattern, Declaration, ExportDefaultDeclarationKind, Expression,
    ObjectPropertyKind, Program, Statement, UnaryOperator, VariableDeclarationKind,
};
use oxc_parser::{ParseOptions as OxcParseOptions, Parser};
use oxc_span::SourceType;
use serde::{Deserialize, Serialize};

use crate::finder::static_string;

const MAX_DEPTH: u32 = 32;

/// A statically known value from a config file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConfigValue {
    /// `null`.
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// A numeric literal.
    Number(f64),
    /// A string literal or a template literal without expressions.
    String(String),
    /// An array literal; spreads of static arrays are flattened, anything else becomes [`ConfigValue::Unknown`].
    Array(Vec<ConfigValue>),
    /// An object literal in source order. A spread of an unknown value is recorded as the key `...`.
    Object(Vec<(String, ConfigValue)>),
    /// Anything that cannot be known without running the code.
    Unknown,
}

impl ConfigValue {
    /// The value of `key` when this is an object.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&ConfigValue> {
        match self {
            Self::Object(entries) => entries.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// The string when this is a string.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s),
            _ => None,
        }
    }

    /// The elements when this is an array.
    #[must_use]
    pub fn as_array(&self) -> Option<&[ConfigValue]> {
        match self {
            Self::Array(items) => Some(items),
            _ => None,
        }
    }

    /// The entries when this is an object.
    #[must_use]
    pub fn as_object(&self) -> Option<&[(String, ConfigValue)]> {
        match self {
            Self::Object(entries) => Some(entries),
            _ => None,
        }
    }

    /// The boolean when this is a boolean.
    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Whether the value, or anything inside it, is [`ConfigValue::Unknown`].
    #[must_use]
    pub fn has_unknown(&self) -> bool {
        match self {
            Self::Unknown => true,
            Self::Array(items) => items.iter().any(Self::has_unknown),
            Self::Object(entries) => entries.iter().any(|(_, v)| v.has_unknown()),
            _ => false,
        }
    }
}

/// Statically evaluates what `source` exports: `export default …`, `module.exports = …` or `export = …`.
///
/// Understood: object, array, string, number, boolean and `null` literals; template literals without expressions;
/// `as`/`satisfies`/`!`/parenthesised wrappers; calls named `define*` (such as `defineConfig`), which evaluate to
/// their first argument; arrow and function expressions that return a single expression; spreads of static values; and
/// identifiers that name a top-level `const` initialised with something static. Everything else is
/// [`ConfigValue::Unknown`]. Returns `None` when the file does not parse or exports nothing recognisable.
#[must_use]
pub fn extract_config_object(source: &str, path: &Path) -> Option<ConfigValue> {
    let source_type = SourceType::from_path(path).ok()?;
    let allocator = Allocator::default();
    let ret = Parser::new(&allocator, source, source_type)
        .with_options(OxcParseOptions {
            preserve_parens: false,
            ..OxcParseOptions::default()
        })
        .parse();
    if ret.panicked || ret.diagnostics.errors().next().is_some() {
        return None;
    }
    let program = &ret.program;
    let scope = top_level_consts(program);
    let eval = Evaluator { consts: &scope };
    exported_expression(program).map(|expr| eval.expr(expr, 0))
}

fn top_level_consts<'p, 'a>(program: &'p Program<'a>) -> HashMap<String, &'p Expression<'a>> {
    let mut map = HashMap::new();
    for stmt in &program.body {
        let decl = match stmt {
            Statement::ExportNamedDeclaration(e) => e.declaration.as_ref(),
            other => other.as_declaration(),
        };
        if let Some(Declaration::VariableDeclaration(v)) = decl
            && v.kind == VariableDeclarationKind::Const
        {
            for d in &v.declarations {
                if let (BindingPattern::BindingIdentifier(id), Some(init)) = (&d.id, &d.init) {
                    map.insert(id.name.as_str().to_string(), init);
                }
            }
        }
    }
    map
}

fn exported_expression<'p, 'a>(program: &'p Program<'a>) -> Option<&'p Expression<'a>> {
    for stmt in &program.body {
        match stmt {
            Statement::ExportDefaultDeclaration(e) => {
                return match &e.declaration {
                    ExportDefaultDeclarationKind::FunctionDeclaration(_)
                    | ExportDefaultDeclarationKind::ClassDeclaration(_)
                    | ExportDefaultDeclarationKind::TSInterfaceDeclaration(_) => None,
                    other => other.as_expression(),
                };
            }
            Statement::TSExportAssignment(e) => return Some(&e.expression),
            Statement::ExpressionStatement(s) => {
                if let Expression::AssignmentExpression(a) = &s.expression
                    && let oxc_ast::ast::AssignmentTarget::StaticMemberExpression(m) = &a.left
                    && matches!(&m.object, Expression::Identifier(id) if id.name.as_str() == "module")
                    && m.property.name.as_str() == "exports"
                {
                    return Some(&a.right);
                }
            }
            _ => {}
        }
    }
    None
}

struct Evaluator<'c, 'p, 'a> {
    consts: &'c HashMap<String, &'p Expression<'a>>,
}

impl Evaluator<'_, '_, '_> {
    fn expr(&self, expr: &Expression<'_>, depth: u32) -> ConfigValue {
        if depth > MAX_DEPTH {
            return ConfigValue::Unknown;
        }
        let depth = depth + 1;
        let inner = expr.get_inner_expression();
        match inner {
            Expression::NullLiteral(_) => ConfigValue::Null,
            Expression::BooleanLiteral(b) => ConfigValue::Bool(b.value),
            Expression::NumericLiteral(n) => ConfigValue::Number(n.value),
            Expression::StringLiteral(_) | Expression::TemplateLiteral(_) => {
                static_string(inner).map_or(ConfigValue::Unknown, ConfigValue::String)
            }
            Expression::UnaryExpression(u) if u.operator == UnaryOperator::UnaryNegation => match self.expr(&u.argument, depth) {
                ConfigValue::Number(n) => ConfigValue::Number(-n),
                _ => ConfigValue::Unknown,
            },
            Expression::ArrayExpression(a) => self.array(&a.elements, depth),
            Expression::ObjectExpression(o) => self.object(&o.properties, depth),
            Expression::Identifier(id) => self
                .consts
                .get(id.name.as_str())
                .map_or(ConfigValue::Unknown, |init| self.expr(init, depth)),
            Expression::CallExpression(c) => self.call(c, depth),
            Expression::ArrowFunctionExpression(f) => self.returned(&f.body.statements, depth),
            Expression::FunctionExpression(f) => f
                .body
                .as_ref()
                .map_or(ConfigValue::Unknown, |b| self.returned(&b.statements, depth)),
            _ => ConfigValue::Unknown,
        }
    }

    fn call(&self, call: &oxc_ast::ast::CallExpression<'_>, depth: u32) -> ConfigValue {
        let callee = match call.callee.get_inner_expression() {
            Expression::Identifier(id) => Some(id.name.as_str()),
            Expression::StaticMemberExpression(m) => Some(m.property.name.as_str()),
            _ => None,
        };
        let is_define = callee.is_some_and(|n| n.starts_with("define") && n.len() > "define".len());
        match call.arguments.first().and_then(Argument::as_expression) {
            Some(first) if is_define => self.expr(first, depth),
            _ => ConfigValue::Unknown,
        }
    }

    /// Value returned by a function body that is a single expression or ends in `return <expr>`.
    fn returned(&self, statements: &[Statement<'_>], depth: u32) -> ConfigValue {
        match statements {
            [Statement::ExpressionStatement(s)] => self.expr(&s.expression, depth),
            [.., Statement::ReturnStatement(r)] => r
                .argument
                .as_ref()
                .map_or(ConfigValue::Unknown, |e| self.expr(e, depth)),
            _ => ConfigValue::Unknown,
        }
    }

    fn array(&self, elements: &[ArrayExpressionElement<'_>], depth: u32) -> ConfigValue {
        let mut out = Vec::new();
        for element in elements {
            match element {
                ArrayExpressionElement::SpreadElement(s) => match self.expr(&s.argument, depth) {
                    ConfigValue::Array(items) => out.extend(items),
                    _ => out.push(ConfigValue::Unknown),
                },
                other => out.push(other.as_expression().map_or(ConfigValue::Unknown, |e| self.expr(e, depth))),
            }
        }
        ConfigValue::Array(out)
    }

    fn object(&self, properties: &[ObjectPropertyKind<'_>], depth: u32) -> ConfigValue {
        let mut out: Vec<(String, ConfigValue)> = Vec::new();
        for property in properties {
            match property {
                ObjectPropertyKind::SpreadProperty(s) => match self.expr(&s.argument, depth) {
                    ConfigValue::Object(entries) => out.extend(entries),
                    _ => out.push(("...".to_string(), ConfigValue::Unknown)),
                },
                ObjectPropertyKind::ObjectProperty(p) => {
                    let Some(key) = p.key.static_name() else {
                        out.push(("...".to_string(), ConfigValue::Unknown));
                        continue;
                    };
                    let value = if p.method || p.kind != oxc_ast::ast::PropertyKind::Init {
                        ConfigValue::Unknown
                    } else {
                        self.expr(&p.value, depth)
                    };
                    out.push((key.to_string(), value));
                }
            }
        }
        ConfigValue::Object(out)
    }
}
