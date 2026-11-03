//! Module resolution on `oxc_resolver`, reading through the host's [`FileSystem`].

use std::collections::{BTreeMap, HashMap};
use std::hash::Hash;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError, RwLock};

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use oxc_resolver::{FileMetadata, ResolveError, ResolveOptions, ResolverGeneric, TsconfigDiscovery};
use serde_json::Value;
use skif_core::{FileSystem, OsFs};
use skif_js_syntax::is_stylesheet;
use skif_workspace::Workspace;

use crate::manifest_paths::{ENTRY_FIELDS, map_to_source, resolve_exports};
use crate::setup::Setup;
use crate::specifier::{
    has_foreign_scheme, is_builtin, node_modules_package, node_modules_specifier_package, package_name, subpath,
};
use crate::tsconfig_paths::{self, TsPaths};

/// Extensions that are parsed and followed.
pub(crate) const CODE_EXTENSIONS: &[&str] = &[
    "js", "mjs", "cjs", "jsx", "ts", "mts", "cts", "tsx", "vue", "svelte", "astro",
];

/// Whether a path has an extension that is parsed and followed.
pub(crate) fn is_code_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| CODE_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str()))
}

/// Adapts the host's file system to the resolver's, so resolution works on in-memory trees.
struct ResolverFs {
    fs: Arc<dyn FileSystem>,
}

impl oxc_resolver::FileSystem for ResolverFs {
    fn new() -> Self {
        Self { fs: Arc::new(OsFs) }
    }

    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.fs.read_to_string(path).map(|text| text.as_bytes().to_vec())
    }

    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        self.fs.read_to_string(path).map(|text| text.to_string())
    }

    fn metadata(&self, path: &Path) -> io::Result<FileMetadata> {
        if self.fs.is_file(path) {
            Ok(FileMetadata::new(true, false, false))
        } else if self.fs.is_dir(path) {
            Ok(FileMetadata::new(false, true, false))
        } else {
            Err(io::Error::new(io::ErrorKind::NotFound, "not found"))
        }
    }

    fn symlink_metadata(&self, path: &Path) -> io::Result<FileMetadata> {
        self.metadata(path)
    }

    fn read_link(&self, _path: &Path) -> Result<PathBuf, ResolveError> {
        Err(io::Error::new(io::ErrorKind::InvalidInput, "not a symbolic link").into())
    }

    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        Ok(path.to_path_buf())
    }
}

/// Joins a relative path written with `/` or `\` separators onto `base`, collapsing `.` and `..` lexically.
///
/// Tool configuration and `tsconfig` paths use `/`, Windows tooling and hand-written configs may use `\`; both
/// join the same way, and no separator of the host leaks into the result.
pub(crate) fn join_normalized(base: &Path, relative: &str) -> PathBuf {
    let mut path = base.to_path_buf();
    for segment in relative.split(['/', '\\']) {
        match segment {
            "" | "." => {}
            ".." => {
                path.pop();
            }
            other => path.push(other),
        }
    }
    path
}

/// What a specifier points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Resolved {
    /// A source file that is parsed and followed.
    File(PathBuf),
    /// A file that is not analysed (stylesheet, image, JSON) or lies outside the workspace.
    Asset,
    /// A package installed from a registry, or a workspace package whose files could not be located.
    External(String),
    /// A Node.js builtin.
    Builtin,
    /// A scheme the analysis does not follow (`data:`, `https:`, `virtual:`).
    Ignored,
    /// Nothing could be found.
    Unresolved,
}

/// The target of a specifier and the package it was addressed through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Outcome {
    pub target: Resolved,
    /// The package name when the specifier is a package import (installed or workspace), `None` otherwise.
    pub package: Option<String>,
}

struct Member {
    dir: PathBuf,
    manifest: Value,
    /// Absolute output directories with the source directory each is built from.
    build_dirs: Vec<(PathBuf, PathBuf)>,
}

/// One alias with its absolute target; `None` when the configuration computes it.
struct Alias {
    find: String,
    target: Option<PathBuf>,
}

/// The aliases declared for a package directory, longest `find` first.
struct AliasScope {
    dir: PathBuf,
    aliases: Vec<Alias>,
    /// Absolute directories bare specifiers are looked up in, in lookup order.
    module_dirs: Vec<PathBuf>,
}

/// Roots of `SvelteKit`'s virtual modules, which no file backs.
const VIRTUAL_ROOTS: &[&str] = &["$app", "$env", "$service-worker"];

/// The rest of `specifier` below the alias `find`, `""` for the alias itself.
fn alias_rest<'a>(specifier: &'a str, find: &str) -> Option<&'a str> {
    if specifier == find {
        return Some("");
    }
    specifier.strip_prefix(find)?.strip_prefix('/')
}

/// A map filled on first use; concurrent first uses may both compute, the results are equal.
struct Memo<K, V>(RwLock<HashMap<K, V>>);

impl<K: Eq + Hash, V: Clone> Memo<K, V> {
    fn new() -> Self {
        Self(RwLock::new(HashMap::new()))
    }

    fn get_or_compute(&self, key: K, compute: impl FnOnce() -> V) -> V {
        if let Some(known) = self.0.read().unwrap_or_else(PoisonError::into_inner).get(&key) {
            return known.clone();
        }
        let value = compute();
        self.0
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key, value.clone());
        value
    }
}

/// Resolves specifiers for the whole workspace. Cheap to share between threads.
pub(crate) struct ModuleResolver {
    script: Resolvers,
    style: Resolvers,
    fs: Arc<dyn FileSystem>,
    root: PathBuf,
    /// The name the repository's Bazel `MODULE.bazel` or `WORKSPACE` gives it, which prefixes root-relative requires.
    workspace_name: Option<String>,
    /// The `.gitignore` files of the root and of the packages, with the directory each applies to.
    ignores: Vec<(PathBuf, Gitignore)>,
    members: BTreeMap<String, Member>,
    scopes: Vec<AliasScope>,
    /// `exports` conditions that select source files, besides the built-in `source`.
    conditions: Vec<String>,
    /// Packages that are not workspace members, by directory, parsed once; `None` when there is no readable manifest.
    directory_packages: Memo<PathBuf, Option<Arc<Member>>>,
    /// Outcomes of the lookup-directory probe by the innermost package directory of the importer and specifier, misses
    /// included.
    module_dir_probes: Memo<(PathBuf, String), Option<Resolved>>,
    /// The nearest `baseUrl` and `paths` by directory, `None` when no config at or above it sets them.
    nearest_paths: Memo<PathBuf, Option<Arc<TsPaths>>>,
}

/// Which language a specifier is written in.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Script,
    Style,
}

/// The resolvers of one language: with the `tsconfig` of the importer, and without for files it cannot be applied to.
struct Resolvers {
    with_tsconfig: ResolverGeneric<ResolverFs>,
    plain: ResolverGeneric<ResolverFs>,
}

impl Resolvers {
    fn new(fs: &Arc<dyn FileSystem>, options: impl Fn(bool) -> ResolveOptions) -> Self {
        let make =
            |tsconfig| ResolverGeneric::new_with_file_system(ResolverFs { fs: Arc::clone(fs) }, options(tsconfig));
        Self {
            with_tsconfig: make(true),
            plain: make(false),
        }
    }

    fn attempt(&self, importer: &Path, specifier: &str) -> Result<PathBuf, ResolveError> {
        match self.with_tsconfig.resolve_file(importer, specifier) {
            Ok(resolution) => Ok(resolution.path().to_path_buf()),
            Err(error @ (ResolveError::NotFound(_) | ResolveError::MatchedAliasNotFound(..))) => Err(error),
            Err(_) => self
                .plain
                .resolve_file(importer, specifier)
                .map(|resolution| resolution.path().to_path_buf()),
        }
    }
}

/// The `name` argument of a Bazel `module(...)` or `workspace(...)` call.
fn bazel_workspace_name(fs: &dyn FileSystem, root: &Path) -> Option<String> {
    ["MODULE.bazel", "WORKSPACE", "WORKSPACE.bazel"]
        .iter()
        .find_map(|file| {
            let text = fs.read_to_string(&root.join(file)).ok()?;
            let call = if *file == "MODULE.bazel" {
                "module("
            } else {
                "workspace("
            };
            let at = text.match_indices(call).find_map(|(at, _)| {
                text[..at]
                    .chars()
                    .next_back()
                    .is_none_or(|before| !(before.is_alphanumeric() || before == '_'))
                    .then_some(at)
            })?;
            let body = &text[at + call.len()..];
            let body = &body[..body.find(')')?];
            let at = body.match_indices("name").find_map(|(at, _)| {
                body[..at]
                    .chars()
                    .next_back()
                    .is_none_or(|before| !(before.is_alphanumeric() || before == '_'))
                    .then_some(at)
            })?;
            let value = body[at + "name".len()..].trim_start().strip_prefix('=')?.trim_start();
            let quote = value.chars().next().filter(|quote| matches!(quote, '"' | '\''))?;
            let value = &value[1..];
            let name = &value[..value.find(quote)?];
            (!name.is_empty()).then(|| name.to_owned())
        })
}

/// The `.gitignore` of `dir`, if it has one.
fn read_gitignore(fs: &dyn FileSystem, dir: &Path) -> Option<(PathBuf, Gitignore)> {
    let text = fs.read_to_string(&dir.join(".gitignore")).ok()?;
    let mut builder = GitignoreBuilder::new(dir);
    for line in text.lines() {
        // A malformed line is skipped, as git does.
        let _ = builder.add_line(None, line);
    }
    Some((dir.to_path_buf(), builder.build().ok()?))
}

/// Options for specifiers written in stylesheets: stylesheet extensions and the `style` export condition.
fn style_options(tsconfig: bool) -> ResolveOptions {
    let owned = |items: &[&str]| items.iter().map(|item| (*item).to_owned()).collect::<Vec<_>>();
    ResolveOptions {
        tsconfig: tsconfig.then_some(TsconfigDiscovery::Auto),
        extensions: owned(&[".scss", ".sass", ".css", ".less", ".pcss"]),
        condition_names: owned(&["style", "sass", "less", "import", "node", "default"]),
        main_fields: owned(&["style", "sass", "less", "main"]),
        symlinks: false,
        node_path: false,
        ..ResolveOptions::default()
    }
}

fn options(tsconfig: bool, conditions: &[String], extensions: &[String]) -> ResolveOptions {
    let owned = |items: &[&str]| items.iter().map(|item| (*item).to_owned()).collect::<Vec<_>>();
    ResolveOptions {
        tsconfig: tsconfig.then_some(TsconfigDiscovery::Auto),
        extensions: owned(&[
            ".ts", ".tsx", ".mts", ".cts", ".js", ".jsx", ".mjs", ".cjs", ".json", ".vue", ".svelte", ".astro", ".d.ts",
        ])
        .into_iter()
        .chain(extensions.iter().cloned())
        .collect(),
        // A declaration file only stands in for a runtime file that is not there: the runtime file is what is loaded.
        extension_alias: vec![
            (".js".to_owned(), owned(&[".ts", ".tsx", ".js", ".d.ts"])),
            (".jsx".to_owned(), owned(&[".tsx", ".jsx"])),
            (".mjs".to_owned(), owned(&[".mts", ".mjs", ".d.mts"])),
            (".cjs".to_owned(), owned(&[".cts", ".cjs", ".d.cts"])),
        ],
        condition_names: owned(&["import", "require", "node", "default", "types"])
            .into_iter()
            .chain(conditions.iter().cloned())
            .collect(),
        main_fields: owned(&["module", "main"]),
        symlinks: false,
        node_path: false,
        ..ResolveOptions::default()
    }
}

impl ModuleResolver {
    pub(crate) fn new(fs: Arc<dyn FileSystem>, workspace: &Workspace, setup: &Setup) -> Self {
        let mut conditions: Vec<String> = setup
            .packages
            .iter()
            .flatten()
            .flat_map(|package| package.conditions.iter().cloned())
            .collect();
        conditions.sort();
        conditions.dedup();
        let mut extensions: Vec<String> = setup
            .packages
            .iter()
            .flatten()
            .flat_map(|package| package.extensions.iter().cloned())
            .collect();
        extensions.sort();
        extensions.dedup();
        let script = Resolvers::new(&fs, |tsconfig| options(tsconfig, &conditions, &extensions));
        let style = Resolvers::new(&fs, style_options);
        let members = workspace
            .all_packages()
            .filter_map(|package| {
                let name = package.name.clone()?;
                let dir = workspace.root.join(&package.dir);
                let build_dirs = setup
                    .package(package.id)
                    .map(|setup| {
                        setup
                            .build_dirs
                            .iter()
                            .map(|dir_pair| {
                                (
                                    join_normalized(&dir, &dir_pair.out_dir),
                                    join_normalized(&dir, &dir_pair.root_dir),
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                Some((
                    name,
                    Member {
                        dir,
                        manifest: package.manifest.value.clone(),
                        build_dirs,
                    },
                ))
            })
            .collect();
        let mut scopes: Vec<AliasScope> = setup
            .packages
            .iter()
            .flatten()
            .map(|package| {
                let mut aliases: Vec<Alias> = package
                    .aliases
                    .iter()
                    .map(|alias| Alias {
                        find: alias.find.clone(),
                        target: alias
                            .target
                            .as_deref()
                            .map(|target| join_normalized(&package.dir, target)),
                    })
                    .collect();
                aliases.push(Alias {
                    find: "$lib".to_owned(),
                    target: Some(join_normalized(&package.dir, "src/lib")),
                });
                aliases.sort_by_key(|alias| std::cmp::Reverse(alias.find.len()));
                let module_dirs = package
                    .module_dirs
                    .iter()
                    .map(|dir| join_normalized(&package.dir, dir))
                    .filter(|dir| !dir.components().any(|part| part.as_os_str() == "node_modules"))
                    .collect();
                AliasScope {
                    dir: package.dir.clone(),
                    aliases,
                    module_dirs,
                }
            })
            .collect();
        scopes.sort_by_key(|scope| std::cmp::Reverse(scope.dir.components().count()));
        let mut ignore_dirs = vec![workspace.root.clone()];
        for package in workspace.all_packages() {
            let dir = workspace.root.join(&package.dir);
            if !ignore_dirs.contains(&dir) {
                ignore_dirs.push(dir);
            }
        }
        let ignores = ignore_dirs
            .iter()
            .filter_map(|dir| read_gitignore(fs.as_ref(), dir))
            .collect();
        Self {
            script,
            style,
            workspace_name: bazel_workspace_name(fs.as_ref(), &workspace.root),
            ignores,
            fs,
            root: workspace.root.clone(),
            members,
            scopes,
            conditions,
            directory_packages: Memo::new(),
            module_dir_probes: Memo::new(),
            nearest_paths: Memo::new(),
        }
    }

    fn classify(&self, path: PathBuf) -> Resolved {
        if path.starts_with(&self.root) && (is_code_file(&path) || is_stylesheet(&path)) {
            Resolved::File(path)
        } else {
            Resolved::Asset
        }
    }

    const fn resolvers(&self, kind: Kind) -> &Resolvers {
        match kind {
            Kind::Script => &self.script,
            Kind::Style => &self.style,
        }
    }

    /// Whether `path` is listed in a `.gitignore`, so it may be generated and absent from a fresh checkout.
    fn is_generated(&self, path: &Path) -> bool {
        self.ignores
            .iter()
            .any(|(dir, ignore)| path.starts_with(dir) && ignore.matched_path_or_any_parents(path, false).is_ignore())
    }

    /// What a relative specifier that names nothing is: unresolved, or unknown when it points at an ignored path.
    fn missing(&self, importer: &Path, specifier: &str) -> Resolved {
        let generated = specifier.starts_with('.')
            && importer
                .parent()
                .is_some_and(|dir| self.is_generated(&join_normalized(dir, specifier)));
        if generated {
            Resolved::Ignored
        } else {
            Resolved::Unresolved
        }
    }

    /// The outcome of an alias: an alias into `node_modules` names the package it points at.
    fn aliased(found: Resolved) -> Outcome {
        let package = match &found {
            Resolved::External(name) => Some(name.clone()),
            _ => None,
        };
        Outcome { target: found, package }
    }

    /// Resolves `specifier` as written in the file `importer`.
    pub(crate) fn resolve(&self, importer: &Path, specifier: &str) -> Outcome {
        let outcome = |target, package| Outcome { target, package };
        if is_builtin(specifier) {
            return outcome(Resolved::Builtin, None);
        }
        if has_foreign_scheme(specifier) {
            return outcome(Resolved::Ignored, None);
        }
        self.resolve_module(Kind::Script, importer, specifier)
    }

    /// A specifier that is neither relative, a builtin nor foreign: lookup directories, installed packages, aliases.
    ///
    /// Lookup directories (`baseUrl`, `resolve.modules`) and the workspace-name prefix come first, as they do for
    /// TypeScript and the bundlers; an alias is tried when nothing is installed.
    fn resolve_module(&self, kind: Kind, importer: &Path, specifier: &str) -> Outcome {
        let outcome = |target, package| Outcome { target, package };
        if let Some(name) = node_modules_specifier_package(specifier) {
            return self.package_target(&name, specifier);
        }
        let name = package_name(specifier);
        if name.is_some()
            && let Some(found) = self
                .through_workspace_name(specifier)
                .or_else(|| self.through_module_dirs(importer, specifier))
        {
            return outcome(found, None);
        }
        let member_name = name.clone().filter(|name| self.members.contains_key(name));
        if let Ok(path) = self.resolvers(kind).attempt(importer, specifier) {
            if let Some(installed) = node_modules_package(&path) {
                let package = name.unwrap_or(installed);
                return self.package_target(&package, specifier);
            }
            return outcome(self.classify(path), member_name);
        }
        let aliased = self.through_alias(importer, specifier);
        if aliased
            .as_ref()
            .is_none_or(|resolved| matches!(resolved, Resolved::Unresolved))
            && let Some(found) = self.through_nearest_tsconfig(importer, specifier)
        {
            return Self::aliased(found);
        }
        // An alias whose target is missing is not final: bundlers fall through to their lookup directories.
        if let Some(found) = aliased
            .as_ref()
            .filter(|resolved| !matches!(resolved, Resolved::Unresolved))
        {
            return Self::aliased(found.clone());
        }
        if aliased.is_some()
            && name.is_none()
            && let Some(found) = self.through_module_dirs(importer, specifier)
        {
            return outcome(found, None);
        }
        if let Some(aliased) = aliased {
            return outcome(aliased, None);
        }
        match name {
            Some(name) => self.package_target(&name, specifier),
            None => outcome(self.missing(importer, specifier), None),
        }
    }

    /// A stylesheet next to `importer`: `specifier` itself, its Sass partial (`_name`) or the index partial of a
    /// directory.
    fn local_style(&self, importer: &Path, specifier: &str) -> Option<PathBuf> {
        let (dir, name) = specifier
            .rsplit_once('/')
            .map_or(("", specifier), |(dir, name)| (dir, name));
        let prefix = if dir.is_empty() {
            String::new()
        } else {
            format!("{dir}/")
        };
        let name = name.trim_end_matches('/');
        [
            specifier.to_owned(),
            format!("{prefix}_{name}"),
            format!("{prefix}{name}/_index"),
        ]
        .iter()
        .find_map(|candidate| self.style.attempt(importer, candidate).ok())
    }

    /// A root-relative stylesheet (`@import "/src/x.css"`): a file system path, else relative to the package that
    /// holds the importer, to the repository root or to the `public` directory, as dev servers serve it.
    ///
    /// Nothing found is unknown rather than unresolved: the server may map the path to anything.
    fn root_relative_style(&self, importer: &Path, specifier: &str) -> Resolved {
        if let Some(path) = self.local_style(importer, specifier) {
            return self.classify(path);
        }
        let rest = specifier.trim_start_matches('/');
        let package_dir = self
            .scopes
            .iter()
            .find(|scope| importer.starts_with(&scope.dir))
            .map_or(self.root.as_path(), |scope| scope.dir.as_path());
        [
            package_dir.join(rest),
            self.root.join(rest),
            package_dir.join("public").join(rest),
        ]
        .iter()
        .find_map(|candidate| self.local_style(importer, &candidate.to_string_lossy()))
        .map_or(Resolved::Ignored, |path| self.classify(path))
    }

    /// Resolves `specifier` as written in the stylesheet `importer`.
    ///
    /// A bare specifier is looked up next to the stylesheet first when the language resolves it that way (Sass and
    /// Less) or it names a stylesheet file (`@import "reset.css"`), and as a package otherwise.
    pub(crate) fn resolve_style(&self, importer: &Path, specifier: &str) -> Outcome {
        let outcome = |target, package| Outcome { target, package };
        if has_foreign_scheme(specifier) {
            return outcome(Resolved::Ignored, None);
        }
        if specifier.starts_with('/') {
            return outcome(self.root_relative_style(importer, specifier), None);
        }
        let relative = specifier.starts_with('.');
        let preprocessed = importer
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| matches!(extension.to_ascii_lowercase().as_str(), "scss" | "sass" | "less"));
        if relative || preprocessed || is_stylesheet(Path::new(specifier)) {
            let local = if relative {
                specifier.to_owned()
            } else {
                format!("./{specifier}")
            };
            if let Some(path) = self.local_style(importer, &local) {
                return outcome(self.classify(path), None);
            }
            if relative {
                return outcome(self.missing(importer, specifier), None);
            }
        }
        self.resolve_module(Kind::Style, importer, specifier)
    }

    /// `ws/<path>` where `ws` is the name the Bazel workspace gives the repository: the file `<path>` under the root.
    fn through_workspace_name(&self, specifier: &str) -> Option<Resolved> {
        let name = self.workspace_name.as_deref()?;
        let rest = specifier.strip_prefix(name)?.strip_prefix('/')?;
        if rest.is_empty() {
            return None;
        }
        let path = join_normalized(&self.root, rest);
        self.script
            .plain
            .resolve(&self.root, &path.to_string_lossy())
            .ok()
            .map(|resolution| self.classify(resolution.path().to_path_buf()))
    }

    /// Looks a bare specifier up in the lookup directories of the importer's package (`baseUrl`, `paths` wildcards,
    /// `resolve.modules`), in order. A specifier that names a file there is a file import, never a package.
    fn through_module_dirs(&self, importer: &Path, specifier: &str) -> Option<Resolved> {
        // The packages whose directories hold the importer form a chain, so the innermost one identifies them all.
        let innermost = self.scopes.iter().find(|scope| importer.starts_with(&scope.dir))?;
        self.module_dir_probes
            .get_or_compute((innermost.dir.clone(), specifier.to_owned()), || {
                self.scopes
                    .iter()
                    .filter(|scope| innermost.dir.starts_with(&scope.dir))
                    .flat_map(|scope| &scope.module_dirs)
                    .find_map(|dir| {
                        let path = join_normalized(dir, specifier);
                        self.script
                            .plain
                            .resolve(&innermost.dir, &path.to_string_lossy())
                            .ok()
                            .map(|resolution| self.classify(resolution.path().to_path_buf()))
                    })
            })
    }

    /// Follows `paths` and `baseUrl` of the nearest `tsconfig.json` or `jsconfig.json` at or above the importer, also
    /// when it is in a directory that is not a package or does not include the importer.
    fn through_nearest_tsconfig(&self, importer: &Path, specifier: &str) -> Option<Resolved> {
        let directory = importer.parent()?;
        let config = self.nearest_paths.get_or_compute(directory.to_path_buf(), || {
            tsconfig_paths::nearest(self.fs.as_ref(), &self.root, directory).map(Arc::new)
        })?;
        config.candidates(specifier).into_iter().find_map(|path| {
            if let Some(package) = node_modules_package(&path) {
                return Some(Resolved::External(package));
            }
            self.script
                .plain
                .resolve(directory, &path.to_string_lossy())
                .ok()
                .map(|resolution| self.classify(resolution.path().to_path_buf()))
        })
    }

    /// Follows the aliases declared for the importer's package (and the packages above it).
    ///
    /// `None` when no alias applies. An alias whose target the configuration computes is not followed but is known not
    /// to be a package; one whose static target does not exist is unresolved.
    fn through_alias(&self, importer: &Path, specifier: &str) -> Option<Resolved> {
        let root = specifier.split('/').next().unwrap_or(specifier);
        if VIRTUAL_ROOTS.contains(&root) {
            return Some(Resolved::Ignored);
        }
        let mut computed = false;
        let mut matched = false;
        let directory = importer.parent()?;
        for scope in self.scopes.iter().filter(|scope| importer.starts_with(&scope.dir)) {
            for alias in &scope.aliases {
                let Some(rest) = alias_rest(specifier, &alias.find) else {
                    continue;
                };
                matched = true;
                let Some(target) = &alias.target else {
                    computed = true;
                    continue;
                };
                let path = join_normalized(target, rest);
                // An alias into `node_modules` is the package there, installed or not.
                if let Some(package) = node_modules_package(&path) {
                    return Some(Resolved::External(package));
                }
                if let Ok(resolution) = self.script.plain.resolve(directory, &path.to_string_lossy()) {
                    return Some(self.classify(resolution.path().to_path_buf()));
                }
                if let Some(found) = self.through_package_directory(target, rest) {
                    return Some(found);
                }
                if self.is_generated(&path) {
                    return Some(Resolved::Ignored);
                }
            }
        }
        match (matched, computed) {
            (false, _) => None,
            (true, true) => Some(Resolved::Ignored),
            (true, false) => Some(Resolved::Unresolved),
        }
    }

    /// An alias target that is a package directory, such as a sibling checkout whose `main` points at output that was
    /// never built: its source files by the usual output-to-source mapping, else a reference that is known to exist.
    ///
    /// `None` when `target` is not a package directory or `rest` names something inside it that does not exist.
    fn through_package_directory(&self, target: &Path, rest: &str) -> Option<Resolved> {
        let adhoc;
        let member = if let Some(known) = self.members.values().find(|member| member.dir == target) {
            known
        } else {
            adhoc = self.directory_package(target)?;
            &*adhoc
        };
        match self.member_file(member, (!rest.is_empty()).then_some(rest)) {
            Some(path) => Some(self.classify(path)),
            None if rest.is_empty() => Some(Resolved::Ignored),
            None => None,
        }
    }

    /// The package in `dir` that is not a workspace member, with its manifest parsed once per directory.
    fn directory_package(&self, dir: &Path) -> Option<Arc<Member>> {
        self.directory_packages.get_or_compute(dir.to_path_buf(), || {
            let text = self.fs.read_to_string(&dir.join("package.json")).ok()?;
            Some(Arc::new(Member {
                dir: dir.to_path_buf(),
                manifest: serde_json::from_str(&text).ok()?,
                build_dirs: Vec::new(),
            }))
        })
    }

    fn package_target(&self, name: &str, specifier: &str) -> Outcome {
        let member_file = self
            .members
            .get(name)
            .and_then(|member| self.member_file(member, subpath(specifier, name)));
        let target = member_file.map_or_else(|| Resolved::External(name.to_owned()), |path| self.classify(path));
        Outcome {
            target,
            package: Some(name.to_owned()),
        }
    }

    /// The source file behind a published path: through the package's `outDir` and `rootDir` first, then by the
    /// `dist` to `src` convention.
    fn map_member_target(&self, member: &Member, target: &str) -> Option<PathBuf> {
        let published = join_normalized(&member.dir, target);
        for (out_dir, root_dir) in &member.build_dirs {
            let Ok(rest) = published.strip_prefix(out_dir) else {
                continue;
            };
            let rest = rest.to_string_lossy().replace('\\', "/");
            let source = join_normalized(root_dir, &rest);
            let Ok(inside) = source.strip_prefix(&member.dir) else {
                continue;
            };
            let mapped = inside.to_string_lossy().replace('\\', "/");
            if let Some(found) = map_to_source(self.fs.as_ref(), &member.dir, &mapped) {
                return Some(found);
            }
        }
        map_to_source(self.fs.as_ref(), &member.dir, target)
    }

    /// The source file a workspace package exposes at `subpath`, or `None` when its build output is all there is.
    fn member_file(&self, member: &Member, subpath: Option<&str>) -> Option<PathBuf> {
        if let Some(exports) = member.manifest.get("exports").filter(|value| !value.is_null()) {
            let key = subpath.map_or_else(|| ".".to_owned(), |sub| format!("./{sub}"));
            let target = resolve_exports(exports, &key, &self.conditions)?;
            return self.map_member_target(member, &target);
        }
        match subpath {
            None => ENTRY_FIELDS
                .iter()
                .filter_map(|field| member.manifest.get(*field)?.as_str())
                .find_map(|target| self.map_member_target(member, target))
                .or_else(|| {
                    self.script
                        .plain
                        .resolve(&member.dir, ".")
                        .ok()
                        .map(|resolution| resolution.path().to_path_buf())
                })
                .or_else(|| map_to_source(self.fs.as_ref(), &member.dir, "src/index")),
            Some(sub) => self
                .script
                .plain
                .resolve(&member.dir, &format!("./{sub}"))
                .ok()
                .map(|resolution| resolution.path().to_path_buf())
                .or_else(|| map_to_source(self.fs.as_ref(), &member.dir, sub))
                .or_else(|| map_to_source(self.fs.as_ref(), &member.dir, &format!("src/{sub}"))),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::path::Path;

    use skif_config::Config;
    use skif_core::MemoryFs;

    use super::*;

    use crate::input::{BuildDir, InputRequest, InputSource, PathAlias, WorkspaceInputs};

    /// Gives the root package fixed aliases, build directories and conditions.
    struct RootInputs(WorkspaceInputs);

    impl InputSource for RootInputs {
        fn inputs(&self, request: &InputRequest<'_>) -> WorkspaceInputs {
            if request.package.is_root {
                self.0.clone()
            } else {
                WorkspaceInputs::default()
            }
        }
    }

    fn resolver(files: &[(&str, &str)]) -> ModuleResolver {
        resolver_with(files, WorkspaceInputs::default())
    }

    fn resolver_with(files: &[(&str, &str)], inputs: WorkspaceInputs) -> ModuleResolver {
        resolver_at(Path::new("/ws"), files, inputs)
    }

    fn resolver_at(root: &Path, files: &[(&str, &str)], inputs: WorkspaceInputs) -> ModuleResolver {
        let fs = files
            .iter()
            .fold(MemoryFs::new(), |fs, (path, text)| fs.with_file(root.join(path), text));
        let config = Config::from_toml_str("").unwrap();
        let workspace = Workspace::discover_with_fs(root, &config, &fs).unwrap();
        let fs: Arc<dyn FileSystem> = Arc::new(fs);
        let sources: Vec<Arc<dyn InputSource>> = vec![Arc::new(RootInputs(inputs))];
        let setup = Setup::new(&workspace, &config, &fs, &sources);
        ModuleResolver::new(fs, &workspace, &setup)
    }

    fn alias(find: &str, target: Option<&str>) -> PathAlias {
        PathAlias {
            find: find.to_owned(),
            target: target.map(str::to_owned),
        }
    }

    const ROOT: &str = r#"{"name":"root","private":true,"workspaces":["packages/*"]}"#;

    #[test]
    fn relative_extensions_and_ts_alias() {
        let r = resolver(&[
            ("package.json", ROOT),
            ("packages/a/package.json", r#"{"name":"@x/a"}"#),
            ("packages/a/src/a.ts", ""),
            ("packages/a/src/b.tsx", ""),
        ]);
        let importer = Path::new("/ws/packages/a/src/a.ts");
        assert_eq!(
            r.resolve(importer, "./b").target,
            Resolved::File(PathBuf::from("/ws/packages/a/src/b.tsx"))
        );
        assert_eq!(
            r.resolve(importer, "./b.js").target,
            Resolved::File(PathBuf::from("/ws/packages/a/src/b.tsx"))
        );
        assert_eq!(r.resolve(importer, "./missing").target, Resolved::Unresolved);
    }

    #[test]
    fn builtins_externals_and_schemes() {
        let r = resolver(&[
            ("package.json", ROOT),
            ("packages/a/package.json", r#"{"name":"@x/a"}"#),
        ]);
        let importer = Path::new("/ws/packages/a/src/a.ts");
        assert_eq!(r.resolve(importer, "node:fs").target, Resolved::Builtin);
        assert_eq!(r.resolve(importer, "path").target, Resolved::Builtin);
        assert_eq!(r.resolve(importer, "https://a.dev/x.js").target, Resolved::Ignored);
        let lodash = r.resolve(importer, "lodash/fp");
        assert_eq!(lodash.target, Resolved::External("lodash".to_owned()));
        assert_eq!(lodash.package.as_deref(), Some("lodash"));
    }

    #[test]
    fn workspace_sibling_through_exports_and_dist_mapping() {
        let r = resolver(&[
            ("package.json", ROOT),
            ("packages/a/package.json", r#"{"name":"@x/a"}"#),
            (
                "packages/b/package.json",
                r#"{"name":"@x/b","exports":{".":{"import":"./dist/index.js"},"./util":"./dist/util.js"}}"#,
            ),
            ("packages/b/src/index.ts", ""),
            ("packages/b/src/util.ts", ""),
        ]);
        let importer = Path::new("/ws/packages/a/src/a.ts");
        let main = r.resolve(importer, "@x/b");
        assert_eq!(
            main.target,
            Resolved::File(PathBuf::from("/ws/packages/b/src/index.ts"))
        );
        assert_eq!(main.package.as_deref(), Some("@x/b"));
        assert_eq!(
            r.resolve(importer, "@x/b/util").target,
            Resolved::File(PathBuf::from("/ws/packages/b/src/util.ts"))
        );
        assert_eq!(
            r.resolve(importer, "@x/b/hidden").target,
            Resolved::External("@x/b".to_owned())
        );
    }

    #[test]
    fn tsconfig_paths() {
        // The resolver only applies a `tsconfig` found below an absolute path, and `/ws` has no drive on Windows.
        let root = Path::new(if cfg!(windows) { "C:/ws" } else { "/ws" });
        let r = resolver_at(
            root,
            &[
                ("package.json", ROOT),
                ("packages/a/package.json", r#"{"name":"@x/a"}"#),
                (
                    "packages/a/tsconfig.json",
                    r#"{"compilerOptions":{"baseUrl":".","paths":{"@app/*":["src/lib/*"]}}}"#,
                ),
                ("packages/a/src/a.ts", ""),
                ("packages/a/src/lib/thing.ts", ""),
            ],
            WorkspaceInputs::default(),
        );
        let importer = root.join("packages/a/src/a.ts");
        assert_eq!(
            r.resolve(&importer, "@app/thing").target,
            Resolved::File(root.join("packages/a/src/lib/thing.ts"))
        );
    }

    #[test]
    fn tsconfig_outside_a_package_applies_to_the_files_below_it() {
        let r = resolver(&[
            ("package.json", ROOT),
            (
                "packages/tsconfig.json",
                r#"{"compilerOptions":{"paths":{"@scope/*":["./*/src/index.ts"]}},"exclude":["**/*.spec.ts"]}"#,
            ),
            ("packages/app/src/main.spec.ts", ""),
            ("packages/app/src/main.js", ""),
            ("packages/lib/src/index.ts", ""),
            ("src/legacy/jsconfig.json", r#"{"compilerOptions":{"baseUrl":"."}}"#),
            ("src/legacy/page.js", ""),
            ("src/legacy/helpers/util.js", ""),
        ]);
        let lib = Resolved::File(PathBuf::from("/ws/packages/lib/src/index.ts"));
        for importer in ["/ws/packages/app/src/main.spec.ts", "/ws/packages/app/src/main.js"] {
            assert_eq!(r.resolve(Path::new(importer), "@scope/lib").target, lib, "{importer}");
        }
        assert_eq!(
            r.resolve(Path::new("/ws/src/legacy/page.js"), "helpers/util").target,
            Resolved::File(PathBuf::from("/ws/src/legacy/helpers/util.js"))
        );
    }

    #[test]
    fn bundler_aliases_resolve_and_unresolved_aliases_are_never_packages() {
        let inputs = WorkspaceInputs {
            aliases: vec![
                alias("@", Some("src")),
                alias("@app", Some("src\\lib")),
                alias("computed", None),
                alias("@gone", Some("nowhere")),
            ],
            ..WorkspaceInputs::default()
        };
        let r = resolver_with(
            &[
                ("package.json", ROOT),
                ("src/main.ts", ""),
                ("src/util.ts", ""),
                ("src/lib/thing.ts", ""),
                ("src/routes/a/x.ts", ""),
                ("src/lib/index.ts", ""),
            ],
            inputs,
        );
        let importer = Path::new("/ws/src/main.ts");
        let file = |path: &str| Resolved::File(PathBuf::from(path));
        assert_eq!(r.resolve(importer, "@/util").target, file("/ws/src/util.ts"));
        assert_eq!(r.resolve(importer, "@app/thing").target, file("/ws/src/lib/thing.ts"));
        assert_eq!(r.resolve(importer, "$lib/thing").target, file("/ws/src/lib/thing.ts"));
        assert_eq!(r.resolve(importer, "$lib").target, file("/ws/src/lib/index.ts"));
        assert_eq!(r.resolve(importer, "$app/stores").target, Resolved::Ignored);
        assert_eq!(r.resolve(importer, "computed/x").target, Resolved::Ignored);
        let gone = r.resolve(importer, "@gone/x");
        assert_eq!(gone.target, Resolved::Unresolved);
        assert_eq!(gone.package, None);
        let unknown = r.resolve(importer, "@/missing");
        assert_eq!(unknown.target, Resolved::Unresolved);
        assert_eq!(unknown.package, None);
        assert_eq!(
            r.resolve(importer, "@scope/real").target,
            Resolved::External("@scope/real".to_owned())
        );
    }

    #[test]
    fn windows_separators_join_like_posix_ones() {
        let base = Path::new("/ws/pkg");
        assert_eq!(
            join_normalized(base, "src\\lib\\..\\app/./x"),
            Path::new("/ws/pkg/src/app/x")
        );
        assert_eq!(join_normalized(base, "..\\other"), Path::new("/ws/other"));
        assert_eq!(join_normalized(base, ""), base);
    }

    #[test]
    fn out_dir_root_dir_and_source_condition_map_to_sources() {
        let inputs = WorkspaceInputs {
            build_dirs: vec![BuildDir {
                out_dir: "packages/b/build".to_owned(),
                root_dir: "packages/b/lib".to_owned(),
            }],
            ..WorkspaceInputs::default()
        };
        let r = resolver_with(
            &[
                ("package.json", ROOT),
                ("packages/a/package.json", r#"{"name":"@x/a"}"#),
                (
                    "packages/b/package.json",
                    r#"{"name":"@x/b","main":"./build/index.js","exports":{"./util":{"source":"./lib/util.ts","default":"./build/util.js"}}}"#,
                ),
                ("packages/b/lib/index.ts", ""),
                ("packages/b/lib/util.ts", ""),
            ],
            inputs,
        );
        let importer = Path::new("/ws/packages/a/src/a.ts");
        assert_eq!(
            r.resolve(importer, "@x/b/util").target,
            Resolved::File(PathBuf::from("/ws/packages/b/lib/util.ts"))
        );
    }

    #[test]
    fn node_modules_specifiers_are_uses_of_the_package_after_the_last_segment() {
        let r = resolver(&[("package.json", ROOT), ("src/main.ts", "")]);
        let importer = Path::new("/ws/src/main.ts");
        for (specifier, package) in [
            ("mylib/node_modules/express", "express"),
            ("../node_modules/lodash/fp", "lodash"),
            ("/abs/node_modules/@scope/pkg/x", "@scope/pkg"),
        ] {
            let outcome = r.resolve(importer, specifier);
            assert_eq!(outcome.target, Resolved::External(package.to_owned()), "{specifier}");
            assert_eq!(outcome.package.as_deref(), Some(package), "{specifier}");
        }
    }

    #[test]
    fn bare_specifiers_resolve_through_lookup_directories_in_order() {
        let inputs = WorkspaceInputs {
            module_dirs: vec![
                "shared".to_owned(),
                ".".to_owned(),
                "node_modules".to_owned(),
                "src".to_owned(),
            ],
            ..WorkspaceInputs::default()
        };
        let r = resolver_with(
            &[
                ("package.json", ROOT),
                ("src/main.ts", ""),
                ("src/services/api.ts", ""),
                ("src/views/home.ts", ""),
                ("config.js", ""),
                ("shared/services/api.ts", ""),
            ],
            inputs,
        );
        let importer = Path::new("/ws/src/main.ts");
        let file = |path: &str| Resolved::File(PathBuf::from(path));
        assert_eq!(
            r.resolve(importer, "services/api").target,
            file("/ws/shared/services/api.ts")
        );
        assert_eq!(r.resolve(importer, "views/home").target, file("/ws/src/views/home.ts"));
        assert_eq!(r.resolve(importer, "config").target, file("/ws/config.js"));
        assert_eq!(
            r.resolve(importer, "react").target,
            Resolved::External("react".to_owned())
        );
        assert_eq!(r.resolve(importer, "missing/thing").package.as_deref(), Some("missing"));
    }

    #[test]
    fn alias_to_a_package_directory_with_unbuilt_output_is_not_unresolved() {
        let inputs = WorkspaceInputs {
            aliases: vec![
                alias("@kit", Some("kit")),
                alias("@bare", Some("bare")),
                alias("@gone", Some("kit/nowhere")),
            ],
            ..WorkspaceInputs::default()
        };
        let r = resolver_with(
            &[
                ("package.json", ROOT),
                ("src/main.ts", ""),
                ("kit/package.json", r#"{"name":"kit","main":"dist/index.js"}"#),
                ("kit/src/index.ts", ""),
                ("kit/src/Button.tsx", ""),
                ("bare/package.json", r#"{"name":"bare","main":"dist/index.js"}"#),
            ],
            inputs,
        );
        let importer = Path::new("/ws/src/main.ts");
        let file = |path: &str| Resolved::File(PathBuf::from(path));
        assert_eq!(r.resolve(importer, "@kit").target, file("/ws/kit/src/index.ts"));
        assert_eq!(
            r.resolve(importer, "@kit/Button").target,
            file("/ws/kit/src/Button.tsx")
        );
        assert_eq!(r.resolve(importer, "@bare").target, Resolved::Ignored);
        assert_eq!(r.resolve(importer, "@gone").target, Resolved::Unresolved);
        assert_eq!(r.resolve(importer, "@kit/nothing").target, Resolved::Unresolved);
    }
    fn file(path: &str) -> Resolved {
        Resolved::File(PathBuf::from(path))
    }

    #[test]
    fn declaration_files_only_stand_in_for_missing_runtime_files() {
        let r = resolver(&[
            ("package.json", ROOT),
            ("src/main.js", ""),
            ("src/Card.js", ""),
            ("src/Card.d.ts", ""),
            ("src/Types.d.ts", ""),
            ("src/Typed.ts", ""),
            ("src/Typed.d.ts", ""),
        ]);
        let importer = Path::new("/ws/src/main.js");
        assert_eq!(r.resolve(importer, "./Card").target, file("/ws/src/Card.js"));
        assert_eq!(r.resolve(importer, "./Card.js").target, file("/ws/src/Card.js"));
        assert_eq!(r.resolve(importer, "./Types").target, file("/ws/src/Types.d.ts"));
        assert_eq!(r.resolve(importer, "./Typed").target, file("/ws/src/Typed.ts"));
    }

    #[test]
    fn lookup_directories_beat_installed_packages() {
        let inputs = WorkspaceInputs {
            module_dirs: vec!["src".to_owned()],
            ..WorkspaceInputs::default()
        };
        let r = resolver_with(
            &[
                ("package.json", ROOT),
                ("src/main.ts", ""),
                ("src/utils/index.ts", ""),
                (
                    "node_modules/utils/package.json",
                    r#"{"name":"utils","main":"index.js"}"#,
                ),
                ("node_modules/utils/index.js", ""),
                (
                    "node_modules/react/package.json",
                    r#"{"name":"react","main":"index.js"}"#,
                ),
                ("node_modules/react/index.js", ""),
            ],
            inputs,
        );
        let importer = Path::new("/ws/src/main.ts");
        let local = r.resolve(importer, "utils");
        assert_eq!(local.target, file("/ws/src/utils/index.ts"));
        assert_eq!(local.package, None);
        assert_eq!(r.resolve(importer, "react").package.as_deref(), Some("react"));
    }

    #[test]
    fn bundler_extensions_let_imports_omit_them() {
        let files = [("package.json", ROOT), ("src/main.js", ""), ("src/widget.page", "")];
        let importer = Path::new("/ws/src/main.js");
        assert_eq!(
            resolver(&files).resolve(importer, "./widget").target,
            Resolved::Unresolved
        );
        let inputs = WorkspaceInputs {
            extensions: vec![".page".to_owned()],
            ..WorkspaceInputs::default()
        };
        assert_eq!(
            resolver_with(&files, inputs).resolve(importer, "./widget").target,
            Resolved::Asset
        );
    }

    #[test]
    fn alias_into_node_modules_is_the_package_even_when_not_installed() {
        let inputs = WorkspaceInputs {
            aliases: vec![alias("svelte", Some("node_modules/svelte"))],
            ..WorkspaceInputs::default()
        };
        let r = resolver_with(&[("package.json", ROOT), ("src/main.js", "")], inputs);
        let outcome = r.resolve(Path::new("/ws/src/main.js"), "svelte");
        assert_eq!(outcome.target, Resolved::External("svelte".to_owned()));
        assert_eq!(outcome.package.as_deref(), Some("svelte"));
        let nested = r.resolve(Path::new("/ws/src/main.js"), "svelte/store");
        assert_eq!(nested.package.as_deref(), Some("svelte"));
    }

    #[test]
    fn root_relative_stylesheets_resolve_from_the_package_root() {
        let r = resolver(&[
            ("package.json", ROOT),
            ("src/styles/main.css", ""),
            ("src/styles/reset.css", ""),
            ("public/fonts.css", ""),
        ]);
        let importer = Path::new("/ws/src/styles/main.css");
        assert_eq!(
            r.resolve_style(importer, "/src/styles/reset.css").target,
            file("/ws/src/styles/reset.css")
        );
        assert_eq!(
            r.resolve_style(importer, "/fonts.css").target,
            file("/ws/public/fonts.css")
        );
        assert_eq!(
            r.resolve_style(importer, "/served/by/server.css").target,
            Resolved::Ignored
        );
        assert_eq!(r.resolve_style(importer, "./missing.css").target, Resolved::Unresolved);
    }

    #[test]
    fn dollar_aliases_resolve_in_stylesheets() {
        let r = resolver(&[
            ("package.json", ROOT),
            ("src/lib/theme.css", ""),
            ("src/routes/page.css", ""),
        ]);
        let importer = Path::new("/ws/src/routes/page.css");
        assert_eq!(
            r.resolve_style(importer, "$lib/theme.css").target,
            file("/ws/src/lib/theme.css")
        );
    }

    #[test]
    fn ignored_targets_are_unknown_rather_than_unresolved() {
        let inputs = WorkspaceInputs {
            aliases: vec![alias("@gen", Some("generated"))],
            ..WorkspaceInputs::default()
        };
        let r = resolver_with(
            &[
                ("package.json", ROOT),
                (".gitignore", "src/consts/\ngenerated\n!generated/kept.ts\n"),
                ("src/main.ts", ""),
            ],
            inputs,
        );
        let importer = Path::new("/ws/src/main.ts");
        assert_eq!(r.resolve(importer, "./consts/index").target, Resolved::Ignored);
        assert_eq!(r.resolve(importer, "@gen/x").target, Resolved::Ignored);
        assert_eq!(r.resolve(importer, "./other/index").target, Resolved::Unresolved);
    }

    #[test]
    fn bazel_workspace_name_prefixes_root_relative_requires() {
        for (marker, text) in [
            (
                "MODULE.bazel",
                "module(\n    name = \"mono\",\n    version = \"1.0\",\n)\n",
            ),
            ("WORKSPACE", "workspace(name = 'mono')\n"),
        ] {
            let r = resolver(&[
                ("package.json", ROOT),
                (marker, text),
                ("src/main.js", ""),
                ("lib/util.js", ""),
            ]);
            let importer = Path::new("/ws/src/main.js");
            let found = r.resolve(importer, "mono/lib/util");
            assert_eq!(found.target, file("/ws/lib/util.js"), "{marker}");
            assert_eq!(found.package, None, "{marker}");
            assert_eq!(
                r.resolve(importer, "mono/lib/missing").target,
                Resolved::External("mono".to_owned()),
                "{marker}"
            );
            assert_eq!(
                r.resolve(importer, "other/lib/util").target,
                Resolved::External("other".to_owned()),
                "{marker}"
            );
        }
    }

    #[test]
    fn bazel_name_is_read_from_the_module_call_only() {
        let fs = MemoryFs::new().with_file(
            "/ws/MODULE.bazel",
            "bazel_dep(name = \"rules_x\", version = \"1\")\nmodule(repo_name = \"r\", name = \"mono\")\n",
        );
        assert_eq!(bazel_workspace_name(&fs, Path::new("/ws")).as_deref(), Some("mono"));
        let none = MemoryFs::new().with_file("/ws/MODULE.bazel", "bazel_dep(name = \"rules_x\")\n");
        assert_eq!(bazel_workspace_name(&none, Path::new("/ws")), None);
    }
}
