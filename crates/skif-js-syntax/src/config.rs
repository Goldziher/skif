//! Static evaluation of the value a tool configuration file exports. Nothing is executed.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;

use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Argument, ArrayExpressionElement, ArrowFunctionExpression, AssignmentOperator, AssignmentTarget,
    AssignmentTargetPropertyIdentifier, BinaryOperator, BindingPattern, CallExpression, ConditionalExpression,
    Declaration, ExportDefaultDeclarationKind, Expression, ExpressionStatement, Function, IfStatement,
    ImportDeclarationSpecifier, ImportOrExportKind, LogicalExpression, LogicalOperator, NewExpression, ObjectProperty,
    ObjectPropertyKind, Program, ReturnStatement, SimpleAssignmentTarget, Statement, StaticMemberExpression,
    TaggedTemplateExpression, TemplateLiteral, UnaryExpression, UnaryOperator, VariableDeclaration,
    VariableDeclarationKind,
};
use oxc_ast_visit::{Visit, walk};
use oxc_parser::{ParseOptions as OxcParseOptions, Parser};
use oxc_span::SourceType;
use serde::{Deserialize, Serialize};

use crate::finder::static_string;
use crate::sharing::{Change, Unstable, changed_by_call, statement_call};

const MAX_DEPTH: u32 = 32;

/// How many constants may be evaluated inside one another; a longer chain is unknown.
const MAX_CONST_CHAIN: usize = 64;

/// The largest value a constant may evaluate to. Constants that reference each other twice at every level
/// (`const b = [a, a]; const c = [b, b]; …`) double in size per level; a value past this is unknown.
const MAX_CONST_NODES: usize = 10_000;

/// How many function calls one evaluation may make; a function that calls itself twice at every level is unknown
/// past this.
const MAX_CALLS: usize = 256;

/// How many modules, the config file included, may be evaluated inside one another; an import past the eighth module
/// of a chain is unknown.
const MAX_IMPORT_DEPTH: usize = 8;

/// How many distinct modules one config may load, so a config that reaches many modules stays cheap.
const MAX_MODULE_LOADS: usize = 64;

/// How many imported exports one config may evaluate in all, calls with arguments included, so imported functions
/// that call one another many times stay cheap.
const MAX_MODULE_EVALUATIONS: usize = 1024;

/// Packages whose default export or named exports list files by glob.
const GLOB_PACKAGES: &[&str] = &["glob", "fast-glob", "globby", "tinyglobby"];

/// Packages whose `merge` and `mergeWith` merge configuration objects. Any other module's `merge` (an `RxJS` stream
/// combinator) is not a config combinator.
const MERGE_PACKAGES: &[&str] = &[
    "lodash",
    "lodash-es",
    "lodash/merge",
    "lodash/mergeWith",
    "lodash.merge",
    "lodash.mergewith",
    "deepmerge",
    "deepmerge-ts",
    "ts-deepmerge",
    "merge",
    "merge-deep",
    "webpack-merge",
    "defu",
];

/// Names a combinator call may use that other libraries use for unrelated functions.
const AMBIGUOUS_MERGES: &[&str] = &["merge", "mergeWith"];

/// Members of a glob function that list files too (`fg.sync`, `glob.globSync`).
const GLOB_MEMBERS: &[&str] = &["sync", "async", "glob", "globSync", "globbySync", "stream"];

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
    /// A regular expression literal: its pattern, without the slashes and flags.
    Regex(String),
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

    /// The pattern when this is a regular expression literal.
    #[must_use]
    pub fn as_regex(&self) -> Option<&str> {
        match self {
            Self::Regex(pattern) => Some(pattern),
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

/// Finds the modules a config file imports, so their values can be evaluated statically too.
///
/// The evaluator asks for every module a config imports a value from (`import base from './base'`,
/// `import { vitestConfig } from '@scope/vitest-config/frontend'`, `require('./base')`). An implementation returns the
/// source of the modules worth following, typically files of the project itself and of its workspace packages, and
/// `None` for anything else (an installed package, a builtin), whose values then stay [`ConfigValue::Unknown`].
pub trait ModuleLoader {
    /// The path and source of the module `specifier` names when imported from the file `from`.
    fn load(&self, from: &Path, specifier: &str) -> Option<(PathBuf, String)>;
}

/// Statically evaluates what `source` exports: `export default …`, `module.exports = …` or `export = …`.
///
/// Understood: object, array, string, number, boolean and `null` literals; template literals without expressions;
/// `as`/`satisfies`/`!`/parenthesised wrappers; calls named `define*` (such as `defineConfig`), which evaluate to
/// their first argument; arrow and function expressions that return a single expression; spreads of static values; and
/// identifiers that name a top-level `const` initialised with something static; and paths computed from the config
/// file's own location (`__dirname`, `import.meta.dirname`, `new URL('./x', import.meta.url)`, `path.resolve`,
/// `path.join`, `path.dirname` and `fileURLToPath`, however `path` was imported), which evaluate to strings relative to
/// the config file's directory (`"."`, `"./src"`, `"../shared"`).
///
/// `a ? b : c` takes the branch a known test selects; with an unknown test it is what both branches share (for two
/// objects, every key either has, unknown where they differ or only one has it). `a && b`, `a || b`, `a ?? b` and `!a`
/// are followed when `a` is known, and `.length` of a known string or array is its length.
/// An `if (test) return a;` before a function's last `return` is followed the same way: a known test takes its branch,
/// and an unknown one makes the value what both returns share. A spread of a conditional with an unknown test
/// (`...(tmp && { cacheDir: tmp })`, `...(ci ? { a } : {})`) adds every key a branch sets: a list gets the elements of
/// every branch (a string counting as a list of one), a nested object merges the same way, and any other value the
/// branches do not share is unknown; only a branch that is not an object (or `null`, a boolean, a number or an empty
/// string) leaves the spread unknown.
///
/// Functions are followed: a returned config function may declare local `const`/`let` bindings and local functions,
/// assign members of a local object, and call top-level or local functions with known arguments, a parameter not passed
/// taking its default and an object pattern parameter (`{ pinCjs = [] } = {}`) its properties or their defaults; `await x` and
/// `Promise.resolve(x)` evaluate to `x`; `require.resolve('pkg')` evaluates to `'pkg'`, and the storybook idiom
/// `dirname(require.resolve(join('pkg', 'package.json')))` to `'pkg'`. A call of a glob function (`glob`,
/// `fast-glob`, `globby`, `tinyglobby`, however imported) evaluates to the array of its literal patterns, so a
/// dynamic `entry` becomes entry globs. `process.cwd()` and a first `path.resolve` argument that is not absolute are
/// taken to be the config file's directory, which is right when the tool runs from the package that holds the
/// config and a heuristic otherwise. The directory itself is `"."`, so `dirname(__dirname)` is `".."`. A URL with a
/// scheme (`new URL(x, 'https://…')`, `fileURLToPath('file:///…')`) and `.href` are [`ConfigValue::Unknown`], as is
/// everything else. Returns `None` when the file does not parse or exports nothing recognisable.
///
/// Imports are not followed; [`extract_config_object_with`] follows them.
#[must_use]
pub fn extract_config_object(source: &str, path: &Path) -> Option<ConfigValue> {
    evaluate_config(source, path, None)
}

/// Like [`extract_config_object`], and follows the imports `loader` finds: a value imported from another module (a
/// default or named export, a namespace member, a `require`d `module.exports`) is evaluated statically in that module,
/// and a call of an imported function with the arguments given. A chain of imports reaches at most eight modules, the
/// config file included, an export whose value depends on itself through imports is [`ConfigValue::Unknown`] there,
/// and one config loads at most 64 distinct modules and evaluates at most 1024 imported exports and calls.
/// Paths an imported module computes from its own location (`__dirname`, `import.meta.url`) come back relative to the
/// directory of the config file at `path`.
#[must_use]
pub fn extract_config_object_with(source: &str, path: &Path, loader: &dyn ModuleLoader) -> Option<ConfigValue> {
    let modules = Modules {
        loader,
        root: path.parent().map(Path::to_path_buf).unwrap_or_default(),
        active: RefCell::new(vec![(path.to_path_buf(), Imported::Default)]),
        loads: RefCell::new(HashSet::new()),
        evaluations: Cell::new(0),
        cuts: Cell::new(0),
        values: RefCell::new(HashMap::new()),
        loaded: RefCell::new(HashMap::new()),
    };
    evaluate_config(source, path, Some(&modules))
}

fn evaluate_config(source: &str, path: &Path, modules: Option<&Modules<'_>>) -> Option<ConfigValue> {
    let source_type = SourceType::from_path(path).ok()?;
    let allocator = Allocator::default();
    let ret = Parser::new(&allocator, source, source_type)
        .with_options(OxcParseOptions {
            preserve_parens: false,
            ..OxcParseOptions::default()
        })
        .parse();
    if ret.diagnostics.errors().next().is_some() {
        return None;
    }
    let program = &ret.program;
    let top = top_level(program);
    let bindings = module_bindings(program);
    let imports = import_bindings(program);
    let file = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    let eval = Evaluator {
        consts: &top.consts,
        changed: &top.changed,
        unstable: &top.unstable,
        functions: &top.functions,
        bindings: &bindings,
        file,
        dir: ".",
        path,
        imports: &imports,
        modules,
        evaluated: RefCell::new(HashMap::new()),
        chain: RefCell::new(Vec::new()),
        frames: RefCell::new(Vec::new()),
        calls: Cell::new(0),
        config_setter: RefCell::new(None),
        captured: RefCell::new(None),
    };
    let exported = exported(program)?;
    let callable = match exported {
        Exported::Expression(expr) => Callable::of_expression(expr),
        Exported::Function(function) => Some(Callable::Function(function)),
    };
    *eval.config_setter.borrow_mut() = callable.and_then(first_parameter);
    let value = match exported {
        Exported::Expression(expr) => eval.expr(expr, 0),
        Exported::Function(function) => eval.invoke(Callable::Function(function), &[], false, 0),
    };
    let value = if exports_changed(program) {
        changed(value)
    } else {
        value
    };
    let captured = eval.captured.borrow_mut().take();
    Some(captured.unwrap_or(value))
}

/// What an import binds: a module's default export, one named export, or the module namespace.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Imported {
    Default,
    Named(String),
    Namespace,
}

impl Imported {
    fn named(name: String) -> Self {
        if name == "default" {
            Self::Default
        } else {
            Self::Named(name)
        }
    }
}

/// A local name bound by an `import` declaration.
struct ImportBinding {
    specifier: String,
    imported: Imported,
}

/// The value imports of a module by local name; type-only imports bind no value.
fn import_bindings(program: &Program<'_>) -> HashMap<String, ImportBinding> {
    let mut imports = HashMap::new();
    for stmt in &program.body {
        let Statement::ImportDeclaration(d) = stmt else {
            continue;
        };
        if d.import_kind.is_type() {
            continue;
        }
        let specifier = crate::js_string(d.source.value);
        for item in d.specifiers.iter().flatten() {
            let (local, imported) = match item {
                ImportDeclarationSpecifier::ImportSpecifier(s) if s.import_kind == ImportOrExportKind::Value => (
                    s.local.name.as_str(),
                    Imported::named(crate::collect::module_export_name(&s.imported)),
                ),
                ImportDeclarationSpecifier::ImportSpecifier(_) => continue,
                ImportDeclarationSpecifier::ImportDefaultSpecifier(s) => (s.local.name.as_str(), Imported::Default),
                ImportDeclarationSpecifier::ImportNamespaceSpecifier(s) => (s.local.name.as_str(), Imported::Namespace),
            };
            imports.insert(
                local.to_owned(),
                ImportBinding {
                    specifier: specifier.clone(),
                    imported,
                },
            );
        }
    }
    imports
}

/// Where the value of an export comes from.
enum ExportSource {
    /// A binding of the module itself, by local name.
    Local(String),
    /// An export of another module (`export { a as b } from './x'`).
    From(String, Imported),
}

/// The named exports of a module, and the modules it re-exports everything of (`export * from './x'`).
fn export_table(program: &Program<'_>) -> (HashMap<String, ExportSource>, Vec<String>) {
    let mut table = HashMap::new();
    let mut stars = Vec::new();
    for stmt in &program.body {
        match stmt {
            Statement::ExportDeclaration(e) => match &e.declaration {
                Declaration::VariableDeclaration(v) => {
                    for d in &v.declarations {
                        if let BindingPattern::BindingIdentifier(id) = &d.id {
                            let name = id.name.as_str().to_owned();
                            table.insert(name.clone(), ExportSource::Local(name));
                        }
                    }
                }
                Declaration::FunctionDeclaration(f) => {
                    if let Some(id) = &f.id {
                        let name = id.name.as_str().to_owned();
                        table.insert(name.clone(), ExportSource::Local(name));
                    }
                }
                _ => {}
            },
            Statement::ExportNamedDeclaration(e) if !e.export_kind.is_type() => {
                for s in e.specifiers.iter().filter(|s| !s.export_kind.is_type()) {
                    table.insert(
                        crate::collect::module_export_name(&s.exported),
                        ExportSource::Local(crate::collect::module_export_name(&s.local)),
                    );
                }
            }
            Statement::ExportFromDeclaration(e) if !e.export_kind.is_type() => {
                let from = crate::js_string(e.source.value);
                for s in e.specifiers.iter().filter(|s| !s.export_kind.is_type()) {
                    table.insert(
                        crate::collect::module_export_name(&s.exported),
                        ExportSource::From(
                            from.clone(),
                            Imported::named(crate::collect::module_export_name(&s.local)),
                        ),
                    );
                }
            }
            Statement::ExportAllDeclaration(e) if !e.export_kind.is_type() => {
                let from = crate::js_string(e.source.value);
                match &e.exported {
                    Some(name) => {
                        table.insert(
                            crate::collect::module_export_name(name),
                            ExportSource::From(from, Imported::Namespace),
                        );
                    }
                    None => stars.push(from),
                }
            }
            _ => {}
        }
    }
    (table, stars)
}

/// The path and source of a module the loader found.
type LoadedModule = (PathBuf, Rc<str>);

/// Follows the imports of a config and the modules it reaches.
struct Modules<'l> {
    loader: &'l dyn ModuleLoader,
    /// The directory of the config file, which paths computed in imported modules are relative to.
    root: PathBuf,
    /// The exports being evaluated, outermost (the config file's default export) first. One that depends on itself
    /// is unknown; other exports of the same module are not, as ES modules may import each other.
    active: RefCell<Vec<(PathBuf, Imported)>>,
    /// The modules evaluated so far.
    loads: RefCell<HashSet<PathBuf>>,
    /// How many exports have been evaluated so far, calls with arguments included.
    evaluations: Cell<usize>,
    /// How many times an evaluation was cut short (a cycle, the depth limit, a budget) and gave
    /// [`ConfigValue::Unknown`]. A value computed while this grew depends on what else was being evaluated at the
    /// time, so it is not cached.
    cuts: Cell<usize>,
    /// The value of every export evaluated without arguments so far, by module and export, computed without a cut.
    values: RefCell<HashMap<(PathBuf, Imported), Option<ConfigValue>>>,
    /// What the loader returned for each importing directory and specifier.
    loaded: RefCell<HashMap<(PathBuf, String), Option<LoadedModule>>>,
}

impl Modules<'_> {
    /// The module `specifier` names from `from`, asking the loader once per directory and specifier.
    fn load(&self, from: &Path, specifier: &str) -> Option<LoadedModule> {
        let key = (from.parent().unwrap_or(from).to_path_buf(), specifier.to_owned());
        if let Some(known) = self.loaded.borrow().get(&key) {
            return known.clone();
        }
        let module = self
            .loader
            .load(from, specifier)
            .map(|(path, source)| (path, Rc::from(source)));
        self.loaded.borrow_mut().insert(key, module.clone());
        module
    }

    /// Whether the module `specifier` names from `from` is followed.
    fn follows(&self, from: &Path, specifier: &str) -> bool {
        self.load(from, specifier).is_some()
    }

    /// The value `imported` names in the module `specifier` resolves to from `from`, called with `args` when given.
    /// `None` when the module is not followed or does not have the export.
    fn evaluate(
        &self,
        from: &Path,
        specifier: &str,
        imported: &Imported,
        args: Option<&[ConfigValue]>,
    ) -> Option<ConfigValue> {
        let (path, source) = self.load(from, specifier)?;
        let key = (path.clone(), imported.clone());
        if args.is_none()
            && let Some(known) = self.values.borrow().get(&key)
        {
            return known.clone();
        }
        if self.cut(&key) {
            self.cuts.set(self.cuts.get() + 1);
            return Some(ConfigValue::Unknown);
        }
        self.loads.borrow_mut().insert(path.clone());
        self.evaluations.set(self.evaluations.get() + 1);
        let cuts = self.cuts.get();
        self.active.borrow_mut().push(key.clone());
        let value = self.evaluate_module(&path, &source, imported, args);
        self.active.borrow_mut().pop();
        if args.is_none() && self.cuts.get() == cuts {
            self.values.borrow_mut().insert(key, value.clone());
        }
        value
    }

    /// Whether evaluating the export `key` would depend on itself, nest past [`MAX_IMPORT_DEPTH`] modules or exceed a
    /// budget.
    fn cut(&self, key: &(PathBuf, Imported)) -> bool {
        let active = self.active.borrow();
        let loads = self.loads.borrow();
        active.contains(key)
            || active.len() >= MAX_IMPORT_DEPTH
            || (!loads.contains(&key.0) && loads.len() >= MAX_MODULE_LOADS)
            || self.evaluations.get() >= MAX_MODULE_EVALUATIONS
    }

    fn evaluate_module(
        &self,
        path: &Path,
        source: &str,
        imported: &Imported,
        args: Option<&[ConfigValue]>,
    ) -> Option<ConfigValue> {
        let source_type = SourceType::from_path(path).ok()?;
        let allocator = Allocator::default();
        let ret = Parser::new(&allocator, source, source_type)
            .with_options(OxcParseOptions {
                preserve_parens: false,
                ..OxcParseOptions::default()
            })
            .parse();
        if ret.diagnostics.errors().next().is_some() {
            return Some(ConfigValue::Unknown);
        }
        let program = &ret.program;
        let top = top_level(program);
        let bindings = module_bindings(program);
        let imports = import_bindings(program);
        let dir = relative_dir(&self.root, path.parent().unwrap_or(path));
        let eval = Evaluator {
            consts: &top.consts,
            changed: &top.changed,
            unstable: &top.unstable,
            functions: &top.functions,
            bindings: &bindings,
            file: path.file_name().and_then(|name| name.to_str()).unwrap_or_default(),
            dir: &dir,
            path,
            imports: &imports,
            modules: Some(self),
            evaluated: RefCell::new(HashMap::new()),
            chain: RefCell::new(Vec::new()),
            frames: RefCell::new(Vec::new()),
            calls: Cell::new(0),
            config_setter: RefCell::new(None),
            captured: RefCell::new(None),
        };
        match imported {
            Imported::Namespace => Some(ConfigValue::Unknown),
            Imported::Default => Some(match exported(program)? {
                Exported::Function(function) => eval.invoke(Callable::Function(function), args.unwrap_or(&[]), true, 0),
                Exported::Expression(expr) => {
                    match (args, Callable::of_expression(expr), expr.get_inner_expression()) {
                        (None, _, _) => eval.expr(expr, 0),
                        (Some(args), Some(callable), _) => eval.invoke(callable, args, true, 0),
                        (Some(args), None, Expression::Identifier(id)) => eval.call_named(id.name.as_str(), args),
                        (Some(_), None, _) => ConfigValue::Unknown,
                    }
                }
            })
            .map(|value| {
                if exports_changed(program) {
                    changed(value)
                } else {
                    value
                }
            }),
            Imported::Named(name) => {
                let (table, stars) = export_table(program);
                match table.get(name) {
                    Some(ExportSource::Local(local)) => Some(match args {
                        None => eval.identifier(local, 0),
                        Some(args) => eval.call_named(local, args),
                    }),
                    Some(ExportSource::From(from, imported)) => self.evaluate(path, from, imported, args),
                    None => stars.iter().find_map(|from| self.evaluate(path, from, imported, args)),
                }
            }
        }
    }
}

/// The directory `to` relative to the directory `from`, lexically: `"."`, `"./a"` or `"../b"`.
fn relative_dir(from: &Path, to: &Path) -> String {
    let from: Vec<Component<'_>> = from.components().collect();
    let to: Vec<Component<'_>> = to.components().collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<String> = vec!["..".to_owned(); from.len() - common];
    parts.extend(
        to[common..]
            .iter()
            .map(|part| part.as_os_str().to_string_lossy().into_owned()),
    );
    let parts: Vec<&str> = parts.iter().map(String::as_str).collect();
    normalized(&parts)
}

/// Name of the first parameter of `callable`, when it is a plain identifier.
fn first_parameter(callable: Callable<'_, '_>) -> Option<String> {
    let params = match callable {
        Callable::Function(f) => &f.params,
        Callable::Arrow(f) => &f.params,
    };
    match &params.items.first()?.pattern {
        BindingPattern::BindingIdentifier(id) => Some(id.name.as_str().to_owned()),
        _ => None,
    }
}

/// The top-level `const` initialisers and function declarations of a module.
struct TopLevel<'p, 'a> {
    consts: HashMap<String, &'p Expression<'a>>,
    functions: HashMap<String, Callable<'p, 'a>>,
    /// The names top-level statements other than function declarations assign or change (`config.plugins.push(x)`,
    /// `if (env) config.a = 1`, `const n = list.push(x)`): a constant among them is not only its initialiser.
    changed: HashSet<String>,
    /// The bindings code the evaluator does not follow may change.
    unstable: Unstable,
}

/// A function the evaluator can call.
#[derive(Clone, Copy)]
enum Callable<'p, 'a> {
    Function(&'p Function<'a>),
    Arrow(&'p ArrowFunctionExpression<'a>),
}

impl<'p, 'a> Callable<'p, 'a> {
    /// Whether both are the same function.
    fn same(self, other: Self) -> bool {
        match (self, other) {
            (Self::Function(a), Self::Function(b)) => std::ptr::eq(a, b),
            (Self::Arrow(a), Self::Arrow(b)) => std::ptr::eq(a, b),
            _ => false,
        }
    }

    fn of_expression(expr: &'p Expression<'a>) -> Option<Self> {
        match expr.get_inner_expression() {
            Expression::ArrowFunctionExpression(f) => Some(Self::Arrow(f)),
            Expression::FunctionExpression(f) => Some(Self::Function(f)),
            _ => None,
        }
    }
}

/// What a name is bound to inside a function being evaluated.
#[derive(Clone)]
enum Binding<'p, 'a> {
    Value(ConfigValue),
    Callable(Callable<'p, 'a>),
}

type Frame<'p, 'a> = HashMap<String, Binding<'p, 'a>>;

fn top_level<'p, 'a>(program: &'p Program<'a>) -> TopLevel<'p, 'a> {
    let mut top = TopLevel {
        consts: HashMap::new(),
        functions: HashMap::new(),
        changed: HashSet::new(),
        unstable: Unstable::of(program),
    };
    for stmt in &program.body {
        let decl = match stmt {
            Statement::ExportDeclaration(e) => Some(&e.declaration),
            other => other.as_declaration(),
        };
        let writes = match stmt {
            // Only what the statement itself does, as for a statement of a function: an exported value may read
            // members of a constant (`base.plugins.map(f)`) without changing it.
            Statement::ExpressionStatement(s) => Some(Writes::of_effect(&s.expression)),
            Statement::IfStatement(_)
            | Statement::BlockStatement(_)
            | Statement::ForStatement(_)
            | Statement::ForInStatement(_)
            | Statement::ForOfStatement(_)
            | Statement::WhileStatement(_)
            | Statement::DoWhileStatement(_)
            | Statement::SwitchStatement(_)
            | Statement::TryStatement(_)
            | Statement::LabeledStatement(_)
            | Statement::VariableDeclaration(_)
            | Statement::ExportNamedDeclaration(_)
            | Statement::ExportDefaultDeclaration(_) => Some(Writes::of_statement(stmt)),
            _ => None,
        };
        if let Some(writes) = writes {
            top.changed.extend(writes.assigned.into_iter().chain(writes.changed));
        }
        match decl {
            Some(Declaration::VariableDeclaration(v)) if v.kind == VariableDeclarationKind::Const => {
                for d in &v.declarations {
                    if let (BindingPattern::BindingIdentifier(id), Some(init)) = (&d.id, &d.init) {
                        top.consts.insert(id.name.as_str().to_string(), init);
                        if let Some(callable) = Callable::of_expression(init) {
                            top.functions.insert(id.name.as_str().to_string(), callable);
                        }
                    }
                }
            }
            Some(Declaration::FunctionDeclaration(f)) => {
                if let Some(id) = &f.id {
                    top.functions
                        .insert(id.name.as_str().to_string(), Callable::Function(f));
                }
            }
            _ => {}
        }
    }
    top
}

/// Functions of `path`, whose results are computed lexically.
const PATH_FUNCTIONS: &[&str] = &["resolve", "join", "dirname", "normalize"];

/// The local names the config file gives to the `path` and `url` modules and their functions.
#[derive(Default)]
struct ModuleBindings {
    path_modules: HashSet<String>,
    path_functions: HashMap<String, String>,
    url_modules: HashSet<String>,
    file_url_to_path: HashSet<String>,
    /// Local names of glob functions and glob modules.
    glob_names: HashSet<String>,
    /// Local names bound to a package that merges configuration objects.
    merge_names: HashSet<String>,
    /// Local `merge` and `mergeWith` imported from a package that does something else with them.
    foreign_merges: HashSet<String>,
    /// Local names of the functions that combine `ESLint` flat configs into one list (`defineConfig` of
    /// `eslint/config`).
    flat_config_functions: HashSet<String>,
    /// Local names of `typescript-eslint`, whose `config` combines flat configs.
    flat_config_modules: HashSet<String>,
}

impl ModuleBindings {
    /// Records `local` as the module itself (`member` is `None`) or as its export `member`.
    fn bind(&mut self, module: &str, member: Option<&str>, local: &str) {
        let local = local.to_owned();
        if MERGE_PACKAGES.contains(&module) {
            self.merge_names.insert(local.clone());
        } else if AMBIGUOUS_MERGES.contains(&local.as_str()) {
            self.foreign_merges.insert(local.clone());
        }
        match (module.strip_prefix("node:").unwrap_or(module), member) {
            ("path" | "path/posix", None | Some("default")) => {
                self.path_modules.insert(local);
            }
            ("path" | "path/posix", Some(name)) if PATH_FUNCTIONS.contains(&name) => {
                self.path_functions.insert(local, name.to_owned());
            }
            ("url", None | Some("default")) => {
                self.url_modules.insert(local);
            }
            ("url", Some("fileURLToPath")) => {
                self.file_url_to_path.insert(local);
            }
            (package, _) if GLOB_PACKAGES.contains(&package) => {
                self.glob_names.insert(local);
            }
            ("fs", Some("globSync")) | ("fs/promises", Some("glob")) => {
                self.glob_names.insert(local);
            }
            ("eslint/config", Some("defineConfig")) | ("typescript-eslint", Some("config")) => {
                self.flat_config_functions.insert(local);
            }
            ("typescript-eslint", None | Some("default")) => {
                self.flat_config_modules.insert(local);
            }
            _ => {}
        }
    }
}

fn module_bindings(program: &Program<'_>) -> ModuleBindings {
    let mut bindings = ModuleBindings::default();
    for stmt in &program.body {
        if let Statement::ImportDeclaration(d) = stmt {
            let module = d.source.value.as_str().unwrap_or_default();
            for specifier in d.specifiers.iter().flatten() {
                match specifier {
                    ImportDeclarationSpecifier::ImportSpecifier(s) => {
                        let imported = crate::collect::module_export_name(&s.imported);
                        bindings.bind(module, Some(&imported), s.local.name.as_str());
                    }
                    ImportDeclarationSpecifier::ImportDefaultSpecifier(s) => {
                        bindings.bind(module, None, s.local.name.as_str());
                    }
                    ImportDeclarationSpecifier::ImportNamespaceSpecifier(s) => {
                        bindings.bind(module, None, s.local.name.as_str());
                    }
                }
            }
        } else if let Some(Declaration::VariableDeclaration(v)) = stmt.as_declaration() {
            for declarator in &v.declarations {
                if let Some(module) = declarator.init.as_ref().and_then(required_module) {
                    bind_required(&mut bindings, &module, &declarator.id);
                } else if let Some(Expression::Identifier(source)) =
                    declarator.init.as_ref().map(Expression::get_inner_expression)
                {
                    // `const { resolve } = path;` destructures a module binding made earlier.
                    let module = if bindings.path_modules.contains(source.name.as_str()) {
                        Some("path")
                    } else if bindings.url_modules.contains(source.name.as_str()) {
                        Some("url")
                    } else {
                        None
                    };
                    if let Some(module) = module {
                        bind_required(&mut bindings, module, &declarator.id);
                    }
                }
            }
        }
    }
    bindings
}

/// The module a `require("module")` call loads.
fn required_module(init: &Expression<'_>) -> Option<String> {
    let Expression::CallExpression(call) = init.get_inner_expression() else {
        return None;
    };
    match &call.callee {
        Expression::Identifier(id) if id.name.as_str() == "require" => call
            .arguments
            .first()
            .and_then(Argument::as_expression)
            .and_then(static_string),
        _ => None,
    }
}

fn bind_required(bindings: &mut ModuleBindings, module: &str, id: &BindingPattern<'_>) {
    match id {
        BindingPattern::BindingIdentifier(local) => bindings.bind(module, None, local.name.as_str()),
        BindingPattern::ObjectPattern(pattern) => {
            for property in &pattern.properties {
                if let (Some(key), BindingPattern::BindingIdentifier(local)) =
                    (property.key.static_name(), &property.value)
                {
                    bindings.bind(module, Some(&crate::js_string(key.as_js_str())), local.name.as_str());
                }
            }
        }
        _ => {}
    }
}

/// Joins path segments lexically into a path relative to the config directory: `"."`, `"./a/b"` or `"../a"`.
fn normalized(parts: &[&str]) -> String {
    let mut segments: Vec<&str> = Vec::new();
    for part in parts.iter().flat_map(|part| part.split(['/', '\\'])) {
        match part {
            "" | "." => {}
            ".." if segments.last().is_some_and(|last| *last != "..") => {
                segments.pop();
            }
            other => segments.push(other),
        }
    }
    match segments.first() {
        None => ".".to_owned(),
        Some(&"..") => segments.join("/"),
        Some(_) => format!("./{}", segments.join("/")),
    }
}

/// The result of a `path` or `url` function, `Unknown` when an argument is not a known relative path.
fn apply_path_function(function: &str, args: &[ConfigValue]) -> ConfigValue {
    let Some(parts) = args.iter().map(ConfigValue::as_str).collect::<Option<Vec<&str>>>() else {
        return ConfigValue::Unknown;
    };
    let absolute = |part: &&str| part.starts_with(['/', '\\']);
    let path = match (function, parts.as_slice()) {
        // Only a path derived from `import.meta.url` is known; a URL with a scheme names something else.
        ("fileURLToPath", [file]) if !file.contains(':') => (*file).to_owned(),
        ("dirname", [path]) => normalized(&[*path, ".."]),
        ("normalize", [path]) => normalized(&[*path]),
        ("resolve", parts) if !parts.iter().any(absolute) => normalized(parts),
        ("join", parts) if !parts.first().is_some_and(absolute) => normalized(parts),
        _ => return ConfigValue::Unknown,
    };
    ConfigValue::String(path)
}

/// Statically evaluates the option `key` of every `new constructor({ … })` in `source`, however the constructor was
/// imported (`new NodejsFunction(…)`, `new lambda_nodejs.NodejsFunction(…)`). Path computations are relative to the
/// directory of `path`. Returns nothing when the file does not parse.
#[must_use]
pub fn extract_constructor_options(source: &str, path: &Path, constructor: &str, key: &str) -> Vec<ConfigValue> {
    let Ok(source_type) = SourceType::from_path(path) else {
        return Vec::new();
    };
    let allocator = Allocator::default();
    let ret = Parser::new(&allocator, source, source_type)
        .with_options(OxcParseOptions {
            preserve_parens: false,
            ..OxcParseOptions::default()
        })
        .parse();
    if ret.diagnostics.errors().next().is_some() {
        return Vec::new();
    }
    let program = &ret.program;
    let top = top_level(program);
    let bindings = module_bindings(program);
    let mut finder = OptionFinder {
        top: &top,
        bindings: &bindings,
        file: path.file_name().and_then(|name| name.to_str()).unwrap_or_default(),
        constructor,
        key,
        found: Vec::new(),
    };
    finder.visit_program(program);
    finder.found
}

/// Statically evaluates the value of every property named `key` of every object literal in `source`, however deeply
/// nested and wherever it appears (`handler: "src/api.handler"` inside a function body), in source order. Values are
/// evaluated as in [`extract_config_object`], with paths relative to the directory of `path`. Methods are left out.
/// Returns nothing when the file does not parse.
#[must_use]
pub fn extract_property_values(source: &str, path: &Path, key: &str) -> Vec<ConfigValue> {
    let Ok(source_type) = SourceType::from_path(path) else {
        return Vec::new();
    };
    let allocator = Allocator::default();
    let ret = Parser::new(&allocator, source, source_type)
        .with_options(OxcParseOptions {
            preserve_parens: false,
            ..OxcParseOptions::default()
        })
        .parse();
    if ret.diagnostics.errors().next().is_some() {
        return Vec::new();
    }
    let program = &ret.program;
    let top = top_level(program);
    let bindings = module_bindings(program);
    let mut finder = PropertyFinder {
        top: &top,
        bindings: &bindings,
        file: path.file_name().and_then(|name| name.to_str()).unwrap_or_default(),
        key,
        evaluated: HashMap::new(),
        found: Vec::new(),
    };
    finder.visit_program(program);
    finder.found
}

/// A call, or a tagged template, of a function a module exports; see [`extract_imported_calls`].
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedCall {
    /// The module the function comes from.
    pub module: String,
    /// The export called: `"default"` for the default export or the module object itself (`pino(…)`), the export's
    /// name for a named import (`execa(…)`), or the member called on the module object (`pino.transport(…)`).
    pub export: String,
    /// The statically evaluated arguments, in order; empty for a tagged template.
    pub arguments: Vec<ConfigValue>,
    /// For a tagged template (`` $`cmd ${x}` ``), its raw text with every substitution replaced by
    /// [`TEMPLATE_SUBSTITUTION`]; `None` for a call.
    pub template: Option<String>,
}

/// What a substitution of a tagged template reads as in [`ImportedCall::template`].
pub const TEMPLATE_SUBSTITUTION: &str = "$__skif__";

/// Every call and tagged template in `source` whose function is imported from one of `modules` (`import`, `import * as`,
/// or a top-level `require`, plain or destructured), with its arguments statically evaluated as in
/// [`extract_config_object`]. `globals` names functions available without an import, as `(name, module)` pairs (the
/// `$` of a zx script). Shadowed names are not tracked. Returns nothing when the file does not parse.
#[must_use]
pub fn extract_imported_calls(
    source: &str,
    path: &Path,
    modules: &[&str],
    globals: &[(&str, &str)],
) -> Vec<ImportedCall> {
    let Ok(source_type) = SourceType::from_path(path) else {
        return Vec::new();
    };
    let allocator = Allocator::default();
    let ret = Parser::new(&allocator, source, source_type)
        .with_options(OxcParseOptions {
            preserve_parens: false,
            ..OxcParseOptions::default()
        })
        .parse();
    if ret.diagnostics.errors().next().is_some() {
        return Vec::new();
    }
    let file = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    imported_calls_in(&ret.program, file, modules, globals)
}

/// The calls and tagged templates in a parsed `program` whose function is imported from one of `modules`, as
/// [`extract_imported_calls`] finds them; `file` is the file name `__filename` evaluates to.
pub(crate) fn imported_calls_in(
    program: &Program<'_>,
    file: &str,
    modules: &[&str],
    globals: &[(&str, &str)],
) -> Vec<ImportedCall> {
    let imported = imported_functions(program, modules, globals);
    if imported.is_empty() {
        return Vec::new();
    }
    let top = top_level(program);
    let bindings = module_bindings(program);
    let mut finder = CallFinder {
        top: &top,
        bindings: &bindings,
        file,
        imported: &imported,
        found: Vec::new(),
    };
    finder.visit_program(program);
    finder.found
}

/// The command line of a `(file, [arguments])` call such as `spawn('eslint', ['.'])`: the file and its literal
/// arguments, a computed argument left out. `None` when the file or a literal argument is not a plain word, because
/// dropping it would change what the command means.
#[must_use]
pub fn literal_command(arguments: &[ConfigValue]) -> Option<String> {
    let is_safe = |word: &str| {
        !word.is_empty()
            && word
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '_' | '@' | '.' | ',' | ':' | '/' | '=' | '+' | '~' | '-'))
    };
    let file = arguments.first()?.as_str().filter(|file| is_safe(file))?;
    let mut words = vec![file];
    if let Some(ConfigValue::Array(items)) = arguments.get(1) {
        for word in items.iter().filter_map(ConfigValue::as_str) {
            if !is_safe(word) {
                return None;
            }
            words.push(word);
        }
    }
    Some(words.join(" "))
}

/// Local name to module and named export (`None` for the default export or the module object) of everything a
/// program imports from `modules`, plus `globals`.
type ImportedFunctions = HashMap<String, (String, Option<String>)>;

/// The entries of one object literal, as a spread branch lists them.
type Entries<'v> = &'v [(String, ConfigValue)];

fn imported_functions(program: &Program<'_>, modules: &[&str], globals: &[(&str, &str)]) -> ImportedFunctions {
    let mut imported: ImportedFunctions = globals
        .iter()
        .map(|(name, module)| ((*name).to_owned(), ((*module).to_owned(), Some((*name).to_owned()))))
        .collect();
    let named = |export: String| Some(export).filter(|e| e != "default");
    for stmt in &program.body {
        if let Statement::ImportDeclaration(d) = stmt {
            let module = d.source.value.as_str().unwrap_or_default();
            if !modules.contains(&module) {
                continue;
            }
            for specifier in d.specifiers.iter().flatten() {
                let (local, export) = match specifier {
                    ImportDeclarationSpecifier::ImportSpecifier(s) => (
                        s.local.name.as_str(),
                        named(crate::collect::module_export_name(&s.imported)),
                    ),
                    ImportDeclarationSpecifier::ImportDefaultSpecifier(s) => (s.local.name.as_str(), None),
                    ImportDeclarationSpecifier::ImportNamespaceSpecifier(s) => (s.local.name.as_str(), None),
                };
                imported.insert(local.to_owned(), (module.to_owned(), export));
            }
        } else if let Some(Declaration::VariableDeclaration(v)) = stmt.as_declaration() {
            for declarator in &v.declarations {
                if let Some(module) = declarator.init.as_ref().and_then(required_module)
                    && modules.contains(&module.as_str())
                {
                    for (local, export) in required_names(&declarator.id) {
                        imported.insert(local, (module.clone(), export.and_then(named)));
                    }
                }
            }
        }
    }
    imported
}

/// The local names a `require` result is bound to, with the export each destructured one takes.
fn required_names(id: &BindingPattern<'_>) -> Vec<(String, Option<String>)> {
    match id {
        BindingPattern::BindingIdentifier(local) => vec![(local.name.as_str().to_owned(), None)],
        BindingPattern::ObjectPattern(pattern) => pattern
            .properties
            .iter()
            .filter_map(|property| match (property.key.static_name(), &property.value) {
                (Some(key), BindingPattern::BindingIdentifier(local)) => {
                    Some((local.name.as_str().to_owned(), Some(crate::js_string(key.as_js_str()))))
                }
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

struct CallFinder<'t, 'p, 'a> {
    top: &'t TopLevel<'p, 'a>,
    bindings: &'t ModuleBindings,
    file: &'t str,
    /// Local name to module and named export (`None` for the default export or the module object).
    imported: &'t ImportedFunctions,
    found: Vec<ImportedCall>,
}

impl<'a> CallFinder<'_, '_, 'a> {
    /// The module and export `callee` calls, when it is an imported function or a member of an imported module.
    fn target(&self, callee: &Expression<'a>) -> Option<(String, String)> {
        match callee.get_inner_expression() {
            Expression::Identifier(id) => self
                .imported
                .get(id.name.as_str())
                .map(|(module, export)| (module.clone(), export.clone().unwrap_or_else(|| "default".to_owned()))),
            Expression::StaticMemberExpression(member) => match member.object.get_inner_expression() {
                Expression::Identifier(id) => self
                    .imported
                    .get(id.name.as_str())
                    .filter(|(_, export)| export.is_none())
                    .map(|(module, _)| (module.clone(), member.property.name.as_str().to_owned())),
                _ => None,
            },
            _ => None,
        }
    }
}

impl<'a> Visit<'a> for CallFinder<'_, '_, 'a> {
    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        if let Some((module, export)) = self.target(&it.callee) {
            let eval = Evaluator {
                consts: &self.top.consts,
                changed: &self.top.changed,
                unstable: &self.top.unstable,
                functions: &self.top.functions,
                bindings: self.bindings,
                file: self.file,
                dir: ".",
                path: Path::new(self.file),
                imports: &HashMap::new(),
                modules: None,
                evaluated: RefCell::new(HashMap::new()),
                chain: RefCell::new(Vec::new()),
                frames: RefCell::new(Vec::new()),
                calls: Cell::new(0),
                config_setter: RefCell::new(None),
                captured: RefCell::new(None),
            };
            let arguments = it
                .arguments
                .iter()
                .map(|argument| {
                    argument
                        .as_expression()
                        .map_or(ConfigValue::Unknown, |e| eval.expr(e, 0))
                })
                .collect();
            self.found.push(ImportedCall {
                module,
                export,
                arguments,
                template: None,
            });
        }
        walk::walk_call_expression(self, it);
    }

    fn visit_tagged_template_expression(&mut self, it: &TaggedTemplateExpression<'a>) {
        let tag = match it.tag.get_inner_expression() {
            Expression::CallExpression(call) => &call.callee,
            other => other,
        };
        if let Some((module, export)) = self.target(tag) {
            let quasis: Vec<&str> = it.quasi.quasis.iter().map(|q| q.value.raw.as_str()).collect();
            self.found.push(ImportedCall {
                module,
                export,
                arguments: Vec::new(),
                template: Some(quasis.join(TEMPLATE_SUBSTITUTION)),
            });
        }
        walk::walk_tagged_template_expression(self, it);
    }
}

struct OptionFinder<'t, 'p, 'a> {
    top: &'t TopLevel<'p, 'a>,
    bindings: &'t ModuleBindings,
    file: &'t str,
    constructor: &'t str,
    key: &'t str,
    found: Vec<ConfigValue>,
}

impl<'a> Visit<'a> for OptionFinder<'_, '_, 'a> {
    fn visit_new_expression(&mut self, it: &NewExpression<'a>) {
        let name = match it.callee.get_inner_expression() {
            Expression::Identifier(id) => Some(id.name.as_str()),
            Expression::StaticMemberExpression(member) => Some(member.property.name.as_str()),
            _ => None,
        };
        if name == Some(self.constructor) {
            let eval = Evaluator {
                consts: &self.top.consts,
                changed: &self.top.changed,
                unstable: &self.top.unstable,
                functions: &self.top.functions,
                bindings: self.bindings,
                file: self.file,
                dir: ".",
                path: Path::new(self.file),
                imports: &HashMap::new(),
                modules: None,
                evaluated: RefCell::new(HashMap::new()),
                chain: RefCell::new(Vec::new()),
                frames: RefCell::new(Vec::new()),
                calls: Cell::new(0),
                config_setter: RefCell::new(None),
                captured: RefCell::new(None),
            };
            for argument in &it.arguments {
                let Some(Expression::ObjectExpression(options)) =
                    argument.as_expression().map(Expression::get_inner_expression)
                else {
                    continue;
                };
                for property in &options.properties {
                    if let ObjectPropertyKind::ObjectProperty(p) = property
                        && !p.method
                        && p.key
                            .static_name()
                            .is_some_and(|name| crate::js_string(name.as_js_str()) == self.key)
                    {
                        self.found.push(eval.expr(&p.value, 0));
                    }
                }
            }
        }
        walk::walk_new_expression(self, it);
    }
}

struct PropertyFinder<'t, 'p, 'a> {
    top: &'t TopLevel<'p, 'a>,
    bindings: &'t ModuleBindings,
    file: &'t str,
    key: &'t str,
    /// The top-level constants evaluated so far, carried from one property to the next so each is evaluated once.
    evaluated: HashMap<String, ConfigValue>,
    found: Vec<ConfigValue>,
}

impl<'a> Visit<'a> for PropertyFinder<'_, '_, 'a> {
    fn visit_object_property(&mut self, it: &ObjectProperty<'a>) {
        if !it.method
            && it
                .key
                .static_name()
                .is_some_and(|name| crate::js_string(name.as_js_str()) == self.key)
        {
            let eval = Evaluator {
                consts: &self.top.consts,
                changed: &self.top.changed,
                unstable: &self.top.unstable,
                functions: &self.top.functions,
                bindings: self.bindings,
                file: self.file,
                dir: ".",
                path: Path::new(self.file),
                imports: &HashMap::new(),
                modules: None,
                evaluated: RefCell::new(std::mem::take(&mut self.evaluated)),
                chain: RefCell::new(Vec::new()),
                frames: RefCell::new(Vec::new()),
                calls: Cell::new(0),
                config_setter: RefCell::new(None),
                captured: RefCell::new(None),
            };
            self.found.push(eval.expr(&it.value, 0));
            self.evaluated = eval.evaluated.into_inner();
        }
        walk::walk_object_property(self, it);
    }
}

/// What a module exports.
#[derive(Clone, Copy)]
enum Exported<'p, 'a> {
    Expression(&'p Expression<'a>),
    Function(&'p Function<'a>),
}

fn exported<'p, 'a>(program: &'p Program<'a>) -> Option<Exported<'p, 'a>> {
    for stmt in &program.body {
        match stmt {
            Statement::ExportDefaultDeclaration(e) => {
                return match &e.declaration {
                    ExportDefaultDeclarationKind::FunctionDeclaration(f) => Some(Exported::Function(f)),
                    ExportDefaultDeclarationKind::ClassDeclaration(_)
                    | ExportDefaultDeclarationKind::TSInterfaceDeclaration(_) => None,
                    other => other.as_expression().map(Exported::Expression),
                };
            }
            Statement::TSExportAssignment(e) => return Some(Exported::Expression(&e.expression)),
            Statement::ExpressionStatement(s) if is_module_exports_assignment(stmt) => {
                if let Expression::AssignmentExpression(a) = &s.expression {
                    return Some(Exported::Expression(&a.right));
                }
            }
            _ => {}
        }
    }
    None
}

/// Whether a statement after the `module.exports = …` that [`exported`] reads changes or replaces what it exports
/// (`module.exports.plugins.push(x)`, `exports.a = 1`).
fn exports_changed(program: &Program<'_>) -> bool {
    let mut statements = program
        .body
        .iter()
        .skip_while(|stmt| !is_module_exports_assignment(stmt));
    statements.next().is_some()
        && statements.any(|stmt| {
            let writes = Writes::of_statement(stmt);
            writes
                .assigned
                .iter()
                .chain(&writes.changed)
                .any(|name| name == "module" || name == "exports")
        })
}

fn is_module_exports_assignment(stmt: &Statement<'_>) -> bool {
    if let Statement::ExpressionStatement(s) = stmt
        && let Expression::AssignmentExpression(a) = &s.expression
        && let AssignmentTarget::StaticMemberExpression(m) = &a.left
    {
        matches!(&m.object, Expression::Identifier(id) if id.name.as_str() == "module")
            && m.property.name.as_str() == "exports"
    } else {
        false
    }
}

/// The number of values `value` consists of, itself included.
fn node_count(value: &ConfigValue) -> usize {
    1 + match value {
        ConfigValue::Array(items) => items.iter().map(node_count).sum(),
        ConfigValue::Object(entries) => entries.iter().map(|(_, item)| node_count(item)).sum(),
        _ => 0,
    }
}

struct Evaluator<'c, 'p, 'a> {
    consts: &'c HashMap<String, &'p Expression<'a>>,
    /// Top-level names changed by statements after their declaration; see [`TopLevel::changed`].
    changed: &'c HashSet<String>,
    /// The bindings code the evaluator does not follow may change; see [`Unstable`].
    unstable: &'c Unstable,
    functions: &'c HashMap<String, Callable<'p, 'a>>,
    bindings: &'c ModuleBindings,
    /// File name of the module, which `__filename` and `import.meta.url` name.
    file: &'c str,
    /// The module's directory relative to the config file's directory: `"."` in the config file itself.
    dir: &'c str,
    /// Path of the module, which its imports are resolved from.
    path: &'c Path,
    /// The module's value imports by local name.
    imports: &'c HashMap<String, ImportBinding>,
    /// Follows imports; `None` when imports are not followed.
    modules: Option<&'c Modules<'c>>,
    /// The value of every top-level constant evaluated so far.
    evaluated: RefCell<HashMap<String, ConfigValue>>,
    /// The constants being evaluated, outermost first; a constant that refers back to one of them is unknown.
    chain: RefCell<Vec<String>>,
    /// The bindings of the functions being evaluated, outermost first.
    frames: RefCell<Vec<Frame<'p, 'a>>>,
    /// How many functions have been called so far.
    calls: Cell<usize>,
    /// The first parameter of an exported config function (`config` in `module.exports = function (config) { … }`):
    /// the object the function hands the real config to with `config.set({ … })`.
    config_setter: RefCell<Option<String>>,
    /// The argument of the last `<config_setter>.set(…)` call evaluated.
    captured: RefCell<Option<ConfigValue>>,
}

impl<'p, 'a> Evaluator<'_, 'p, 'a> {
    fn expr(&self, expr: &'p Expression<'a>, depth: u32) -> ConfigValue {
        if depth > MAX_DEPTH {
            return ConfigValue::Unknown;
        }
        let depth = depth + 1;
        let inner = expr.get_inner_expression();
        match inner {
            Expression::NullLiteral(_) => ConfigValue::Null,
            Expression::BooleanLiteral(b) => ConfigValue::Bool(b.value),
            Expression::NumericLiteral(n) => ConfigValue::Number(n.value),
            Expression::StringLiteral(_) => static_string(inner).map_or(ConfigValue::Unknown, ConfigValue::String),
            Expression::RegExpLiteral(r) => ConfigValue::Regex(r.regex.pattern.text.as_str().to_owned()),
            Expression::TemplateLiteral(template) => self.template(template, depth),
            Expression::BinaryExpression(b) if b.operator == BinaryOperator::Addition => {
                match (self.expr(&b.left, depth), self.expr(&b.right, depth)) {
                    (ConfigValue::String(left), ConfigValue::String(right)) => ConfigValue::String(left + &right),
                    _ => ConfigValue::Unknown,
                }
            }
            Expression::StaticMemberExpression(member) => self.member(member, depth),
            Expression::NewExpression(new) => self.url(new, depth),
            Expression::UnaryExpression(u) if u.operator == UnaryOperator::UnaryNegation => {
                match self.expr(&u.argument, depth) {
                    ConfigValue::Number(n) => ConfigValue::Number(-n),
                    _ => ConfigValue::Unknown,
                }
            }
            Expression::UnaryExpression(u) if u.operator == UnaryOperator::LogicalNot => {
                truthiness(&self.expr(&u.argument, depth)).map_or(ConfigValue::Unknown, |t| ConfigValue::Bool(!t))
            }
            Expression::LogicalExpression(l) => self.logical(l, depth),
            Expression::ConditionalExpression(c) => self.conditional(c, depth),
            Expression::AwaitExpression(a) => self.expr(&a.argument, depth),
            Expression::ArrayExpression(a) => self.array(&a.elements, depth),
            Expression::ObjectExpression(o) => self.object(&o.properties, depth),
            Expression::Identifier(id) => {
                let value = self.identifier(id.name.as_str(), depth);
                match self.unstable.of_reference(id) {
                    Some(change) => unstable(value, change),
                    None => value,
                }
            }
            Expression::CallExpression(c) => self.call(c, depth),
            Expression::ArrowFunctionExpression(f) => self.invoke(Callable::Arrow(f), &[], false, depth),
            Expression::FunctionExpression(f) => self.invoke(Callable::Function(f), &[], false, depth),
            _ => ConfigValue::Unknown,
        }
    }

    /// The value of a name: a binding of a function being evaluated, a top-level constant (evaluated once however
    /// often it is referenced), a top-level function called without arguments, or a path global.
    fn identifier(&self, name: &str, depth: u32) -> ConfigValue {
        if let Some(binding) = self.lookup(name) {
            return match binding {
                Binding::Value(value) => value,
                Binding::Callable(_) => ConfigValue::Unknown,
            };
        }
        let Some(&init) = self.consts.get(name) else {
            return match self.functions.get(name) {
                Some(callable) => self.invoke(*callable, &[], true, depth),
                None => self.import_value(name, None).unwrap_or_else(|| self.global(name)),
            };
        };
        if let Some(known) = self.evaluated.borrow().get(name) {
            return known.clone();
        }
        {
            let chain = self.chain.borrow();
            if chain.len() >= MAX_CONST_CHAIN || chain.iter().any(|active| active == name) {
                return ConfigValue::Unknown;
            }
        }
        self.chain.borrow_mut().push(name.to_owned());
        // The value does not depend on where it is referenced from: evaluate it outside any function's bindings and
        // from a fresh depth budget.
        let saved = std::mem::take(&mut *self.frames.borrow_mut());
        let cuts = self.modules.map(|modules| modules.cuts.get());
        let value = self.expr(init, 0);
        *self.frames.borrow_mut() = saved;
        self.chain.borrow_mut().pop();
        let value = if node_count(&value) > MAX_CONST_NODES {
            ConfigValue::Unknown
        } else if self.changed.contains(name) {
            changed(value)
        } else if let Some(change) = self.unstable.of_top(name) {
            unstable(value, change)
        } else {
            value
        };
        // A value an import cycle or budget cut short depends on the order of evaluation; it is computed again.
        if self.modules.map(|modules| modules.cuts.get()) == cuts {
            self.evaluated.borrow_mut().insert(name.to_owned(), value.clone());
        }
        value
    }

    /// The innermost binding of `name` in the functions being evaluated.
    fn lookup(&self, name: &str) -> Option<Binding<'p, 'a>> {
        self.frames
            .borrow()
            .iter()
            .rev()
            .find_map(|frame| frame.get(name).cloned())
    }

    /// Binds `name` in the innermost frame, or replaces the binding where it is already made.
    fn assign(&self, name: &str, binding: Binding<'p, 'a>) {
        let mut frames = self.frames.borrow_mut();
        if let Some(frame) = frames.iter_mut().rev().find(|frame| frame.contains_key(name)) {
            frame.insert(name.to_owned(), binding);
        } else if let Some(frame) = frames.last_mut() {
            frame.insert(name.to_owned(), binding);
        }
    }

    /// The path of the module's directory (`__dirname`) or of the module itself (`__filename`).
    fn global(&self, name: &str) -> ConfigValue {
        match name {
            "__dirname" => ConfigValue::String(normalized(&[self.dir])),
            "__filename" => ConfigValue::String(self.file_path()),
            _ => ConfigValue::Unknown,
        }
    }

    /// The module's path relative to the config file's directory.
    fn file_path(&self) -> String {
        normalized(&[self.dir, self.file])
    }

    /// The value an imported name stands for, called with `args` when given. `None` when the name is not imported or
    /// its module is not followed.
    fn import_value(&self, name: &str, args: Option<&[ConfigValue]>) -> Option<ConfigValue> {
        let binding = self.imports.get(name)?;
        self.modules?
            .evaluate(self.path, &binding.specifier, &binding.imported, args)
    }

    /// A call of an imported function whose module is followed; `None` otherwise, before evaluating any argument.
    fn imported_call(&self, name: &str, call: &'p CallExpression<'a>, depth: u32) -> Option<ConfigValue> {
        let binding = self.imports.get(name)?;
        let modules = self.modules?;
        if !modules.follows(self.path, &binding.specifier) {
            return None;
        }
        let args = self.arguments(call, depth);
        modules.evaluate(self.path, &binding.specifier, &binding.imported, Some(&args))
    }

    /// `ns.name` of a namespace import `ns`: the export `name` of its module.
    fn namespace_member(&self, namespace: &str, name: &str) -> ConfigValue {
        let (Some(binding), Some(modules)) = (self.imports.get(namespace), self.modules) else {
            return ConfigValue::Unknown;
        };
        modules
            .evaluate(self.path, &binding.specifier, &Imported::named(name.to_owned()), None)
            .unwrap_or(ConfigValue::Unknown)
    }

    /// Calls the function `name` of this module, declared or imported, with `args`.
    fn call_named(&self, name: &str, args: &[ConfigValue]) -> ConfigValue {
        match self.functions.get(name) {
            Some(callable) => self.invoke(*callable, args, true, 0),
            None => self.import_value(name, Some(args)).unwrap_or(ConfigValue::Unknown),
        }
    }

    /// `module.exports` of the module a `require('./x')` call loads. A package is followed only where its value is the
    /// config (`module.exports = require('@scope/config')`); nested in a config a required package is a name.
    fn required_value(&self, call: &'p CallExpression<'a>, depth: u32) -> Option<ConfigValue> {
        let modules = self.modules?;
        if !matches!(call.callee.get_inner_expression(), Expression::Identifier(id) if id.name.as_str() == "require") {
            return None;
        }
        let specifier = single_argument(call).and_then(static_string)?;
        if depth >= 2 && !specifier.starts_with('.') {
            return None;
        }
        modules.evaluate(self.path, &specifier, &Imported::Default, None)
    }

    fn template(&self, template: &'p TemplateLiteral<'a>, depth: u32) -> ConfigValue {
        let mut out = String::new();
        for (index, quasi) in template.quasis.iter().enumerate() {
            out.push_str(
                &quasi
                    .value
                    .cooked
                    .map_or_else(|| quasi.value.raw.as_str().to_owned(), crate::js_string),
            );
            if let Some(expression) = template.expressions.get(index) {
                let ConfigValue::String(text) = self.expr(expression, depth) else {
                    return ConfigValue::Unknown;
                };
                out.push_str(&text);
            }
        }
        ConfigValue::String(out)
    }

    /// `import.meta.dirname`, `import.meta.filename`, `import.meta.url` and the `pathname` of a `new URL(…)`.
    fn member(&self, member: &'p StaticMemberExpression<'a>, depth: u32) -> ConfigValue {
        let property = member.property.name.as_str();
        match (member.object.get_inner_expression(), property) {
            (Expression::ImportMeta(_), "dirname") => ConfigValue::String(normalized(&[self.dir])),
            (Expression::ImportMeta(_), "filename" | "url") => ConfigValue::String(self.file_path()),
            (object @ Expression::NewExpression(_), "pathname") => self.expr(object, depth),
            (Expression::Identifier(object), _)
                if self
                    .imports
                    .get(object.name.as_str())
                    .is_some_and(|b| b.imported == Imported::Namespace)
                    && self.lookup(object.name.as_str()).is_none() =>
            {
                self.namespace_member(object.name.as_str(), property)
            }
            (object, "length") => {
                let length = match self.expr(object, depth) {
                    ConfigValue::String(text) => Some(text.encode_utf16().count()),
                    // An unknown element may stand for a spread of any length.
                    ConfigValue::Array(items) if !items.contains(&ConfigValue::Unknown) => Some(items.len()),
                    _ => None,
                };
                length
                    .and_then(|length| u32::try_from(length).ok())
                    .map_or(ConfigValue::Unknown, |length| ConfigValue::Number(f64::from(length)))
            }
            _ => ConfigValue::Unknown,
        }
    }

    /// `a && b`, `a || b` and `a ?? b`, when `a` decides which side is the value.
    fn logical(&self, logical: &'p LogicalExpression<'a>, depth: u32) -> ConfigValue {
        let left = self.expr(&logical.left, depth);
        match left_is_value(logical.operator, &left) {
            Some(true) => left,
            Some(false) => self.expr(&logical.right, depth),
            None => ConfigValue::Unknown,
        }
    }

    /// The values a spread argument may have: one for an expression whose value is known or not decided by a
    /// condition, and one per branch for `test ? a : b`, `a && b`, `a || b` and `a ?? b` whose test is unknown
    /// (parenthesised or not). A left side that adds nothing when spread is [`ConfigValue::Null`]: the falsy `a` of
    /// `a && b`, and a comparison, negation or `void` on the left of `||` or `??`.
    fn possibilities(&self, expr: &'p Expression<'a>, depth: u32) -> Vec<ConfigValue> {
        if depth > MAX_DEPTH {
            return vec![ConfigValue::Unknown];
        }
        match expr.get_inner_expression() {
            Expression::LogicalExpression(logical) => {
                let left = self.expr(&logical.left, depth);
                let primitive = is_primitive(&logical.left);
                let decided = match logical.operator {
                    LogicalOperator::Coalesce if primitive && !is_void(&logical.left) => Some(true),
                    operator => left_is_value(operator, &left),
                };
                // The left side as the value: a falsy `a` of `a && b`, a comparison, a negation and `void` add
                // nothing.
                let left = if primitive || logical.operator == LogicalOperator::And {
                    ConfigValue::Null
                } else {
                    left
                };
                match decided {
                    Some(true) => vec![left],
                    Some(false) => self.possibilities(&logical.right, depth + 1),
                    None => {
                        let mut out = vec![left];
                        out.extend(self.possibilities(&logical.right, depth + 1));
                        out
                    }
                }
            }
            Expression::ConditionalExpression(conditional) => match truthiness(&self.expr(&conditional.test, depth)) {
                Some(true) => self.possibilities(&conditional.consequent, depth + 1),
                Some(false) => self.possibilities(&conditional.alternate, depth + 1),
                None => {
                    let mut out = self.possibilities(&conditional.consequent, depth + 1);
                    out.extend(self.possibilities(&conditional.alternate, depth + 1));
                    out
                }
            },
            _ => vec![self.expr(expr, depth)],
        }
    }

    /// Adds to `out` what spreading `argument` adds. A conditional spread whose branches are all known adds every key
    /// a branch sets, with what the branches have in common (see [`any_of`]); a branch that is not known adds the
    /// marker key `...`.
    fn spread(&self, argument: &'p Expression<'a>, out: &mut Vec<(String, ConfigValue)>, depth: u32) {
        let branches: Option<Vec<Vec<(String, ConfigValue)>>> = self
            .possibilities(argument, depth)
            .into_iter()
            .map(spread_entries)
            .collect();
        match branches {
            None => out.push(("...".to_owned(), ConfigValue::Unknown)),
            Some(mut branches) if branches.len() == 1 => out.extend(branches.pop().unwrap_or_default()),
            Some(branches) => {
                let added = spread_any(out, &branches);
                out.extend(added);
            }
        }
    }

    /// `test ? a : b`: the branch a known test takes, or what both branches have in common when the test is unknown.
    fn conditional(&self, conditional: &'p ConditionalExpression<'a>, depth: u32) -> ConfigValue {
        match truthiness(&self.expr(&conditional.test, depth)) {
            Some(true) => self.expr(&conditional.consequent, depth),
            Some(false) => self.expr(&conditional.alternate, depth),
            None => either(
                &self.expr(&conditional.consequent, depth),
                &self.expr(&conditional.alternate, depth),
            ),
        }
    }

    /// `new URL('./x', import.meta.url)` as the path it names.
    fn url(&self, new: &'p NewExpression<'a>, depth: u32) -> ConfigValue {
        let is_url =
            matches!(new.callee.get_inner_expression(), Expression::Identifier(id) if id.name.as_str() == "URL");
        let mut args = new.arguments.iter().map(|arg| {
            arg.as_expression()
                .map_or(ConfigValue::Unknown, |e| self.expr(e, depth))
        });
        match (is_url, args.next(), args.next(), args.next()) {
            (true, Some(ConfigValue::String(relative)), Some(ConfigValue::String(base)), None)
                if !relative.starts_with('/') && !relative.contains(':') && !base.contains(':') =>
            {
                ConfigValue::String(normalized(&[base.as_str(), "..", relative.as_str()]))
            }
            _ => ConfigValue::Unknown,
        }
    }

    /// Whether `callee` is the function `wanted` of the `path` module, however it was imported.
    fn is_path_function(&self, callee: &Expression<'_>, wanted: &str) -> bool {
        match callee.get_inner_expression() {
            Expression::Identifier(id) => self
                .bindings
                .path_functions
                .get(id.name.as_str())
                .is_some_and(|function| function == wanted),
            Expression::StaticMemberExpression(member) => {
                member.property.name.as_str() == wanted
                    && matches!(
                        member.object.get_inner_expression(),
                        Expression::Identifier(object) if self.bindings.path_modules.contains(object.name.as_str())
                    )
            }
            _ => false,
        }
    }

    /// A call of a `path` or `url` function, or of `process.cwd()`; `None` for any other call.
    fn path_call(&self, call: &'p CallExpression<'a>, depth: u32) -> Option<ConfigValue> {
        let function = match call.callee.get_inner_expression() {
            Expression::Identifier(id) => {
                let name = id.name.as_str();
                if self.bindings.file_url_to_path.contains(name) {
                    "fileURLToPath"
                } else {
                    self.bindings.path_functions.get(name)?.as_str()
                }
            }
            Expression::StaticMemberExpression(member) => {
                // `path.posix.join` computes like `path.join` for the relative paths a config holds.
                let object = match member.object.get_inner_expression() {
                    Expression::StaticMemberExpression(inner) if inner.property.name.as_str() == "posix" => {
                        inner.object.get_inner_expression()
                    }
                    other => other,
                };
                let Expression::Identifier(object) = object else {
                    return None;
                };
                let (object, property) = (object.name.as_str(), member.property.name.as_str());
                let path_function = self.bindings.path_modules.contains(object) && PATH_FUNCTIONS.contains(&property);
                let url_function = self.bindings.url_modules.contains(object) && property == "fileURLToPath";
                if path_function || url_function {
                    property
                } else if object == "process" && property == "cwd" {
                    return Some(ConfigValue::String(".".to_owned()));
                } else {
                    return None;
                }
            }
            _ => return None,
        };
        Some(apply_path_function(function, &self.arguments(call, depth)))
    }

    /// The values of a call's arguments; a spread argument is unknown.
    fn arguments(&self, call: &'p CallExpression<'a>, depth: u32) -> Vec<ConfigValue> {
        call.arguments
            .iter()
            .map(|arg| {
                arg.as_expression()
                    .map_or(ConfigValue::Unknown, |e| self.expr(e, depth))
            })
            .collect()
    }

    /// `dirname(require.resolve(join(pkg, 'package.json')))`, the directory of a package, which is `pkg` itself.
    fn package_directory(&self, call: &'p CallExpression<'a>, depth: u32) -> Option<ConfigValue> {
        if !self.is_path_function(&call.callee, "dirname") {
            return None;
        }
        let Some(Expression::CallExpression(resolve)) = single_argument(call).map(Expression::get_inner_expression)
        else {
            return None;
        };
        if !is_require_resolve(resolve) {
            return None;
        }
        let Some(Expression::CallExpression(join)) = single_argument(resolve).map(Expression::get_inner_expression)
        else {
            return None;
        };
        if !self.is_path_function(&join.callee, "join") {
            return None;
        }
        let mut arguments = join.arguments.iter().filter_map(Argument::as_expression);
        let (package, file) = (arguments.next()?, arguments.next()?);
        if arguments.next().is_some() || static_string(file).as_deref() != Some("package.json") {
            return None;
        }
        Some(self.expr(package, depth))
    }

    /// `require.resolve(x)` and `Promise.resolve(x)`, which evaluate to `x`.
    fn resolve_call(&self, call: &'p CallExpression<'a>, depth: u32) -> Option<ConfigValue> {
        let Expression::StaticMemberExpression(member) = call.callee.get_inner_expression() else {
            return None;
        };
        let Expression::Identifier(object) = member.object.get_inner_expression() else {
            return None;
        };
        let known = matches!(
            (object.name.as_str(), member.property.name.as_str()),
            ("require" | "Promise", "resolve")
        );
        known.then(|| single_argument(call).map_or(ConfigValue::Unknown, |arg| self.expr(arg, depth)))
    }

    /// A call of a glob function as the array of its literal patterns.
    fn glob_call(&self, call: &'p CallExpression<'a>, depth: u32) -> Option<ConfigValue> {
        let is_glob = match call.callee.get_inner_expression() {
            Expression::Identifier(id) => self.bindings.glob_names.contains(id.name.as_str()),
            Expression::StaticMemberExpression(member) => {
                GLOB_MEMBERS.contains(&member.property.name.as_str())
                    && matches!(
                        member.object.get_inner_expression(),
                        Expression::Identifier(object) if self.bindings.glob_names.contains(object.name.as_str())
                    )
            }
            _ => false,
        };
        if !is_glob {
            return None;
        }
        Some(match call.arguments.first().and_then(Argument::as_expression) {
            Some(patterns) => match self.expr(patterns, depth) {
                ConfigValue::String(pattern) => ConfigValue::Array(vec![ConfigValue::String(pattern)]),
                array @ ConfigValue::Array(_) => array,
                _ => ConfigValue::Unknown,
            },
            None => ConfigValue::Unknown,
        })
    }

    /// `defineConfig(a, [b, c], ...d)` of `eslint/config` and `tseslint.config(…)`: the flat config list of all their
    /// arguments, nested lists flattened.
    fn flat_config_call(&self, call: &'p CallExpression<'a>, depth: u32) -> Option<ConfigValue> {
        let combines = match call.callee.get_inner_expression() {
            Expression::Identifier(id) => self.bindings.flat_config_functions.contains(id.name.as_str()),
            Expression::StaticMemberExpression(member) => {
                member.property.name.as_str() == "config"
                    && matches!(
                        member.object.get_inner_expression(),
                        Expression::Identifier(object) if self.bindings.flat_config_modules.contains(object.name.as_str())
                    )
            }
            _ => false,
        };
        if !combines {
            return None;
        }
        let mut list = Vec::new();
        for argument in &call.arguments {
            let value = match argument {
                Argument::SpreadElement(spread) => self.expr(&spread.argument, depth),
                other => other
                    .as_expression()
                    .map_or(ConfigValue::Unknown, |e| self.expr(e, depth)),
            };
            flatten_into(value, &mut list);
        }
        Some(ConfigValue::Array(list))
    }

    /// A call of a top-level function or of a function declared inside the function being evaluated.
    fn local_call(&self, call: &'p CallExpression<'a>, depth: u32) -> Option<ConfigValue> {
        let Expression::Identifier(id) = call.callee.get_inner_expression() else {
            return None;
        };
        let name = id.name.as_str();
        let (callable, isolate) = match self.lookup(name) {
            Some(Binding::Callable(callable)) => (callable, false),
            Some(Binding::Value(_)) => return None,
            None => match self.functions.get(name) {
                Some(callable) => (*callable, true),
                None => return self.imported_call(name, call, depth),
            },
        };
        Some(self.invoke(callable, &self.arguments(call, depth), isolate, depth))
    }

    /// A call of a config combinator (`mergeConfig`, `defu`, `Object.assign`, `deepmerge`, `merge`): the merge of the
    /// object arguments. An argument that cannot be known contributes a `...` entry, so the known part still applies.
    fn merge_call(&self, call: &'p CallExpression<'a>, depth: u32) -> Option<ConfigValue> {
        let style = merge_style(&call.callee, self.bindings)?;
        let mut merged: Option<Vec<(String, ConfigValue)>> = None;
        let mut values = self.arguments(call, depth);
        if style == MergeStyle::FirstWins {
            values.reverse();
        }
        for value in values {
            match value {
                ConfigValue::Null | ConfigValue::Bool(_) => {}
                ConfigValue::Object(entries) => {
                    let base = merged.get_or_insert_with(Vec::new);
                    merge_objects(base, entries, style != MergeStyle::Shallow);
                }
                _ => merge_objects(
                    merged.get_or_insert_with(Vec::new),
                    vec![("...".to_owned(), ConfigValue::Unknown)],
                    false,
                ),
            }
        }
        Some(merged.map_or(ConfigValue::Unknown, ConfigValue::Object))
    }

    /// Records the argument of `<config_setter>.set(…)` as the config; returns whether `call` is such a call.
    fn capture_set(&self, call: &'p CallExpression<'a>, depth: u32) -> bool {
        let Some(setter) = self.config_setter.borrow().clone() else {
            return false;
        };
        let Expression::StaticMemberExpression(member) = call.callee.get_inner_expression() else {
            return false;
        };
        let on_setter =
            matches!(member.object.get_inner_expression(), Expression::Identifier(id) if id.name.as_str() == setter);
        let Some(argument) = single_argument(call).filter(|_| on_setter && member.property.name.as_str() == "set")
        else {
            return false;
        };
        let value = self.expr(argument, depth);
        *self.captured.borrow_mut() = Some(value);
        true
    }

    fn call(&self, call: &'p CallExpression<'a>, depth: u32) -> ConfigValue {
        if self.capture_set(call, depth) {
            return ConfigValue::Unknown;
        }
        let special = self
            .required_value(call, depth)
            .or_else(|| required_package(call, depth))
            .or_else(|| self.package_directory(call, depth))
            .or_else(|| self.path_call(call, depth))
            .or_else(|| self.resolve_call(call, depth))
            .or_else(|| self.glob_call(call, depth))
            .or_else(|| self.flat_config_call(call, depth))
            .or_else(|| self.local_call(call, depth))
            .or_else(|| self.merge_call(call, depth));
        if let Some(value) = special {
            return value;
        }
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

    /// Calls `callable` with `args`. With `isolate`, the function cannot see the bindings of the function evaluating it.
    fn invoke(&self, callable: Callable<'p, 'a>, args: &[ConfigValue], isolate: bool, depth: u32) -> ConfigValue {
        if self.calls.get() >= MAX_CALLS {
            return ConfigValue::Unknown;
        }
        self.calls.set(self.calls.get() + 1);
        let params = match callable {
            Callable::Function(f) => &f.params,
            Callable::Arrow(f) => &f.params,
        };
        let saved = if isolate {
            std::mem::take(&mut *self.frames.borrow_mut())
        } else {
            Vec::new()
        };
        // Parameters are bound in order inside the function, so a default sees the parameters before it.
        self.frames.borrow_mut().push(Frame::new());
        for (index, param) in params.items.iter().enumerate() {
            let value = match (args.get(index), &param.initializer) {
                (Some(value), _) => value.clone(),
                (None, Some(default)) => self.expr(default, depth),
                (None, None) => ConfigValue::Unknown,
            };
            self.bind_pattern(&param.pattern, value, depth);
        }
        if let Some(param) = &params.rest {
            self.bind_pattern(&param.rest.argument, ConfigValue::Unknown, depth);
        }
        let value = match callable {
            Callable::Arrow(f) => match f.get_expression() {
                Some(body) => self.expr(body, depth),
                None => f
                    .get_function_body()
                    .map_or(ConfigValue::Unknown, |b| self.returned(&b.statements, depth)),
            },
            Callable::Function(f) => f
                .body
                .as_ref()
                .map_or(ConfigValue::Unknown, |b| self.returned(&b.statements, depth)),
        };
        self.frames.borrow_mut().pop();
        if isolate {
            *self.frames.borrow_mut() = saved;
        }
        value
    }

    /// Binds the names `pattern` declares to the parts of `value` they take, in the innermost frame. A property of an
    /// object pattern takes its key's value or, when the object has no such key, its default; a name that cannot be
    /// known (an array pattern, a rest element, a key an unknown spread may provide) is bound to
    /// [`ConfigValue::Unknown`], so it does not resolve to a binding of an outer scope.
    fn bind_pattern(&self, pattern: &'p BindingPattern<'a>, value: ConfigValue, depth: u32) {
        match pattern {
            BindingPattern::BindingIdentifier(id) => self.bind(id.name.as_str(), value),
            BindingPattern::AssignmentPattern(p) => self.bind_pattern(&p.left, value, depth),
            BindingPattern::ObjectPattern(object) => {
                for property in &object.properties {
                    let key = property.key.static_name().map(|key| crate::js_string(key.as_js_str()));
                    let field = match (&value, key) {
                        (ConfigValue::Object(entries), Some(key)) => entries.iter().rev().find_map(|(name, field)| {
                            // An unknown spread after the key may replace it.
                            (name == &key)
                                .then(|| field.clone())
                                .or_else(|| (name == "...").then_some(ConfigValue::Unknown))
                        }),
                        _ => Some(ConfigValue::Unknown),
                    };
                    match (&property.value, field) {
                        (BindingPattern::AssignmentPattern(p), None) => {
                            let default = self.expr(&p.right, depth);
                            self.bind_pattern(&p.left, default, depth);
                        }
                        (element, field) => self.bind_pattern(element, field.unwrap_or(ConfigValue::Unknown), depth),
                    }
                }
                if let Some(rest) = &object.rest {
                    self.bind_pattern(&rest.argument, ConfigValue::Unknown, depth);
                }
            }
            BindingPattern::ArrayPattern(_) => {
                for id in pattern.get_binding_identifiers() {
                    self.bind(id.name.as_str(), ConfigValue::Unknown);
                }
            }
        }
    }

    /// Binds `name` to `value` in the innermost frame.
    fn bind(&self, name: &str, value: ConfigValue) {
        self.bind_to(name, Binding::Value(value));
    }

    /// Binds `name` in the innermost frame, shadowing any binding of it in an outer one.
    fn bind_to(&self, name: &str, binding: Binding<'p, 'a>) {
        if let Some(frame) = self.frames.borrow_mut().last_mut() {
            frame.insert(name.to_owned(), binding);
        }
    }

    /// Value returned by a function body: its single expression or the value of the `return` it reaches.
    /// Declarations before it are bound in the innermost frame.
    fn returned(&self, statements: &'p [Statement<'a>], depth: u32) -> ConfigValue {
        if let [Statement::ExpressionStatement(s)] = statements {
            return self.expr(&s.expression, depth);
        }
        self.run(statements, depth).unwrap_or(ConfigValue::Unknown)
    }

    /// Runs `statements` in the innermost frame: the value of the `return` they reach, or `None` when they finish
    /// without one. An `if` with a known test runs the branch it takes; with an unknown test, a branch that is a single
    /// `return x` makes the value what `x` and the other path's value have in common, and any other `return` that may
    /// or may not be reached (in a loop, a `switch`, a `try`, a larger branch) makes the value unknown, never only the
    /// last `return`. A statement that may or may not run (a branch of an unknown test, a loop) leaves every local it
    /// assigns or changes unknown, so the value after it is never the one from before it.
    fn run(&self, statements: &'p [Statement<'a>], depth: u32) -> Option<ConfigValue> {
        for (index, statement) in statements.iter().enumerate() {
            match statement {
                Statement::ReturnStatement(r) => {
                    // What the evaluator reads expressions for, it does not run for what they change.
                    if let Some(argument) = &r.argument {
                        self.havoc(Writes::of_expression(argument));
                    }
                    return Some(
                        r.argument
                            .as_ref()
                            .map_or(ConfigValue::Unknown, |e| self.expr(e, depth)),
                    );
                }
                Statement::ExpressionStatement(s) => self.mutation(&s.expression, depth),
                Statement::BlockStatement(b) => {
                    if let Some(value) = self.block(&b.body, depth) {
                        return Some(value);
                    }
                }
                Statement::IfStatement(i) => {
                    self.havoc(Writes::of_expression(&i.test));
                    let taken = match truthiness(&self.expr(&i.test, depth)) {
                        Some(true) => Some(&i.consequent),
                        Some(false) => i.alternate.as_ref(),
                        None => {
                            if !returns(statement) {
                                self.either_branch(i, depth);
                                continue;
                            }
                            let rest = &statements[index + 1..];
                            return Some(self.either_return(i, rest, depth));
                        }
                    };
                    if let Some(value) = taken.and_then(|branch| self.block(std::slice::from_ref(branch), depth)) {
                        return Some(value);
                    }
                }
                other if returns(other) => return Some(ConfigValue::Unknown),
                other => match other.as_declaration() {
                    Some(Declaration::VariableDeclaration(v)) => {
                        self.havoc(Writes::of_statement(other));
                        self.declare(v, depth);
                    }
                    Some(Declaration::FunctionDeclaration(f)) => {
                        if let Some(id) = &f.id {
                            self.bind_to(id.name.as_str(), Binding::Callable(Callable::Function(f)));
                        }
                    }
                    Some(_) => {}
                    None => self.havoc(Writes::of_statement(other)),
                },
            }
        }
        None
    }

    /// Runs the statements of a block in a frame of their own, so a `const` or `let` declared in it does not replace a
    /// binding outside it. A `var` it declares is function-scoped, and is left unknown outside the block.
    fn block(&self, statements: &'p [Statement<'a>], depth: u32) -> Option<ConfigValue> {
        self.frames.borrow_mut().push(Frame::new());
        let value = self.run(statements, depth);
        self.frames.borrow_mut().pop();
        let mut vars = Vars::default();
        for statement in statements {
            vars.visit_statement(statement);
        }
        for name in vars.0 {
            self.assign(&name, Binding::Value(ConfigValue::Unknown));
        }
        value
    }

    /// Runs an `if` with an unknown test whose branches do not return: each branch from the same bindings, after which
    /// a binding is what both branches leave it with in common.
    fn either_branch(&self, statement: &'p IfStatement<'a>, depth: u32) {
        let before = self.frames.borrow().clone();
        let captured = self.captured.borrow().clone();
        self.block(std::slice::from_ref(&statement.consequent), depth);
        let consequent = std::mem::replace(&mut *self.frames.borrow_mut(), before);
        let consequent_captured = std::mem::replace(&mut *self.captured.borrow_mut(), captured);
        if let Some(alternate) = &statement.alternate {
            self.block(std::slice::from_ref(alternate), depth);
        }
        let mut frames = self.frames.borrow_mut();
        for (frame, other) in frames.iter_mut().zip(consequent) {
            let names: Vec<String> = frame.keys().chain(other.keys()).cloned().collect();
            for name in names {
                let joined = match (frame.get(&name), other.get(&name)) {
                    (Some(Binding::Value(a)), Some(Binding::Value(b))) => Binding::Value(join(a, b)),
                    (Some(Binding::Callable(a)), Some(Binding::Callable(b))) if a.same(*b) => Binding::Callable(*a),
                    _ => Binding::Value(ConfigValue::Unknown),
                };
                frame.insert(name, joined);
            }
        }
        let mut captured = self.captured.borrow_mut();
        if *captured != consequent_captured {
            *captured = Some(match (&*captured, &consequent_captured) {
                (Some(a), Some(b)) => either(a, b),
                _ => ConfigValue::Unknown,
            });
        }
    }

    /// The value of an `if` with an unknown test followed by `rest`, where a branch returns: what a branch's single
    /// `return x` and the other path's value have in common, or unknown when a branch does more. A branch that goes on
    /// to `rest` leaves what it assigns unknown there.
    fn either_return(&self, statement: &'p IfStatement<'a>, rest: &'p [Statement<'a>], depth: u32) -> ConfigValue {
        let path = |branch: Option<&'p Statement<'a>>| -> Option<ConfigValue> {
            match branch.map(single_return) {
                // A branch that returns its value.
                Some(Some(r)) => Some(
                    r.argument
                        .as_ref()
                        .map_or(ConfigValue::Unknown, |e| self.expr(e, depth)),
                ),
                // A branch that does more than return, and may return.
                Some(None) if branch.is_some_and(returns) => Some(ConfigValue::Unknown),
                // No branch, or one that does not return: control goes on to the statements after the `if`.
                _ => None,
            }
        };
        // Both returns are evaluated before `rest` runs, which may change the locals they read.
        let consequent = path(Some(&statement.consequent));
        let alternate = path(statement.alternate.as_ref());
        let after = || {
            for branch in [Some(&statement.consequent), statement.alternate.as_ref()]
                .into_iter()
                .flatten()
            {
                if !returns(branch) {
                    self.havoc(Writes::of_statement(branch));
                }
            }
            self.run(rest, depth).unwrap_or(ConfigValue::Unknown)
        };
        let (a, b) = match (consequent, alternate) {
            (Some(a), Some(b)) => (a, b),
            (Some(a), None) => (a, after()),
            (None, Some(b)) => (after(), b),
            (None, None) => return ConfigValue::Unknown,
        };
        either(&a, &b)
    }

    /// Binds the names a `const`/`let`/`var` declaration declares in the innermost frame: a destructured name to the
    /// part of the value it takes, or to [`ConfigValue::Unknown`].
    fn declare(&self, declaration: &'p VariableDeclaration<'a>, depth: u32) {
        for declarator in &declaration.declarations {
            if let BindingPattern::BindingIdentifier(id) = &declarator.id
                && let Some(callable) = declarator.init.as_ref().and_then(Callable::of_expression)
            {
                self.bind_to(id.name.as_str(), Binding::Callable(callable));
                continue;
            }
            let value = declarator
                .init
                .as_ref()
                .map_or(ConfigValue::Unknown, |init| self.expr(init, depth));
            let value = if node_count(&value) > MAX_CONST_NODES {
                ConfigValue::Unknown
            } else {
                value
            };
            self.bind_pattern(&declarator.id, value, depth);
        }
    }

    /// Leaves every local `writes` assigns unknown, and every local object or array it changes partly unknown.
    fn havoc(&self, writes: Writes) {
        for name in writes.assigned {
            self.assign(&name, Binding::Value(ConfigValue::Unknown));
        }
        let mut changed = writes.changed;
        changed.sort_unstable();
        changed.dedup();
        for name in changed {
            self.taint(&name);
        }
    }

    /// Applies what an expression statement does to a local object: `name = x`, `name.key = x`, and anything else
    /// that reaches into the object (`name.list.push(x)`, `name.a.b = x`), which leaves it partly unknown. Any other
    /// expression leaves what it may write unknown.
    fn mutation(&self, expr: &'p Expression<'a>, depth: u32) {
        match expr.get_inner_expression() {
            Expression::AwaitExpression(a) => self.mutation(&a.argument, depth),
            Expression::AssignmentExpression(a) => {
                self.havoc(Writes::of_expression(&a.right));
                let assign = a.operator == AssignmentOperator::Assign;
                match &a.left {
                    AssignmentTarget::AssignmentTargetIdentifier(id) => {
                        let value = if assign {
                            self.expr(&a.right, depth)
                        } else {
                            ConfigValue::Unknown
                        };
                        self.assign(id.name.as_str(), Binding::Value(value));
                    }
                    AssignmentTarget::StaticMemberExpression(m) => {
                        if let Expression::Identifier(object) = m.object.get_inner_expression()
                            && assign
                        {
                            let value = self.expr(&a.right, depth);
                            self.set_key(object.name.as_str(), m.property.name.as_str(), value);
                        } else if let Some(root) = member_root(&m.object) {
                            self.taint(root);
                        }
                    }
                    _ => self.havoc(Writes::of_expression(expr)),
                }
            }
            Expression::CallExpression(c) => {
                if !self.capture_set(c, depth) {
                    self.havoc(Writes::of_effect(expr));
                }
            }
            // `cond && (x = 1)`, `x++`, `a = b = 1`: what it writes may or may not be written.
            _ => self.havoc(Writes::of_expression(expr)),
        }
    }

    /// Sets `key` of the local object `name`.
    fn set_key(&self, name: &str, key: &str, value: ConfigValue) {
        let mut frames = self.frames.borrow_mut();
        let binding = frames.iter_mut().rev().find_map(|frame| frame.get_mut(name));
        match binding {
            Some(Binding::Value(ConfigValue::Object(entries))) => {
                match entries.iter_mut().find(|(existing, _)| existing == key) {
                    Some((_, slot)) => *slot = value,
                    None => entries.push((key.to_owned(), value)),
                }
            }
            // `list.length = 0` on an array.
            Some(Binding::Value(ConfigValue::Array(items))) => items.push(ConfigValue::Unknown),
            _ => {}
        }
    }

    /// Records that the local `name` was changed in a way the evaluator does not follow.
    fn taint(&self, name: &str) {
        let mut frames = self.frames.borrow_mut();
        let binding = frames.iter_mut().rev().find_map(|frame| frame.get_mut(name));
        match binding {
            Some(Binding::Value(ConfigValue::Object(entries))) => {
                entries.push(("...".to_owned(), ConfigValue::Unknown));
            }
            Some(Binding::Value(ConfigValue::Array(items))) => items.push(ConfigValue::Unknown),
            _ => {}
        }
    }

    fn array(&self, elements: &'p [ArrayExpressionElement<'a>], depth: u32) -> ConfigValue {
        let mut out = Vec::new();
        for element in elements {
            match element {
                ArrayExpressionElement::SpreadElement(s) => match self.expr(&s.argument, depth) {
                    ConfigValue::Array(items) => out.extend(items),
                    _ => out.push(ConfigValue::Unknown),
                },
                other => out.push(
                    other
                        .as_expression()
                        .map_or(ConfigValue::Unknown, |e| self.expr(e, depth)),
                ),
            }
        }
        ConfigValue::Array(out)
    }

    fn object(&self, properties: &'p [ObjectPropertyKind<'a>], depth: u32) -> ConfigValue {
        let mut out: Vec<(String, ConfigValue)> = Vec::new();
        for property in properties {
            match property {
                ObjectPropertyKind::SpreadProperty(s) => self.spread(&s.argument, &mut out, depth),
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
                    out.push((crate::js_string(key.as_js_str()), value));
                }
            }
        }
        ConfigValue::Object(out)
    }
}

/// How a config combinator merges its arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MergeStyle {
    /// Nested objects merge and arrays concatenate; later arguments win (`mergeConfig`, `deepmerge`, `merge`).
    Deep,
    /// Top-level keys of later arguments replace earlier ones (`Object.assign`).
    Shallow,
    /// Like [`MergeStyle::Deep`], but earlier arguments win (`defu`).
    FirstWins,
}

/// The merge style of a call to a well-known config combinator, by callee. `merge` and `mergeWith` count only when
/// the file does not import them from another package, and as members only of `lodash` or a merge package.
fn merge_style(callee: &Expression<'_>, bindings: &ModuleBindings) -> Option<MergeStyle> {
    match callee.get_inner_expression() {
        Expression::Identifier(id) => match id.name.as_str() {
            name if bindings.foreign_merges.contains(name) => None,
            "mergeConfig" | "merge" | "deepmerge" | "deepMerge" | "mergeDeep" | "mergeWith" => Some(MergeStyle::Deep),
            "defu" => Some(MergeStyle::FirstWins),
            _ => None,
        },
        Expression::StaticMemberExpression(member) => {
            let object_is = |name: &str| matches!(member.object.get_inner_expression(), Expression::Identifier(o) if o.name.as_str() == name);
            let merge_object = matches!(
                member.object.get_inner_expression(),
                Expression::Identifier(o)
                    if matches!(o.name.as_str(), "_" | "lodash") || bindings.merge_names.contains(o.name.as_str())
            );
            match member.property.name.as_str() {
                "assign" if object_is("Object") => Some(MergeStyle::Shallow),
                "merge" | "mergeWith" if merge_object => Some(MergeStyle::Deep),
                "mergeConfig" | "deepmerge" | "mergeDeep" => Some(MergeStyle::Deep),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Merges `incoming` into `base`; with `deep`, an object meeting an object merges and an array meeting an array
/// concatenates, otherwise the incoming value replaces the earlier one in place.
fn merge_objects(base: &mut Vec<(String, ConfigValue)>, incoming: Vec<(String, ConfigValue)>, deep: bool) {
    for (key, value) in incoming {
        let existing = if key == "..." {
            base.iter().position(|(k, _)| k == "...")
        } else {
            base.iter().position(|(k, _)| *k == key)
        };
        let Some(index) = existing else {
            base.push((key, value));
            continue;
        };
        if key == "..." {
            continue;
        }
        let slot = &mut base[index].1;
        match (deep, std::mem::replace(slot, ConfigValue::Unknown), value) {
            (true, ConfigValue::Object(mut left), ConfigValue::Object(right)) => {
                merge_objects(&mut left, right, true);
                *slot = ConfigValue::Object(left);
            }
            (true, ConfigValue::Array(mut left), ConfigValue::Array(right)) => {
                left.extend(right);
                *slot = ConfigValue::Array(left);
            }
            // A deep merge keeps what the known side has and whatever the unknown side may add to it.
            (true, ConfigValue::Object(mut known), ConfigValue::Unknown)
            | (true, ConfigValue::Unknown, ConfigValue::Object(mut known)) => {
                if !known.iter().any(|(k, _)| k == "...") {
                    known.push(("...".to_owned(), ConfigValue::Unknown));
                }
                *slot = ConfigValue::Object(known);
            }
            (true, ConfigValue::Array(mut known), ConfigValue::Unknown)
            | (true, ConfigValue::Unknown, ConfigValue::Array(mut known)) => {
                known.push(ConfigValue::Unknown);
                *slot = ConfigValue::Array(known);
            }
            (_, _, value) => *slot = value,
        }
    }
}

/// A statement that is only a `return` (or a block holding just one).
fn single_return<'p, 'a>(statement: &'p Statement<'a>) -> Option<&'p ReturnStatement<'a>> {
    match statement {
        Statement::ReturnStatement(r) => Some(r),
        Statement::BlockStatement(b) => match b.body.as_slice() {
            [only] => single_return(only),
            _ => None,
        },
        _ => None,
    }
}

/// Whether a statement may `return` from the function it is in (a `return` inside a nested function does not count).
fn returns(statement: &Statement<'_>) -> bool {
    #[derive(Default)]
    struct Finder(bool);
    impl<'a> Visit<'a> for Finder {
        fn visit_return_statement(&mut self, _: &ReturnStatement<'a>) {
            self.0 = true;
        }
        fn visit_function(&mut self, _: &Function<'a>, _: oxc_semantic::ScopeFlags) {}
        fn visit_arrow_function_expression(&mut self, _: &ArrowFunctionExpression<'a>) {}
        fn visit_class(&mut self, _: &oxc_ast::ast::Class<'a>) {}
    }
    let mut finder = Finder::default();
    finder.visit_statement(statement);
    finder.0
}

/// The locals a statement or expression may write, outside the functions it declares: the names it assigns
/// (`x = …`, `x++`, `[x] = …`) and the objects it may change (`x.a = …`, `x.list.push(…)`, `Object.assign(x, …)`).
#[derive(Default)]
struct Writes {
    assigned: Vec<String>,
    changed: Vec<String>,
}

impl Writes {
    fn of_statement(statement: &Statement<'_>) -> Self {
        let mut writes = Self::default();
        writes.visit_statement(statement);
        writes
    }

    fn of_expression(expression: &Expression<'_>) -> Self {
        let mut writes = Self::default();
        writes.visit_expression(expression);
        writes
    }

    /// What an expression statement writes itself, as [`Evaluator::mutation`] reads it: the target of an assignment
    /// and the object a method call (or `Object.assign`) may change; anything else, in full.
    fn of_effect(expression: &Expression<'_>) -> Self {
        let mut writes = Self::default();
        match expression.get_inner_expression() {
            Expression::AssignmentExpression(a) => match a.left.as_simple_assignment_target() {
                Some(target) => writes.visit_simple_assignment_target(target),
                None => writes.visit_assignment_target(&a.left),
            },
            Expression::CallExpression(c) => {
                writes.visit_call_expression(c);
                writes.arguments(c);
            }
            Expression::AwaitExpression(a) => return Self::of_effect(&a.argument),
            other => writes.visit_expression(other),
        }
        writes
    }

    /// The objects a call made for what it does is given, which it may change.
    fn arguments(&mut self, call: &CallExpression<'_>) {
        let arguments = call.arguments.iter().filter_map(Argument::as_expression);
        self.changed
            .extend(arguments.filter_map(member_root).map(str::to_owned));
    }
}

impl<'a> Visit<'a> for Writes {
    fn visit_simple_assignment_target(&mut self, it: &SimpleAssignmentTarget<'a>) {
        match it {
            SimpleAssignmentTarget::AssignmentTargetIdentifier(id) => self.assigned.push(id.name.as_str().to_owned()),
            other => {
                if let Some(root) = other.as_member_expression().and_then(|m| member_root(m.object())) {
                    self.changed.push(root.to_owned());
                }
            }
        }
        walk::walk_simple_assignment_target(self, it);
    }

    fn visit_assignment_target_property_identifier(&mut self, it: &AssignmentTargetPropertyIdentifier<'a>) {
        self.assigned.push(it.binding.name.as_str().to_owned());
        walk::walk_assignment_target_property_identifier(self, it);
    }

    fn visit_unary_expression(&mut self, it: &UnaryExpression<'a>) {
        if it.operator == UnaryOperator::Delete
            && let Some(root) = it
                .argument
                .get_inner_expression()
                .as_member_expression()
                .and_then(|m| member_root(m.object()))
        {
            self.changed.push(root.to_owned());
        }
        walk::walk_unary_expression(self, it);
    }

    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        let target = changed_by_call(it);
        if let Some(root) = target.and_then(member_root) {
            self.changed.push(root.to_owned());
        }
        walk::walk_call_expression(self, it);
    }

    fn visit_expression_statement(&mut self, it: &ExpressionStatement<'a>) {
        if let Some(call) = statement_call(&it.expression) {
            self.arguments(call);
        }
        walk::walk_expression_statement(self, it);
    }

    fn visit_function(&mut self, _: &Function<'a>, _: oxc_semantic::ScopeFlags) {}
    fn visit_arrow_function_expression(&mut self, _: &ArrowFunctionExpression<'a>) {}
    fn visit_class(&mut self, _: &oxc_ast::ast::Class<'a>) {}
}

/// The names `var` declarations bind in a statement, outside the functions it declares.
#[derive(Default)]
struct Vars(Vec<String>);

impl<'a> Visit<'a> for Vars {
    fn visit_variable_declaration(&mut self, it: &VariableDeclaration<'a>) {
        if it.kind == VariableDeclarationKind::Var {
            for declarator in &it.declarations {
                self.0.extend(
                    declarator
                        .id
                        .get_binding_identifiers()
                        .iter()
                        .map(|id| id.name.as_str().to_owned()),
                );
            }
        }
        walk::walk_variable_declaration(self, it);
    }

    fn visit_function(&mut self, _: &Function<'a>, _: oxc_semantic::ScopeFlags) {}
    fn visit_arrow_function_expression(&mut self, _: &ArrowFunctionExpression<'a>) {}
    fn visit_class(&mut self, _: &oxc_ast::ast::Class<'a>) {}
}

/// A value that code the evaluator does not follow may have changed: an object may have any other key and an array
/// any other element, as [`Evaluator::taint`] leaves them; anything else is unknown.
fn changed(value: ConfigValue) -> ConfigValue {
    match value {
        ConfigValue::Object(mut entries) => {
            entries.push(("...".to_owned(), ConfigValue::Unknown));
            ConfigValue::Object(entries)
        }
        ConfigValue::Array(mut items) => {
            items.push(ConfigValue::Unknown);
            ConfigValue::Array(items)
        }
        _ => ConfigValue::Unknown,
    }
}

/// A value code the evaluator does not follow may have changed as `change` says.
fn unstable(value: ConfigValue, change: Change) -> ConfigValue {
    match (change, value) {
        (
            Change::Contents,
            value @ (ConfigValue::Null
            | ConfigValue::Bool(_)
            | ConfigValue::Number(_)
            | ConfigValue::String(_)
            | ConfigValue::Regex(_)),
        ) => value,
        (_, value) => changed(value),
    }
}

/// Whether the left side of `left <operator> right` is the value of the expression; `None` when that is not known.
fn left_is_value(operator: LogicalOperator, left: &ConfigValue) -> Option<bool> {
    match operator {
        LogicalOperator::And => truthiness(left).map(|truthy| !truthy),
        LogicalOperator::Or => truthiness(left),
        LogicalOperator::Coalesce => match left {
            ConfigValue::Unknown => None,
            ConfigValue::Null => Some(false),
            _ => Some(true),
        },
    }
}

/// Whether `expr` is a comparison, a negation or a `void`, whose value is `true`, `false` or `undefined` and so adds
/// nothing when spread.
fn is_primitive(expr: &Expression<'_>) -> bool {
    match expr.get_inner_expression() {
        Expression::BinaryExpression(binary) => {
            binary.operator.is_equality() || binary.operator.is_compare() || binary.operator.is_relational()
        }
        Expression::UnaryExpression(unary) => matches!(unary.operator, UnaryOperator::LogicalNot | UnaryOperator::Void),
        _ => false,
    }
}

/// Whether `expr` is a `void`, whose value is `undefined`.
fn is_void(expr: &Expression<'_>) -> bool {
    matches!(expr.get_inner_expression(), Expression::UnaryExpression(unary) if unary.operator == UnaryOperator::Void)
}

/// The entries spreading `value` adds to an object: those of an object, none for `null`, a boolean, a number or an
/// empty string, and `None` for anything else, whose entries are not known.
fn spread_entries(value: ConfigValue) -> Option<Vec<(String, ConfigValue)>> {
    match value {
        ConfigValue::Object(entries) => Some(entries),
        ConfigValue::Null | ConfigValue::Bool(_) | ConfigValue::Number(_) => Some(Vec::new()),
        ConfigValue::String(s) => s.is_empty().then(Vec::new),
        ConfigValue::Array(_) | ConfigValue::Regex(_) | ConfigValue::Unknown => None,
    }
}

/// The value of `key` in the entries of an object: the last entry for it, unknown when an unknown spread follows it,
/// and `None` when the object has neither.
fn entry_value(entries: &[(String, ConfigValue)], key: &str) -> Option<ConfigValue> {
    entries
        .iter()
        .rev()
        .find(|(name, _)| name == key || name == "...")
        .map(|(name, value)| {
            if name == "..." {
                ConfigValue::Unknown
            } else {
                value.clone()
            }
        })
}

/// The entries a spread of one of `branches` adds after `before`; see [`any_entries`].
fn spread_any(before: &[(String, ConfigValue)], branches: &[Vec<(String, ConfigValue)>]) -> Vec<(String, ConfigValue)> {
    let objects: Vec<Option<&[(String, ConfigValue)]>> =
        branches.iter().map(|branch| Some(branch.as_slice())).collect();
    any_entries(&objects, before)
}

/// The entries of an object that is one of `objects`, `None` standing for a branch without the object: each key any
/// of them has, with [`any_of`] what each leaves it (its own value, or the one in `before` when it does not have the
/// key), and the marker key `...` last when one of them has an unknown spread.
fn any_entries(
    objects: &[Option<&[(String, ConfigValue)]>],
    before: &[(String, ConfigValue)],
) -> Vec<(String, ConfigValue)> {
    let entries = || objects.iter().flatten().copied().flatten();
    let mut keys: Vec<&str> = Vec::new();
    for (key, _) in entries() {
        if key != "..." && !keys.contains(&key.as_str()) {
            keys.push(key);
        }
    }
    let mut out: Vec<(String, ConfigValue)> = keys
        .into_iter()
        .map(|key| {
            let values: Vec<Option<ConfigValue>> = objects
                .iter()
                .map(|object| {
                    object
                        .and_then(|object| entry_value(object, key))
                        .or_else(|| entry_value(before, key))
                })
                .collect();
            (key.to_owned(), any_of(&values))
        })
        .collect();
    if entries().any(|(key, _)| key == "...") {
        out.push(("...".to_owned(), ConfigValue::Unknown));
    }
    out
}

/// The value of a key that holds one of `values`, `None` standing for a branch that leaves it out: the value itself
/// when every branch sets it alike; for lists, the elements of all of them, a string counting as a list of one, so
/// that every file or package a branch names is credited; for objects, each key any of them has, merged the same way;
/// anything else is unknown, such as two different strings or a string only some branches set.
fn any_of(values: &[Option<ConfigValue>]) -> ConfigValue {
    let present: Vec<&ConfigValue> = values.iter().flatten().collect();
    let Some(&first) = present.first() else {
        return ConfigValue::Unknown;
    };
    if present.len() == values.len() && present.iter().all(|value| *value == first) {
        return first.clone();
    }
    let lists: Option<Vec<&[ConfigValue]>> = present
        .iter()
        .map(|&value| match value {
            ConfigValue::Array(list) => Some(list.as_slice()),
            ConfigValue::String(_) => Some(std::slice::from_ref(value)),
            _ => None,
        })
        .collect();
    if let Some(lists) = lists
        && present.iter().any(|value| matches!(value, ConfigValue::Array(_)))
    {
        let mut items: Vec<ConfigValue> = Vec::new();
        for item in lists.into_iter().flatten() {
            if !items.contains(item) {
                items.push(item.clone());
            }
        }
        return ConfigValue::Array(items);
    }
    let objects: Option<Vec<Option<Entries<'_>>>> = values
        .iter()
        .map(|value| match value {
            None => Some(None),
            Some(ConfigValue::Object(entries)) => Some(Some(entries.as_slice())),
            Some(_) => None,
        })
        .collect();
    objects.map_or(ConfigValue::Unknown, |objects| {
        ConfigValue::Object(any_entries(&objects, &[]))
    })
}

/// Whether a value is truthy in JavaScript; `None` when it is unknown.
fn truthiness(value: &ConfigValue) -> Option<bool> {
    match value {
        ConfigValue::Null => Some(false),
        ConfigValue::Bool(b) => Some(*b),
        ConfigValue::Number(n) => Some(*n != 0.0 && !n.is_nan()),
        ConfigValue::String(s) => Some(!s.is_empty()),
        ConfigValue::Array(_) | ConfigValue::Object(_) | ConfigValue::Regex(_) => Some(true),
        ConfigValue::Unknown => None,
    }
}

/// The value of an expression that is `a` or `b`, not knowing which: the value itself when both are equal; for two
/// objects, an object with every key either has, whose value is what both have in common when both have the key and
/// [`ConfigValue::Unknown`] when only one does (it may be present or not); anything else is unknown. No key appears
/// that neither branch has, so other keys stay known, and a key that may differ is unknown wherever it is read.
fn either(a: &ConfigValue, b: &ConfigValue) -> ConfigValue {
    if a == b {
        return a.clone();
    }
    let (ConfigValue::Object(left), ConfigValue::Object(right)) = (a, b) else {
        return ConfigValue::Unknown;
    };
    let mut keys: Vec<&str> = Vec::new();
    for (key, _) in left.iter().chain(right) {
        if key != "..." && !keys.contains(&key.as_str()) {
            keys.push(key);
        }
    }
    let mut out: Vec<(String, ConfigValue)> = keys
        .into_iter()
        .map(|key| {
            let value = match (a.get(key), b.get(key)) {
                (Some(x), Some(y)) => either(x, y),
                _ => ConfigValue::Unknown,
            };
            (key.to_owned(), value)
        })
        .collect();
    if a.get("...").is_some() || b.get("...").is_some() {
        out.push(("...".to_owned(), ConfigValue::Unknown));
    }
    ConfigValue::Object(out)
}

/// What a binding holds after one of two branches left it `a` and the other `b`: as [`either`], except that a list
/// one branch appended to (`plugins.push(x)`) keeps the elements both have.
fn join(a: &ConfigValue, b: &ConfigValue) -> ConfigValue {
    match (a, b) {
        (ConfigValue::Array(x), ConfigValue::Array(y)) if x != y && (x.starts_with(y) || y.starts_with(x)) => {
            let mut common = if x.len() < y.len() { x.clone() } else { y.clone() };
            common.push(ConfigValue::Unknown);
            ConfigValue::Array(common)
        }
        _ => either(a, b),
    }
}

/// Appends a flat config, or the configs of a (nested) list of them, to `list`.
fn flatten_into(value: ConfigValue, list: &mut Vec<ConfigValue>) {
    match value {
        ConfigValue::Array(items) => {
            for item in items {
                flatten_into(item, list);
            }
        }
        other => list.push(other),
    }
}

/// The only argument of a call, when it is a plain expression.
fn single_argument<'p, 'a>(call: &'p CallExpression<'a>) -> Option<&'p Expression<'a>> {
    let mut arguments = call.arguments.iter();
    match (arguments.next(), arguments.next()) {
        (Some(only), None) => only.as_expression(),
        _ => None,
    }
}

/// `require('pkg')` and the factory call `require('pkg')(options)` nested in a config value (a `PostCSS` plugin list),
/// as the package name. A top-level `require` stays unknown: it is a whole config, not a name. Relative paths stay
/// unknown too, as they load a module rather than name a package.
fn required_package(call: &CallExpression<'_>, depth: u32) -> Option<ConfigValue> {
    if depth < 2 {
        return None;
    }
    let call = match call.callee.get_inner_expression() {
        Expression::CallExpression(factory) => &**factory,
        _ => call,
    };
    let Expression::Identifier(callee) = call.callee.get_inner_expression() else {
        return None;
    };
    if callee.name.as_str() != "require" {
        return None;
    }
    let module = single_argument(call).and_then(static_string)?;
    (!module.starts_with(['.', '/'])).then_some(ConfigValue::String(module))
}

fn is_require_resolve(call: &CallExpression<'_>) -> bool {
    matches!(
        call.callee.get_inner_expression(),
        Expression::StaticMemberExpression(m)
            if m.property.name.as_str() == "resolve"
                && matches!(m.object.get_inner_expression(), Expression::Identifier(o) if o.name.as_str() == "require")
    )
}

/// The identifier at the start of a member chain (`config` in `config.a.b`).
fn member_root<'a>(expr: &'a Expression<'_>) -> Option<&'a str> {
    match expr.get_inner_expression() {
        Expression::Identifier(id) => Some(id.name.as_str()),
        Expression::StaticMemberExpression(m) => member_root(&m.object),
        Expression::ComputedMemberExpression(m) => member_root(&m.object),
        _ => None,
    }
}
