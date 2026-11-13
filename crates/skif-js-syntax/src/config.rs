//! Static evaluation of the value a tool configuration file exports. Nothing is executed.

use skif_core::{HashMap, HashSet};
use std::cell::{Cell, RefCell};
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;
use std::sync::LazyLock;

use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Argument, ArrayExpressionElement, ArrowFunctionExpression, AssignmentOperator, AssignmentTarget,
    AssignmentTargetPropertyIdentifier, BinaryExpression, BinaryOperator, BindingIdentifier, BindingPattern,
    BlockStatement, CallExpression, CatchClause, ChainElement, ChainExpression, ComputedMemberExpression,
    ConditionalExpression, Declaration, ExportDefaultDeclarationKind, Expression, ExpressionStatement, ForInStatement,
    ForOfStatement, ForStatement, Function, IdentifierReference, IfStatement, ImportDeclarationSpecifier,
    ImportOrExportKind, LogicalExpression, LogicalOperator, MemberExpression, NewExpression, ObjectProperty,
    ObjectPropertyKind, Program, ReturnStatement, SimpleAssignmentTarget, Statement, StaticMemberExpression,
    TaggedTemplateExpression, TemplateLiteral, UnaryExpression, UnaryOperator, VariableDeclaration,
    VariableDeclarationKind, VariableDeclarator,
};
use oxc_ast_visit::{Visit, walk};
use oxc_parser::{ParseOptions as OxcParseOptions, Parser};
use oxc_span::{SourceType, Span};
use serde::{Deserialize, Serialize};

use crate::finder::static_string;
use crate::sharing::{
    Change, Imports, Unstable, Write, alias_names, alias_source, changed_by_call, given, given_parts, property_defined,
    reach, statement_call,
};

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

/// How many module analyses one config may run in all ([`Modules::facts`]). What an analysis finds under a pending
/// module is not kept, so a cycle of imports is analysed again at every entry; past this, a module reads as unknown.
const MAX_MODULE_ANALYSES: usize = 4 * MAX_MODULE_LOADS;

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

/// The members every object inherits from `Object.prototype`.
const OBJECT_PROTOTYPE_MEMBERS: &[&str] = &[
    "__defineGetter__",
    "__defineSetter__",
    "__lookupGetter__",
    "__lookupSetter__",
    "__proto__",
    "constructor",
    "hasOwnProperty",
    "isPrototypeOf",
    "propertyIsEnumerable",
    "toLocaleString",
    "toString",
    "valueOf",
];

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
    /// A string literal, or a string computed from known values (a template literal, a concatenation, a path).
    String(String),
    /// An array literal; spreads of static arrays are flattened, anything else becomes [`ConfigValue::Unknown`].
    Array(Vec<ConfigValue>),
    /// An object literal in source order. A spread of an unknown value is recorded as the key [`UNKNOWN_SPREAD`], or
    /// [`OWN_SPREAD`] when the module computes the value itself.
    Object(Vec<(String, ConfigValue)>),
    /// A regular expression literal.
    Regex {
        /// The pattern, without the slashes.
        pattern: String,
        /// The flags after the closing slash, in alphabetical order (`gi`).
        flags: String,
    },
    /// Anything that cannot be known without running the code.
    Unknown,
    /// One of these values, without knowing which: the branches of a condition that cannot be decided statically
    /// (`process.env.CI ? 'github' : 'list'`). There are at least two alternatives, all different, and none is
    /// [`ConfigValue::Unknown`] or another `OneOf`, though an array or object alternative may hold unknown parts.
    /// [`ConfigValue::is_unknown`] and [`ConfigValue::has_unknown`] count it as unknown, so code that does not ask
    /// for its [`ConfigValue::alternatives`] treats it as an unknown value.
    OneOf(Vec<ConfigValue>),
}

/// The key under which an object records an unknown spread, or an unknown argument of a merge, of a value the module
/// takes from elsewhere: an imported base config, a merge of imported ones, a global. It may add or replace any key.
pub const UNKNOWN_SPREAD: &str = "...";

/// Like [`UNKNOWN_SPREAD`], for an unknown value the module computes itself: a spread or merge of a call of, or a
/// reference to, a function or constant it defines, or a binding of a function being evaluated
/// (`build: { ...makeLib() }`). A value imported from another module records these as [`UNKNOWN_SPREAD`] too.
pub const OWN_SPREAD: &str = "...own";

impl ConfigValue {
    /// Whether `key` is a key an object records an unknown spread under, [`UNKNOWN_SPREAD`] or [`OWN_SPREAD`].
    #[must_use]
    pub fn is_spread_key(key: &str) -> bool {
        key == UNKNOWN_SPREAD || key == OWN_SPREAD
    }

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
            Self::Regex { pattern, .. } => Some(pattern),
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

    /// Whether the value is not known as one value: [`ConfigValue::Unknown`], or [`ConfigValue::OneOf`] of several.
    #[must_use]
    pub fn is_unknown(&self) -> bool {
        matches!(self, Self::Unknown | Self::OneOf(_))
    }

    /// The values this may be: the alternatives of a [`ConfigValue::OneOf`], or the value itself.
    #[must_use]
    pub fn alternatives(&self) -> &[ConfigValue] {
        match self {
            Self::OneOf(alternatives) => alternatives,
            other => std::slice::from_ref(other),
        }
    }

    /// Whether the value, or anything inside it, is [`ConfigValue::Unknown`] or [`ConfigValue::OneOf`].
    #[must_use]
    pub fn has_unknown(&self) -> bool {
        match self {
            Self::Unknown | Self::OneOf(_) => true,
            Self::Array(items) => items.iter().any(Self::has_unknown),
            Self::Object(entries) => entries.iter().any(|(_, v)| v.has_unknown()),
            _ => false,
        }
    }
}

/// A value as the evaluator holds it: a [`ConfigValue`] that may also record that a value is not definite. Only
/// the evaluator sees it; what it hands out is converted to a [`ConfigValue`], which has no such record.
#[derive(Debug, Clone, PartialEq)]
enum Value {
    Null,
    /// `undefined`: the value of a key an object does not have. A default applies to it, unlike to `null`; it is
    /// handed out as [`ConfigValue::Null`].
    Undefined,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Value>),
    Object(Vec<(String, Value)>),
    Regex {
        pattern: String,
        flags: String,
    },
    Unknown,
    /// One of several values, not knowing which; see [`one_of`]. Handed out as [`ConfigValue::OneOf`].
    OneOf(Vec<Value>),
    /// A value that is not definite: a key merged from the branches of a condition the evaluator cannot decide
    /// ([`any_entries`]), which the object may lack or hold any one of the values its union lists in, and every value
    /// that carries a read of one on: a key of an object or a binding holding it, an argument, a value a function
    /// returns, a fallback over it (`x.setupFiles ?? []`) and an array spreading it. Only the values the evaluator
    /// hands out drop it.
    Maybe(Box<Value>),
}

impl Value {
    /// The value of `key` when this is an object, as [`ConfigValue::get`] finds it.
    fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Self::Object(entries) => entries.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// The string when this is a string.
    fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s),
            _ => None,
        }
    }

    /// `value`, or a [`Value::Maybe`] of it when it is not `definite`. An unknown value is unknown either way.
    fn doubtful(value: Self, definite: bool) -> Self {
        if definite || matches!(value, Self::Unknown | Self::Maybe(_)) {
            value
        } else {
            Self::Maybe(Box::new(value))
        }
    }

    /// The value without its [`Value::Maybe`], and whether it is definite.
    fn split(self) -> (Self, bool) {
        match self {
            Self::Maybe(value) => (*value, false),
            value => (value, true),
        }
    }

    /// Whether the value is definite, not a [`Value::Maybe`].
    fn is_definite(&self) -> bool {
        !matches!(self, Self::Maybe(_))
    }

    /// The value inside a [`Value::Maybe`], or the value itself.
    fn definite(&self) -> &Self {
        match self {
            Self::Maybe(value) => value,
            value => value,
        }
    }

    /// The value inside a [`Value::Maybe`], or the value itself, to change in place.
    fn definite_mut(&mut self) -> &mut Self {
        match self {
            Self::Maybe(value) => value,
            value => value,
        }
    }
}

impl From<ConfigValue> for Value {
    fn from(value: ConfigValue) -> Self {
        match value {
            ConfigValue::Null => Self::Null,
            ConfigValue::Bool(b) => Self::Bool(b),
            ConfigValue::Number(n) => Self::Number(n),
            ConfigValue::String(s) => Self::String(s),
            ConfigValue::Array(items) => Self::Array(items.into_iter().map(Self::from).collect()),
            ConfigValue::Object(entries) => Self::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, Self::from(value)))
                    .collect(),
            ),
            ConfigValue::Regex { pattern, flags } => Self::Regex { pattern, flags },
            ConfigValue::Unknown => Self::Unknown,
            ConfigValue::OneOf(alternatives) => Self::OneOf(alternatives.into_iter().map(Self::from).collect()),
        }
    }
}

impl From<Value> for ConfigValue {
    fn from(value: Value) -> Self {
        match value {
            Value::Null | Value::Undefined => Self::Null,
            Value::Bool(b) => Self::Bool(b),
            Value::Number(n) => Self::Number(n),
            Value::String(s) => Self::String(s),
            Value::Array(items) => Self::Array(items.into_iter().map(Self::from).collect()),
            Value::Object(entries) => Self::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, Self::from(value)))
                    .collect(),
            ),
            Value::Regex { pattern, flags } => Self::Regex { pattern, flags },
            Value::Unknown => Self::Unknown,
            Value::OneOf(alternatives) => {
                // `null` and `undefined` are both handed out as `null`.
                let mut out: Vec<Self> = Vec::new();
                for alternative in alternatives.into_iter().map(Self::from) {
                    if !out.contains(&alternative) {
                        out.push(alternative);
                    }
                }
                match out.len() {
                    1 => out.pop().unwrap_or(Self::Unknown),
                    _ => Self::OneOf(out),
                }
            }
            Value::Maybe(value) => Self::from(*value),
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

    /// Whether `specifier`, a bare specifier imported from the file `from` that [`Self::load`] does not load, names an
    /// installed package rather than code of the project: not a workspace package the loader cannot map to a source
    /// file, nor an alias of a project directory (a `tsconfig` path such as `@app/tools`). Only for an installed
    /// package does the evaluator take a `define…` helper (`defineConfig`) to be the value it is given, a copying merge
    /// (`mergeConfig`) the merge of what it is given, and the methods of a binding the config only calls methods on to
    /// keep nothing they are given; code of the project may do anything. `false` unless the loader says otherwise, so
    /// that nothing it does not vouch for is trusted. Without a loader, every bare specifier counts as one.
    fn is_installed(&self, _from: &Path, _specifier: &str) -> bool {
        false
    }

    /// Whether `specifier`, a relative import from the file `from` that [`Self::load`] does not load, names a file of
    /// the project that holds no code: a declaration file (`./types` beside `types.d.ts`), JSON, a stylesheet or an
    /// asset. A re-export of one changes nothing; one of any other module the loader does not load may run code of the
    /// project. `false` unless the loader says otherwise.
    fn holds_no_code(&self, _from: &Path, _specifier: &str) -> bool {
        false
    }
}

/// Statically evaluates what `source` exports: `export default …`, `module.exports = …` or `export = …`.
///
/// Understood: object, array, string, number, boolean and `null` literals; template literals whose substitutions are
/// known strings, numbers or booleans, and `+` of such values with a string (or of two numbers);
/// `as`/`satisfies`/`!`/parenthesised wrappers; calls named `define*` (such as `defineConfig`) of a global or of a
/// package that is not followed, which evaluate to their first argument; arrow and function expressions that return a
/// single expression; spreads of static values; and identifiers that name a top-level `const` initialised with something static; and paths computed from the config
/// file's own location (`__dirname`, `import.meta.dirname`, `new URL('./x', import.meta.url)`, `path.resolve`,
/// `path.join`, `path.dirname` and `fileURLToPath`, however `path` was imported), which evaluate to strings relative to
/// the config file's directory (`"."`, `"./src"`, `"../shared"`).
///
/// `a ? b : c` takes the branch a known test selects; with an unknown test it is what both branches share (for two
/// objects, every key either has, one of their values where they differ and unknown where only one has it), and for
/// other values that differ [`ConfigValue::OneOf`] the two (`process.env.CI ? 'github' : 'list'`), unknown when a
/// branch is unknown or there would be more than eight alternatives. `a && b`, `a || b`, `a ?? b` and `!a`
/// are followed when `a` is known, and `.length` of a known string or array is its length. A member `a.b` of a known
/// object is the value of its key `b`, unknown when an unknown spread may set it or the object may inherit it (a member
/// of `Object.prototype`, an object with a `__proto__`), and `undefined` (handed out as `null`) when the object does
/// not have it, so `[...base.plugins, 'x']` and `options.setupFiles ?? []` are known. An optional member `a?.b` reads
/// the same way and is `undefined` when `a` is `null` or `undefined`, the rest of its chain unread, and so does a
/// computed member whose key evaluates to a string (`a['b']`, `a[KEY]`). A destructuring reads a key the same way,
/// and a parameter's or a destructuring's default applies to `undefined`, not to a literal `null`. A key merged from a
/// spread whose test is unknown (below) may be absent or hold any of the values merged for it, so a read of it is not
/// definite, and neither is a value that carries the read on: a key of an object or a binding holding
/// it, the argument it is passed as, the value a function returns it as, a fallback over it and an array spreading it,
/// in the module that reads it or one importing it. Over a value that is not definite, `a ?? c` and `a || c` credit
/// both sides, a default is credited next to it, and a test on it and its `.length` are unknown.
/// An `if (test) return a;` before a function's last `return` is followed the same way: a known test takes its branch,
/// and an unknown one makes the value what both returns share. A spread of a conditional with an unknown test
/// (`...(tmp && { cacheDir: tmp })`, `...(ci ? { a } : {})`) adds every key a branch sets: a list gets the elements of
/// every branch (a string counting as a list of one), a nested object merges the same way, any other value every
/// branch sets is one of them and one only some branches set is unknown; only a branch that is not an object (or
/// `null`, a boolean, a number or an empty string) leaves the spread unknown. A spread of a value that is one of
/// several adds the keys of each the same way. A path function, a template literal or a `+` over a value that is one
/// of several is one of the results of each combination (`path.join(ci ? 'a' : 'b', 'setup.ts')` is one of two
/// paths), unknown when a result is unknown or there would be more than eight. Spread into an array, such a condition
/// or value adds the elements of every branch (`[...(ci ? ['a'] : [])]`), a branch of `null`, `undefined` or a
/// boolean none. `.filter(Boolean)` of a known array drops the falsy elements and keeps one that may be falsy as the
/// values it has when it is not (`[ci ? 'github' : null, ci && 'blob'].filter(Boolean)` keeps `'github'` and
/// `'blob'`).
/// `.map`, `.flatMap` and `.filter` of a known array call an inline callback, or one the module declares, with each
/// element and its index (`` ['light', 'dark'].map((theme) => ({ name: `browser-${theme}` })) ``); an unknown element,
/// which may stand for a spread of any length, stays unknown, and a predicate whose result is not known keeps the
/// element and leaves the length unknown. `Object.fromEntries` of a known list of `[key, value]` pairs is their object,
/// an element that is not such a pair adding an unknown spread. `a === b` and `a !== b` of known primitives are known,
/// and a known array's element at a constant index (`LIST[0]`) is its value, `undefined` past its end and unknown at
/// or after an unknown element.
///
/// Functions are followed: a returned config function may declare local `const`/`let` bindings and local functions,
/// assign members of a local object, and call top-level or local functions with known arguments, a parameter not passed
/// taking its default and an object pattern parameter (`{ pinCjs = [] } = {}`) its properties or their defaults. A
/// function the tool calls (the exported config function, the callback given to `defineConfig`) is passed what the
/// config does not say, so each of its parameters is unknown, a default included: `(env = {}) => env.analyze ? a : b`
/// keeps both branches. `await x` and
/// `Promise.resolve(x)` evaluate to `x`; `require.resolve('pkg')` and `import.meta.resolve('pkg')` evaluate to
/// `'pkg'`, and the storybook idioms `dirname(require.resolve(join('pkg', 'package.json')))` and
/// `` dirname(fileURLToPath(import.meta.resolve(`${pkg}/package.json`))) `` to `'pkg'`. A call of a glob function
/// (`glob`, `fast-glob`, `globby`, `tinyglobby`, however imported) evaluates to the array of its literal patterns, so a
/// dynamic `entry` becomes entry globs. `process.cwd()` and a first `path.resolve` argument that is not absolute are
/// taken to be the config file's directory, which is right when the tool runs from the package that holds the
/// config and a heuristic otherwise. The directory itself is `"."`, so `dirname(__dirname)` is `".."`. A URL with a
/// scheme (`new URL(x, 'https://…')`, `fileURLToPath('file:///…')`) and `.href` are [`ConfigValue::Unknown`], as is
/// everything else. Returns `None` when the file does not parse or exports nothing recognisable.
///
/// Imports are not followed; [`extract_config`] with a loader follows them.
#[must_use]
pub fn extract_config_object(source: &str, path: &Path) -> Option<ConfigValue> {
    evaluate_config(source, path, None).map(|config| config.value)
}

/// What a config file exports, with the arguments of the calls it is made of that cannot be followed.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ExtractedConfig {
    /// The exported value, as [`extract_config_object`] evaluates it.
    pub value: ConfigValue,
    /// The evaluated arguments of a call the evaluator cannot follow (`createConfig({ setupFiles: ['./setup.ts'] })`
    /// of an installed package) whose result is the exported value, which is then [`ConfigValue::Unknown`], or is an
    /// argument of the merge the file exports (`mergeConfig(createBase({ … }), { … })`), which then has an unknown
    /// spread. A `define*` call (`defineConfig(createBase(options))`) is looked through to the expression it wraps, a
    /// top-level constant to its initialiser, and a config function the tool calls whose body is one expression or a
    /// single `return` (`export default () => createConfig({ … })`) to what it returns, its parameters unknown.
    /// Such a call nested in an object or array literal of that value keeps its arguments too, each in an object under
    /// the keys it is found at: `{ test: createTest({ setupFiles }) }` keeps `{ test: { setupFiles } }`, so a file
    /// named under a key that excludes files stays under it. The values are those the evaluation computed. Empty
    /// otherwise; an unfollowed call anywhere else, such as one a function body computes in a local first, keeps no
    /// arguments.
    pub call_arguments: Vec<ConfigValue>,
}

/// Statically evaluates what `source` exports, as [`extract_config_object`] does, keeping the arguments of the calls
/// it is made of that cannot be followed ([`ExtractedConfig::call_arguments`]).
///
/// With a `loader`, imports are followed: a value imported from another module (a default or named export, a
/// namespace member, a `require`d `module.exports`) is evaluated statically in that module, and a call of an imported
/// function with the arguments given. A chain of imports reaches at most eight modules, the config file included, an
/// export whose value depends on itself through imports is [`ConfigValue::Unknown`] there, and one config loads at
/// most 64 distinct modules and evaluates at most 1024 imported exports and calls. Paths an imported module computes
/// from its own location (`__dirname`, `import.meta.url`) come back relative to the directory of the config file at
/// `path`. Without one, imports are unknown.
#[must_use]
pub fn extract_config(source: &str, path: &Path, loader: Option<&dyn ModuleLoader>) -> Option<ExtractedConfig> {
    let Some(loader) = loader else {
        return evaluate_config(source, path, None);
    };
    let modules = Modules {
        loader,
        root: path.parent().map(Path::to_path_buf).unwrap_or_default(),
        active: RefCell::new(vec![(path.to_path_buf(), Imported::Default)]),
        loads: RefCell::new(HashSet::default()),
        evaluations: Cell::new(0),
        cuts: Cell::new(0),
        values: RefCell::new(HashMap::default()),
        loaded: RefCell::new(HashMap::default()),
        changing: RefCell::new(HashMap::default()),
        pending: RefCell::new(Vec::new()),
        shallowest_cut: Cell::new(usize::MAX),
        analyses: Cell::new(0),
        installed: RefCell::new(HashMap::default()),
    };
    evaluate_config(source, path, Some(&modules))
}

/// The exported value [`extract_config`] finds with `loader`, following the imports it finds.
#[must_use]
pub fn extract_config_object_with(source: &str, path: &Path, loader: &dyn ModuleLoader) -> Option<ConfigValue> {
    extract_config(source, path, Some(loader)).map(|config| config.value)
}

fn evaluate_config(source: &str, path: &Path, modules: Option<&Modules<'_>>) -> Option<ExtractedConfig> {
    crate::with_arena(|allocator| {
        let program = parse_program(allocator, source, path)?;
        let followed = modules.map(|modules| FollowedImports { modules, from: path });
        let module_imports: &dyn Imports = match &followed {
            Some(followed) => followed,
            None => &NoImports,
        };
        let top = top_level(&program, module_imports);
        let bindings = module_bindings(&program);
        let imports = import_bindings(&program);
        let file = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
        let eval = Evaluator::new(&top, &bindings, &imports, file, ".", path, modules);
        let exported = exported(&program)?;
        let value = eval.exported_value(exported, exports_changed(&program));
        let call_arguments = eval.exported_call_arguments(exported, &value);
        Some(ExtractedConfig {
            value: ConfigValue::from(value),
            call_arguments: call_arguments.into_iter().map(ConfigValue::from).collect(),
        })
    })
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
    let mut imports = HashMap::default();
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
    let mut table = HashMap::default();
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
    values: RefCell<HashMap<(PathBuf, Imported), Option<Value>>>,
    /// What the loader returned for each importing directory and specifier.
    loaded: RefCell<HashMap<(PathBuf, String), Option<LoadedModule>>>,
    /// What the analysis of each module found ([`Modules::facts`]), by path: `None` while that is being found.
    changing: RefCell<HashMap<PathBuf, Option<Rc<ModuleFacts>>>>,
    /// The modules being analysed, outermost first.
    pending: RefCell<Vec<PathBuf>>,
    /// The index in [`Self::pending`] of the outermost module an analysis under way found pending, or past a limit
    /// (`0`): what it finds then depends on that module, so it is not kept for a module inside it.
    shallowest_cut: Cell<usize>,
    /// How many module analyses have run, kept or not ([`MAX_MODULE_ANALYSES`]).
    analyses: Cell<usize>,
    /// What the loader said of each importing directory and specifier ([`ModuleLoader::is_installed`]).
    installed: RefCell<HashMap<(PathBuf, String), bool>>,
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

    /// Whether `specifier` names an installed package from `from` ([`ModuleLoader::is_installed`]), asking the loader
    /// once per directory and specifier.
    fn is_installed(&self, from: &Path, specifier: &str) -> bool {
        let key = (from.parent().unwrap_or(from).to_path_buf(), specifier.to_owned());
        if let Some(&known) = self.installed.borrow().get(&key) {
            return known;
        }
        let installed = self.loader.is_installed(from, specifier);
        self.installed.borrow_mut().insert(key, installed);
        installed
    }

    /// Whether a function of the followed module `specifier` names from `from` may change a value of a module when it
    /// is called ([`Unstable::changes_modules`]), one it re-exports or hands out included
    /// ([`Self::reexports_change_modules`], [`Unstable::exposed_modules`]): so it may when the module does not parse,
    /// when finding out would reach past [`MAX_IMPORT_DEPTH`] or [`MAX_MODULE_LOADS`] modules, and in a cycle of modules
    /// whose functions call each other's. Not one of a module that is not followed, whose functions the config's
    /// analysis does not reach.
    fn changes_modules(&self, from: &Path, specifier: &str) -> bool {
        self.facts(from, specifier).is_some_and(|(_, facts)| facts.changes)
    }

    /// The places in what a call of the export `export` of the followed module `specifier` names from `from` returns
    /// that may hold an object of a value of a module ([`Unstable::result_places`]), the export followed through
    /// re-exports; `None` when that is not known, as for an export that is not a function the module declares.
    fn result_places(&self, from: &Path, specifier: &str, export: &str) -> Option<Vec<Write>> {
        match self.export_result_places(from, specifier, export, 0) {
            ExportResult::Places(places) => Some(places),
            ExportResult::Missing | ExportResult::Unknown => None,
        }
    }

    /// [`Self::result_places`], `depth` re-exports deep.
    fn export_result_places(&self, from: &Path, specifier: &str, export: &str, depth: usize) -> ExportResult {
        if depth >= MAX_IMPORT_DEPTH {
            return ExportResult::Unknown;
        }
        let Some((path, facts)) = self.facts(from, specifier) else {
            return ExportResult::Missing;
        };
        let local = match facts.exports.get(export) {
            Some(ExportSource::Local(local)) => Some(local.as_str()),
            Some(ExportSource::From(next, imported)) => {
                let name = match imported {
                    Imported::Default => "default",
                    Imported::Named(name) => name.as_str(),
                    Imported::Namespace => return ExportResult::Unknown,
                };
                return match self.export_result_places(&path, next, name, depth + 1) {
                    ExportResult::Missing => ExportResult::Unknown,
                    found => found,
                };
            }
            None if export == "default" => facts.default.as_deref(),
            None => {
                return facts
                    .stars
                    .iter()
                    .map(|next| self.export_result_places(&path, next, export, depth + 1))
                    .find(|found| !matches!(found, ExportResult::Missing))
                    .unwrap_or(ExportResult::Missing);
            }
        };
        local
            .and_then(|local| facts.results.get(local).cloned())
            .map_or(ExportResult::Unknown, ExportResult::Places)
    }

    /// What the analysis of the followed module `specifier` names from `from` finds, with its path; what may hold for
    /// any module ([`ModuleFacts::unknown`]) while the module is being analysed, past [`MAX_IMPORT_DEPTH`] modules being
    /// analysed, [`MAX_MODULE_LOADS`] analysed or [`MAX_MODULE_ANALYSES`] analyses run, and when the module does not parse. What one finds while a module that
    /// encloses it is pending is not kept, as it depends on that module.
    fn facts(&self, from: &Path, specifier: &str) -> Option<(PathBuf, Rc<ModuleFacts>)> {
        let (path, source) = self.load(from, specifier)?;
        if let Some(known) = self.changing.borrow().get(&path) {
            let facts = known.clone().unwrap_or_else(|| {
                let at = self.pending.borrow().iter().position(|pending| *pending == path);
                self.shallowest_cut.set(self.shallowest_cut.get().min(at.unwrap_or(0)));
                Rc::new(ModuleFacts::unknown())
            });
            return Some((path, facts));
        }
        let depth = self.pending.borrow().len();
        if depth >= MAX_IMPORT_DEPTH
            || self.changing.borrow().len() >= MAX_MODULE_LOADS
            || self.analyses.get() >= MAX_MODULE_ANALYSES
        {
            self.shallowest_cut.set(0);
            return Some((path, Rc::new(ModuleFacts::unknown())));
        }
        self.analyses.set(self.analyses.get() + 1);
        self.changing.borrow_mut().insert(path.clone(), None);
        self.pending.borrow_mut().push(path.clone());
        let outer = self.shallowest_cut.replace(usize::MAX);
        let facts = Rc::new(self.module_facts(&path, &source));
        self.pending.borrow_mut().pop();
        let cut = self.shallowest_cut.get();
        if cut < depth {
            self.changing.borrow_mut().remove(&path);
            self.shallowest_cut.set(outer.min(cut));
        } else {
            self.changing.borrow_mut().insert(path.clone(), Some(Rc::clone(&facts)));
            self.shallowest_cut.set(outer);
        }
        Some((path, facts))
    }

    /// [`Self::facts`] of the module at `path` with the text `source`.
    fn module_facts(&self, path: &Path, source: &str) -> ModuleFacts {
        let Ok(source_type) = SourceType::from_path(path) else {
            return ModuleFacts::unknown();
        };
        let allocator = Allocator::default();
        let ret = Parser::new(&allocator, source, source_type).parse();
        if ret.diagnostics.errors().next().is_some() {
            return ModuleFacts::unknown();
        }
        let unstable = Unstable::of(
            &ret.program,
            &FollowedImports {
                modules: self,
                from: path,
            },
        );
        let changes = unstable.changes_modules()
            || unstable
                .exposed_modules()
                .iter()
                .any(|specifier| self.changes_modules(path, specifier))
            || self.reexports_change_modules(path, &ret.program);
        let (exports, stars) = export_table(&ret.program);
        let default = exported(&ret.program).and_then(|exported| match exported {
            Exported::Function(function) => function.id.as_ref().map(|id| id.name.as_str().to_owned()),
            Exported::Expression(expr) => match expr.get_inner_expression() {
                Expression::Identifier(id) => Some(id.name.as_str().to_owned()),
                _ => None,
            },
        });
        ModuleFacts {
            changes,
            results: unstable.result_places().clone(),
            exports,
            stars,
            default,
        }
    }

    /// Whether a function the module at `path` re-exports (`export { tweak } from './tweak'`, `export * from './tools'`)
    /// may change a value of a module when it is called: one of a followed module that may ([`Self::changes_modules`]),
    /// or of a module that is neither followed, nor an installed package ([`ModuleLoader::is_installed`]), nor a file
    /// that holds no code ([`ModuleLoader::holds_no_code`]), whose code is not known.
    fn reexports_change_modules(&self, path: &Path, program: &Program<'_>) -> bool {
        reexported(program).into_iter().any(|specifier| {
            if self.follows(path, &specifier) {
                self.changes_modules(path, &specifier)
            } else {
                !self.is_installed(path, &specifier) && !self.loader.holds_no_code(path, &specifier)
            }
        })
    }

    /// The value `imported` names in the module `specifier` resolves to from `from`, called with `args` when given.
    /// `None` when the module is not followed or does not have the export.
    fn evaluate(&self, from: &Path, specifier: &str, imported: &Imported, args: Option<&[Value]>) -> Option<Value> {
        let (path, source) = self.load(from, specifier)?;
        let key = (path.clone(), imported.clone());
        if args.is_none()
            && let Some(known) = self.values.borrow().get(&key)
        {
            return known.clone();
        }
        if self.cut(&key) {
            self.cuts.set(self.cuts.get() + 1);
            return Some(Value::Unknown);
        }
        self.loads.borrow_mut().insert(path.clone());
        self.evaluations.set(self.evaluations.get() + 1);
        let cuts = self.cuts.get();
        self.active.borrow_mut().push(key.clone());
        let value = self.evaluate_module(&path, &source, imported, args).map(as_imported);
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

    fn evaluate_module(&self, path: &Path, source: &str, imported: &Imported, args: Option<&[Value]>) -> Option<Value> {
        let source_type = SourceType::from_path(path).ok()?;
        let allocator = Allocator::default();
        let ret = Parser::new(&allocator, source, source_type)
            .with_options(OxcParseOptions {
                preserve_parens: false,
                ..OxcParseOptions::default()
            })
            .parse();
        if ret.diagnostics.errors().next().is_some() {
            return Some(Value::Unknown);
        }
        let program = &ret.program;
        let top = top_level(
            program,
            &FollowedImports {
                modules: self,
                from: path,
            },
        );
        let bindings = module_bindings(program);
        let imports = import_bindings(program);
        let dir = relative_dir(&self.root, path.parent().unwrap_or(path));
        let file = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
        let eval = Evaluator::new(&top, &bindings, &imports, file, &dir, path, Some(self));
        match imported {
            Imported::Namespace => Some(Value::Unknown),
            Imported::Default => Some(match exported(program)? {
                Exported::Function(function) => eval.invoke(Callable::Function(function), args, true, 0),
                Exported::Expression(expr) => {
                    match (args, Callable::of_expression(expr), expr.get_inner_expression()) {
                        (None, _, _) => eval.expr(expr, 0),
                        (Some(args), Some(callable), _) => eval.invoke(callable, Some(args), true, 0),
                        (Some(args), None, Expression::Identifier(id)) => eval.call_named(id.name.as_str(), args),
                        (Some(_), None, _) => Value::Unknown,
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

/// The modules `program` re-exports a value of (`export { tweak } from './tweak'`, `export * from './tools'`), by
/// specifier.
fn reexported(program: &Program<'_>) -> Vec<String> {
    program
        .body
        .iter()
        .filter_map(|stmt| match stmt {
            Statement::ExportFromDeclaration(e) if !e.export_kind.is_type() => Some(&e.source),
            Statement::ExportAllDeclaration(e) if !e.export_kind.is_type() => Some(&e.source),
            _ => None,
        })
        .map(|source| crate::js_string(source.value))
        .collect()
}

/// What a followed module's export returns, as far as [`Modules::export_result_places`] finds it.
enum ExportResult {
    /// The module is not followed or has no such export.
    Missing,
    /// The module has the export, and what it returns is not known.
    Unknown,
    /// The places of the module's values the export may return.
    Places(Vec<Write>),
}

/// What the analysis of a followed module finds ([`Modules::facts`]).
struct ModuleFacts {
    /// Whether a function of the module may change a value of a module when it is called ([`Modules::changes_modules`]).
    changes: bool,
    /// The places in what a call of each top-level function of the module returns that may hold an object of a value
    /// of a module, by name ([`Unstable::result_places`]).
    results: HashMap<String, Vec<Write>>,
    /// The named exports of the module ([`export_table`]).
    exports: HashMap<String, ExportSource>,
    /// The modules it re-exports everything of.
    stars: Vec<String>,
    /// The name of the binding it exports as its default, when it exports one.
    default: Option<String>,
}

impl ModuleFacts {
    /// What may hold for any module: its functions may change a value of a module, and nothing is known of what they
    /// return.
    fn unknown() -> Self {
        Self {
            changes: true,
            results: HashMap::default(),
            exports: HashMap::default(),
            stars: Vec::new(),
            default: None,
        }
    }
}

/// A value of another module as the module importing it sees it: what that module computes itself is a value taken
/// from elsewhere, so each [`OWN_SPREAD`] becomes an [`UNKNOWN_SPREAD`].
fn as_imported(value: Value) -> Value {
    match value {
        Value::Object(entries) => Value::Object(
            entries
                .into_iter()
                .map(|(key, value)| {
                    let key = if key == OWN_SPREAD {
                        UNKNOWN_SPREAD.to_owned()
                    } else {
                        key
                    };
                    (key, as_imported(value))
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(as_imported).collect()),
        Value::OneOf(alternatives) => Value::OneOf(alternatives.into_iter().map(as_imported).collect()),
        Value::Maybe(value) => Value::Maybe(Box::new(as_imported(*value))),
        other => other,
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
    /// `if (env) config.a = 1`, `const n = list.push(x)`), with how: a constant among them is not only its initialiser.
    changed: HashMap<String, TopChange>,
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

/// What a name is bound to in the functions being evaluated.
#[derive(Clone, Copy)]
enum LocalBinding<'p, 'a> {
    Function(Callable<'p, 'a>),
    Other,
}

/// What a name is bound to inside a function being evaluated.
#[derive(Clone)]
enum Binding<'p, 'a> {
    Value(Value),
    Callable(Callable<'p, 'a>),
    /// A `const` that takes a part of a local of its function (`const { coverage } = config.test`), whose changes the
    /// evaluator applies to that local too ([`Unstable::follows`]).
    Alias(Alias),
}

/// The value of an alias binding, and the part of a local it takes: the local `root`, bound in the frame at index
/// `frame`, at `keys` below it.
#[derive(Clone)]
struct Alias {
    value: Value,
    root: String,
    frame: usize,
    keys: Vec<String>,
}

impl Alias {
    /// Whether `other` takes the same part of the same local.
    fn same_part(&self, other: &Self) -> bool {
        self.root == other.root && self.frame == other.frame && self.keys == other.keys
    }
}

/// A change the evaluator applies to a local object.
enum Edit {
    /// `name.key = value`.
    Set(String, Value),
    /// `name.push(…)`, or with `front` `name.unshift(…)`: the list then holds `items` too.
    Insert { front: bool, items: Vec<Value> },
    /// A change it does not follow, which leaves the object partly unknown.
    Taint,
}

/// How top-level statements the evaluator does not run change a top-level binding.
enum TopChange {
    /// They assign it: it may hold any value.
    Assigned,
    /// They change the parts of the objects it holds that these say.
    Written(Vec<Write>),
}

type Frame<'p, 'a> = HashMap<String, Binding<'p, 'a>>;

/// The imports of a module whose imports the evaluator does not follow.
struct NoImports;

impl Imports for NoImports {
    fn follows(&self, _: &str) -> bool {
        false
    }

    fn changes_modules(&self, _: &str) -> bool {
        false
    }

    /// Every package is taken to be installed when no loader can tell.
    fn is_installed(&self, _: &str) -> bool {
        true
    }
}

/// The imports of the module at `from`, as `modules` follows them.
struct FollowedImports<'m, 'l> {
    modules: &'m Modules<'l>,
    from: &'m Path,
}

impl Imports for FollowedImports<'_, '_> {
    fn follows(&self, specifier: &str) -> bool {
        self.modules.follows(self.from, specifier)
    }

    fn changes_modules(&self, specifier: &str) -> bool {
        self.modules.changes_modules(self.from, specifier)
    }

    fn result_places(&self, specifier: &str, export: &str) -> Option<Vec<Write>> {
        self.modules.result_places(self.from, specifier, export)
    }

    fn is_installed(&self, specifier: &str) -> bool {
        self.modules.is_installed(self.from, specifier)
    }
}

/// The top-level bindings of `program`, where `imports` says what the evaluator knows of the modules it imports.
fn top_level<'p, 'a>(program: &'p Program<'a>, imports: &dyn Imports) -> TopLevel<'p, 'a> {
    let unstable = Unstable::of(program, imports);
    let mut top = TopLevel {
        consts: HashMap::default(),
        functions: HashMap::default(),
        changed: HashMap::default(),
        unstable: Unstable::default(),
    };
    for stmt in &program.body {
        let decl = match stmt {
            Statement::ExportDeclaration(e) => Some(&e.declaration),
            other => other.as_declaration(),
        };
        let writes = match stmt {
            // Only what the statement itself does, as for a statement of a function: an exported value may read
            // members of a constant (`base.plugins.map(f)`) without changing it.
            Statement::ExpressionStatement(s) => Some(Writes::of_effect(&s.expression, &unstable)),
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
            | Statement::WithStatement(_)
            | Statement::VariableDeclaration(_)
            | Statement::ExportNamedDeclaration(_)
            | Statement::ExportDefaultDeclaration(_) => Some(Writes::of_statement(stmt, &unstable)),
            _ => None,
        };
        if let Some(writes) = writes {
            top.record(writes);
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
    top.unstable = unstable;
    top
}

/// Array methods [`Evaluator::array_callback_call`] runs over a known array.
const ARRAY_CALLBACK_METHODS: &[&str] = &["map", "flatMap", "filter"];

impl TopLevel<'_, '_> {
    /// Records what a top-level statement writes ([`Writes`]).
    fn record(&mut self, writes: Writes<'_>) {
        for name in writes.assigned {
            self.changed.insert(name, TopChange::Assigned);
        }
        for (name, write) in writes.changed {
            if let TopChange::Written(list) = self
                .changed
                .entry(name)
                .or_insert_with(|| TopChange::Written(Vec::new()))
            {
                list.push(write);
            }
        }
    }
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
fn apply_path_function(function: &str, args: &[Value]) -> Value {
    let Some(parts) = args.iter().map(Value::as_str).collect::<Option<Vec<&str>>>() else {
        return Value::Unknown;
    };
    let absolute = |part: &&str| part.starts_with(['/', '\\']);
    let path = match (function, parts.as_slice()) {
        // Only a path derived from `import.meta.url` is known; a URL with a scheme names something else.
        ("fileURLToPath", [file]) if !file.contains(':') => (*file).to_owned(),
        ("dirname", [path]) => normalized(&[*path, ".."]),
        ("normalize", [path]) => normalized(&[*path]),
        ("resolve", parts) if !parts.iter().any(absolute) => normalized(parts),
        ("join", parts) if !parts.first().is_some_and(absolute) => normalized(parts),
        _ => return Value::Unknown,
    };
    Value::String(path)
}

/// Statically evaluates the option `key` of every `new constructor({ … })` in `source`, however the constructor was
/// imported (`new NodejsFunction(…)`, `new lambda_nodejs.NodejsFunction(…)`). Path computations are relative to the
/// directory of `path`. Returns nothing when the file does not parse.
#[must_use]
pub fn extract_constructor_options(source: &str, path: &Path, constructor: &str, key: &str) -> Vec<ConfigValue> {
    let Ok(source_type) = SourceType::from_path(path) else {
        return Vec::new();
    };
    crate::with_arena(|allocator| {
        let ret = Parser::new(allocator, source, source_type)
            .with_options(OxcParseOptions {
                preserve_parens: false,
                ..OxcParseOptions::default()
            })
            .parse();
        if ret.diagnostics.errors().next().is_some() {
            return Vec::new();
        }
        let program = &ret.program;
        let top = top_level(program, &NoImports);
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
        finder.found.into_iter().map(ConfigValue::from).collect()
    })
}

/// Statically evaluates the value of every property named `key` of every object literal in `source`, however deeply
/// nested and wherever it appears (`handler: "src/api.handler"` inside a function body), in source order. Values are
/// evaluated as in [`extract_config_object`], against the top-level bindings, with paths relative to the directory of
/// `path`; a name a function, block, loop or `catch` around the property declares is unknown, whatever top-level
/// binding it shadows. Methods are left out. Returns nothing when the file does not parse.
#[must_use]
pub fn extract_property_values(source: &str, path: &Path, key: &str) -> Vec<ConfigValue> {
    find_properties(source, path, key, None)
}

/// Statically calls every function that is the property `key` of an object literal in `source`, a method
/// (`configureWebpack(config) { … }`) or a function or arrow value, with `argument` as its first parameter, in source
/// order. Each result is the value the function returns or, when its body finishes without a `return`, what it leaves
/// its first parameter as: a function that changes the object it is given (`config.entry = { … }`) yields the changed
/// object. Calls are evaluated as in [`extract_config_object`]; a property that is not a function is left out.
/// Returns nothing when the file does not parse.
#[must_use]
pub fn extract_property_calls(source: &str, path: &Path, key: &str, argument: &ConfigValue) -> Vec<ConfigValue> {
    find_properties(source, path, key, Some(argument))
}

/// The values of the properties `key` in `source`, or the results of calling them with `call_with`.
fn find_properties(source: &str, path: &Path, key: &str, call_with: Option<&ConfigValue>) -> Vec<ConfigValue> {
    crate::with_arena(|allocator| {
        let Some(program) = parse_program(allocator, source, path) else {
            return Vec::new();
        };
        let top = top_level(&program, &NoImports);
        let bindings = module_bindings(&program);
        let mut finder = PropertyFinder {
            top: &top,
            bindings: &bindings,
            file: path.file_name().and_then(|name| name.to_str()).unwrap_or_default(),
            key,
            call_with,
            evaluated: HashMap::default(),
            shadowed: Vec::new(),
            found: Vec::new(),
        };
        finder.visit_program(&program);
        finder.found.into_iter().map(ConfigValue::from).collect()
    })
}

/// Statically evaluates the arguments of every call of a method named `method` in `source`, whatever object it is
/// called on (`eleventyConfig.addPassthroughCopy("img")`), in source order: one list of arguments per call. Arguments
/// are evaluated as in [`extract_config_object`], so a top-level constant is known and a local of the enclosing
/// function is not. Returns nothing when the file does not parse.
#[must_use]
pub fn extract_method_calls(source: &str, path: &Path, method: &str) -> Vec<Vec<ConfigValue>> {
    crate::with_arena(|allocator| {
        let Some(program) = parse_program(allocator, source, path) else {
            return Vec::new();
        };
        let top = top_level(&program, &NoImports);
        let bindings = module_bindings(&program);
        let mut finder = MethodCallFinder {
            top: &top,
            bindings: &bindings,
            file: path.file_name().and_then(|name| name.to_str()).unwrap_or_default(),
            method,
            found: Vec::new(),
        };
        finder.visit_program(&program);
        finder
            .found
            .into_iter()
            .map(|arguments| arguments.into_iter().map(ConfigValue::from).collect())
            .collect()
    })
}

/// Parses `source` as the JavaScript or TypeScript file `path` names, without keeping parentheses; `None` for another
/// kind of file or a file that does not parse.
fn parse_program<'a>(allocator: &'a Allocator, source: &'a str, path: &Path) -> Option<Program<'a>> {
    let source_type = SourceType::from_path(path).ok()?;
    let ret = Parser::new(allocator, source, source_type)
        .with_options(OxcParseOptions {
            preserve_parens: false,
            ..OxcParseOptions::default()
        })
        .parse();
    if ret.diagnostics.errors().next().is_some() {
        return None;
    }
    Some(ret.program)
}

/// A call, or a tagged template, of a function a module exports; see [`extract_imported_calls`].
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedCall {
    /// The module the function comes from.
    pub module: String,
    /// The export called: `"default"` for the default export or the module object itself (`pino(…)`), the export's
    /// name for a named import (`execa(…)`), or the member called on the module object (`pino.transport(…)`).
    pub export: String,
    /// The statically evaluated arguments, in order. For a call that runs a command ([`extract_command_calls`]), the
    /// `cwd` option of an options object is left out when it names the directory the call runs in anyway
    /// (`process.cwd()`, `'.'`, `undefined`), and starts with [`FILE_DIR`] when the file's own location computes it.
    /// A tagged template has none, except that one run in another directory by a chained `.cwd(dir)` (Bun Shell's
    /// `` $`…`.cwd(dir) ``) has an options object with that `cwd`, marked the same way. Such a call also marks each
    /// string of the `entrypoints` array of an options object (`Bun.build`) that the file's own location computes
    /// (`path.join(import.meta.dir, 'src/x.ts')`): it starts with [`FILE_DIR`] and `/`, followed by the path relative
    /// to the file's directory.
    pub arguments: Vec<ConfigValue>,
    /// For a tagged template (`` $`cmd ${x}` ``), its raw text with every substitution replaced by
    /// [`TEMPLATE_SUBSTITUTION`]; `None` for a call.
    pub template: Option<String>,
}

/// What a substitution of a tagged template reads as in [`ImportedCall::template`].
pub const TEMPLATE_SUBSTITUTION: &str = "$__skif__";

/// What the directory of the calling file reads as in the `cwd` option of an [`ImportedCall`] that the file's own
/// location computes (`__dirname`, `import.meta.dirname`, `path.join(__dirname, 'fixtures')`), followed by `/` and the
/// path relative to the file's directory (`$__skif_file_dir__/./fixtures`). [`command_in_call_dir`] changes to it, and
/// [`place_file_dir`] puts the file's directory in its place.
pub const FILE_DIR: &str = "$__skif_file_dir__";

/// The directory the arguments of a call are evaluated from a second time to tell a path the file's own location
/// computes, which changes with it, from one that does not (`process.cwd()`, a literal).
const PROBE_DIR: &str = "__skif__/probe";

/// Every call and tagged template in `source` whose function is imported from one of `modules` (`import`, `import * as`,
/// or a top-level `require`, plain or destructured), with its arguments statically evaluated as in
/// [`extract_config_object`]. `globals` names functions available without an import, as `(name, module)` pairs (the
/// `$` of a zx script); a name written `Bun.*` stands for the module object itself, so `` Bun.$`…` `` and
/// `Bun.build(…)` call the exports `$` and `build` of the module. Shadowed names are not tracked. Returns nothing when
/// the file does not parse.
#[must_use]
pub fn extract_imported_calls(
    source: &str,
    path: &Path,
    modules: &[&str],
    globals: &[(&str, &str)],
) -> Vec<ImportedCall> {
    parse_imported_calls(source, path, modules, globals, false)
}

/// The calls of [`extract_imported_calls`] for functions that run a command (`execa`, `nano-spawn`), whose `cwd`
/// option is read as the directory the command runs in (see [`ImportedCall::arguments`]).
#[must_use]
pub fn extract_command_calls(
    source: &str,
    path: &Path,
    modules: &[&str],
    globals: &[(&str, &str)],
) -> Vec<ImportedCall> {
    parse_imported_calls(source, path, modules, globals, true)
}

fn parse_imported_calls(
    source: &str,
    path: &Path,
    modules: &[&str],
    globals: &[(&str, &str)],
    commands: bool,
) -> Vec<ImportedCall> {
    let Ok(source_type) = SourceType::from_path(path) else {
        return Vec::new();
    };
    crate::with_arena(|allocator| {
        let ret = Parser::new(allocator, source, source_type)
            .with_options(OxcParseOptions {
                preserve_parens: false,
                ..OxcParseOptions::default()
            })
            .parse();
        if ret.diagnostics.errors().next().is_some() {
            return Vec::new();
        }
        let file = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
        imported_calls_in(&ret.program, file, modules, globals, commands)
    })
}

/// The calls and tagged templates in a parsed `program` whose function is imported from one of `modules`, as
/// [`extract_imported_calls`] finds them, or with `commands` as [`extract_command_calls`] does; `file` is the file name
/// `__filename` evaluates to.
pub(crate) fn imported_calls_in(
    program: &Program<'_>,
    file: &str,
    modules: &[&str],
    globals: &[(&str, &str)],
    commands: bool,
) -> Vec<ImportedCall> {
    let imported = imported_functions(program, modules, globals);
    if imported.is_empty() {
        return Vec::new();
    }
    let top = top_level(program, &NoImports);
    let bindings = module_bindings(program);
    let mut finder = CallFinder {
        top: &top,
        bindings: &bindings,
        file,
        imported: &imported,
        commands,
        chained_cwd: HashMap::default(),
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

/// The command of a `(command line, options)` call such as `execSync('tsc -b', { cwd })`, in the directory its `cwd`
/// option names (see [`command_in_call_dir`]); `None` unless the command line is a string.
#[must_use]
pub fn command_line_call(arguments: &[ConfigValue]) -> Option<String> {
    let command = arguments.first().and_then(ConfigValue::as_str)?;
    Some(command_in_call_dir(command.to_owned(), arguments.get(1)))
}

/// The options object of a `(file, [arguments], options)` call such as `spawn('eslint', ['.'], { cwd })`: the argument
/// after the arguments array, or the second argument when that is no array.
#[must_use]
pub fn file_call_options(arguments: &[ConfigValue]) -> Option<&ConfigValue> {
    match arguments.get(1) {
        Some(ConfigValue::Array(_)) => arguments.get(2),
        other => other,
    }
}

/// `command` as a call whose options object is `options` runs it. A `cwd` option the file's own location computes
/// ([`FILE_DIR`]) runs it in that directory, so the command line first changes to it (`cd $__skif_file_dir__/. && …`).
/// Any other `cwd` runs it in a directory the script parser cannot place (a literal path is relative to the process,
/// which the parser does not know), so the command line first changes to a directory a substitution names
/// (`cd $__skif__ && …`): a package manager's script run there is another package's, as with `bun --cwd`.
#[must_use]
pub fn command_in_call_dir(command: String, options: Option<&ConfigValue>) -> String {
    match options.and_then(|options| options.get("cwd")) {
        None => command,
        Some(ConfigValue::String(dir)) if dir.starts_with(FILE_DIR) => format!("cd {dir} && {command}"),
        Some(_) => format!("cd {TEMPLATE_SUBSTITUTION} && {command}"),
    }
}

/// `command` with the directory of `file`, the file that runs it as a `/`-separated path relative to the directory
/// where the command starts, in place of [`FILE_DIR`], single-quoted so that a shell reads it as part of one word
/// whatever it holds (`cd 'my scripts'/../sub`).
#[must_use]
pub fn place_file_dir(command: &str, file: &str) -> String {
    let dir = file.rsplit_once('/').map_or("", |(dir, _)| dir);
    let quoted = if dir.is_empty() {
        ".".to_owned()
    } else {
        format!("'{}'", dir.replace('\'', r"'\''"))
    };
    command.replace(FILE_DIR, &quoted)
}

/// Marks the `cwd` option of `options`, a call argument evaluated from the file's directory, by what it is relative
/// to: left out when it names the directory the call runs in anyway (`process.cwd()`, `'.'`, `undefined`), and
/// [`FILE_DIR`] joined with the path when `probe`, an evaluator from [`PROBE_DIR`], finds that the file's own location
/// computes it. A path a command line cannot hold as one plain word is left as it is.
fn mark_cwd<'p, 'a>(options: &mut ConfigValue, expression: &'p Expression<'a>, probe: &Evaluator<'_, 'p, 'a>) {
    let ConfigValue::Object(entries) = options else {
        return;
    };
    let Some(index) = entries.iter().rposition(|(key, _)| key == "cwd") else {
        return;
    };
    let probed = ConfigValue::from(probe.expr(expression, 0));
    let cwd = std::mem::replace(&mut entries[index].1, ConfigValue::Unknown);
    match marked_cwd(cwd, probed.get("cwd"), sets_undefined(expression, "cwd")) {
        Some(cwd) => entries[index].1 = cwd,
        None => entries.retain(|(key, _)| key != "cwd"),
    }
}

/// The option of a command call's options object that lists files to bundle (`Bun.build`), relative to the working
/// directory unless the file's own location computes them.
const ENTRYPOINTS: &str = "entrypoints";

/// Marks each string of the `entrypoints` array of `options`, a call argument evaluated from the file's directory, that
/// `probe`, an evaluator from [`PROBE_DIR`], evaluates differently, so that the file's own location computes it: it
/// becomes [`FILE_DIR`] joined with the path. Any other entry is left as it is.
fn mark_entrypoints<'p, 'a>(options: &mut ConfigValue, expression: &'p Expression<'a>, probe: &Evaluator<'_, 'p, 'a>) {
    let ConfigValue::Object(entries) = options else {
        return;
    };
    let Some(index) = entries.iter().rposition(|(key, _)| key == ENTRYPOINTS) else {
        return;
    };
    let ConfigValue::Array(items) = &mut entries[index].1 else {
        return;
    };
    let probed = ConfigValue::from(probe.expr(expression, 0));
    let Some(ConfigValue::Array(probed)) = probed.get(ENTRYPOINTS) else {
        return;
    };
    for (item, probed) in items.iter_mut().zip(probed) {
        let file_relative = probed != &*item;
        if file_relative && let ConfigValue::String(path) = item {
            *path = format!("{FILE_DIR}/{path}");
        }
    }
}

/// A `cwd` evaluated from the file's directory, marked as [`mark_cwd`] does: `None` when it names the directory the
/// call runs in anyway (`'.'`, or `undefined` set explicitly), [`FILE_DIR`] joined with the path when `probed`, the
/// same `cwd` evaluated from [`PROBE_DIR`], differs from it, so that the file's own location computes it.
fn marked_cwd(cwd: ConfigValue, probed: Option<&ConfigValue>, undefined: bool) -> Option<ConfigValue> {
    let file_relative = matches!(cwd, ConfigValue::String(_)) && probed != Some(&cwd);
    let unchanged = !file_relative && matches!(cwd.as_str(), Some("." | "./"));
    if unchanged || undefined {
        return None;
    }
    match cwd {
        ConfigValue::String(path)
            if file_relative
                && path
                    .chars()
                    .all(|c| c.is_alphanumeric() || matches!(c, '_' | '@' | '.' | '/' | '+' | '-')) =>
        {
            Some(ConfigValue::String(format!("{FILE_DIR}/{path}")))
        }
        other => Some(other),
    }
}

/// Whether `expression` is `undefined` (or `void 0`).
fn is_undefined(expression: &Expression<'_>) -> bool {
    match expression.get_inner_expression() {
        Expression::Identifier(id) => id.name.as_str() == "undefined",
        Expression::UnaryExpression(u) => u.operator == UnaryOperator::Void,
        _ => false,
    }
}

/// Whether `expression` is an object literal whose last property `key` is `undefined` (or `void 0`).
fn sets_undefined(expression: &Expression<'_>, key: &str) -> bool {
    let Expression::ObjectExpression(object) = expression.get_inner_expression() else {
        return false;
    };
    let last = object.properties.iter().rev().find_map(|property| match property {
        ObjectPropertyKind::ObjectProperty(p)
            if p.key
                .static_name()
                .is_some_and(|name| crate::js_string(name.as_js_str()) == key) =>
        {
            Some(&p.value)
        }
        _ => None,
    });
    last.is_some_and(is_undefined)
}

/// Local name to module and named export (`None` for the default export or the module object) of everything a
/// program imports from `modules`, plus `globals`.
type ImportedFunctions = HashMap<String, (String, Option<String>)>;

/// The entries of one object literal, as a spread branch lists them.
type Entries<'v> = &'v [(String, Value)];

fn imported_functions(program: &Program<'_>, modules: &[&str], globals: &[(&str, &str)]) -> ImportedFunctions {
    let mut imported: ImportedFunctions = globals
        .iter()
        .map(|(name, module)| match name.strip_suffix(".*") {
            Some(object) => (object.to_owned(), ((*module).to_owned(), None)),
            None => ((*name).to_owned(), ((*module).to_owned(), Some((*name).to_owned()))),
        })
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
    /// The calls run commands, so their `cwd` option is marked (see [`mark_cwd`]).
    commands: bool,
    /// The `cwd` a chained `.cwd(dir)` sets for the tagged template at a span, marked (see [`marked_cwd`]); `None`
    /// when it names the directory the command runs in anyway.
    chained_cwd: HashMap<Span, Option<ConfigValue>>,
    found: Vec<ImportedCall>,
}

/// The function a tagged template calls: its tag, or the callee of a tag that is itself a call (`` $(opts)`…` ``).
fn template_tag<'e, 'a>(template: &'e TaggedTemplateExpression<'a>) -> &'e Expression<'a> {
    match template.tag.get_inner_expression() {
        Expression::CallExpression(call) => &call.callee,
        other => other,
    }
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

    /// The tagged template of an imported function that a chain of method calls starts from: `object` in
    /// `` object.cwd(dir) `` for `` $`…`.cwd(dir) `` or `` $`…`.quiet().cwd(dir) ``.
    fn chained_template<'e>(&self, mut object: &'e Expression<'a>) -> Option<&'e TaggedTemplateExpression<'a>> {
        loop {
            match object.get_inner_expression() {
                Expression::TaggedTemplateExpression(template) => {
                    return self.target(template_tag(template)).is_some().then_some(&**template);
                }
                Expression::CallExpression(call) => match call.callee.get_inner_expression() {
                    Expression::StaticMemberExpression(member) => object = &member.object,
                    _ => return None,
                },
                _ => return None,
            }
        }
    }

    /// Records the directory `` $`…`.cwd(dir) `` (Bun Shell) runs a command template in, for the template to carry
    /// as its `cwd` option. The outermost `.cwd` of a chain is the one that applies, and it is visited first.
    fn record_chained_cwd(&mut self, it: &CallExpression<'a>) {
        let Expression::StaticMemberExpression(member) = it.callee.get_inner_expression() else {
            return;
        };
        if member.property.name.as_str() != "cwd" {
            return;
        }
        let Some(template) = self.chained_template(&member.object) else {
            return;
        };
        let Some(directory) = it.arguments.first().and_then(Argument::as_expression) else {
            return;
        };
        let eval = Evaluator::in_file(self.top, self.bindings, self.file, HashMap::default());
        let probe = Evaluator {
            dir: PROBE_DIR,
            ..Evaluator::in_file(self.top, self.bindings, self.file, HashMap::default())
        };
        let probed = ConfigValue::from(probe.expr(directory, 0));
        let cwd = marked_cwd(
            ConfigValue::from(eval.expr(directory, 0)),
            Some(&probed),
            is_undefined(directory),
        );
        self.chained_cwd.entry(template.span).or_insert(cwd);
    }
}

impl<'a> Visit<'a> for CallFinder<'_, '_, 'a> {
    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        if self.commands {
            self.record_chained_cwd(it);
        }
        if let Some((module, export)) = self.target(&it.callee) {
            let eval = Evaluator::in_file(self.top, self.bindings, self.file, HashMap::default());
            let mut arguments: Vec<ConfigValue> = it
                .arguments
                .iter()
                .map(|argument| {
                    argument
                        .as_expression()
                        .map_or(ConfigValue::Unknown, |e| ConfigValue::from(eval.expr(e, 0)))
                })
                .collect();
            if self.commands
                && arguments
                    .iter()
                    .any(|argument| argument.get("cwd").is_some() || argument.get(ENTRYPOINTS).is_some())
            {
                let probe = Evaluator {
                    dir: PROBE_DIR,
                    ..Evaluator::in_file(self.top, self.bindings, self.file, HashMap::default())
                };
                for (argument, expression) in arguments.iter_mut().zip(&it.arguments) {
                    if let Some(expression) = expression.as_expression() {
                        mark_entrypoints(argument, expression, &probe);
                        mark_cwd(argument, expression, &probe);
                    }
                }
            }
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
        if let Some((module, export)) = self.target(template_tag(it)) {
            let quasis: Vec<&str> = it.quasi.quasis.iter().map(|q| q.value.raw.as_str()).collect();
            let arguments = self
                .chained_cwd
                .remove(&it.span)
                .flatten()
                .map(|cwd| vec![ConfigValue::Object(vec![("cwd".to_owned(), cwd)])])
                .unwrap_or_default();
            self.found.push(ImportedCall {
                module,
                export,
                arguments,
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
    found: Vec<Value>,
}

impl<'a> Visit<'a> for OptionFinder<'_, '_, 'a> {
    fn visit_new_expression(&mut self, it: &NewExpression<'a>) {
        let name = match it.callee.get_inner_expression() {
            Expression::Identifier(id) => Some(id.name.as_str()),
            Expression::StaticMemberExpression(member) => Some(member.property.name.as_str()),
            _ => None,
        };
        if name == Some(self.constructor) {
            let eval = Evaluator::in_file(self.top, self.bindings, self.file, HashMap::default());
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

struct MethodCallFinder<'t, 'p, 'a> {
    top: &'t TopLevel<'p, 'a>,
    bindings: &'t ModuleBindings,
    file: &'t str,
    method: &'t str,
    found: Vec<Vec<Value>>,
}

impl<'a> Visit<'a> for MethodCallFinder<'_, '_, 'a> {
    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        if let Expression::StaticMemberExpression(member) = it.callee.get_inner_expression()
            && member.property.name.as_str() == self.method
        {
            let eval = Evaluator::in_file(self.top, self.bindings, self.file, HashMap::default());
            let arguments = it
                .arguments
                .iter()
                .map(|argument| argument.as_expression().map_or(Value::Unknown, |e| eval.expr(e, 0)))
                .collect();
            self.found.push(arguments);
        }
        walk::walk_call_expression(self, it);
    }
}

struct PropertyFinder<'t, 'p, 'a> {
    top: &'t TopLevel<'p, 'a>,
    bindings: &'t ModuleBindings,
    file: &'t str,
    key: &'t str,
    /// When set, the property is a function called with this argument (see [`extract_property_calls`]) rather than a
    /// value.
    call_with: Option<&'t ConfigValue>,
    /// The top-level constants evaluated so far, carried from one property to the next so each is evaluated once.
    evaluated: HashMap<String, Value>,
    /// The names the scopes around the current node declare, innermost last: they shadow top-level bindings.
    shadowed: Vec<Vec<String>>,
    found: Vec<Value>,
}

impl PropertyFinder<'_, '_, '_> {
    /// Visits a scope with `walk`, the names `declared` holds shadowing top-level bindings inside it.
    fn in_scope(&mut self, declared: DeclaredNames, walk: impl FnOnce(&mut Self)) {
        self.shadowed.push(declared.0);
        walk(self);
        self.shadowed.pop();
    }
}

/// The names every binding of a syntax subtree declares.
#[derive(Default)]
struct DeclaredNames(Vec<String>);

impl<'a> Visit<'a> for DeclaredNames {
    fn visit_binding_identifier(&mut self, it: &BindingIdentifier<'a>) {
        self.0.push(it.name.as_str().to_owned());
    }
}

/// What `visit` finds a [`DeclaredNames`] visitor.
fn declared(visit: impl FnOnce(&mut DeclaredNames)) -> DeclaredNames {
    let mut names = DeclaredNames::default();
    visit(&mut names);
    names
}

impl<'a> Visit<'a> for PropertyFinder<'_, '_, 'a> {
    fn visit_function(&mut self, it: &Function<'a>, flags: oxc_semantic::ScopeFlags) {
        let names = declared(|names| names.visit_function(it, flags));
        self.in_scope(names, |finder| walk::walk_function(finder, it, flags));
    }

    fn visit_arrow_function_expression(&mut self, it: &ArrowFunctionExpression<'a>) {
        let names = declared(|names| names.visit_arrow_function_expression(it));
        self.in_scope(names, |finder| walk::walk_arrow_function_expression(finder, it));
    }

    fn visit_block_statement(&mut self, it: &BlockStatement<'a>) {
        let names = declared(|names| names.visit_block_statement(it));
        self.in_scope(names, |finder| walk::walk_block_statement(finder, it));
    }

    fn visit_for_statement(&mut self, it: &ForStatement<'a>) {
        let names = declared(|names| names.visit_for_statement(it));
        self.in_scope(names, |finder| walk::walk_for_statement(finder, it));
    }

    fn visit_for_in_statement(&mut self, it: &ForInStatement<'a>) {
        let names = declared(|names| names.visit_for_in_statement(it));
        self.in_scope(names, |finder| walk::walk_for_in_statement(finder, it));
    }

    fn visit_for_of_statement(&mut self, it: &ForOfStatement<'a>) {
        let names = declared(|names| names.visit_for_of_statement(it));
        self.in_scope(names, |finder| walk::walk_for_of_statement(finder, it));
    }

    fn visit_catch_clause(&mut self, it: &CatchClause<'a>) {
        let names = declared(|names| names.visit_catch_clause(it));
        self.in_scope(names, |finder| walk::walk_catch_clause(finder, it));
    }

    fn visit_object_property(&mut self, it: &ObjectProperty<'a>) {
        let callable = self.call_with.and_then(|_| Callable::of_expression(&it.value));
        let wanted = match self.call_with {
            None => !it.method,
            Some(_) => callable.is_some() && it.kind == oxc_ast::ast::PropertyKind::Init,
        };
        if wanted
            && it
                .key
                .static_name()
                .is_some_and(|name| crate::js_string(name.as_js_str()) == self.key)
        {
            let eval = Evaluator::in_file(self.top, self.bindings, self.file, std::mem::take(&mut self.evaluated));
            let shadowed: Frame<'_, '_> = self
                .shadowed
                .iter()
                .flatten()
                .map(|name| (name.clone(), Binding::Value(Value::Unknown)))
                .collect();
            if !shadowed.is_empty() {
                eval.frames.borrow_mut().push(shadowed);
            }
            let value = match (self.call_with, callable) {
                (Some(argument), Some(callable)) => eval.call_with(callable, Value::from(argument.clone()), 0),
                _ => eval.expr(&it.value, 0),
            };
            self.found.push(value);
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
    // With no [`Unstable`] of the module, no call counts as one that changes nothing it is given.
    let unstable = Unstable::default();
    statements.next().is_some()
        && statements.any(|stmt| {
            let writes = Writes::of_statement(stmt, &unstable);
            writes
                .assigned
                .iter()
                .chain(writes.changed.iter().map(|(name, _)| name))
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
fn node_count(value: &Value) -> usize {
    1 + match value {
        Value::Array(items) | Value::OneOf(items) => items.iter().map(node_count).sum(),
        Value::Object(entries) => entries.iter().map(|(_, item)| node_count(item)).sum(),
        Value::Maybe(value) => node_count(value),
        _ => 0,
    }
}

struct Evaluator<'c, 'p, 'a> {
    consts: &'c HashMap<String, &'p Expression<'a>>,
    /// Top-level names changed by statements after their declaration; see [`TopLevel::changed`].
    changed: &'c HashMap<String, TopChange>,
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
    evaluated: RefCell<HashMap<String, Value>>,
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
    captured: RefCell<Option<Value>>,
    /// The argument values of every call whose arguments were evaluated, by the call's span, as first evaluated.
    arguments_of: RefCell<HashMap<Span, Vec<Value>>>,
}

/// The imports of an evaluator that follows none.
static NO_IMPORTS: LazyLock<HashMap<String, ImportBinding>> = LazyLock::new(HashMap::default);

impl<'c, 'p, 'a> Evaluator<'c, 'p, 'a> {
    /// An evaluator of the module at `path`, named `file` and in the directory `dir` relative to the config file's,
    /// with its top-level bindings, module bindings and imports, following imports with `modules` when given.
    fn new(
        top: &'c TopLevel<'p, 'a>,
        bindings: &'c ModuleBindings,
        imports: &'c HashMap<String, ImportBinding>,
        file: &'c str,
        dir: &'c str,
        path: &'c Path,
        modules: Option<&'c Modules<'c>>,
    ) -> Self {
        Self {
            consts: &top.consts,
            changed: &top.changed,
            unstable: &top.unstable,
            functions: &top.functions,
            bindings,
            file,
            dir,
            path,
            imports,
            modules,
            evaluated: RefCell::new(HashMap::default()),
            chain: RefCell::new(Vec::new()),
            frames: RefCell::new(Vec::new()),
            calls: Cell::new(0),
            config_setter: RefCell::new(None),
            captured: RefCell::new(None),
            arguments_of: RefCell::new(HashMap::default()),
        }
    }

    /// An evaluator of the config file `file` alone, which follows no import; `evaluated` holds the top-level
    /// constants evaluated so far.
    fn in_file(
        top: &'c TopLevel<'p, 'a>,
        bindings: &'c ModuleBindings,
        file: &'c str,
        evaluated: HashMap<String, Value>,
    ) -> Self {
        let eval = Self::new(top, bindings, &NO_IMPORTS, file, ".", Path::new(file), None);
        *eval.evaluated.borrow_mut() = evaluated;
        eval
    }
}

impl<'p, 'a> Evaluator<'_, 'p, 'a> {
    /// The value of what a module exports, `exported`: a config function is the one the tool calls, the config it
    /// hands to `config.set(…)` when it does, and with `changed_later` the value is one later statements change.
    fn exported_value(&self, exported: Exported<'p, 'a>, changed_later: bool) -> Value {
        let callable = match exported {
            Exported::Expression(expr) => Callable::of_expression(expr),
            Exported::Function(function) => Some(Callable::Function(function)),
        };
        *self.config_setter.borrow_mut() = callable.and_then(first_parameter);
        let value = match exported {
            Exported::Expression(expr) => self.expr(expr, 0),
            Exported::Function(function) => self.invoke(Callable::Function(function), None, false, 0),
        };
        let value = if changed_later { changed(value) } else { value };
        self.captured.borrow_mut().take().unwrap_or(value)
    }

    /// The arguments of the calls the evaluator cannot follow that make up what a module exports, `exported`, which
    /// evaluated to `value`; see [`ExtractedConfig::call_arguments`].
    fn exported_call_arguments(&self, exported: Exported<'p, 'a>, value: &Value) -> Vec<Value> {
        match exported {
            Exported::Expression(expr) => self.unknown_call_arguments(expr, value, 0),
            Exported::Function(function) => self.returned_call_arguments(Callable::Function(function), value, 0),
        }
    }

    fn expr(&self, expr: &'p Expression<'a>, depth: u32) -> Value {
        if depth > MAX_DEPTH {
            return Value::Unknown;
        }
        let depth = depth + 1;
        let inner = expr.get_inner_expression();
        match inner {
            Expression::NullLiteral(_) => Value::Null,
            Expression::BooleanLiteral(b) => Value::Bool(b.value),
            Expression::NumericLiteral(n) => Value::Number(n.value),
            Expression::StringLiteral(_) => static_string(inner).map_or(Value::Unknown, Value::String),
            Expression::RegExpLiteral(r) => Value::Regex {
                pattern: r.regex.pattern.text.as_str().to_owned(),
                flags: r.regex.flags.to_string(),
            },
            Expression::TemplateLiteral(template) => self.template(template, depth),
            Expression::BinaryExpression(b) => self.binary(b, depth),
            Expression::StaticMemberExpression(member) => self.member(member, depth),
            Expression::ComputedMemberExpression(member) => self.computed_member(member, depth),
            Expression::ChainExpression(chain) => self.chain(chain, depth),
            Expression::NewExpression(new) => self.url(new, depth),
            Expression::UnaryExpression(u) if u.operator == UnaryOperator::UnaryNegation => {
                match self.expr(&u.argument, depth) {
                    Value::Number(n) => Value::Number(-n),
                    _ => Value::Unknown,
                }
            }
            Expression::UnaryExpression(u) if u.operator == UnaryOperator::LogicalNot => self
                .truthy(&u.argument, depth)
                .map_or(Value::Unknown, |t| Value::Bool(!t)),
            Expression::LogicalExpression(l) => self.logical(l, depth),
            Expression::ConditionalExpression(c) => self.conditional(c, depth),
            Expression::AwaitExpression(a) => self.expr(&a.argument, depth),
            Expression::ArrayExpression(a) => self.array(&a.elements, depth),
            Expression::ObjectExpression(o) => self.object(&o.properties, depth),
            Expression::Identifier(id) => self.reference(id, depth),
            Expression::CallExpression(c) => self.call(c, depth),
            Expression::ArrowFunctionExpression(f) => self.invoke(Callable::Arrow(f), None, false, depth),
            Expression::FunctionExpression(f) => self.invoke(Callable::Function(f), None, false, depth),
            _ => Value::Unknown,
        }
    }

    /// The value the name `id` refers to ([`Self::identifier`]), as code the evaluator does not follow may leave it,
    /// as definite as the value bound to it.
    fn reference(&self, id: &IdentifierReference<'a>, depth: u32) -> Value {
        let value = self.identifier(id.name.as_str(), depth);
        let Some(change) = self.unstable.of_reference(id) else {
            return value;
        };
        let (value, definite) = value.split();
        Value::doubtful(unstable(value, change), definite)
    }

    /// The value of a name: a binding of a function being evaluated, a top-level constant (evaluated once however
    /// often it is referenced), a top-level function called with arguments that cannot be known (as the tool calls a
    /// config function the config exports by name), or a path global. A name bound to a read that is not definite
    /// ([`Self::read`]) is a [`Value::Maybe`].
    fn identifier(&self, name: &str, depth: u32) -> Value {
        if let Some(binding) = self.lookup(name) {
            return match binding {
                Binding::Value(value) | Binding::Alias(Alias { value, .. }) => value,
                Binding::Callable(_) => Value::Unknown,
            };
        }
        let Some(&init) = self.consts.get(name) else {
            return match self.functions.get(name) {
                Some(callable) => self.invoke(*callable, None, true, depth),
                None => match self.import_value(name, None) {
                    Some(value) => {
                        let (value, definite) = value.split();
                        Value::doubtful(self.changed_top(name, value), definite)
                    }
                    None => self.global(name),
                },
            };
        };
        if let Some(known) = self.evaluated.borrow().get(name) {
            return known.clone();
        }
        {
            let chain = self.chain.borrow();
            if chain.len() >= MAX_CONST_CHAIN || chain.iter().any(|active| active == name) {
                return Value::Unknown;
            }
        }
        self.chain.borrow_mut().push(name.to_owned());
        // The value does not depend on where it is referenced from: evaluate it outside any function's bindings and
        // from a fresh depth budget.
        let saved = std::mem::take(&mut *self.frames.borrow_mut());
        let cuts = self.modules.map(|modules| modules.cuts.get());
        let (value, definite) = self.expr(init, 0).split();
        *self.frames.borrow_mut() = saved;
        self.chain.borrow_mut().pop();
        let value = if node_count(&value) > MAX_CONST_NODES {
            Value::Unknown
        } else {
            self.changed_top(name, value)
        };
        let value = Value::doubtful(value, definite);
        // A value an import cycle or budget cut short depends on the order of evaluation; it is computed again.
        if self.modules.map(|modules| modules.cuts.get()) == cuts {
            self.evaluated.borrow_mut().insert(name.to_owned(), value.clone());
        }
        value
    }

    /// `value` of the top-level binding `name`, a constant or an import, as the top-level statements the evaluator does
    /// not run ([`TopLevel::changed`]) and code it does not follow ([`Unstable::of_top`]) may leave it.
    fn changed_top(&self, name: &str, value: Value) -> Value {
        match self.changed.get(name) {
            Some(TopChange::Assigned) => changed(value),
            written => {
                let mut value = value;
                if let Some(TopChange::Written(writes)) = written {
                    for write in writes {
                        edit_at(&mut value, &write.keys, &write.edit());
                    }
                }
                match self.unstable.of_top(name) {
                    Some(change) => unstable(value, change),
                    None => value,
                }
            }
        }
    }

    /// The innermost binding of `name` in the functions being evaluated.
    fn lookup(&self, name: &str) -> Option<Binding<'p, 'a>> {
        self.frames
            .borrow()
            .iter()
            .rev()
            .find_map(|frame| frame.get(name).cloned())
    }

    /// Whether a function being evaluated binds `name`.
    fn is_local(&self, name: &str) -> bool {
        self.frames.borrow().iter().any(|frame| frame.contains_key(name))
    }

    /// Binds `name` in the innermost frame, or replaces the binding where it is already made.
    fn assign(&self, name: &str, binding: Binding<'p, 'a>) {
        let mut frames = self.frames.borrow_mut();
        if let Some(held) = frames.iter_mut().rev().find_map(|frame| frame.get_mut(name)) {
            *held = binding;
        } else if let Some(frame) = frames.last_mut() {
            frame.insert(name.to_owned(), binding);
        }
    }

    /// The path of the module's directory (`__dirname`) or of the module itself (`__filename`).
    fn global(&self, name: &str) -> Value {
        match name {
            "__dirname" => Value::String(normalized(&[self.dir])),
            "__filename" => Value::String(self.file_path()),
            "undefined" => Value::Undefined,
            _ => Value::Unknown,
        }
    }

    /// The module's path relative to the config file's directory.
    fn file_path(&self) -> String {
        normalized(&[self.dir, self.file])
    }

    /// The value an imported name stands for, called with `args` when given. `None` when the name is not imported or
    /// its module is not followed.
    fn import_value(&self, name: &str, args: Option<&[Value]>) -> Option<Value> {
        let binding = self.imports.get(name)?;
        self.modules?
            .evaluate(self.path, &binding.specifier, &binding.imported, args)
    }

    /// The key an object records the unknown value `expr` spreads or merges under: [`OWN_SPREAD`] when the module
    /// computes it itself ([`Self::computes_itself`]), [`UNKNOWN_SPREAD`] otherwise.
    fn spread_key(&self, expr: &Expression<'_>) -> &'static str {
        if self.computes_itself(expr, 0) {
            OWN_SPREAD
        } else {
            UNKNOWN_SPREAD
        }
    }

    /// Whether `expr` calls or refers to a binding the module defines: a binding of a function being evaluated, a
    /// top-level function, or a top-level constant that does not only hold an imported value
    /// ([`Self::takes_imported`]). Members, calls, `await` and the branches of a conditional or logical expression are
    /// looked through; an imported binding, a global and any other expression are not the module's own.
    fn computes_itself(&self, expr: &Expression<'_>, depth: usize) -> bool {
        if depth > MAX_CONST_CHAIN {
            return false;
        }
        let depth = depth + 1;
        match expr.get_inner_expression() {
            Expression::AwaitExpression(e) => self.computes_itself(&e.argument, depth),
            Expression::StaticMemberExpression(member) => self.computes_itself(&member.object, depth),
            Expression::ComputedMemberExpression(member) => self.computes_itself(&member.object, depth),
            Expression::ChainExpression(chain) => {
                chain_object(chain).is_some_and(|object| self.computes_itself(object, depth))
            }
            Expression::CallExpression(call) => self.computes_itself(&call.callee, depth),
            Expression::ConditionalExpression(conditional) => {
                self.computes_itself(&conditional.consequent, depth)
                    || self.computes_itself(&conditional.alternate, depth)
            }
            Expression::LogicalExpression(logical) => {
                self.computes_itself(&logical.left, depth) || self.computes_itself(&logical.right, depth)
            }
            Expression::Identifier(id) => {
                let name = id.name.as_str();
                self.is_local(name)
                    || self.functions.contains_key(name)
                    || self
                        .consts
                        .get(name)
                        .is_some_and(|init| !self.takes_imported(init, depth))
            }
            _ => false,
        }
    }

    /// Whether `expr` only takes a value from another module: an imported binding, a `require(…)`, a member or a call
    /// of one, or a top-level constant holding one (`const base = require('./base')`).
    fn takes_imported(&self, expr: &Expression<'_>, depth: usize) -> bool {
        if depth > MAX_CONST_CHAIN {
            return false;
        }
        let depth = depth + 1;
        match expr.get_inner_expression() {
            Expression::AwaitExpression(e) => self.takes_imported(&e.argument, depth),
            Expression::StaticMemberExpression(member) => self.takes_imported(&member.object, depth),
            Expression::ComputedMemberExpression(member) => self.takes_imported(&member.object, depth),
            Expression::ChainExpression(chain) => {
                chain_object(chain).is_some_and(|object| self.takes_imported(object, depth))
            }
            Expression::CallExpression(call) => {
                let required = matches!(
                    call.callee.get_inner_expression(),
                    Expression::Identifier(id) if id.name.as_str() == "require"
                );
                required || self.takes_imported(&call.callee, depth)
            }
            Expression::Identifier(id) => {
                let name = id.name.as_str();
                self.imports.contains_key(name)
                    || (!self.functions.contains_key(name)
                        && self
                            .consts
                            .get(name)
                            .is_some_and(|init| self.takes_imported(init, depth)))
            }
            _ => false,
        }
    }

    /// A call of an imported function whose module is followed; `None` otherwise, before evaluating any argument.
    fn imported_call(&self, name: &str, call: &'p CallExpression<'a>, depth: u32) -> Option<Value> {
        let binding = self.imports.get(name)?;
        let modules = self.modules?;
        if !modules.follows(self.path, &binding.specifier) {
            return None;
        }
        let args = self.arguments(call, depth);
        modules.evaluate(self.path, &binding.specifier, &binding.imported, Some(&args))
    }

    /// `ns.name` of a namespace import `ns`: the export `name` of its module.
    fn namespace_member(&self, namespace: &str, name: &str) -> Value {
        let (Some(binding), Some(modules)) = (self.imports.get(namespace), self.modules) else {
            return Value::Unknown;
        };
        modules
            .evaluate(self.path, &binding.specifier, &Imported::named(name.to_owned()), None)
            .unwrap_or(Value::Unknown)
    }

    /// Calls the function `name` of this module, declared or imported, with `args`.
    fn call_named(&self, name: &str, args: &[Value]) -> Value {
        match self.functions.get(name) {
            Some(callable) => self.invoke(*callable, Some(args), true, 0),
            None => self.import_value(name, Some(args)).unwrap_or(Value::Unknown),
        }
    }

    /// `module.exports` of the module a `require('./x')` call loads. A package is followed only where its value is the
    /// config (`module.exports = require('@scope/config')`); nested in a config a required package is a name.
    fn required_value(&self, call: &'p CallExpression<'a>, depth: u32) -> Option<Value> {
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

    /// `a + b` of known values ([`add`]), and `a === b` or `a !== b` of two known primitives ([`strictly_equal`]);
    /// any other operator is unknown.
    fn binary(&self, binary: &'p BinaryExpression<'a>, depth: u32) -> Value {
        let equal = match binary.operator {
            BinaryOperator::Addition => None,
            BinaryOperator::StrictEquality => Some(true),
            BinaryOperator::StrictInequality => Some(false),
            _ => return Value::Unknown,
        };
        let (left, right) = (self.expr(&binary.left, depth), self.expr(&binary.right, depth));
        match equal {
            None => each_combination(&[left, right], |pair| match pair {
                [left, right] => add(left, right),
                _ => Value::Unknown,
            }),
            Some(equal) => strictly_equal(&left, &right).map_or(Value::Unknown, |same| Value::Bool(same == equal)),
        }
    }

    /// A template literal whose substitutions are known strings, numbers or booleans ([`primitive_text`]), or one of
    /// several of them ([`each_combination`]).
    fn template(&self, template: &'p TemplateLiteral<'a>, depth: u32) -> Value {
        let mut substitutions = Vec::with_capacity(template.expressions.len());
        for expression in &template.expressions {
            let value = self.expr(expression, depth);
            if !matches!(value, Value::OneOf(_)) && primitive_text(&value).is_none() {
                return Value::Unknown;
            }
            substitutions.push(value);
        }
        each_combination(&substitutions, |texts| {
            let mut out = String::new();
            for (index, quasi) in template.quasis.iter().enumerate() {
                out.push_str(
                    &quasi
                        .value
                        .cooked
                        .map_or_else(|| quasi.value.raw.as_str().to_owned(), crate::js_string),
                );
                if let Some(value) = texts.get(index) {
                    let Some(text) = primitive_text(value) else {
                        return Value::Unknown;
                    };
                    out.push_str(&text);
                }
            }
            Value::String(out)
        })
    }

    /// The value of `expr` without its [`Value::Maybe`], and whether it is definite.
    fn read(&self, expr: &'p Expression<'a>, depth: u32) -> (Value, bool) {
        self.expr(expr, depth).split()
    }

    /// Whether `expr` is truthy; `None` when that is not known, as for a value that is not definite ([`Self::read`]).
    fn truthy(&self, expr: &'p Expression<'a>, depth: u32) -> Option<bool> {
        match self.read(expr, depth) {
            (value, true) => truthiness(&value),
            (_, false) => None,
        }
    }

    /// `import.meta.dirname`, `import.meta.filename`, `import.meta.url`, Bun's `import.meta.dir`, `import.meta.path`
    /// and `import.meta.file` (the file name), the `pathname` of a `new URL(…)`, an export of a namespace import, the
    /// `length` of a known string or array, and a key of a known object ([`property_of`]). The `length` of a value that
    /// is not definite is unknown.
    fn member(&self, member: &'p StaticMemberExpression<'a>, depth: u32) -> Value {
        self.member_of(&member.object, member.property.name.as_str(), depth)
    }

    /// `object[key]` with a key known as a string or a number (`base['setupFiles']`, `base[KEY]` of a constant,
    /// `LIST[0]`), read as `object.key` is; unknown for any other key.
    fn computed_member(&self, member: &'p ComputedMemberExpression<'a>, depth: u32) -> Value {
        self.member_key(&member.expression, depth)
            .map_or(Value::Unknown, |key| self.member_of(&member.object, &key, depth))
    }

    /// The member `property` of `object`, as [`Self::member`] reads it.
    fn member_of(&self, object: &'p Expression<'a>, property: &str, depth: u32) -> Value {
        self.special_member(object, property, depth)
            .unwrap_or_else(|| property_of(&self.expr(object, depth), property))
    }

    /// A member whose value is not a key of the object's value: a path `import.meta` names, the `pathname` of a
    /// `new URL(…)` and an export of a namespace import. `None` for any other member.
    fn special_member(&self, object: &'p Expression<'a>, property: &str, depth: u32) -> Option<Value> {
        Some(match (object.get_inner_expression(), property) {
            (Expression::ImportMeta(_), "dirname" | "dir") => Value::String(normalized(&[self.dir])),
            (Expression::ImportMeta(_), "filename" | "url" | "path") => Value::String(self.file_path()),
            (Expression::ImportMeta(_), "file") => Value::String(self.file.to_owned()),
            (url @ Expression::NewExpression(_), "pathname") => self.expr(url, depth),
            (Expression::Identifier(namespace), _) if self.is_namespace(namespace.name.as_str()) => {
                // What the module changes below the namespace, at the export read, applies to it, as does what code
                // the evaluator does not follow may change where it is read.
                let name = namespace.name.as_str();
                let exports = Value::Object(vec![(property.to_owned(), self.namespace_member(name, property))]);
                let exports = self.changed_top(name, exports);
                let exports = match self.unstable.of_reference(namespace) {
                    Some(change) => unstable(exports, change),
                    None => exports,
                };
                property_of(&exports, property)
            }
            _ => return None,
        })
    }

    /// The key of a computed member when it evaluates to a string, or to a number, as JavaScript writes it
    /// ([`number_text`]): `list[0]` reads the key `"0"`, which [`property_of`] finds in an array.
    fn member_key(&self, key: &'p Expression<'a>, depth: u32) -> Option<String> {
        match self.expr(key, depth) {
            Value::String(key) => Some(key),
            Value::Number(number) => number_text(number),
            _ => None,
        }
    }

    /// An optional chain (`base?.setupFiles`, `base?.['setupFiles']`, `base?.test.include`): its members read as
    /// [`Self::member`] and [`Self::computed_member`] read them, and the chain is `undefined` when the object of an
    /// optional member is `null` or `undefined`, the rest of it unread. A call in the chain is evaluated as any call.
    fn chain(&self, chain: &'p ChainExpression<'a>, depth: u32) -> Value {
        let link = match &chain.expression {
            ChainElement::StaticMemberExpression(m) => {
                self.chain_member(&m.object, Some(m.property.name.as_str().to_owned()), m.optional, depth)
            }
            ChainElement::ComputedMemberExpression(m) => {
                let key = self.member_key(&m.expression, depth);
                self.chain_member(&m.object, key, m.optional, depth)
            }
            ChainElement::CallExpression(call) => Some(self.call(call, depth)),
            ChainElement::TSNonNullExpression(e) => self.chain_link(&e.expression, depth),
            ChainElement::PrivateFieldExpression(_) => Some(Value::Unknown),
        };
        link.unwrap_or(Value::Undefined)
    }

    /// The value of `expr` inside an optional chain; `None` when an optional member in it short-circuits.
    fn chain_link(&self, expr: &'p Expression<'a>, depth: u32) -> Option<Value> {
        match expr.get_inner_expression() {
            Expression::StaticMemberExpression(m) => {
                self.chain_member(&m.object, Some(m.property.name.as_str().to_owned()), m.optional, depth)
            }
            Expression::ComputedMemberExpression(m) => {
                let key = self.member_key(&m.expression, depth);
                self.chain_member(&m.object, key, m.optional, depth)
            }
            _ => Some(self.expr(expr, depth)),
        }
    }

    /// The member `key` of `object` inside an optional chain, unknown when the key is not known: `None` when the
    /// member is `optional` and the object is `null` or `undefined`.
    fn chain_member(
        &self,
        object: &'p Expression<'a>,
        key: Option<String>,
        optional: bool,
        depth: u32,
    ) -> Option<Value> {
        if let Some(special) = key.as_deref().and_then(|key| self.special_member(object, key, depth)) {
            return Some(special);
        }
        let base = self.chain_link(object, depth)?;
        if optional && matches!(base, Value::Null | Value::Undefined) {
            return None;
        }
        Some(key.map_or(Value::Unknown, |key| property_of(&base, &key)))
    }

    /// Whether `name` is a namespace import that no binding of a function being evaluated shadows.
    fn is_namespace(&self, name: &str) -> bool {
        self.imports
            .get(name)
            .is_some_and(|binding| binding.imported == Imported::Namespace)
            && !self.is_local(name)
    }

    /// `a && b`, `a || b` and `a ?? b`, when `a` decides which side is the value: over an `a` that is not definite,
    /// `a ?? b` and `a || b` are the union of both sides ([`any_of`]), not definite either, and `a && b` is unknown;
    /// the side a definite `a` selects is as definite as it is.
    fn logical(&self, logical: &'p LogicalExpression<'a>, depth: u32) -> Value {
        let (left, definite) = self.read(&logical.left, depth);
        if !definite {
            return match logical.operator {
                LogicalOperator::And => Value::Unknown,
                LogicalOperator::Or | LogicalOperator::Coalesce => {
                    let right = self.read(&logical.right, depth).0;
                    Value::doubtful(any_of(&[Some(left), Some(right)]), false)
                }
            };
        }
        match left_is_value(logical.operator, &left) {
            Some(true) => left,
            Some(false) => self.expr(&logical.right, depth),
            None => Value::Unknown,
        }
    }

    /// The values a spread argument may have: one for an expression whose value is known or not decided by a
    /// condition, two for one that is not definite (`undefined` and the value, which it may lack), one per branch
    /// for `test ? a : b`, `a && b`, `a || b` and `a ?? b` whose test is unknown (parenthesised or not), and one per
    /// alternative of a value that is one of several. A left side
    /// that adds nothing when spread is [`ConfigValue::Null`]: the falsy `a` of `a && b`, and a comparison, negation or
    /// `void` on the left of `||` or `??`.
    fn possibilities(&self, expr: &'p Expression<'a>, depth: u32) -> Vec<Value> {
        if depth > MAX_DEPTH {
            return vec![Value::Unknown];
        }
        match expr.get_inner_expression() {
            Expression::LogicalExpression(logical) => {
                let (left, definite) = self.read(&logical.left, depth);
                let primitive = is_primitive(&logical.left);
                let decided = match logical.operator {
                    LogicalOperator::Coalesce if primitive && !is_void(&logical.left) => Some(true),
                    _ if !definite => None,
                    operator => left_is_value(operator, &left),
                };
                // The left side as the value: a falsy `a` of `a && b`, a comparison, a negation and `void` add
                // nothing.
                let left = if primitive || logical.operator == LogicalOperator::And {
                    Value::Null
                } else {
                    left
                };
                match decided {
                    Some(true) => alternatives(left),
                    Some(false) => self.possibilities(&logical.right, depth + 1),
                    None => {
                        let mut out = alternatives(left);
                        out.extend(self.possibilities(&logical.right, depth + 1));
                        out
                    }
                }
            }
            Expression::ConditionalExpression(conditional) => match self.truthy(&conditional.test, depth) {
                Some(true) => self.possibilities(&conditional.consequent, depth + 1),
                Some(false) => self.possibilities(&conditional.alternate, depth + 1),
                None => {
                    let mut out = self.possibilities(&conditional.consequent, depth + 1);
                    out.extend(self.possibilities(&conditional.alternate, depth + 1));
                    out
                }
            },
            _ => match self.expr(expr, depth) {
                Value::Maybe(value) => {
                    let mut out = vec![Value::Undefined];
                    out.extend(alternatives(*value));
                    out
                }
                value => alternatives(value),
            },
        }
    }

    /// Adds to `out` what spreading `argument` adds. A conditional spread whose branches are all known adds every key
    /// a branch sets, with what the branches have in common (see [`any_of`]); a branch that is not known adds the
    /// marker key `...`.
    fn spread(&self, argument: &'p Expression<'a>, out: &mut Vec<(String, Value)>, depth: u32) {
        let branches: Option<Vec<Vec<(String, Value)>>> = self
            .possibilities(argument, depth)
            .into_iter()
            .map(spread_entries)
            .collect();
        match branches {
            None => out.push((self.spread_key(argument).to_owned(), Value::Unknown)),
            Some(mut branches) if branches.len() == 1 => out.extend(branches.pop().unwrap_or_default()),
            Some(branches) => {
                let added = spread_any(out, &branches);
                out.extend(added);
            }
        }
    }

    /// `test ? a : b`: the branch a known test takes, or when the test is unknown what both branches have in common or
    /// one of them ([`either`]).
    fn conditional(&self, conditional: &'p ConditionalExpression<'a>, depth: u32) -> Value {
        match self.truthy(&conditional.test, depth) {
            Some(true) => self.expr(&conditional.consequent, depth),
            Some(false) => self.expr(&conditional.alternate, depth),
            None => either(
                &self.expr(&conditional.consequent, depth),
                &self.expr(&conditional.alternate, depth),
            ),
        }
    }

    /// `new URL('./x', import.meta.url)` as the path it names.
    fn url(&self, new: &'p NewExpression<'a>, depth: u32) -> Value {
        let is_url =
            matches!(new.callee.get_inner_expression(), Expression::Identifier(id) if id.name.as_str() == "URL");
        let mut args = new
            .arguments
            .iter()
            .map(|arg| arg.as_expression().map_or(Value::Unknown, |e| self.expr(e, depth)));
        match (is_url, args.next(), args.next(), args.next()) {
            (true, Some(Value::String(relative)), Some(Value::String(base)), None)
                if !relative.starts_with('/') && !relative.contains(':') && !base.contains(':') =>
            {
                Value::String(normalized(&[base.as_str(), "..", relative.as_str()]))
            }
            _ => Value::Unknown,
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
    fn path_call(&self, call: &'p CallExpression<'a>, depth: u32) -> Option<Value> {
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
                    return Some(Value::String(".".to_owned()));
                } else {
                    return None;
                }
            }
            _ => return None,
        };
        Some(each_combination(&self.arguments(call, depth), |args| {
            apply_path_function(function, args)
        }))
    }

    /// The values of a call's arguments, a spread argument unknown, recorded for [`Self::unknown_call_arguments`].
    fn arguments(&self, call: &'p CallExpression<'a>, depth: u32) -> Vec<Value> {
        let values: Vec<Value> = call
            .arguments
            .iter()
            .map(|arg| arg.as_expression().map_or(Value::Unknown, |e| self.expr(e, depth)))
            .collect();
        self.arguments_of
            .borrow_mut()
            .entry(call.span)
            .or_insert_with(|| values.clone());
        values
    }

    /// The directory of a package's manifest, which is the package itself: `dirname(require.resolve(join(pkg,
    /// 'package.json')))`, the idiom the Storybook CLI writes, also with `import.meta.resolve` for `require.resolve`,
    /// its URL turned into a path with `fileURLToPath`, the package written in several `join` arguments
    /// (`'@scope', 'name'`), and the manifest written as a string (`` `${pkg}/package.json` ``,
    /// `'pkg/package.json'`) instead of the `join`. The directory is classified by [`manifest_directory`]: `None`
    /// leaves a relative path to the path functions.
    fn package_directory(&self, call: &'p CallExpression<'a>, depth: u32) -> Option<Value> {
        if !self.is_path_function(&call.callee, "dirname") {
            return None;
        }
        let mut resolved = single_argument(call)?.get_inner_expression();
        if let Expression::CallExpression(to_path) = resolved
            && self.is_file_url_to_path(&to_path.callee)
        {
            resolved = single_argument(to_path)?.get_inner_expression();
        }
        let Expression::CallExpression(resolve) = resolved else {
            return None;
        };
        if !is_module_resolve(resolve) {
            return None;
        }
        let manifest = single_argument(resolve)?.get_inner_expression();
        if let Expression::CallExpression(join) = manifest
            && self.is_path_function(&join.callee, "join")
        {
            let arguments: Vec<&Expression<'a>> = join
                .arguments
                .iter()
                .map(Argument::as_expression)
                .collect::<Option<_>>()?;
            let (file, segments) = arguments.split_last()?;
            if segments.is_empty() || static_string(file).as_deref() != Some("package.json") {
                return None;
            }
            let segments: Option<Vec<String>> = segments
                .iter()
                .copied()
                .map(|segment| match self.expr(segment, depth) {
                    Value::String(text) => Some(text),
                    _ => None,
                })
                .collect();
            return match segments {
                Some(segments) => manifest_directory(&segments.join("/")),
                None => Some(Value::Unknown),
            };
        }
        let Value::String(manifest) = self.expr(manifest, depth) else {
            return None;
        };
        manifest_directory(manifest.strip_suffix("/package.json")?)
    }

    /// Whether `callee` is `fileURLToPath` of the `url` module, however it was imported.
    fn is_file_url_to_path(&self, callee: &Expression<'_>) -> bool {
        match callee.get_inner_expression() {
            Expression::Identifier(id) => self.bindings.file_url_to_path.contains(id.name.as_str()),
            Expression::StaticMemberExpression(member) => {
                member.property.name.as_str() == "fileURLToPath"
                    && matches!(
                        member.object.get_inner_expression(),
                        Expression::Identifier(object) if self.bindings.url_modules.contains(object.name.as_str())
                    )
            }
            _ => false,
        }
    }

    /// `require.resolve(x)`, `import.meta.resolve(x)` and `Promise.resolve(x)`, which evaluate to `x`.
    fn resolve_call(&self, call: &'p CallExpression<'a>, depth: u32) -> Option<Value> {
        let known = is_module_resolve(call) || is_resolve_of(call, "Promise");
        known.then(|| single_argument(call).map_or(Value::Unknown, |arg| self.expr(arg, depth)))
    }

    /// A call of a glob function as the array of its literal patterns.
    fn glob_call(&self, call: &'p CallExpression<'a>, depth: u32) -> Option<Value> {
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
            Some(patterns) => match self.read(patterns, depth).0 {
                Value::String(pattern) => Value::Array(vec![Value::String(pattern)]),
                array @ Value::Array(_) => array,
                _ => Value::Unknown,
            },
            None => Value::Unknown,
        })
    }

    /// `defineConfig(a, [b, c], ...d)` of `eslint/config` and `tseslint.config(…)`: the flat config list of all their
    /// arguments, nested lists flattened.
    fn flat_config_call(&self, call: &'p CallExpression<'a>, depth: u32) -> Option<Value> {
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
                other => other.as_expression().map_or(Value::Unknown, |e| self.expr(e, depth)),
            };
            flatten_into(value, &mut list);
        }
        Some(Value::Array(list))
    }

    /// A call of a top-level function or of a function declared inside the function being evaluated.
    fn local_call(&self, call: &'p CallExpression<'a>, depth: u32) -> Option<Value> {
        let Expression::Identifier(id) = call.callee.get_inner_expression() else {
            return None;
        };
        let name = id.name.as_str();
        let (callable, isolate) = match self.local_function(name) {
            Some(LocalBinding::Function(callable)) => (callable, false),
            Some(LocalBinding::Other) => return None,
            None => match self.functions.get(name) {
                Some(callable) => (*callable, true),
                None => return self.imported_call(name, call, depth),
            },
        };
        Some(self.invoke(callable, Some(self.arguments(call, depth).as_slice()), isolate, depth))
    }

    /// The innermost binding of `name` in the functions being evaluated; `None` when no function being evaluated binds
    /// it. The binding's kind decides, so a value bound to the name is not cloned to find out.
    fn local_function(&self, name: &str) -> Option<LocalBinding<'p, 'a>> {
        self.frames.borrow().iter().rev().find_map(|frame| {
            frame.get(name).map(|binding| match binding {
                Binding::Callable(callable) => LocalBinding::Function(*callable),
                Binding::Value(_) | Binding::Alias(_) => LocalBinding::Other,
            })
        })
    }

    /// The function a callback argument `expr` is, and whether a call of it is isolated from the bindings of the
    /// functions being evaluated: an inline function, which sees them, a function one of them declares, or a top-level
    /// function. `None` for anything else, such as an imported function.
    fn callback(&self, expr: &'p Expression<'a>) -> Option<(Callable<'p, 'a>, bool)> {
        if let Some(callable) = Callable::of_expression(expr) {
            return Some((callable, false));
        }
        let Expression::Identifier(id) = expr.get_inner_expression() else {
            return None;
        };
        let name = id.name.as_str();
        match self.local_function(name) {
            Some(LocalBinding::Function(callable)) => Some((callable, false)),
            Some(LocalBinding::Other) => None,
            None => self.functions.get(name).map(|callable| (*callable, true)),
        }
    }

    /// `list.map(f)`, `list.flatMap(f)` and `list.filter(f)` of a known array with a callback the evaluator can call
    /// ([`Self::callback`]): `f` is called with each element and its index (the array itself is not known to it),
    /// within the call budget. An unknown element, which may stand for a spread of any length, is not passed to `f` and
    /// stays unknown, and the indexes after it are unknown. `flatMap` adds the elements of an array `f` returns, or the
    /// value itself; `filter` keeps the elements `f` is truthy for, and one it may be truthy for, which leaves the result
    /// not definite. A result is not definite when the array is not. `None` for any other call, and when the array or
    /// the callback is not known.
    fn array_callback_call(&self, call: &'p CallExpression<'a>, depth: u32) -> Option<Value> {
        let Expression::StaticMemberExpression(member) = call.callee.get_inner_expression() else {
            return None;
        };
        let method = member.property.name.as_str();
        if !ARRAY_CALLBACK_METHODS.contains(&method) {
            return None;
        }
        let (callable, isolate) = self.callback(call.arguments.first()?.as_expression()?)?;
        let (Value::Array(items), mut definite) = self.read(&member.object, depth) else {
            return None;
        };
        let mut out = Vec::new();
        let mut indexed = true;
        for (index, item) in items.into_iter().enumerate() {
            if item == Value::Unknown {
                indexed = false;
                out.push(Value::Unknown);
                continue;
            }
            let index = u32::try_from(index)
                .ok()
                .filter(|_| indexed)
                .map_or(Value::Unknown, |index| Value::Number(f64::from(index)));
            let args = [item.clone(), index, Value::Unknown];
            let (result, result_definite) = self.invoke(callable, Some(args.as_slice()), isolate, depth).split();
            match method {
                "map" => out.push(Value::doubtful(result, result_definite)),
                "flatMap" => match result {
                    Value::Array(listed) => {
                        out.extend(listed);
                        definite &= result_definite;
                    }
                    Value::OneOf(_) => out.push(Value::Unknown),
                    other => out.push(Value::doubtful(other, result_definite)),
                },
                _ => match truthiness(&result).filter(|_| result_definite) {
                    Some(false) => {}
                    Some(true) => out.push(item),
                    None => {
                        definite = false;
                        out.push(item);
                    }
                },
            }
        }
        Some(Value::doubtful(Value::Array(out), definite))
    }

    /// `Object.fromEntries(list)` of a known list of `[key, value]` pairs: the object they make, a later pair for a key
    /// replacing the value of an earlier one, and a key that is a number or a boolean written as JavaScript writes it
    /// ([`primitive_text`]). An element that is not such a pair with a known key may add any key, recorded under the
    /// marker key `list` spreads under ([`Self::spread_key`]), before the pairs after it. The keys are not definite
    /// when the list is not. Unknown for any other argument; `None` for any other call, and for an `Object` the module
    /// binds.
    fn object_from_entries(&self, call: &'p CallExpression<'a>, depth: u32) -> Option<Value> {
        let Expression::StaticMemberExpression(member) = call.callee.get_inner_expression() else {
            return None;
        };
        let on_object = matches!(
            member.object.get_inner_expression(),
            Expression::Identifier(object) if object.name.as_str() == "Object"
        );
        if !on_object || member.property.name.as_str() != "fromEntries" || self.binds("Object") {
            return None;
        }
        let Some(argument) = single_argument(call) else {
            return Some(Value::Unknown);
        };
        let (Value::Array(pairs), definite) = self.read(argument, depth) else {
            return Some(Value::Unknown);
        };
        let mut entries: Vec<(String, Value)> = Vec::new();
        for pair in pairs {
            let Some((key, value)) = entry_pair(pair) else {
                let marker = self.spread_key(argument);
                if entries.last().is_none_or(|(last, _)| last != marker) {
                    entries.push((marker.to_owned(), Value::Unknown));
                }
                continue;
            };
            let value = Value::doubtful(value, definite);
            let found = entries
                .iter()
                .rposition(|(name, _)| *name == key || ConfigValue::is_spread_key(name))
                .filter(|&at| entries[at].0 == key);
            match found {
                Some(at) => entries[at].1 = value,
                None => entries.push((key, value)),
            }
        }
        Some(Value::Object(entries))
    }

    /// A call of a config combinator (`mergeConfig`, `defu`, `Object.assign`, `deepmerge`, `merge`): the merge of the
    /// object arguments. An argument that cannot be known contributes a `...` entry, so the known part still applies.
    /// A merge given options that may say how it merges (`deepmerge`'s third argument, `mergeWith`'s customizer) is
    /// unknown.
    fn merge_call(&self, call: &'p CallExpression<'a>, depth: u32) -> Option<Value> {
        let style = self.merge_style(call)?;
        if takes_options(&call.callee, call.arguments.len()) {
            // The arguments are still recorded, for the files they name ([`Self::unknown_call_arguments`]).
            self.arguments(call, depth);
            return Some(Value::Unknown);
        }
        let mut merged: Option<Vec<(String, Value)>> = None;
        let expressions = call.arguments.iter().map(Argument::as_expression);
        let mut values: Vec<_> = self.arguments(call, depth).into_iter().zip(expressions).collect();
        if style == MergeStyle::FirstWins {
            values.reverse();
        }
        for (value, expression) in values {
            let (value, definite) = value.split();
            match value {
                Value::Null | Value::Undefined | Value::Bool(_) => {}
                Value::Object(entries) => {
                    // An argument that may be absent may set each of its keys or not.
                    let entries = entries
                        .into_iter()
                        .map(|(key, value)| (key, Value::doubtful(value, definite)))
                        .collect();
                    let base = merged.get_or_insert_with(Vec::new);
                    merge_objects(base, entries, style != MergeStyle::Shallow);
                }
                _ => {
                    let key = expression.map_or(UNKNOWN_SPREAD, |expression| self.spread_key(expression));
                    merge_objects(
                        merged.get_or_insert_with(Vec::new),
                        vec![(key.to_owned(), Value::Unknown)],
                        false,
                    );
                }
            }
        }
        Some(merged.map_or(Value::Unknown, Value::Object))
    }

    /// `list.filter(Boolean)` of a known array: its elements that may be truthy. An element that may be falsy is kept
    /// as the values it has when it is not (`ci ? 'github' : null` as `'github'`, and in an array literal `ci &&
    /// 'github'` as `'github'`), and the array is then not definite, as its length is not.
    fn filter_call(&self, call: &'p CallExpression<'a>, depth: u32) -> Option<Value> {
        let Expression::StaticMemberExpression(member) = call.callee.get_inner_expression() else {
            return None;
        };
        let by_boolean = matches!(
            single_argument(call).map(Expression::get_inner_expression),
            Some(Expression::Identifier(id))
                if id.name.as_str() == "Boolean" && !self.binds("Boolean")
        );
        if member.property.name.as_str() != "filter" || !by_boolean {
            return None;
        }
        let (items, mut definite) = match member.object.get_inner_expression() {
            Expression::ArrayExpression(array) => self.filtered_elements(&array.elements, depth),
            object => match self.read(object, depth) {
                (Value::Array(items), definite) => (items, definite),
                _ => return None,
            },
        };
        let mut kept = Vec::new();
        for item in items {
            if let Some((value, may_drop)) = truthy_part(item) {
                definite &= !may_drop;
                kept.push(value);
            }
        }
        Some(Value::doubtful(Value::Array(kept), definite))
    }

    /// Whether the module or a function being evaluated binds `name` itself, shadowing the global.
    fn binds(&self, name: &str) -> bool {
        self.is_local(name)
            || self.consts.contains_key(name)
            || self.functions.contains_key(name)
            || self.imports.contains_key(name)
    }

    /// The elements of an array literal that `.filter(Boolean)` filters, and whether they are definite: as
    /// [`Self::array`] has them, except that `test && x` with an unknown test, which is dropped when it is falsy, is
    /// `x` and leaves them not definite.
    fn filtered_elements(&self, elements: &'p [ArrayExpressionElement<'a>], depth: u32) -> (Vec<Value>, bool) {
        let mut items = Vec::new();
        let mut definite = true;
        for element in elements {
            if let Some(Expression::LogicalExpression(and)) =
                element.as_expression().map(Expression::get_inner_expression)
                && and.operator == LogicalOperator::And
                && self.truthy(&and.left, depth).is_none()
            {
                items.push(self.expr(&and.right, depth));
                definite = false;
                continue;
            }
            let (part, part_definite) = self.array(std::slice::from_ref(element), depth).split();
            if let Value::Array(part) = part {
                items.extend(part);
            }
            definite &= part_definite;
        }
        (items, definite)
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

    fn call(&self, call: &'p CallExpression<'a>, depth: u32) -> Value {
        if self.capture_set(call, depth) {
            return Value::Unknown;
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
            .or_else(|| self.merge_call(call, depth))
            .or_else(|| self.filter_call(call, depth))
            .or_else(|| self.array_callback_call(call, depth))
            .or_else(|| self.object_from_entries(call, depth));
        if let Some(value) = special {
            return value;
        }
        self.defined(call)
            .map_or(Value::Unknown, |first| self.expr(first, depth))
    }

    /// What a `define…` helper is given ([`defined`]) when no code of the project is behind it
    /// ([`Unstable::calls_foreign`]): a helper of a module of the project or of a workspace package, or a method of
    /// an object the module declares (`const { defineProperty } = Object`), may return anything.
    fn defined(&self, call: &'p CallExpression<'a>) -> Option<&'p Expression<'a>> {
        defined(call).filter(|_| self.unstable.calls_foreign(call))
    }

    /// How `call` merges its arguments ([`merge_style`]) when no code of the project is behind it
    /// ([`Unstable::calls_foreign`]): a `merge` of a module of the project may merge any way.
    fn merge_style(&self, call: &CallExpression<'_>) -> Option<MergeStyle> {
        merge_style(&call.callee, self.bindings).filter(|_| self.unstable.calls_foreign(call))
    }

    /// The arguments of the calls the evaluator cannot follow whose results `expr`, which evaluated to `value`, is or
    /// merges: a call that evaluated to unknown, looking through `await`, a `define*` call to the expression it wraps,
    /// a top-level constant to its initialiser, a function the tool calls to the expression it returns
    /// ([`Self::returned_call_arguments`]), a merge (`mergeConfig`, `Object.assign`) to each of its arguments, and an
    /// object or array literal to what it holds ([`Self::nested_call_arguments`]). The values are those the evaluation
    /// computed; arguments it did not evaluate are evaluated now.
    fn unknown_call_arguments(&self, expr: &'p Expression<'a>, value: &Value, depth: u32) -> Vec<Value> {
        if depth > MAX_DEPTH {
            return Vec::new();
        }
        let depth = depth + 1;
        match expr.get_inner_expression() {
            Expression::AwaitExpression(a) => self.unknown_call_arguments(&a.argument, value, depth),
            Expression::CallExpression(call) => self.unknown_arguments_of_call(call, value, depth),
            Expression::Identifier(id) => self.constant_call_arguments(id.name.as_str(), value, depth),
            Expression::ArrowFunctionExpression(f) => self.returned_call_arguments(Callable::Arrow(f), value, depth),
            Expression::FunctionExpression(f) => self.returned_call_arguments(Callable::Function(f), value, depth),
            Expression::ObjectExpression(object) => self.object_call_arguments(&object.properties, value, depth),
            Expression::ArrayExpression(array) => self.array_call_arguments(&array.elements, value, depth),
            _ => Vec::new(),
        }
    }

    /// [`Self::unknown_call_arguments`] of `expr`, which evaluated to `value`, inside an object or array literal: of
    /// a call, `await` looked through, and of the object and array literals it nests. A name is not looked up there,
    /// so constants that refer to each other are not walked once per reference.
    fn nested_call_arguments(&self, expr: &'p Expression<'a>, value: &Value, depth: u32) -> Vec<Value> {
        if depth > MAX_DEPTH {
            return Vec::new();
        }
        let depth = depth + 1;
        match expr.get_inner_expression() {
            Expression::AwaitExpression(a) => self.nested_call_arguments(&a.argument, value, depth),
            Expression::CallExpression(call) => self.unknown_arguments_of_call(call, value, depth),
            Expression::ObjectExpression(object) => self.object_call_arguments(&object.properties, value, depth),
            Expression::ArrayExpression(array) => self.array_call_arguments(&array.elements, value, depth),
            _ => Vec::new(),
        }
    }

    /// The arguments of the calls the evaluator cannot follow in the keys of an object literal that evaluated to
    /// `value`, each put in an object under the key it is found at (`{ test: <argument> }`), so a file named under an
    /// exclusion key (`coverage: { exclude: makeList(['./a.ts']) }`) is still found under it. A spread counts as
    /// unknown only when the object has an unknown spread.
    fn object_call_arguments(&self, properties: &'p [ObjectPropertyKind<'a>], value: &Value, depth: u32) -> Vec<Value> {
        let object = value.definite();
        let spread = if object.get(UNKNOWN_SPREAD).is_some() || object.get(OWN_SPREAD).is_some() {
            Value::Unknown
        } else {
            Value::Null
        };
        let mut out = Vec::new();
        for property in properties {
            match property {
                ObjectPropertyKind::ObjectProperty(p) => {
                    let Some(key) = p.key.static_name() else {
                        continue;
                    };
                    let key = crate::js_string(key.as_js_str());
                    let field = object.get(&key).cloned().unwrap_or(Value::Unknown);
                    out.extend(
                        self.nested_call_arguments(&p.value, &field, depth)
                            .into_iter()
                            .map(|argument| Value::Object(vec![(key.clone(), argument)])),
                    );
                }
                ObjectPropertyKind::SpreadProperty(s) => {
                    out.extend(self.nested_call_arguments(&s.argument, &spread, depth));
                }
            }
        }
        out
    }

    /// The arguments of the calls the evaluator cannot follow in the elements of an array literal that evaluated to
    /// `value`, up to its first spread, after which an element's value is not known by its position.
    fn array_call_arguments(
        &self,
        elements: &'p [ArrayExpressionElement<'a>],
        value: &Value,
        depth: u32,
    ) -> Vec<Value> {
        let unknown = Value::Unknown;
        let items: &[Value] = match value.definite() {
            Value::Array(items) => items,
            _ => &[],
        };
        elements
            .iter()
            .take_while(|element| !matches!(element, ArrayExpressionElement::SpreadElement(_)))
            .enumerate()
            .filter_map(|(index, element)| Some((element.as_expression()?, items.get(index).unwrap_or(&unknown))))
            .flat_map(|(element, item)| self.nested_call_arguments(element, item, depth))
            .collect()
    }

    /// [`Self::unknown_call_arguments`] of `call`.
    fn unknown_arguments_of_call(&self, call: &'p CallExpression<'a>, value: &Value, depth: u32) -> Vec<Value> {
        if let Some(first) = self.defined(call) {
            return self.unknown_call_arguments(first, value, depth);
        }
        let recorded = self.arguments_of.borrow().get(&call.span).cloned();
        if self.merge_style(call).is_some() {
            let values = recorded.unwrap_or_default();
            return call
                .arguments
                .iter()
                .zip(&values)
                .filter_map(|(argument, value)| Some((argument.as_expression()?, value)))
                .flat_map(|(argument, value)| self.unknown_call_arguments(argument, value, depth))
                .collect();
        }
        if !matches!(value.definite(), Value::Unknown | Value::OneOf(_)) {
            return Vec::new();
        }
        recorded.unwrap_or_else(|| self.arguments(call, depth))
    }

    /// [`Self::unknown_call_arguments`] of the top-level constant `name`, outside any function's bindings as it is
    /// evaluated; none for any other name.
    fn constant_call_arguments(&self, name: &str, value: &Value, depth: u32) -> Vec<Value> {
        if self.is_local(name) || self.functions.contains_key(name) {
            return Vec::new();
        }
        let Some(&init) = self.consts.get(name) else {
            return Vec::new();
        };
        let saved = std::mem::take(&mut *self.frames.borrow_mut());
        let arguments = self.unknown_call_arguments(init, value, depth);
        *self.frames.borrow_mut() = saved;
        arguments
    }

    /// [`Self::unknown_call_arguments`] of what a function the tool calls returns, `value`, when its body is one
    /// expression or a single `return`: its parameters are unknown there, as when the evaluation called it. Any other
    /// body has none.
    fn returned_call_arguments(&self, callable: Callable<'p, 'a>, value: &Value, depth: u32) -> Vec<Value> {
        let (params, body, statements) = match callable {
            Callable::Arrow(f) => (&f.params, f.get_expression(), f.get_function_body()),
            Callable::Function(f) => (&f.params, None, f.body.as_deref()),
        };
        let returned = body.or_else(|| match statements?.statements.as_slice() {
            [only] => single_return(only)?.argument.as_ref(),
            _ => None,
        });
        let Some(returned) = returned else {
            return Vec::new();
        };
        self.frames.borrow_mut().push(Frame::default());
        for param in &params.items {
            self.bind_pattern(&param.pattern, Value::Unknown, depth);
        }
        if let Some(param) = &params.rest {
            self.bind_pattern(&param.rest.argument, Value::Unknown, depth);
        }
        let arguments = self.unknown_call_arguments(returned, value, depth);
        self.frames.borrow_mut().pop();
        arguments
    }

    /// Calls `callable` with `args`, or with arguments that cannot be known when it is `None`: a function the tool calls
    /// (an exported config function, the callback of `defineConfig`) is given what the config does not say, so each of
    /// its parameters is unknown, a default included. With `isolate`, the function cannot see the bindings of the
    /// function evaluating it.
    fn invoke(&self, callable: Callable<'p, 'a>, args: Option<&[Value]>, isolate: bool, depth: u32) -> Value {
        if self.calls.get() >= MAX_CALLS {
            return Value::Unknown;
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
        self.frames.borrow_mut().push(Frame::default());
        for (index, param) in params.items.iter().enumerate() {
            let value = match (args.map(|args| args.get(index)), &param.initializer) {
                (Some(Some(value)), Some(default)) => self.defaulted(value.clone(), default, depth),
                (Some(Some(value)), None) => value.clone(),
                (Some(None), Some(default)) => self.expr(default, depth),
                (None | Some(None), _) => Value::Unknown,
            };
            self.bind_pattern(&param.pattern, value, depth);
        }
        if let Some(param) = &params.rest {
            self.bind_pattern(&param.rest.argument, Value::Unknown, depth);
        }
        let value = match callable {
            Callable::Arrow(f) => match f.get_expression() {
                Some(body) => self.expr(body, depth),
                None => f
                    .get_function_body()
                    .map_or(Value::Unknown, |b| self.returned(&b.statements, depth)),
            },
            Callable::Function(f) => f
                .body
                .as_ref()
                .map_or(Value::Unknown, |b| self.returned(&b.statements, depth)),
        };
        self.frames.borrow_mut().pop();
        if isolate {
            *self.frames.borrow_mut() = saved;
        }
        value
    }

    /// Calls `callable` with `argument` as its first parameter: the value it returns or, when a block body finishes
    /// without a `return`, the value it leaves that parameter with.
    fn call_with(&self, callable: Callable<'p, 'a>, argument: Value, depth: u32) -> Value {
        let statements = match callable {
            Callable::Arrow(f) => f.get_function_body().map(|body| &body.statements),
            Callable::Function(f) => f.body.as_ref().map(|body| &body.statements),
        };
        let (Some(name), Some(statements)) = (first_parameter(callable), statements) else {
            return self.invoke(callable, Some(std::slice::from_ref(&argument)), false, depth);
        };
        let params = match callable {
            Callable::Function(f) => &f.params,
            Callable::Arrow(f) => &f.params,
        };
        self.frames.borrow_mut().push(Frame::default());
        self.bind(&name, argument);
        for param in params.items.iter().skip(1) {
            self.bind_pattern(&param.pattern, Value::Unknown, depth);
        }
        if let Some(param) = &params.rest {
            self.bind_pattern(&param.rest.argument, Value::Unknown, depth);
        }
        let returned = self.run(statements, depth);
        let frame = self.frames.borrow_mut().pop();
        returned.unwrap_or_else(|| match frame.and_then(|mut frame| frame.remove(&name)) {
            Some(Binding::Value(value)) => value,
            _ => Value::Unknown,
        })
    }

    /// Binds the names `pattern` declares to the parts of `value` they take, in the innermost frame. A property of an
    /// object pattern takes its key's value as a member read does ([`property_of`]), with its default applied
    /// ([`Self::defaulted`]); a name that cannot be known (an array pattern, a rest element, a key an unknown spread
    /// may provide) is bound to [`ConfigValue::Unknown`], so it does not resolve to a binding of an outer scope.
    fn bind_pattern(&self, pattern: &'p BindingPattern<'a>, value: Value, depth: u32) {
        match pattern {
            BindingPattern::BindingIdentifier(id) => self.bind(id.name.as_str(), value),
            BindingPattern::AssignmentPattern(p) => {
                let value = self.defaulted(value, &p.right, depth);
                self.bind_pattern(&p.left, value, depth);
            }
            BindingPattern::ObjectPattern(object) => {
                for property in &object.properties {
                    let field = property.key.static_name().map_or(Value::Unknown, |key| {
                        property_of(&value, &crate::js_string(key.as_js_str()))
                    });
                    self.bind_pattern(&property.value, field, depth);
                }
                if let Some(rest) = &object.rest {
                    self.bind_pattern(&rest.argument, Value::Unknown, depth);
                }
            }
            BindingPattern::ArrayPattern(_) => {
                for id in pattern.get_binding_identifiers() {
                    self.bind(id.name.as_str(), Value::Unknown);
                }
            }
        }
    }

    /// `value` with the `default` of a parameter or a destructuring applied: the default for `undefined`, the union of
    /// the value and the default, not definite, for a value that is not definite and may be absent, and the value
    /// itself otherwise, `null` included.
    fn defaulted(&self, value: Value, default: &'p Expression<'a>, depth: u32) -> Value {
        match value {
            Value::Undefined => self.expr(default, depth),
            Value::Maybe(value) => {
                let default = self.read(default, depth).0;
                Value::doubtful(any_of(&[Some(*value), Some(default)]), false)
            }
            value => value,
        }
    }

    /// Binds `name` to `value` in the innermost frame.
    fn bind(&self, name: &str, value: Value) {
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
    fn returned(&self, statements: &'p [Statement<'a>], depth: u32) -> Value {
        if let [Statement::ExpressionStatement(s)] = statements {
            return self.expr(&s.expression, depth);
        }
        self.run(statements, depth).unwrap_or(Value::Unknown)
    }

    /// Runs `statements` in the innermost frame: the value of the `return` they reach, or `None` when they finish
    /// without one. An `if` with a known test runs the branch it takes; with an unknown test, a branch that is a single
    /// `return x` makes the value what `x` and the other path's value have in common, and any other `return` that may
    /// or may not be reached (in a loop, a `switch`, a `try`, a larger branch) makes the value unknown, never only the
    /// last `return`. A statement that may or may not run (a branch of an unknown test, a loop) leaves every local it
    /// assigns or changes unknown, so the value after it is never the one from before it.
    fn run(&self, statements: &'p [Statement<'a>], depth: u32) -> Option<Value> {
        for (index, statement) in statements.iter().enumerate() {
            match statement {
                Statement::ReturnStatement(r) => {
                    // What the evaluator reads expressions for, it does not run for what they change.
                    if let Some(argument) = &r.argument {
                        self.havoc(Writes::of_expression(argument, self.unstable));
                    }
                    return Some(r.argument.as_ref().map_or(Value::Unknown, |e| self.expr(e, depth)));
                }
                Statement::ExpressionStatement(s) => self.mutation(&s.expression, depth),
                Statement::BlockStatement(b) => {
                    if let Some(value) = self.block(&b.body, depth) {
                        return Some(value);
                    }
                }
                Statement::IfStatement(i) => {
                    self.havoc(Writes::of_expression(&i.test, self.unstable));
                    let taken = match self.truthy(&i.test, depth) {
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
                other if returns(other) => return Some(Value::Unknown),
                other => match other.as_declaration() {
                    Some(Declaration::VariableDeclaration(v)) => {
                        self.havoc(Writes::of_statement(other, self.unstable));
                        self.declare(v, depth);
                    }
                    Some(Declaration::FunctionDeclaration(f)) => {
                        if let Some(id) = &f.id {
                            self.bind_to(id.name.as_str(), Binding::Callable(Callable::Function(f)));
                        }
                    }
                    Some(_) => {}
                    None => self.havoc(Writes::of_statement(other, self.unstable)),
                },
            }
        }
        None
    }

    /// Runs the statements of a block in a frame of their own, so a `const` or `let` declared in it does not replace a
    /// binding outside it. A `var` it declares is function-scoped, and is left unknown outside the block.
    fn block(&self, statements: &'p [Statement<'a>], depth: u32) -> Option<Value> {
        self.frames.borrow_mut().push(Frame::default());
        let value = self.run(statements, depth);
        self.frames.borrow_mut().pop();
        let mut vars = Vars::default();
        for statement in statements {
            vars.visit_statement(statement);
        }
        for name in vars.0 {
            self.assign(&name, Binding::Value(Value::Unknown));
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
            // Each name once: joining a binding with a branch again is not the same as joining it once.
            let mut names: Vec<String> = frame.keys().chain(other.keys()).cloned().collect();
            names.sort_unstable();
            names.dedup();
            for name in names {
                let joined = match (frame.get(&name), other.get(&name)) {
                    (Some(Binding::Value(a)), Some(Binding::Value(b))) => Binding::Value(join(a, b)),
                    (Some(Binding::Alias(a)), Some(Binding::Alias(b))) if a.same_part(b) => Binding::Alias(Alias {
                        value: join(&a.value, &b.value),
                        ..a.clone()
                    }),
                    (Some(Binding::Callable(a)), Some(Binding::Callable(b))) if a.same(*b) => Binding::Callable(*a),
                    _ => Binding::Value(Value::Unknown),
                };
                frame.insert(name, joined);
            }
        }
        let mut captured = self.captured.borrow_mut();
        if *captured != consequent_captured {
            *captured = Some(match (&*captured, &consequent_captured) {
                (Some(a), Some(b)) => either(a, b),
                _ => Value::Unknown,
            });
        }
    }

    /// The value of an `if` with an unknown test followed by `rest`, where a branch returns: what a branch's single
    /// `return x` and the other path's value have in common, or unknown when a branch does more. A branch that goes on
    /// to `rest` leaves what it assigns unknown there.
    fn either_return(&self, statement: &'p IfStatement<'a>, rest: &'p [Statement<'a>], depth: u32) -> Value {
        let path = |branch: Option<&'p Statement<'a>>| -> Option<Value> {
            match branch.map(single_return) {
                // A branch that returns its value.
                Some(Some(r)) => Some(r.argument.as_ref().map_or(Value::Unknown, |e| self.expr(e, depth))),
                // A branch that does more than return, and may return.
                Some(None) if branch.is_some_and(returns) => Some(Value::Unknown),
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
                    self.havoc(Writes::of_statement(branch, self.unstable));
                }
            }
            self.run(rest, depth).unwrap_or(Value::Unknown)
        };
        let (a, b) = match (consequent, alternate) {
            (Some(a), Some(b)) => (a, b),
            (Some(a), None) => (a, after()),
            (None, Some(b)) => (after(), b),
            (None, None) => return Value::Unknown,
        };
        either(&a, &b)
    }

    /// Binds the names a `const`/`let`/`var` declaration declares in the innermost frame: a name to the value, as
    /// definite as it is, and a destructured name to the part of the value it takes, or to [`ConfigValue::Unknown`].
    /// A name that is an alias the evaluator follows is bound as one ([`Binding::Alias`]).
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
                .map_or(Value::Unknown, |init| self.expr(init, depth));
            let value = if node_count(&value) > MAX_CONST_NODES {
                Value::Unknown
            } else {
                value
            };
            let aliases = self.aliases(declaration.kind, declarator);
            self.bind_pattern(&declarator.id, value, depth);
            for (name, alias) in aliases {
                self.bind_alias(&name, alias);
            }
        }
    }

    /// The aliases a declaration makes that the evaluator follows ([`Unstable::follows`]), by name, before they are
    /// bound: each takes the part of the local the declaration reads ([`alias_source`]) at the key it destructures.
    fn aliases(&self, kind: VariableDeclarationKind, declarator: &'p VariableDeclarator<'a>) -> Vec<(String, Alias)> {
        if kind != VariableDeclarationKind::Const {
            return Vec::new();
        }
        let Some((root, keys)) = declarator.init.as_ref().and_then(|init| alias_source(init)) else {
            return Vec::new();
        };
        let root = root.name.as_str();
        let Some(frame) = self.frames.borrow().iter().rposition(|frame| frame.contains_key(root)) else {
            return Vec::new();
        };
        alias_names(&declarator.id)
            .unwrap_or_default()
            .into_iter()
            .filter(|(id, _)| self.unstable.follows(id))
            .map(|(id, key)| {
                let mut keys = keys.clone();
                keys.extend(key);
                let alias = Alias {
                    value: Value::Unknown,
                    root: root.to_owned(),
                    frame,
                    keys,
                };
                (id.name.as_str().to_owned(), alias)
            })
            .collect()
    }

    /// Makes the binding `name` of the innermost frame `alias`, holding the value it is bound to.
    fn bind_alias(&self, name: &str, mut alias: Alias) {
        let mut frames = self.frames.borrow_mut();
        if let Some(binding) = frames.last_mut().and_then(|frame| frame.get_mut(name))
            && let Binding::Value(value) = &mut *binding
        {
            alias.value = std::mem::replace(value, Value::Unknown);
            *binding = Binding::Alias(alias);
        }
    }

    /// Leaves every local `writes` assigns unknown, and every part of a local object or array it changes partly
    /// unknown.
    fn havoc(&self, writes: Writes<'_>) {
        for name in writes.assigned {
            self.assign(&name, Binding::Value(Value::Unknown));
        }
        let mut changed = writes.changed;
        changed.sort_unstable();
        changed.dedup();
        for (name, write) in changed {
            self.edit(&name, &write.keys, &write.edit());
        }
    }

    /// Applies what an expression statement does to a local object: `name = x`, `name.a.key = x`, `name.list.push(x)`
    /// and `name.list.unshift(x)`; anything else that reaches into the object (`name.list.splice(0, 1)`,
    /// `name.list[i] = x`) leaves the part it reaches partly unknown. Any other expression leaves what it may write
    /// unknown.
    fn mutation(&self, expr: &'p Expression<'a>, depth: u32) {
        match expr.get_inner_expression() {
            Expression::AwaitExpression(a) => self.mutation(&a.argument, depth),
            Expression::AssignmentExpression(a) => {
                self.havoc(Writes::of_expression(&a.right, self.unstable));
                let assign = a.operator == AssignmentOperator::Assign;
                match &a.left {
                    AssignmentTarget::AssignmentTargetIdentifier(id) => {
                        let value = if assign {
                            self.expr(&a.right, depth)
                        } else {
                            Value::Unknown
                        };
                        self.assign(id.name.as_str(), Binding::Value(value));
                    }
                    AssignmentTarget::StaticMemberExpression(m) => match reach(&m.object) {
                        Some((root, keys, true)) => {
                            let value = if assign {
                                self.expr(&a.right, depth)
                            } else {
                                Value::Unknown
                            };
                            let edit = Edit::Set(m.property.name.as_str().to_owned(), value);
                            self.edit(root.name.as_str(), &keys, &edit);
                        }
                        Some((root, keys, false)) => self.edit(root.name.as_str(), &keys, &Edit::Taint),
                        // An object that is not a name or a member chain (`list.find(…).loader = …`) may be any part
                        // of a local it may hold.
                        None => {
                            let mut writes = Writes::new(self.unstable);
                            writes.visit_assignment_target(&a.left);
                            self.havoc(writes.through_aliases());
                        }
                    },
                    _ => self.havoc(Writes::of_expression(expr, self.unstable)),
                }
            }
            Expression::CallExpression(c) => {
                if !self.capture_set(c, depth) && !self.insert(c, depth) {
                    self.havoc(Writes::of_effect(expr, self.unstable));
                }
            }
            // `cond && (x = 1)`, `x++`, `a = b = 1`: what it writes may or may not be written.
            _ => self.havoc(Writes::of_expression(expr, self.unstable)),
        }
    }

    /// Applies `name.list.push(…)` or `name.list.unshift(…)` of a local list, given no spread, to the list; returns
    /// whether `call` is such a call. What is known to be a list only adds what it is given; anything else may have a
    /// method of that name that changes it.
    fn insert(&self, call: &'p CallExpression<'a>, depth: u32) -> bool {
        let Expression::StaticMemberExpression(member) = call.callee.get_inner_expression() else {
            return false;
        };
        let front = match member.property.name.as_str() {
            "push" => false,
            "unshift" => true,
            _ => return false,
        };
        let Some((root, keys, true)) = reach(&member.object) else {
            return false;
        };
        let Some(arguments) = call
            .arguments
            .iter()
            .map(Argument::as_expression)
            .collect::<Option<Vec<_>>>()
        else {
            return false;
        };
        if !self.is_list(root.name.as_str(), &keys) {
            return false;
        }
        for argument in &arguments {
            self.havoc(Writes::of_expression(argument, self.unstable));
        }
        let items = arguments
            .into_iter()
            .map(|argument| self.expr(argument, depth))
            .collect();
        self.edit(root.name.as_str(), &keys, &Edit::Insert { front, items });
        true
    }

    /// Whether the innermost local `name` holds a list at `keys`, in every value it may be.
    fn is_list(&self, name: &str, keys: &[String]) -> bool {
        let frames = self.frames.borrow();
        let Some(binding) = frames.iter().rev().find_map(|frame| frame.get(name)) else {
            return false;
        };
        match binding {
            Binding::Value(value) | Binding::Alias(Alias { value, .. }) => {
                value_at(value.definite(), keys).is_some_and(holds_list)
            }
            Binding::Callable(_) => false,
        }
    }

    /// Applies `edit` at `keys` below the innermost local `name`, and through it when it is an alias
    /// ([`edit_binding`]).
    fn edit(&self, name: &str, keys: &[String], edit: &Edit) {
        let mut frames = self.frames.borrow_mut();
        if let Some(index) = frames.iter().rposition(|frame| frame.contains_key(name)) {
            edit_binding(&mut frames, index, name, keys, edit);
        }
    }

    /// An array literal: a spread adds the elements [`Self::spread_elements`] finds. A spread of an array that is not
    /// definite leaves the array not definite, as its length and elements are not.
    fn array(&self, elements: &'p [ArrayExpressionElement<'a>], depth: u32) -> Value {
        let mut out = Vec::new();
        let mut definite = true;
        for element in elements {
            match element {
                ArrayExpressionElement::SpreadElement(s) => {
                    let (items, spread_definite) = self.spread_elements(&s.argument, depth);
                    out.extend(items);
                    definite &= spread_definite;
                }
                other => out.push(other.as_expression().map_or(Value::Unknown, |e| self.expr(e, depth))),
            }
        }
        Value::doubtful(Value::Array(out), definite)
    }

    /// The elements spreading `argument` into an array adds, and whether they are definite: those of a known array;
    /// for a value with several possibilities ([`Self::possibilities`]: `...(ci ? [a] : [])`, `...(ci && [a])`, a
    /// value that is one of several arrays or not definite), the elements of each, a possibility of `null`,
    /// `undefined` or a boolean adding none, which are then not definite; one unknown element for anything else.
    fn spread_elements(&self, argument: &'p Expression<'a>, depth: u32) -> (Vec<Value>, bool) {
        let unknown = || (vec![Value::Unknown], true);
        let mut branches = self.possibilities(argument, depth);
        if branches.len() == 1 {
            return match branches.pop() {
                Some(Value::Array(items)) => (items, true),
                _ => unknown(),
            };
        }
        let mut items: Vec<Value> = Vec::new();
        for branch in branches {
            match branch {
                Value::Array(listed) => {
                    for item in listed {
                        if !items.contains(&item) {
                            items.push(item);
                        }
                    }
                }
                Value::Null | Value::Undefined | Value::Bool(_) => {}
                _ => return unknown(),
            }
        }
        (items, false)
    }

    fn object(&self, properties: &'p [ObjectPropertyKind<'a>], depth: u32) -> Value {
        let mut out: Vec<(String, Value)> = Vec::new();
        for property in properties {
            match property {
                ObjectPropertyKind::SpreadProperty(s) => self.spread(&s.argument, &mut out, depth),
                ObjectPropertyKind::ObjectProperty(p) => {
                    let Some(key) = p.key.static_name() else {
                        out.push((UNKNOWN_SPREAD.to_owned(), Value::Unknown));
                        continue;
                    };
                    let value = if p.method || p.kind != oxc_ast::ast::PropertyKind::Init {
                        Value::Unknown
                    } else {
                        self.expr(&p.value, depth)
                    };
                    out.push((crate::js_string(key.as_js_str()), value));
                }
            }
        }
        Value::Object(out)
    }
}

/// Applies `edit` at `keys` below the binding `name` of the frame at `index`. Through an alias it applies to the part
/// of the local the alias takes as well, when that part still holds what the alias held, and leaves the part partly
/// unknown when it does not (the local was given another object there).
fn edit_binding(frames: &mut [Frame<'_, '_>], index: usize, name: &str, keys: &[String], edit: &Edit) {
    let Some(binding) = frames.get_mut(index).and_then(|frame| frame.get_mut(name)) else {
        return;
    };
    let (root, frame, part, before) = match binding {
        Binding::Value(value) => return edit_at(value, keys, edit),
        Binding::Callable(_) => return,
        Binding::Alias(alias) => {
            let before = alias.value.clone();
            edit_at(&mut alias.value, keys, edit);
            (alias.root.clone(), alias.frame, alias.keys.clone(), before)
        }
    };
    let held = frames
        .get(frame)
        .and_then(|bindings| bindings.get(&root))
        .and_then(|binding| match binding {
            Binding::Value(value) | Binding::Alias(Alias { value, .. }) => value_at(value, &part),
            Binding::Callable(_) => None,
        });
    if held == Some(&before) {
        let mut path = part;
        path.extend_from_slice(keys);
        edit_binding(frames, frame, &root, &path, edit);
    } else {
        edit_binding(frames, frame, &root, &part, &Edit::Taint);
    }
}

/// Applies `edit` to the object at `keys` below `value`, in each value it may be ([`Value::OneOf`]). A key on the way
/// that an unknown spread may set, or that the object may inherit ([`inherits`], such as `__proto__` and
/// `constructor`), leaves the object above it partly unknown instead. One the object neither has nor inherits leaves
/// it as it is: the key is `undefined`, so reaching below it throws and changes nothing.
fn edit_at(value: &mut Value, keys: &[String], edit: &Edit) {
    let target = value.definite_mut();
    if let Value::OneOf(alternatives) = &mut *target {
        for alternative in alternatives {
            edit_at(alternative, keys, edit);
        }
        return;
    }
    let Some((key, rest)) = keys.split_first() else {
        return match edit {
            Edit::Set(key, field) => set_entry(target, key, field.clone()),
            Edit::Insert { front, items } => insert_items(target, *front, items),
            Edit::Taint => taint_value(target),
        };
    };
    match target {
        Value::Object(entries) => {
            let found = entries
                .iter()
                .rposition(|(name, _)| name == key || ConfigValue::is_spread_key(name));
            match found {
                Some(at) if entries[at].0 == *key => edit_at(&mut entries[at].1, rest, edit),
                // An unknown spread may have set the key.
                Some(_) => open_entries(entries),
                None if inherits(entries, key) => open_entries(entries),
                None => {}
            }
        }
        other => taint_value(other),
    }
}

/// Sets `key` of an object to `field`; on an array (`list.length = 0`) the elements are no longer known.
fn set_entry(value: &mut Value, key: &str, field: Value) {
    match value.definite_mut() {
        Value::Object(entries) => match entries.iter_mut().find(|(existing, _)| existing == key) {
            Some((_, slot)) => *slot = field,
            None => entries.push((key.to_owned(), field)),
        },
        Value::Array(items) => items.push(Value::Unknown),
        _ => {}
    }
}

/// Whether a value is a list, in every value it may be.
fn holds_list(value: &Value) -> bool {
    match value.definite() {
        Value::Array(_) => true,
        Value::OneOf(alternatives) => alternatives.iter().all(holds_list),
        _ => false,
    }
}

/// Adds `items` to a list, at its start with `front`; anything else is left partly unknown.
fn insert_items(value: &mut Value, front: bool, items: &[Value]) {
    match value {
        Value::Array(list) if front => {
            list.splice(0..0, items.iter().cloned());
        }
        Value::Array(list) => list.extend(items.iter().cloned()),
        other => taint_value(other),
    }
}

/// Leaves an object or array partly unknown: an object may have any other key, and an array any other element and
/// any change below each of its elements, as a change of what it holds (`rules[0].loader = …`) does not say which
/// element it reaches.
fn taint_value(value: &mut Value) {
    match value.definite_mut() {
        Value::Object(entries) => open_entries(entries),
        Value::Array(items) => {
            items.iter_mut().for_each(open_below);
            if !matches!(items.last(), Some(Value::Unknown)) {
                items.push(Value::Unknown);
            }
        }
        _ => {}
    }
}

/// Leaves every object and array in a value open, at every depth, in each value it may be.
fn open_below(value: &mut Value) {
    match value {
        Value::Object(entries) => {
            for (_, field) in entries.iter_mut() {
                open_below(field);
            }
            open_entries(entries);
        }
        Value::Array(items) => {
            items.iter_mut().for_each(open_below);
            if !matches!(items.last(), Some(Value::Unknown)) {
                items.push(Value::Unknown);
            }
        }
        Value::OneOf(alternatives) => alternatives.iter_mut().for_each(open_below),
        Value::Maybe(inner) => open_below(inner),
        _ => {}
    }
}

/// Leaves the entries of an object open to any other key, once: a marker that already ends them says as much.
fn open_entries(entries: &mut Vec<(String, Value)>) {
    if entries.last().is_none_or(|(key, _)| key != UNKNOWN_SPREAD) {
        entries.push((UNKNOWN_SPREAD.to_owned(), Value::Unknown));
    }
}

/// The value at `keys` below `value`, as member reads find it; `None` where a key is missing or an unknown spread may
/// set it.
fn value_at<'v>(value: &'v Value, keys: &[String]) -> Option<&'v Value> {
    let Some((key, rest)) = keys.split_first() else {
        return Some(value);
    };
    let Value::Object(entries) = value else {
        return None;
    };
    entries
        .iter()
        .rev()
        .find(|(name, _)| name == key || ConfigValue::is_spread_key(name))
        .filter(|(name, _)| name == key)
        .and_then(|(_, child)| value_at(child, rest))
}

/// The first argument of a call of a function named `define*` (`defineConfig`, `vitest.defineProject`), which
/// evaluates to that argument; `None` for any other call, among them a method of `Object` or `Reflect`
/// (`Object.defineProperty`), which changes that argument.
pub(crate) fn defined<'p, 'a>(call: &'p CallExpression<'a>) -> Option<&'p Expression<'a>> {
    let callee = match call.callee.get_inner_expression() {
        Expression::Identifier(id) => id.name.as_str(),
        Expression::StaticMemberExpression(m) if !on_reflection(&m.object) => m.property.name.as_str(),
        _ => return None,
    };
    let is_define = callee.starts_with("define") && callee.len() > "define".len();
    call.arguments
        .first()
        .and_then(Argument::as_expression)
        .filter(|_| is_define)
}

/// Whether a method is called on `Object` or `Reflect`, whose `define…` methods change what they are given.
fn on_reflection(object: &Expression<'_>) -> bool {
    matches!(
        object.get_inner_expression(),
        Expression::Identifier(id) if matches!(id.name.as_str(), "Object" | "Reflect")
    )
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

/// Whether a merge called with `count` arguments is given options that may say how it merges: a third argument of
/// `deepmerge`, and any argument of `mergeWith`, whose last one may be a function that merges each key.
fn takes_options(callee: &Expression<'_>, count: usize) -> bool {
    let name = match callee.get_inner_expression() {
        Expression::Identifier(id) => id.name.as_str(),
        Expression::StaticMemberExpression(member) => member.property.name.as_str(),
        _ => return false,
    };
    match name {
        "deepmerge" | "deepMerge" => count > 2,
        "mergeWith" => true,
        _ => false,
    }
}

/// Merges `incoming` into `base`; with `deep`, an object meeting an object merges and an array meeting an array
/// concatenates, otherwise the incoming value replaces the earlier one in place ([`merged_value`]).
fn merge_objects(base: &mut Vec<(String, Value)>, incoming: Vec<(String, Value)>, deep: bool) {
    for (key, value) in incoming {
        let Some(index) = base.iter().position(|(k, _)| *k == key) else {
            base.push((key, value));
            continue;
        };
        if ConfigValue::is_spread_key(&key) {
            continue;
        }
        let slot = &mut base[index].1;
        *slot = merged_value(std::mem::replace(slot, Value::Unknown), value, deep);
    }
}

/// What a key holds after a merge ([`merge_objects`]) meets its `current` value with an `incoming` one: with `deep`,
/// the merge of two objects or two lists, or of one and an unknown value, definite only when both sides are;
/// otherwise the incoming value, or when it is not definite and so may be absent, the union of both ([`any_of`]),
/// not definite either.
fn merged_value(current: Value, incoming: Value, deep: bool) -> Value {
    let (current, current_definite) = current.split();
    let (incoming, incoming_definite) = incoming.split();
    let merged = match (deep, current, incoming) {
        (true, Value::Object(mut left), Value::Object(right)) => {
            merge_objects(&mut left, right, true);
            Value::Object(left)
        }
        (true, Value::Array(mut left), Value::Array(right)) => {
            left.extend(right);
            Value::Array(left)
        }
        // A deep merge keeps what the known side has and whatever the unknown side may add to it.
        (true, Value::Object(mut known), Value::Unknown | Value::OneOf(_))
        | (true, Value::Unknown | Value::OneOf(_), Value::Object(mut known)) => {
            if !known.iter().any(|(k, _)| ConfigValue::is_spread_key(k)) {
                known.push((UNKNOWN_SPREAD.to_owned(), Value::Unknown));
            }
            Value::Object(known)
        }
        (true, Value::Array(mut known), Value::Unknown | Value::OneOf(_))
        | (true, Value::Unknown | Value::OneOf(_), Value::Array(mut known)) => {
            known.push(Value::Unknown);
            Value::Array(known)
        }
        (_, current, incoming) if !incoming_definite => {
            return Value::doubtful(any_of(&[Some(current), Some(incoming)]), false);
        }
        (_, _, incoming) => return incoming,
    };
    Value::doubtful(merged, current_definite && incoming_definite)
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
/// (`x = …`, `x++`, `[x] = …`) and the parts of objects it may change (`x.a = …`, `x.list.push(…)`,
/// `Object.assign(x.part, …)`), among them the whole local an alias declared there takes a part of when the alias is
/// changed.
struct Writes<'u> {
    /// The bindings code the evaluator does not follow may change, which also say what a call is ([`Unstable`]).
    unstable: &'u Unstable,
    assigned: Vec<String>,
    changed: Vec<(String, Write)>,
    /// The aliases declared ([`alias_source`]), each with the local it takes a part of.
    aliases: Vec<(String, String)>,
}

impl Write {
    /// The edit that leaves what the change may have changed unknown.
    fn edit(&self) -> Edit {
        self.key
            .clone()
            .map_or(Edit::Taint, |key| Edit::Set(key, Value::Unknown))
    }
}

impl<'u> Writes<'u> {
    fn new(unstable: &'u Unstable) -> Self {
        Self {
            unstable,
            assigned: Vec::new(),
            changed: Vec::new(),
            aliases: Vec::new(),
        }
    }

    fn of_statement(statement: &Statement<'_>, unstable: &'u Unstable) -> Self {
        let mut writes = Self::new(unstable);
        writes.visit_statement(statement);
        writes.through_aliases()
    }

    fn of_expression(expression: &Expression<'_>, unstable: &'u Unstable) -> Self {
        let mut writes = Self::new(unstable);
        writes.visit_expression(expression);
        writes.through_aliases()
    }

    /// The writes with the local each changed alias takes a part of changed too, in full, as the evaluator, which
    /// does not run the code, cannot apply them to it.
    fn through_aliases(mut self) -> Self {
        let mut index = 0;
        while let Some((name, _)) = self.changed.get(index).cloned() {
            for (alias, root) in &self.aliases {
                let whole = |(changed, write): &(String, Write)| changed == root && *write == Write::default();
                if *alias == name && !self.changed.iter().any(whole) {
                    self.changed.push((root.clone(), Write::default()));
                }
            }
            index += 1;
        }
        self
    }

    /// What an expression statement writes itself, as [`Evaluator::mutation`] reads it: the target of an assignment
    /// and what its value may change, and what a call made as a statement may change ([`Self::arguments`]); anything
    /// else, in full.
    fn of_effect(expression: &Expression<'_>, unstable: &'u Unstable) -> Self {
        let mut writes = Self::new(unstable);
        match expression.get_inner_expression() {
            Expression::AssignmentExpression(a) => {
                match a.left.as_simple_assignment_target() {
                    Some(target) => writes.visit_simple_assignment_target(target),
                    None => writes.visit_assignment_target(&a.left),
                }
                writes.visit_expression(&a.right);
            }
            Expression::CallExpression(c) => {
                writes.visit_call_expression(c);
                writes.arguments(c.span, &c.arguments, true);
            }
            Expression::AwaitExpression(a) => return Self::of_effect(&a.argument, unstable),
            other => writes.visit_expression(other),
        }
        writes
    }

    /// Records a change of what the object `expr` holds: the part of a local a name or a member chain is, or each part
    /// any other object may be ([`Unstable::parts_of`]).
    fn object(&mut self, expr: &Expression<'_>) {
        if let Some((root, keys, _)) = reach(expr) {
            return self.change(root.name.as_str(), Write { keys, key: None });
        }
        let unstable = self.unstable;
        for (name, keys, _) in unstable.parts_of(expr) {
            self.change(
                name,
                Write {
                    keys: keys.clone(),
                    key: None,
                },
            );
        }
    }

    /// Records the change `write` of the local `name`, once.
    fn change(&mut self, name: &str, write: Write) {
        let change = (name.to_owned(), write);
        if !self.changed.contains(&change) {
            self.changed.push(change);
        }
    }

    /// The objects the call at `span`, made as a statement or not (`statement`), is given, which it may change
    /// ([`Unstable::changes_arguments`]): each part of a local an argument gives it, a literal's included
    /// ([`given_parts`]), where the change may be read after the call ([`Unstable::seen_after`]).
    fn arguments(&mut self, span: Span, arguments: &[Argument<'_>], statement: bool) {
        if !self.unstable.changes_arguments(span, statement) {
            return;
        }
        for argument in arguments.iter().filter_map(given) {
            for (root, keys) in given_parts(argument) {
                if self.unstable.seen_after(root) {
                    self.change(root.name.as_str(), Write { keys, key: None });
                }
            }
        }
    }
}

impl<'a> Visit<'a> for Writes<'_> {
    fn visit_variable_declarator(&mut self, it: &VariableDeclarator<'a>) {
        if let Some((root, _)) = it.init.as_ref().and_then(|init| alias_source(init)) {
            for (id, _) in alias_names(&it.id).unwrap_or_default() {
                self.aliases
                    .push((id.name.as_str().to_owned(), root.name.as_str().to_owned()));
            }
        }
        walk::walk_variable_declarator(self, it);
    }

    fn visit_simple_assignment_target(&mut self, it: &SimpleAssignmentTarget<'a>) {
        match it {
            SimpleAssignmentTarget::AssignmentTargetIdentifier(id) => self.assigned.push(id.name.as_str().to_owned()),
            SimpleAssignmentTarget::StaticMemberExpression(m) => match reach(&m.object) {
                // `x.a.key = …` sets one key of the object at `x.a`.
                Some((root, keys, true)) => {
                    let key = Some(m.property.name.as_str().to_owned());
                    self.change(root.name.as_str(), Write { keys, key });
                }
                Some(_) => self.object(&m.object),
                // `(a ? x.a : x.b).key = …` sets the key of each part of a local the object is.
                None => {
                    let unstable = self.unstable;
                    for (name, keys, exact) in unstable.parts_of(&m.object) {
                        let key = exact.then(|| m.property.name.as_str().to_owned());
                        self.change(
                            name,
                            Write {
                                keys: keys.clone(),
                                key,
                            },
                        );
                    }
                }
            },
            other => {
                if let Some(member) = other.as_member_expression() {
                    self.object(member.object());
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
            && let Some(member) = it.argument.get_inner_expression().as_member_expression()
        {
            self.object(member.object());
        }
        walk::walk_unary_expression(self, it);
    }

    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        if let Expression::Identifier(callee) = it.callee.get_inner_expression()
            && self.unstable.calls_value(it)
        {
            self.change(callee.name.as_str(), Write::default());
        }
        if let Some(target) = changed_by_call(it, self.unstable.is_pure(it), self.unstable.calls_back(it))
            && !self.unstable.is_namespace(target)
        {
            match (property_defined(it), reach(target)) {
                (Some(key), Some((root, keys, true))) => {
                    self.change(root.name.as_str(), Write { keys, key: Some(key) });
                }
                _ => self.object(target),
            }
        }
        self.arguments(it.span, &it.arguments, false);
        walk::walk_call_expression(self, it);
    }

    fn visit_new_expression(&mut self, it: &NewExpression<'a>) {
        self.arguments(it.span, &it.arguments, false);
        walk::walk_new_expression(self, it);
    }

    fn visit_expression_statement(&mut self, it: &ExpressionStatement<'a>) {
        if let Some(call) = statement_call(&it.expression) {
            self.arguments(call.span, &call.arguments, true);
        }
        walk::walk_expression_statement(self, it);
    }

    fn visit_with_statement(&mut self, it: &oxc_ast::ast::WithStatement<'a>) {
        // A name the body writes may be a key of the object.
        self.object(&it.object);
        walk::walk_with_statement(self, it);
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
/// any other element, as [`Evaluator::edit`] leaves them partly unknown; anything else is unknown.
fn changed(value: Value) -> Value {
    match value {
        Value::Object(mut entries) => {
            open_entries(&mut entries);
            Value::Object(entries)
        }
        Value::Array(mut items) => {
            items.push(Value::Unknown);
            Value::Array(items)
        }
        Value::Maybe(value) => Value::Maybe(Box::new(changed(*value))),
        _ => Value::Unknown,
    }
}

/// A value code the evaluator does not follow may have changed as `change` says. A change of contents leaves only the
/// parts it reaches partly unknown, as [`Evaluator::edit`] does: an object or array there may hold more, a key given a
/// new value is unknown, and a value that holds no object or array is as it is, in each alternative of a
/// [`Value::OneOf`] on its own, so a binding that only may share an object with a changed one keeps the values it may
/// be.
fn unstable(value: Value, change: &Change) -> Value {
    let Change::Contents(writes) = change else {
        return changed(value);
    };
    let mut value = value;
    for write in writes {
        edit_at(&mut value, &write.keys, &write.edit());
    }
    value
}

/// Whether the left side of `left <operator> right` is the value of the expression; `None` when that is not known.
fn left_is_value(operator: LogicalOperator, left: &Value) -> Option<bool> {
    match operator {
        LogicalOperator::And => truthiness(left).map(|truthy| !truthy),
        LogicalOperator::Or => truthiness(left),
        LogicalOperator::Coalesce => match left {
            Value::Unknown | Value::OneOf(_) | Value::Maybe(_) => None,
            Value::Null | Value::Undefined => Some(false),
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

/// The entries spreading `value` adds to an object: those of an object, none for `null`, `undefined`, a boolean, a
/// number or an empty string, and `None` for anything else, whose entries are not known.
fn spread_entries(value: Value) -> Option<Vec<(String, Value)>> {
    match value {
        Value::Object(entries) => Some(entries),
        Value::Null | Value::Undefined | Value::Bool(_) | Value::Number(_) => Some(Vec::new()),
        Value::String(s) => s.is_empty().then(Vec::new),
        Value::Array(_) | Value::Regex { .. } | Value::Unknown | Value::OneOf(_) | Value::Maybe(_) => None,
    }
}

/// The value of `key` in the entries of an object: the last entry for it, a [`Value::Maybe`] when that is not
/// definite, and unknown when an unknown spread follows it; `None` when the object has neither.
fn entry_value(entries: &[(String, Value)], key: &str) -> Option<Value> {
    entries
        .iter()
        .rev()
        .find(|(name, _)| name == key || ConfigValue::is_spread_key(name))
        .map(|(name, value)| if name == key { value.clone() } else { Value::Unknown })
}

/// The `length` of a string or an array; unknown for an array with an unknown element, which may stand for a spread of
/// any length.
fn length_of(value: &Value) -> Value {
    let length = match value {
        Value::String(text) => Some(text.encode_utf16().count()),
        Value::Array(items) if !items.contains(&Value::Unknown) => Some(items.len()),
        _ => None,
    };
    length
        .and_then(|length| u32::try_from(length).ok())
        .map_or(Value::Unknown, |length| Value::Number(f64::from(length)))
}

/// `value.key`, as a member read or a destructuring takes it: the `length` of a string or an array; the value of `key`
/// in a known object ([`entry_value`]), not definite when the key or the object is not; unknown for the `length` of a
/// value that is not definite, a key the object may inherit ([`inherits`]) and a member of anything else; and
/// [`Value::Undefined`] when the object has neither the key nor an unknown spread.
fn property_of(value: &Value, key: &str) -> Value {
    let definite = value.is_definite();
    match (value.definite(), key) {
        (_, "length") if !definite => Value::Unknown,
        (known @ (Value::String(_) | Value::Array(_)), "length") => length_of(known),
        (Value::Object(entries), _) => match entry_value(entries, key) {
            Some(field) => Value::doubtful(field, definite),
            None if inherits(entries, key) => Value::Unknown,
            None => Value::Undefined,
        },
        (Value::Array(items), _) if definite => {
            array_index(key).map_or(Value::Unknown, |index| element_at(items, index))
        }
        _ => Value::Unknown,
    }
}

/// The array index a property key names: a canonical non-negative integer (`"0"`, `"12"`, not `"01"` or `"+1"`).
fn array_index(key: &str) -> Option<usize> {
    let index: u32 = key.parse().ok()?;
    (index.to_string() == key)
        .then(|| usize::try_from(index).ok())
        .flatten()
}

/// The element at `index` of a known array: unknown at or after an unknown element, which may stand for a spread of any
/// length, and `undefined` past the end.
fn element_at(items: &[Value], index: usize) -> Value {
    if items
        .iter()
        .take(index.saturating_add(1))
        .any(|item| *item == Value::Unknown)
    {
        return Value::Unknown;
    }
    items.get(index).cloned().unwrap_or(Value::Undefined)
}

/// The key and value of a `[key, value]` pair `Object.fromEntries` takes: a known array whose first element is a known
/// string, number or boolean ([`primitive_text`]), the value `undefined` when the pair has no second element. `None`
/// for anything else.
fn entry_pair(pair: Value) -> Option<(String, Value)> {
    let Value::Array(parts) = pair else {
        return None;
    };
    let mut parts = parts.into_iter();
    let key = primitive_text(&parts.next()?)?;
    Some((key, parts.next().unwrap_or(Value::Undefined)))
}

/// The text JavaScript makes of a known string, number ([`number_text`]) or boolean in a template literal, a `+` with a
/// string or a property key; `None` for anything else, `null` and `undefined` included, which a path or name written
/// from them is unlikely to mean.
fn primitive_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => number_text(*number),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// How JavaScript writes the number `number`, when it writes it without an exponent (zero, or a magnitude from
/// `1e-6` up to `1e21`), where Rust writes the same shortest digits; `None` for any other number, `NaN` and the
/// infinities included.
fn number_text(number: f64) -> Option<String> {
    if number == 0.0 {
        // `-0` is written `0` too.
        return Some("0".to_owned());
    }
    (1e-6..1e21).contains(&number.abs()).then(|| format!("{number}"))
}

/// `left + right` of known values: the sum of two numbers, or the text of both ([`primitive_text`]) when one is a
/// string; unknown otherwise.
fn add(left: &Value, right: &Value) -> Value {
    match (left, right) {
        (Value::Number(a), Value::Number(b)) => Value::Number(a + b),
        (Value::String(_), _) | (_, Value::String(_)) => match (primitive_text(left), primitive_text(right)) {
            (Some(a), Some(b)) => Value::String(a + &b),
            _ => Value::Unknown,
        },
        _ => Value::Unknown,
    }
}

/// Whether `left === right` for two known primitives; `None` when either is anything else, such as an object or an
/// array, which compare by identity, or a value that is not known or not definite.
fn strictly_equal(left: &Value, right: &Value) -> Option<bool> {
    let primitive = |value: &Value| {
        matches!(
            value,
            Value::Null | Value::Undefined | Value::Bool(_) | Value::Number(_) | Value::String(_)
        )
    };
    (primitive(left) && primitive(right)).then(|| left == right)
}

/// Whether an object with `entries` may inherit `key` it does not have itself: a member of `Object.prototype`, or any
/// key when the object sets its prototype (`{ __proto__: base }`), which the evaluator does not follow.
fn inherits(entries: &[(String, Value)], key: &str) -> bool {
    OBJECT_PROTOTYPE_MEMBERS.contains(&key) || entries.iter().any(|(name, _)| name == "__proto__")
}

/// The entries a spread of one of `branches` adds after `before`; see [`any_entries`].
fn spread_any(before: &[(String, Value)], branches: &[Vec<(String, Value)>]) -> Vec<(String, Value)> {
    let objects: Vec<Option<&[(String, Value)]>> = branches.iter().map(|branch| Some(branch.as_slice())).collect();
    any_entries(&objects, before)
}

/// The entries of an object that is one of `objects`, `None` standing for a branch without the object: each key any
/// of them has, with [`any_of`] what each leaves it (its own value, or the one in `before` when it does not have the
/// key), a [`Value::Maybe`] unless every branch leaves it the same definite value, and the marker key `...` last when
/// one of them has an unknown spread.
fn any_entries(objects: &[Option<&[(String, Value)]>], before: &[(String, Value)]) -> Vec<(String, Value)> {
    let entries = || objects.iter().flatten().copied().flatten();
    let mut keys: Vec<&str> = Vec::new();
    for (key, _) in entries() {
        if !ConfigValue::is_spread_key(key) && !keys.contains(&key.as_str()) {
            keys.push(key);
        }
    }
    let mut out: Vec<(String, Value)> = Vec::new();
    for key in keys {
        let fields: Vec<Option<(Value, bool)>> = objects
            .iter()
            .map(|object| {
                object
                    .and_then(|object| entry_value(object, key))
                    .or_else(|| entry_value(before, key))
                    .map(Value::split)
            })
            .collect();
        let each_definite = fields
            .iter()
            .all(|field| field.as_ref().is_some_and(|(_, definite)| *definite));
        let values: Vec<Option<Value>> = fields.into_iter().map(|field| field.map(|(value, _)| value)).collect();
        let definite = each_definite && agree(&values);
        out.push((key.to_owned(), Value::doubtful(any_of(&values), definite)));
    }
    for marker in [UNKNOWN_SPREAD, OWN_SPREAD] {
        if entries().any(|(key, _)| key == marker) {
            out.push((marker.to_owned(), Value::Unknown));
        }
    }
    out
}

/// The value of a key that holds one of `values`, `None` standing for a branch that leaves it out: the value itself
/// when every branch sets it alike; for lists, the elements of all of them, a string counting as a list of one, so
/// that every file or package a branch names is credited; for objects, each key any of them has, merged the same way;
/// for other values every branch sets, such as two different strings, one of them ([`one_of`]); anything else is
/// unknown, such as a string only some branches set.
fn any_of(values: &[Option<Value>]) -> Value {
    let present: Vec<&Value> = values.iter().flatten().collect();
    let Some(&first) = present.first() else {
        return Value::Unknown;
    };
    if agree(values) {
        return first.clone();
    }
    let lists: Option<Vec<&[Value]>> = present
        .iter()
        .map(|&value| match value {
            Value::Array(list) => Some(list.as_slice()),
            Value::String(_) => Some(std::slice::from_ref(value)),
            _ => None,
        })
        .collect();
    if let Some(lists) = lists
        && present.iter().any(|value| matches!(value, Value::Array(_)))
    {
        let mut items: Vec<Value> = Vec::new();
        for item in lists.into_iter().flatten() {
            if !items.contains(item) {
                items.push(item.clone());
            }
        }
        return Value::Array(items);
    }
    let objects: Option<Vec<Option<Entries<'_>>>> = values
        .iter()
        .map(|value| match value {
            None => Some(None),
            Some(Value::Object(entries)) => Some(Some(entries.as_slice())),
            Some(_) => None,
        })
        .collect();
    if let Some(objects) = objects {
        return Value::Object(any_entries(&objects, &[]));
    }
    if present.len() < values.len() {
        return Value::Unknown;
    }
    one_of(present.into_iter().cloned())
}

/// The most values a [`Value::OneOf`] holds; a value that may be one of more is unknown.
const MAX_ALTERNATIVES: usize = 8;

/// A value that is one of `values`, not knowing which: the value itself when they are all the same, otherwise a
/// [`Value::OneOf`] of the different ones in the order first seen, a `OneOf` among them flattened into its
/// alternatives. Unknown when one of them is unknown or not definite, or when there are more than
/// [`MAX_ALTERNATIVES`]; a caller that knows the value is not definite marks that itself.
fn one_of(values: impl IntoIterator<Item = Value>) -> Value {
    let mut out: Vec<Value> = Vec::new();
    for value in values {
        let nested = match value {
            Value::Unknown | Value::Maybe(_) => return Value::Unknown,
            Value::OneOf(nested) => nested,
            other => vec![other],
        };
        for alternative in nested {
            if !out.contains(&alternative) {
                out.push(alternative);
            }
        }
        if out.len() > MAX_ALTERNATIVES {
            return Value::Unknown;
        }
    }
    match out.len() {
        0 => Value::Unknown,
        1 => out.pop().unwrap_or(Value::Unknown),
        _ => Value::OneOf(out),
    }
}

/// `apply` of every combination of the values `args` may be ([`alternatives`]), as [`one_of`] the results: a file
/// joined to a directory that is one of two is one of two paths. Unknown when a result is, or when there are more
/// combinations than [`MAX_ALTERNATIVES`].
fn each_combination(args: &[Value], apply: impl Fn(&[Value]) -> Value) -> Value {
    if !args.iter().any(|arg| matches!(arg, Value::OneOf(_))) {
        return apply(args);
    }
    let mut combinations: Vec<Vec<Value>> = vec![Vec::new()];
    for arg in args {
        let choices = alternatives(arg.clone());
        if combinations.len().saturating_mul(choices.len()) > MAX_ALTERNATIVES {
            return Value::Unknown;
        }
        let mut next = Vec::with_capacity(combinations.len() * choices.len());
        for combination in &combinations {
            for choice in &choices {
                let mut extended = combination.clone();
                extended.push(choice.clone());
                next.push(extended);
            }
        }
        combinations = next;
    }
    one_of(combinations.iter().map(|combination| apply(combination.as_slice())))
}

/// The values `value` may be: the alternatives of a [`Value::OneOf`], or the value itself.
fn alternatives(value: Value) -> Vec<Value> {
    match value {
        Value::OneOf(alternatives) => alternatives,
        other => vec![other],
    }
}

/// What `.filter(Boolean)` keeps of the element `value`: `None` when it is falsy; otherwise the values it may have that
/// are not falsy, and whether it may be dropped.
fn truthy_part(value: Value) -> Option<(Value, bool)> {
    match truthiness(&value) {
        Some(false) => None,
        Some(true) => Some((value, false)),
        None => match value {
            Value::OneOf(alternatives) => {
                let count = alternatives.len();
                let kept: Vec<Value> = alternatives
                    .into_iter()
                    .filter(|alternative| truthiness(alternative) != Some(false))
                    .collect();
                let may_drop = kept.len() < count || kept.iter().any(|alternative| truthiness(alternative).is_none());
                (!kept.is_empty()).then(|| (one_of(kept), may_drop))
            }
            other => Some((other, true)),
        },
    }
}

/// Whether every one of `values` is present and they are all the same.
fn agree(values: &[Option<Value>]) -> bool {
    values
        .first()
        .is_some_and(|first| first.is_some() && values.iter().all(|value| value == first))
}

/// Whether a value is truthy in JavaScript; `None` when it is unknown.
fn truthiness(value: &Value) -> Option<bool> {
    match value {
        Value::Null | Value::Undefined => Some(false),
        Value::Bool(b) => Some(*b),
        Value::Number(n) => Some(*n != 0.0 && !n.is_nan()),
        Value::String(s) => Some(!s.is_empty()),
        Value::Array(_) | Value::Object(_) | Value::Regex { .. } => Some(true),
        Value::Unknown | Value::OneOf(_) | Value::Maybe(_) => None,
    }
}

/// The value of an expression that is `a` or `b`, not knowing which: the value itself when both are equal; for two
/// objects, an object with every key either has, whose value is what both have in common when both have the key and
/// [`ConfigValue::Unknown`] when only one does (it may be present or not); anything else is one of the two
/// ([`one_of`]), unknown when either is. No key appears that neither branch has, so other keys stay known, and a key
/// that may differ is one of its values, or unknown, wherever it is read. It is not definite ([`Value::Maybe`]) when
/// `a` or `b` is not.
fn either(a: &Value, b: &Value) -> Value {
    if a == b {
        return a.clone();
    }
    if !a.is_definite() || !b.is_definite() {
        return Value::doubtful(either(a.definite(), b.definite()), false);
    }
    let (Value::Object(left), Value::Object(right)) = (a, b) else {
        return one_of([a.clone(), b.clone()]);
    };
    let mut keys: Vec<&str> = Vec::new();
    for (key, _) in left.iter().chain(right) {
        if !ConfigValue::is_spread_key(key) && !keys.contains(&key.as_str()) {
            keys.push(key);
        }
    }
    let mut out: Vec<(String, Value)> = keys
        .into_iter()
        .map(|key| {
            let value = match (a.get(key), b.get(key)) {
                (Some(x), Some(y)) => either(x, y),
                (Some(only), None) | (None, Some(only)) => Value::doubtful(Value::Unknown, only.is_definite()),
                (None, None) => Value::Unknown,
            };
            (key.to_owned(), value)
        })
        .collect();
    for marker in [UNKNOWN_SPREAD, OWN_SPREAD] {
        if a.get(marker).is_some() || b.get(marker).is_some() {
            out.push((marker.to_owned(), Value::Unknown));
        }
    }
    Value::Object(out)
}

/// What a binding holds after one of two branches left it `a` and the other `b`: as [`either`], except that a list
/// one branch appended to (`plugins.push(x)`) keeps the elements both have.
fn join(a: &Value, b: &Value) -> Value {
    if !a.is_definite() || !b.is_definite() {
        return Value::doubtful(join(a.definite(), b.definite()), false);
    }
    match (a, b) {
        (Value::Array(x), Value::Array(y)) if x != y && (x.starts_with(y) || y.starts_with(x)) => {
            let mut common = if x.len() < y.len() { x.clone() } else { y.clone() };
            common.push(Value::Unknown);
            Value::Array(common)
        }
        _ => either(a, b),
    }
}

/// Appends a flat config, or the configs of a (nested) list of them, to `list`.
fn flatten_into(value: Value, list: &mut Vec<Value>) {
    match value {
        Value::Array(items) => {
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
fn required_package(call: &CallExpression<'_>, depth: u32) -> Option<Value> {
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
    (!module.starts_with(['.', '/'])).then_some(Value::String(module))
}

/// What the directory `dir` of a resolved `package.json` evaluates to: the package for a bare specifier (`pkg`,
/// `@scope/pkg`, a subpath of one), `None` for a relative path, which the path functions compute, and unknown for
/// anything else, which names no package: an absolute path, a URL, a subpath import (`#x`), or a path with an empty,
/// `.` or `..` segment.
fn manifest_directory(dir: &str) -> Option<Value> {
    if dir.starts_with('.') {
        return None;
    }
    let package = !dir.starts_with(['/', '#'])
        && !dir.contains([':', '\\'])
        && dir.split('/').all(|segment| !matches!(segment, "" | "." | ".."));
    Some(if package {
        Value::String(dir.to_owned())
    } else {
        Value::Unknown
    })
}

/// Whether `call` resolves a module specifier: `require.resolve(…)` or `import.meta.resolve(…)`.
fn is_module_resolve(call: &CallExpression<'_>) -> bool {
    is_resolve_of(call, "require")
        || matches!(
            call.callee.get_inner_expression(),
            Expression::StaticMemberExpression(m)
                if m.property.name.as_str() == "resolve"
                    && matches!(m.object.get_inner_expression(), Expression::ImportMeta(_))
        )
}

/// Whether `call` is `<object>.resolve(…)` of the global `object`.
fn is_resolve_of(call: &CallExpression<'_>, object: &str) -> bool {
    matches!(
        call.callee.get_inner_expression(),
        Expression::StaticMemberExpression(m)
            if m.property.name.as_str() == "resolve"
                && matches!(m.object.get_inner_expression(), Expression::Identifier(o) if o.name.as_str() == object)
    )
}

/// What an optional chain reads from: the object of its outermost member, or the callee of its call.
fn chain_object<'b, 'a>(chain: &'b ChainExpression<'a>) -> Option<&'b Expression<'a>> {
    match &chain.expression {
        ChainElement::CallExpression(call) => Some(&call.callee),
        ChainElement::TSNonNullExpression(e) => Some(&e.expression),
        element => element.as_member_expression().map(MemberExpression::object),
    }
}
