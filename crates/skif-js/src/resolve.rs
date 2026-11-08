//! Module resolution on `oxc_resolver`, reading through the host's [`FileSystem`].

use std::borrow::Borrow;
use std::collections::{BTreeMap, HashMap};
use std::hash::Hash;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, PoisonError, RwLock};

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use oxc_resolver::{FileMetadata, ResolveError, ResolveOptions, ResolverGeneric, TsconfigDiscovery};
use rustc_hash::{FxBuildHasher, FxHashMap};
use serde_json::Value;
use skif_core::{FileSystem, OsFs};
use skif_js_syntax::is_stylesheet;
use skif_workspace::Workspace;

use crate::manifest_paths::{ENTRY_FIELDS, map_to_source, resolve_exports, resolve_imports};
use crate::setup::Setup;
use crate::specifier::{
    assumed_package, has_foreign_scheme, is_builtin, node_modules_package, node_modules_specifier_package,
    package_name, subpath,
};
use crate::tsconfig_paths::{TsConfigs, TsPaths};

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
        let kind = self.fs.kind(path);
        if kind.is_file {
            Ok(FileMetadata::new(true, false, false))
        } else if kind.is_dir {
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

/// A path as the specifier the resolver is asked for: with `/` separators, which it accepts for absolute paths on
/// every platform.
fn specifier_of(path: &Path) -> String {
    let text = path.to_string_lossy();
    if cfg!(windows) {
        text.replace('\\', "/")
    } else {
        text.into_owned()
    }
}

/// Whether `path` lies below `root`. Windows paths are compared without regard to case, as the file system does.
fn is_below(path: &Path, root: &Path) -> bool {
    if cfg!(windows) {
        let lower = |path: &Path| PathBuf::from(path.to_string_lossy().to_lowercase());
        lower(path).starts_with(lower(root))
    } else {
        path.starts_with(root)
    }
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
    manifest: Arc<Value>,
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

/// Extensions of files that are not modules; an import of a missing one is not reported.
const FOREIGN_FILE_EXTENSIONS: &[&str] = &[
    ".avif", ".css", ".eot", ".gif", ".html", ".ico", ".jpeg", ".jpg", ".less", ".mp3", ".otf", ".png", ".sass",
    ".scss", ".sh", ".styl", ".stylus", ".svg", ".ttf", ".webp", ".woff", ".woff2", ".yaml", ".yml",
];

/// The source extensions an import extension stands for.
const EXTENSION_ALIASES: &[(&str, &[&str])] = &[
    (".js", &[".ts", ".tsx", ".d.ts"]),
    (".jsx", &[".tsx"]),
    (".mjs", &[".mts", ".d.mts"]),
    (".cjs", &[".cts", ".d.cts"]),
];

/// Roots of `SvelteKit`'s virtual modules, which no file backs.
const VIRTUAL_ROOTS: &[&str] = &["$app", "$env", "$service-worker"];

/// The rest of `specifier` below the alias `find`, `""` for the alias itself.
fn alias_rest<'a>(specifier: &'a str, find: &str) -> Option<&'a str> {
    if specifier == find {
        return Some("");
    }
    specifier.strip_prefix(find)?.strip_prefix('/')
}

/// A map filled on first use; concurrent first uses may both compute, the results are equal. A hit allocates
/// nothing; keys are hashed with `FxHash`, as they are paths and names of the run's own tree.
struct Memo<K, V>(RwLock<FxHashMap<K, V>>);

impl<K: Eq + Hash, V: Clone> Memo<K, V> {
    fn new() -> Self {
        Self(RwLock::new(HashMap::with_hasher(FxBuildHasher)))
    }

    fn get<Q>(&self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.0.read().unwrap_or_else(PoisonError::into_inner).get(key).cloned()
    }

    fn get_or_compute<Q>(&self, key: &Q, compute: impl FnOnce() -> V) -> V
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ToOwned<Owned = K> + ?Sized,
    {
        if let Some(known) = self.get(key) {
            return known;
        }
        let value = compute();
        self.0
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key.to_owned(), value.clone());
        value
    }
}

/// Values memoised by a pair of keys (a scope index or a package name, and a specifier), misses included. A hit
/// allocates nothing.
struct PairMemo<K, V>(RwLock<FxHashMap<K, FxHashMap<String, V>>>);

impl<K: Eq + Hash + Clone, V: Clone> PairMemo<K, V> {
    fn new() -> Self {
        Self(RwLock::new(HashMap::with_hasher(FxBuildHasher)))
    }

    fn get_or_compute<Q>(&self, first: &Q, specifier: &str, compute: impl FnOnce() -> V) -> V
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ToOwned<Owned = K> + ?Sized,
    {
        if let Some(known) = self
            .0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(first)
            .and_then(|probes| probes.get(specifier))
        {
            return known.clone();
        }
        let value = compute();
        self.0
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(first.to_owned())
            .or_default()
            .insert(specifier.to_owned(), value.clone());
        value
    }
}

/// Resolves specifiers for the whole workspace. Cheap to share between threads.
pub(crate) struct ModuleResolver {
    script: Resolvers,
    /// Script resolvers for the packages whose bundler config declares `resolve.extensions`, by package directory.
    script_with_extensions: HashMap<PathBuf, Resolvers>,
    style: Resolvers,
    fs: Arc<dyn FileSystem>,
    root: PathBuf,
    /// The name the repository's Bazel `MODULE.bazel` or `WORKSPACE` gives it, which prefixes root-relative requires.
    workspace_name: Option<String>,
    /// The `.gitignore` files of the root and of the packages, with the directory each applies to.
    ignores: Vec<(PathBuf, Gitignore)>,
    members: BTreeMap<String, Member>,
    scopes: Vec<AliasScope>,
    /// The position in `scopes` of the scope of each package directory.
    scope_index: FxHashMap<PathBuf, usize>,
    /// For each scope, the lookup directories of the scopes whose directories hold it (itself included), in the
    /// order of `scopes`.
    module_dir_chains: Vec<Vec<PathBuf>>,
    /// The positions of the scopes that hold a path, innermost first, by path: every import of a file asks for them.
    scope_chains: Memo<PathBuf, Arc<[usize]>>,
    /// Whether any scope has lookup directories; without one the lookup-directory probe has nothing to do.
    has_module_dirs: bool,
    /// `exports` conditions that select source files, besides the built-in `source`.
    conditions: Vec<String>,
    /// Packages that are not workspace members, by directory, parsed once; `None` when there is no readable manifest.
    directory_packages: Memo<PathBuf, Option<Arc<Member>>>,
    /// Outcomes of the lookup-directory probe by the innermost scope of the importer and specifier.
    module_dir_probes: PairMemo<usize, Option<Resolved>>,
    /// The same for specifiers written in stylesheets.
    style_module_dir_probes: PairMemo<usize, Option<Resolved>>,
    /// The source file a workspace member exposes for a specifier, by member name and specifier.
    member_files: PairMemo<String, Option<PathBuf>>,
    /// The nearest `baseUrl` and `paths` by directory, `None` when no config at or above it sets them.
    nearest_paths: Memo<PathBuf, Option<Arc<TsPaths>>>,
    /// The `tsconfig.json` and `jsconfig.json` files read so far.
    tsconfigs: TsConfigs,
    /// Whether the nearest `package.json` declares `imports`, by the directory of the importer.
    declared_imports: Memo<PathBuf, bool>,
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
    /// Shared by every set, as the `oxc_resolver` cache is.
    tsconfigs: Arc<TsconfigFailures>,
}

/// The `tsconfig.json` files `oxc_resolver` cannot load, and the directories whose importers meet them first.
///
/// `oxc_resolver` keeps a `tsconfig` only once it loaded, so one that fails (it extends a package that is not
/// installed) is read and parsed again for every importer below it, before the resolution falls back to the
/// resolver without a `tsconfig`. The failure is the same for every importer whose nearest `tsconfig` it is.
struct TsconfigFailures {
    fs: Arc<dyn FileSystem>,
    /// Why the nearest `tsconfig.json` at or above a directory cannot be loaded, by directory; `None` when it loads,
    /// when there is none, or when `oxc_resolver` walks on past it. Filled for directories where a resolution met a
    /// `tsconfig` error.
    by_directory: Memo<PathBuf, Option<ResolveError>>,
    /// The error of loading a `tsconfig.json`, by path; `None` when it loads.
    by_file: Memo<PathBuf, Option<ResolveError>>,
    /// Whether a resolution met a `tsconfig` error; until then no directory is looked at.
    seen: AtomicBool,
}

impl TsconfigFailures {
    /// The directory whose memo answers for `importer`, `None` for an importer `oxc_resolver` treats differently:
    /// one it looks up no `tsconfig` for (inside `node_modules`, relative), one whose path it would read as a
    /// specifier with a query or fragment, or one that is not normalised.
    fn directory(importer: &Path) -> Option<&Path> {
        let directory = importer.parent()?;
        let text = importer.as_os_str().as_encoded_bytes();
        let plain = importer.is_absolute()
            && !text.contains(&b'#')
            && !text.contains(&b'?')
            && importer.components().all(|component| match component {
                Component::Normal(name) => name != "node_modules",
                Component::RootDir | Component::Prefix(_) => true,
                Component::CurDir | Component::ParentDir => false,
            });
        plain.then_some(directory)
    }

    /// The `tsconfig` error every resolution from `importer` meets, once a resolution of the run met one.
    fn known(&self, resolver: &ResolverGeneric<ResolverFs>, importer: &Path) -> Option<ResolveError> {
        if !self.seen.load(Ordering::Relaxed) {
            return None;
        }
        self.of_directory(resolver, Self::directory(importer)?)
    }

    /// Records that a resolution from `importer` met a `tsconfig` error.
    fn record(&self, resolver: &ResolverGeneric<ResolverFs>, importer: &Path) {
        self.seen.store(true, Ordering::Relaxed);
        if let Some(directory) = Self::directory(importer) {
            self.of_directory(resolver, directory);
        }
    }

    /// The `tsconfig` error of the importers in `directory`: `oxc_resolver` walks up from the importer to the first
    /// `tsconfig.json`; one that fails to load is the error of the resolution, one that cannot be read is skipped.
    fn of_directory(&self, resolver: &ResolverGeneric<ResolverFs>, directory: &Path) -> Option<ResolveError> {
        self.by_directory.get_or_compute(directory, || {
            let config = directory
                .ancestors()
                .map(|dir| dir.join("tsconfig.json"))
                .find(|config| self.fs.is_file(config))?;
            self.by_file
                .get_or_compute(&config, || match resolver.resolve_tsconfig(&config) {
                    Ok(_) => None,
                    Err(ResolveError::TsconfigLoadFailed { source, .. })
                        if matches!(*source, ResolveError::IOError(_)) =>
                    {
                        None
                    }
                    Err(error) => Some(error),
                })
        })
    }
}

impl Resolvers {
    /// The first set of resolvers, which owns the path, `package.json` and `tsconfig` cache every other set shares.
    fn new(fs: &Arc<dyn FileSystem>, options: impl Fn(bool) -> ResolveOptions) -> Self {
        let with_tsconfig = ResolverGeneric::new_with_file_system(ResolverFs { fs: Arc::clone(fs) }, options(true));
        let plain = with_tsconfig.clone_with_options(options(false));
        Self {
            with_tsconfig,
            plain,
            tsconfigs: Arc::new(TsconfigFailures {
                fs: Arc::clone(fs),
                by_directory: Memo::new(),
                by_file: Memo::new(),
                seen: AtomicBool::new(false),
            }),
        }
    }

    /// Another set with other options over the same cache, so a path is looked up on disk once however many sets
    /// (one per package with bundler extensions, one for styles) ask for it.
    fn sharing_cache(&self, options: impl Fn(bool) -> ResolveOptions) -> Self {
        Self {
            with_tsconfig: self.with_tsconfig.clone_with_options(options(true)),
            plain: self.with_tsconfig.clone_with_options(options(false)),
            tsconfigs: Arc::clone(&self.tsconfigs),
        }
    }

    fn attempt(&self, importer: &Path, specifier: &str) -> Result<PathBuf, ResolveError> {
        // A `tsconfig` known not to load is the error `resolve_file` would return for every importer below it: go
        // straight to the fallback instead of reading and parsing it again.
        let with_tsconfig = match self.tsconfigs.known(&self.with_tsconfig, importer) {
            Some(error) => Err(error),
            None => self.with_tsconfig.resolve_file(importer, specifier),
        };
        match with_tsconfig {
            Ok(resolution) => Ok(resolution.path().to_path_buf()),
            Err(error @ (ResolveError::NotFound(_) | ResolveError::MatchedAliasNotFound(..))) => Err(error),
            Err(error) => {
                if matches!(
                    error,
                    ResolveError::TsconfigNotFound(_)
                        | ResolveError::TsconfigLoadFailed { .. }
                        | ResolveError::TsconfigCircularExtend(_)
                        | ResolveError::TsconfigSelfReference(_)
                ) {
                    self.tsconfigs.record(&self.with_tsconfig, importer);
                }
                self.plain
                    .resolve_file(importer, specifier)
                    .map(|resolution| resolution.path().to_path_buf())
            }
        }
    }
}

/// For each scope, the lookup directories of the scopes whose directories hold it (itself included), in the order of
/// `scopes`.
fn module_dir_chains(scopes: &[AliasScope]) -> Vec<Vec<PathBuf>> {
    scopes
        .iter()
        .map(|innermost| {
            scopes
                .iter()
                .filter(|scope| innermost.dir.starts_with(&scope.dir))
                .flat_map(|scope| scope.module_dirs.iter().cloned())
                .collect()
        })
        .collect()
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

/// The forms a stylesheet specifier can name a file by: itself, its Sass partial (`_name`) and the index partial of a
/// directory.
fn style_forms(specifier: &str) -> [String; 3] {
    let trimmed = specifier.trim_end_matches('/');
    let (dir, name) = trimmed
        .rsplit_once('/')
        .map_or(("", trimmed), |(dir, name)| (dir, name));
    let prefix = if dir.is_empty() {
        String::new()
    } else {
        format!("{dir}/")
    };
    [
        specifier.to_owned(),
        format!("{prefix}_{name}"),
        format!("{prefix}{name}/_index"),
    ]
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
        let script = Resolvers::new(&fs, |tsconfig| options(tsconfig, &conditions, &[]));
        let script_with_extensions = setup
            .packages
            .iter()
            .flatten()
            .filter(|package| !package.extensions.is_empty())
            .map(|package| {
                let resolvers = script.sharing_cache(|tsconfig| options(tsconfig, &conditions, &package.extensions));
                (package.dir.clone(), resolvers)
            })
            .collect();
        let style = script.sharing_cache(style_options);
        let members: BTreeMap<String, Member> = workspace
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
                        manifest: package.manifest.shared_value(),
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
        let scope_index = scopes
            .iter()
            .enumerate()
            .map(|(position, scope)| (scope.dir.clone(), position))
            .collect();
        let has_module_dirs = scopes.iter().any(|scope| !scope.module_dirs.is_empty());
        let module_dir_chains = if has_module_dirs {
            module_dir_chains(&scopes)
        } else {
            Vec::new()
        };
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
        let tsconfigs = TsConfigs::new(
            Arc::clone(&fs),
            workspace.root.clone(),
            members
                .iter()
                .map(|(name, member)| (name.clone(), member.dir.clone()))
                .collect(),
        );
        Self {
            script,
            script_with_extensions,
            style,
            workspace_name: bazel_workspace_name(fs.as_ref(), &workspace.root),
            ignores,
            fs,
            root: workspace.root.clone(),
            members,
            scopes,
            scope_index,
            module_dir_chains,
            scope_chains: Memo::new(),
            has_module_dirs,
            conditions,
            directory_packages: Memo::new(),
            module_dir_probes: PairMemo::new(),
            style_module_dir_probes: PairMemo::new(),
            member_files: PairMemo::new(),
            nearest_paths: Memo::new(),
            tsconfigs,
            declared_imports: Memo::new(),
        }
    }

    fn classify(&self, path: PathBuf) -> Resolved {
        if is_below(&path, &self.root) && (is_code_file(&path) || is_stylesheet(&path)) {
            Resolved::File(path)
        } else {
            Resolved::Asset
        }
    }

    /// The script resolvers for files below `within`: those with the `resolve.extensions` of the innermost package
    /// that holds it, when that package declares any.
    fn script_resolvers(&self, within: &Path) -> &Resolvers {
        if self.script_with_extensions.is_empty() {
            return &self.script;
        }
        self.scopes_of(within)
            .next()
            .and_then(|(_, scope)| self.script_with_extensions.get(&scope.dir))
            .unwrap_or(&self.script)
    }

    fn resolvers(&self, kind: Kind, within: &Path) -> &Resolvers {
        match kind {
            Kind::Script => self.script_resolvers(within),
            Kind::Style => &self.style,
        }
    }

    /// Whether `path` is listed in a `.gitignore`, so it may be generated and absent from a fresh checkout.
    fn is_generated(&self, path: &Path) -> bool {
        self.ignores
            .iter()
            .any(|(dir, ignore)| path.starts_with(dir) && ignore.matched_path_or_any_parents(path, false).is_ignore())
    }

    /// Whether the nearest `package.json` above `importer` declares `imports`, the only place a `#` specifier can be
    /// resolved from. An unreadable manifest counts as declaring them.
    fn declares_imports(&self, importer: &Path) -> bool {
        let Some(directory) = importer.parent() else {
            return false;
        };
        self.declared_imports.get_or_compute(directory, || {
            for dir in directory.ancestors().take_while(|dir| dir.starts_with(&self.root)) {
                if let Some(member) = self.members.values().find(|member| member.dir == dir) {
                    return member.manifest.get("imports").is_some();
                }
                if let Ok(text) = self.fs.read_to_string(&dir.join("package.json")) {
                    return serde_json::from_str::<Value>(&text)
                        .map_or(true, |manifest| manifest.get("imports").is_some());
                }
            }
            false
        })
    }

    /// Whether a `paths` pattern of the nearest `tsconfig.json` or `jsconfig.json` maps `specifier`, whether or not
    /// its targets exist.
    fn ts_paths_map(&self, importer: &Path, specifier: &str) -> bool {
        let Some(directory) = importer.parent() else {
            return false;
        };
        self.nearest_paths
            .get_or_compute(directory, || self.tsconfigs.nearest(directory).map(Arc::new))
            .is_some_and(|config| config.maps(specifier))
    }

    /// What a specifier that names nothing is. A relative one is unknown when it points at an ignored path (also
    /// through the source extensions its extension stands for, or, without a code extension, the files and directory
    /// index it can name) or, in a script, at a JSON or foreign file. A `#` specifier is unknown when its package
    /// declares no `imports` and no `paths` pattern maps it: a tool, not Node.js, resolves it. (A configured alias
    /// that maps it never gets here.) Anything else is unresolved.
    fn missing(&self, kind: Kind, importer: &Path, specifier: &str) -> Resolved {
        if specifier.starts_with('#') && !self.declares_imports(importer) && !self.ts_paths_map(importer, specifier) {
            return Resolved::Ignored;
        }
        if !specifier.starts_with('.') {
            return Resolved::Unresolved;
        }
        let extension = Path::new(specifier)
            .extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| format!(".{}", extension.to_ascii_lowercase()));
        if kind == Kind::Script
            && extension
                .as_deref()
                .is_some_and(|extension| extension == ".json" || FOREIGN_FILE_EXTENSIONS.contains(&extension))
        {
            return Resolved::Ignored;
        }
        let Some(dir) = importer.parent() else {
            return Resolved::Unresolved;
        };
        let candidate = join_normalized(dir, specifier);
        let aliased = extension.as_deref().and_then(|extension| {
            let (_, aliases) = EXTENSION_ALIASES.iter().find(|(from, _)| *from == extension)?;
            let stem = specifier.get(..specifier.len() - extension.len())?;
            Some(
                aliases
                    .iter()
                    .map(move |alias| join_normalized(dir, &format!("{stem}{alias}"))),
            )
        });
        // An extensionless specifier names a file by one of the code extensions, or the index file of a directory:
        // `./migrations/sqlite` is generated when `migrations/sqlite/index.ts` is ignored.
        let implied = extension
            .as_deref()
            .is_none_or(|extension| !CODE_EXTENSIONS.contains(&&extension[1..]))
            .then(|| {
                CODE_EXTENSIONS.iter().flat_map(|code| {
                    let mut sibling = candidate.as_os_str().to_owned();
                    sibling.push(format!(".{code}"));
                    [PathBuf::from(sibling), candidate.join(format!("index.{code}"))]
                })
            });
        if self.is_generated(&candidate)
            || aliased.is_some_and(|mut paths| paths.any(|path| self.is_generated(&path)))
            || implied.is_some_and(|mut paths| paths.any(|path| self.is_generated(&path)))
        {
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
            // A `node_modules` directory that is part of the sources is followed like any other path.
            if let Ok(path) = self.resolvers(kind, importer).attempt(importer, specifier)
                && node_modules_package(&path).is_none()
            {
                return outcome(self.classify(path), None);
            }
            return self.package_target(&name, specifier);
        }
        let name = package_name(specifier);
        if name.is_some()
            && let Some(found) = self
                .through_workspace_name(specifier)
                .or_else(|| self.through_module_dirs(kind, importer, specifier))
        {
            return outcome(found, None);
        }
        let member_name = name
            .as_ref()
            .filter(|name| self.members.contains_key(name.as_str()))
            .cloned();
        if let Ok(path) = self.resolvers(kind, importer).attempt(importer, specifier) {
            if let Some(installed) = node_modules_package(&path) {
                let package = name.unwrap_or(installed);
                return self.package_target(&package, specifier);
            }
            return outcome(self.classify(path), member_name);
        }
        if specifier.starts_with('.')
            && let Some(found) = self
                .through_root_dirs(importer, specifier)
                .or_else(|| self.through_relative_package_directory(importer, specifier))
        {
            return outcome(found, None);
        }
        if specifier.starts_with('#')
            && let Some(found) = self.through_imports(importer, specifier)
        {
            return outcome(found, None);
        }
        let aliased = self.through_alias(kind, importer, specifier);
        if aliased
            .as_ref()
            .is_none_or(|resolved| matches!(resolved, Resolved::Unresolved))
            && let Some(found) = self.through_nearest_tsconfig(kind, importer, specifier)
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
            && let Some(found) = self.through_module_dirs(kind, importer, specifier)
        {
            return outcome(found, None);
        }
        // A specifier that starts like a package name and resolves to nothing is taken for a dependency, which the
        // dependency rules then judge, as an alias that points nowhere is.
        let assumed = || assumed_package(specifier).map(|package| self.package_target(&package, specifier));
        match (aliased, name) {
            (Some(Resolved::Unresolved), _) => assumed().unwrap_or_else(|| outcome(Resolved::Unresolved, None)),
            (Some(aliased), _) => outcome(aliased, None),
            (None, Some(name)) => self.package_target(&name, specifier),
            (None, None) => assumed().unwrap_or_else(|| outcome(self.missing(kind, importer, specifier), None)),
        }
    }

    /// A stylesheet next to `importer`: `specifier` itself, its Sass partial (`_name`) or the index partial of a
    /// directory.
    fn local_style(&self, importer: &Path, specifier: &str) -> Option<PathBuf> {
        style_forms(specifier)
            .iter()
            .find_map(|candidate| self.style.attempt(importer, candidate).ok())
    }

    /// The file `path` names for a language, looked up from `directory`: a script as the resolver finds it, a
    /// stylesheet also as a Sass partial (`_name`) or the index partial of a directory.
    fn resolve_path(&self, kind: Kind, directory: &Path, path: &Path) -> Option<PathBuf> {
        let plain = &self.resolvers(kind, directory).plain;
        match kind {
            Kind::Script => plain
                .resolve(directory, &specifier_of(path))
                .ok()
                .map(|resolution| resolution.path().to_path_buf()),
            Kind::Style => style_forms(&specifier_of(path))
                .iter()
                .find_map(|candidate| plain.resolve(directory, candidate).ok())
                .map(|resolution| resolution.path().to_path_buf()),
        }
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
            .scopes_of(importer)
            .next()
            .map_or(self.root.as_path(), |(_, scope)| scope.dir.as_path());
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
                return outcome(self.missing(Kind::Style, importer, specifier), None);
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
            .resolve(&self.root, &specifier_of(&path))
            .ok()
            .map(|resolution| self.classify(resolution.path().to_path_buf()))
    }

    /// The scopes whose directory holds `importer`, innermost first, with their position in `scopes`.
    ///
    /// Found by walking up the importer's ancestors instead of testing every scope, once per importer.
    fn scopes_of(&self, importer: &Path) -> impl Iterator<Item = (usize, &AliasScope)> + '_ {
        let chain = self.scope_chains.get_or_compute(importer, || {
            importer
                .ancestors()
                .filter_map(|dir| self.scope_index.get(dir).copied())
                .collect()
        });
        (0..chain.len()).map(move |at| {
            let position = chain[at];
            (position, &self.scopes[position])
        })
    }

    /// Looks a bare specifier up in the lookup directories of the importer's package (`baseUrl`, `paths` wildcards,
    /// `resolve.modules`), in order. A specifier that names a file there is a file import, never a package.
    fn through_module_dirs(&self, kind: Kind, importer: &Path, specifier: &str) -> Option<Resolved> {
        if !self.has_module_dirs {
            return None;
        }
        // The packages whose directories hold the importer form a chain, so the innermost one identifies them all.
        let (position, innermost) = self.scopes_of(importer).next()?;
        let probes = match kind {
            Kind::Script => &self.module_dir_probes,
            Kind::Style => &self.style_module_dir_probes,
        };
        probes.get_or_compute(&position, specifier, || {
            self.module_dir_chains[position].iter().find_map(|dir| {
                let path = join_normalized(dir, specifier);
                self.resolve_path(kind, &innermost.dir, &path)
                    .map(|found| self.classify(found))
            })
        })
    }

    /// Follows `paths` and `baseUrl` of the nearest `tsconfig.json` or `jsconfig.json` at or above the importer, also
    /// when it is in a directory that is not a package or does not include the importer.
    fn through_nearest_tsconfig(&self, kind: Kind, importer: &Path, specifier: &str) -> Option<Resolved> {
        let directory = importer.parent()?;
        let config = self
            .nearest_paths
            .get_or_compute(directory, || self.tsconfigs.nearest(directory).map(Arc::new))?;
        config.candidates(specifier).into_iter().find_map(|path| {
            if let Some(package) = node_modules_package(&path) {
                return Some(Resolved::External(package));
            }
            self.resolve_path(kind, directory, &path)
                .map(|found| self.classify(found))
        })
    }

    /// A relative specifier through the `rootDirs` of the nearest `tsconfig.json`: the same path below another root
    /// directory, as for generated code that lives beside the sources in a virtual directory.
    fn through_root_dirs(&self, importer: &Path, specifier: &str) -> Option<Resolved> {
        let directory = importer.parent()?;
        let config = self
            .nearest_paths
            .get_or_compute(directory, || self.tsconfigs.nearest(directory).map(Arc::new))?;
        config
            .root_dir_candidates(directory, specifier)
            .into_iter()
            .find_map(|path| {
                self.resolve_path(Kind::Script, directory, &path)
                    .map(|found| self.classify(found))
            })
    }

    /// A `#` specifier through the `imports` of the package that holds the importer, for targets that point at build
    /// output: the source file behind it, found through the package's `outDir` and `rootDir` like `exports` targets.
    fn through_imports(&self, importer: &Path, specifier: &str) -> Option<Resolved> {
        let member = self
            .members
            .values()
            .filter(|member| importer.starts_with(&member.dir))
            .max_by_key(|member| member.dir.components().count())?;
        let imports = member.manifest.get("imports")?;
        let target = resolve_imports(imports, specifier, &self.conditions).filter(|target| target.starts_with("./"))?;
        self.map_member_target(member, &target).map(|path| self.classify(path))
    }

    /// Follows the aliases declared for the importer's package (and the packages above it).
    ///
    /// `None` when no alias applies. An alias whose target the configuration computes is not followed but is known not
    /// to be a package; one whose static target does not exist is unresolved.
    fn through_alias(&self, kind: Kind, importer: &Path, specifier: &str) -> Option<Resolved> {
        let root = specifier.split('/').next().unwrap_or(specifier);
        if VIRTUAL_ROOTS.contains(&root) {
            return Some(Resolved::Ignored);
        }
        let mut computed = false;
        let mut matched = false;
        let directory = importer.parent()?;
        for (_, scope) in self.scopes_of(importer) {
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
                if let Some(found) = self.resolve_path(kind, directory, &path) {
                    return Some(self.classify(found));
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

    /// A relative specifier that names a package directory (`..`, `../../automation`) whose `main` or `exports` point
    /// at output that was never built: its source file by the usual output-to-source mapping, else a reference that is
    /// known to exist.
    fn through_relative_package_directory(&self, importer: &Path, specifier: &str) -> Option<Resolved> {
        let directory = join_normalized(importer.parent()?, specifier);
        self.through_package_directory(&directory, "")
    }

    /// The package in `dir` that is not a workspace member, with its manifest parsed once per directory.
    fn directory_package(&self, dir: &Path) -> Option<Arc<Member>> {
        self.directory_packages.get_or_compute(dir, || {
            let text = self.fs.read_to_string(&dir.join("package.json")).ok()?;
            Some(Arc::new(Member {
                dir: dir.to_path_buf(),
                manifest: Arc::new(serde_json::from_str(&text).ok()?),
                build_dirs: Vec::new(),
            }))
        })
    }

    fn package_target(&self, name: &str, specifier: &str) -> Outcome {
        let member_file = self.members.get(name).and_then(|member| {
            self.member_files
                .get_or_compute(name, specifier, || self.member_file(member, subpath(specifier, name)))
        });
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
        // The member's own build directories first, then those other packages declare (a root tsconfig can name the
        // output directories of every member).
        let declared = member.build_dirs.iter().chain(
            self.members
                .values()
                .filter(|other| other.dir != member.dir)
                .flat_map(|other| other.build_dirs.iter()),
        );
        for (out_dir, root_dir) in declared {
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
        let setup = Setup::new(&workspace, &config, &fs, &sources, true);
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
        assert_eq!(gone.target, Resolved::External("@gone/x".to_owned()));
        assert_eq!(gone.package, Some("@gone/x".to_owned()));
        let unknown = r.resolve(importer, "@/missing");
        assert_eq!(unknown.target, Resolved::Unresolved);
        assert_eq!(unknown.package, None);
        assert_eq!(
            r.resolve(importer, "@scope/real").target,
            Resolved::External("@scope/real".to_owned())
        );
    }

    #[test]
    fn unresolvable_specifiers_that_start_like_packages_are_dependencies() {
        let r = resolver(&[
            ("package.json", ROOT),
            ("packages/a/package.json", r#"{"name":"@x/a"}"#),
            (
                "packages/ui/package.json",
                r#"{"name":"@x/ui","main":"./dist/index.js","exports":{".":"./dist/index.js"}}"#,
            ),
            ("packages/a/src/a.ts", ""),
            ("packages/a/src/style.scss", ""),
        ]);
        let importer = Path::new("/ws/packages/a/src/a.ts");
        for specifier in ["@x/ui", "@x/ui/dist/missing", "left-pad/lib/x", "pkg/css/index?inline"] {
            let outcome = r.resolve(importer, specifier);
            assert!(
                matches!(outcome.target, Resolved::External(_)),
                "{specifier}: {outcome:?}"
            );
        }
        let style = r.resolve_style(Path::new("/ws/packages/a/src/style.scss"), "pkg/css/index");
        assert_eq!(style.target, Resolved::External("pkg".to_owned()));
        assert_eq!(
            r.resolve(importer, "https://example.com/x.js").target,
            Resolved::Ignored
        );
        assert_eq!(r.resolve(importer, "@/lib/x").target, Resolved::Unresolved);
        assert_eq!(r.resolve(importer, "./missing").target, Resolved::Unresolved);
    }

    #[test]
    fn missing_relative_files_of_foreign_types_and_json_are_not_unresolved() {
        let r = resolver(&[("package.json", ROOT), ("src/a.ts", "")]);
        let importer = Path::new("/ws/src/a.ts");
        for specifier in [
            "./data.json",
            "./theme.css",
            "../img/logo.svg",
            "./fonts/x.woff2",
            "./a.scss",
        ] {
            assert_eq!(r.resolve(importer, specifier).target, Resolved::Ignored, "{specifier}");
        }
        assert_eq!(r.resolve(importer, "./missing.ts").target, Resolved::Unresolved);
    }

    #[test]
    fn missing_relative_files_ignored_by_git_are_not_unresolved_through_extension_aliases() {
        let r = resolver(&[
            ("package.json", ROOT),
            (".gitignore", "generated/\n*.gen.ts\n"),
            ("src/a.ts", ""),
        ]);
        let importer = Path::new("/ws/src/a.ts");
        assert_eq!(r.resolve(importer, "../generated/x").target, Resolved::Ignored);
        assert_eq!(r.resolve(importer, "./schema.gen.js").target, Resolved::Ignored);
        assert_eq!(r.resolve(importer, "./schema.gen.ts").target, Resolved::Ignored);
        assert_eq!(r.resolve(importer, "./schema.js").target, Resolved::Unresolved);
    }

    #[test]
    fn missing_extensionless_files_and_directories_ignored_by_git_are_not_unresolved() {
        let r = resolver(&[
            ("package.json", ROOT),
            (
                ".gitignore",
                "src/migrations/sqlite/index.ts
src/schema.gen.ts
",
            ),
            ("src/a.ts", ""),
        ]);
        let importer = Path::new("/ws/src/a.ts");
        assert_eq!(r.resolve(importer, "./migrations/sqlite").target, Resolved::Ignored);
        assert_eq!(r.resolve(importer, "./schema.gen").target, Resolved::Ignored);
        assert_eq!(
            r.resolve(importer, "./migrations/postgres").target,
            Resolved::Unresolved
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
    fn relative_imports_cross_root_dirs() {
        let r = resolver(&[
            ("package.json", ROOT),
            (
                "packages/a/tsconfig.json",
                r#"{"compilerOptions":{"rootDirs":["src","bazel-out/bin/src"]}}"#,
            ),
            ("packages/a/src/ui/view.ts", ""),
            ("packages/a/bazel-out/bin/src/ui/generated.ts", ""),
        ]);
        let importer = Path::new("/ws/packages/a/src/ui/view.ts");
        assert_eq!(
            r.resolve(importer, "./generated").target,
            Resolved::File(PathBuf::from("/ws/packages/a/bazel-out/bin/src/ui/generated.ts"))
        );
        assert_eq!(r.resolve(importer, "./absent").target, Resolved::Unresolved);
    }

    #[test]
    fn hash_specifiers_are_unresolved_only_where_the_package_declares_imports() {
        let r = resolver(&[
            ("package.json", ROOT),
            (
                "packages/a/package.json",
                r##"{"name":"@x/a","imports":{"#x":"./src/x.js"}}"##,
            ),
            ("packages/a/src/a.ts", ""),
            ("packages/b/package.json", r#"{"name":"@x/b"}"#),
            ("packages/b/src/b.ts", ""),
            (
                "packages/b/templates/t/package.json",
                r##"{"imports":{"#t":"./t.js"}}"##,
            ),
            ("packages/b/templates/t/index.ts", ""),
        ]);
        let a = Path::new("/ws/packages/a/src/a.ts");
        let b = Path::new("/ws/packages/b/src/b.ts");
        let template = Path::new("/ws/packages/b/templates/t/index.ts");
        assert_eq!(r.resolve(a, "#missing").target, Resolved::Unresolved);
        assert_eq!(r.resolve(b, "#missing").target, Resolved::Ignored);
        assert_eq!(r.resolve(template, "#missing").target, Resolved::Unresolved);
    }

    #[test]
    fn hash_specifiers_mapped_by_tsconfig_paths_or_an_alias_stay_unresolved_without_imports() {
        let inputs = WorkspaceInputs {
            aliases: vec![alias("#aliased", Some("src/aliased"))],
            ..WorkspaceInputs::default()
        };
        let r = resolver_with(
            &[
                ("package.json", r#"{"name":"root","private":true}"#),
                (
                    "tsconfig.json",
                    r##"{"compilerOptions":{"paths":{"#gen/*":["./src/gen/*"]}}}"##,
                ),
                ("src/a.ts", ""),
            ],
            inputs,
        );
        let importer = Path::new("/ws/src/a.ts");
        assert_eq!(r.resolve(importer, "#gen/missing").target, Resolved::Unresolved);
        assert_eq!(r.resolve(importer, "#aliased/missing").target, Resolved::Unresolved);
        assert_eq!(r.resolve(importer, "#unmapped").target, Resolved::Ignored);
    }

    #[test]
    fn imports_pointing_at_build_output_map_back_to_sources() {
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
                (
                    "packages/a/package.json",
                    r##"{"name":"@x/a","imports":{"#x":"./dist/x.js","#y/*":"./dist/y/*.js","#z":{"node":"./dist/z.js","default":"./dist/z.alt.js"}}}"##,
                ),
                ("packages/a/src/x.ts", ""),
                ("packages/a/src/y/deep.ts", ""),
                ("packages/a/src/z.ts", ""),
                ("packages/a/src/a.ts", ""),
                (
                    "packages/b/package.json",
                    r##"{"name":"@x/b","imports":{"#util":"./build/util.mjs"}}"##,
                ),
                ("packages/b/lib/util.ts", ""),
                ("packages/b/lib/b.ts", ""),
            ],
            inputs,
        );
        let a = Path::new("/ws/packages/a/src/a.ts");
        for (specifier, source) in [("#x", "x.ts"), ("#y/deep", "y/deep.ts"), ("#z", "z.ts")] {
            assert_eq!(
                r.resolve(a, specifier).target,
                Resolved::File(PathBuf::from(format!("/ws/packages/a/src/{source}"))),
                "{specifier}"
            );
        }
        assert_eq!(
            r.resolve(Path::new("/ws/packages/b/lib/b.ts"), "#util").target,
            Resolved::File(PathBuf::from("/ws/packages/b/lib/util.ts"))
        );
        assert_eq!(r.resolve(a, "#missing").target, Resolved::Unresolved);
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
        assert_eq!(
            r.resolve(importer, "@kit/nothing").target,
            Resolved::External("@kit/nothing".to_owned())
        );
    }

    #[test]
    fn a_relative_package_directory_with_unbuilt_output_maps_to_source() {
        let r = resolver(&[
            ("package.json", ROOT),
            (
                "packages/cli/package.json",
                r#"{"name":"cli","main":"./build/index.js","bin":{"cli":"./bin/cli.js"}}"#,
            ),
            ("packages/cli/bin/cli.js", ""),
            ("packages/cli/src/index.ts", ""),
            (
                "test/automation/package.json",
                r#"{"name":"automation","main":"out/index.js"}"#,
            ),
            ("test/automation/src/index.ts", ""),
            ("test/smoke/src/main.ts", ""),
            ("test/bare/package.json", r#"{"name":"bare","main":"out/index.js"}"#),
        ]);
        assert_eq!(
            r.resolve(Path::new("/ws/packages/cli/bin/cli.js"), "..").target,
            file("/ws/packages/cli/src/index.ts")
        );
        let smoke = Path::new("/ws/test/smoke/src/main.ts");
        assert_eq!(
            r.resolve(smoke, "../../automation").target,
            file("/ws/test/automation/src/index.ts")
        );
        assert_eq!(r.resolve(smoke, "../../bare").target, Resolved::Ignored);
        assert_eq!(r.resolve(smoke, "../../missing").target, Resolved::Unresolved);
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

    /// A [`MemoryFs`] that counts how often each path is asked for its kind.
    struct KindCounts {
        inner: MemoryFs,
        kinds: std::sync::Mutex<HashMap<PathBuf, usize>>,
    }

    impl FileSystem for KindCounts {
        fn read_to_string(&self, path: &Path) -> io::Result<Arc<str>> {
            self.inner.read_to_string(path)
        }

        fn read_dir(&self, path: &Path) -> io::Result<Vec<skif_core::DirEntry>> {
            self.inner.read_dir(path)
        }

        fn is_file(&self, path: &Path) -> bool {
            self.kind(path).is_file
        }

        fn is_dir(&self, path: &Path) -> bool {
            self.kind(path).is_dir
        }

        fn kind(&self, path: &Path) -> skif_core::PathKind {
            *self.kinds.lock().unwrap().entry(path.to_path_buf()).or_default() += 1;
            self.inner.kind(path)
        }
    }

    #[test]
    fn stylesheet_forms_of_a_directory_specifier_have_no_double_slash() {
        assert_eq!(
            style_forms("bootstrap/scss/"),
            ["bootstrap/scss/", "bootstrap/_scss", "bootstrap/scss/_index"]
        );
        assert_eq!(style_forms("theme"), ["theme", "_theme", "theme/_index"]);
    }

    #[test]
    fn every_set_of_resolvers_shares_one_lookup_of_a_path() {
        let root = Path::new("/ws");
        let files = [
            ("package.json", ROOT),
            ("src/main.js", ""),
            ("src/main.scss", ""),
            ("src/widget.page", ""),
        ];
        let memory = files
            .iter()
            .fold(MemoryFs::new(), |fs, (path, text)| fs.with_file(root.join(path), text));
        let config = Config::from_toml_str("").unwrap();
        let workspace = Workspace::discover_with_fs(root, &config, &memory).unwrap();
        let counting = Arc::new(KindCounts {
            inner: memory,
            kinds: std::sync::Mutex::default(),
        });
        let fs: Arc<dyn FileSystem> = Arc::clone(&counting) as Arc<dyn FileSystem>;
        let inputs = WorkspaceInputs {
            extensions: vec![".page".to_owned()],
            ..WorkspaceInputs::default()
        };
        let sources: Vec<Arc<dyn InputSource>> = vec![Arc::new(RootInputs(inputs))];
        let setup = Setup::new(&workspace, &config, &fs, &sources, true);
        let r = ModuleResolver::new(fs, &workspace, &setup);
        assert_eq!(r.script_with_extensions.len(), 1);
        let with_extensions = r.script_with_extensions.values().next().unwrap();

        let importer = Path::new("/ws/src/main.js");
        for resolvers in [&r.script, with_extensions, &r.style] {
            let _ = resolvers.attempt(importer, "./widget");
        }

        let kinds = counting.kinds.lock().unwrap();
        assert!(kinds.len() > 3, "the resolvers looked at {kinds:?}");
        let repeated: Vec<_> = kinds.iter().filter(|(_, count)| **count > 1).collect();
        assert_eq!(repeated, Vec::<(&PathBuf, &usize)>::new());
    }

    /// A [`MemoryFs`] that counts the reads of files named `tsconfig.json`.
    struct TsconfigReads {
        inner: MemoryFs,
        reads: std::sync::atomic::AtomicUsize,
    }

    impl FileSystem for TsconfigReads {
        fn read_to_string(&self, path: &Path) -> io::Result<Arc<str>> {
            if path.file_name().is_some_and(|name| name == "tsconfig.json") {
                self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            self.inner.read_to_string(path)
        }

        fn read_dir(&self, path: &Path) -> io::Result<Vec<skif_core::DirEntry>> {
            self.inner.read_dir(path)
        }

        fn is_file(&self, path: &Path) -> bool {
            self.inner.is_file(path)
        }

        fn is_dir(&self, path: &Path) -> bool {
            self.inner.is_dir(path)
        }
    }

    #[test]
    fn a_tsconfig_that_cannot_be_loaded_is_read_once_for_every_importer_below_it() {
        // The resolver only applies a `tsconfig` found below an absolute path, and `/ws` has no drive on Windows.
        let root = Path::new(if cfg!(windows) { "C:/ws" } else { "/ws" });
        let files = [
            ("package.json", ROOT),
            ("packages/a/package.json", r#"{"name":"@x/a"}"#),
            // The base config is a package that is not installed, as in a checkout before `install`.
            ("packages/a/tsconfig.json", r#"{"extends":"@x/tsconfig/base.json"}"#),
            ("packages/a/src/a.ts", ""),
            ("packages/a/src/b.ts", ""),
            ("packages/a/src/c.ts", ""),
            ("packages/a/src/deep/d.ts", ""),
        ];
        let memory = files
            .iter()
            .fold(MemoryFs::new(), |fs, (path, text)| fs.with_file(root.join(path), text));
        let config = Config::from_toml_str("").unwrap();
        let workspace = Workspace::discover_with_fs(root, &config, &memory).unwrap();
        let counting = Arc::new(TsconfigReads {
            inner: memory,
            reads: std::sync::atomic::AtomicUsize::new(0),
        });
        let fs: Arc<dyn FileSystem> = Arc::clone(&counting) as Arc<dyn FileSystem>;
        let sources: Vec<Arc<dyn InputSource>> = vec![Arc::new(RootInputs(WorkspaceInputs::default()))];
        let setup = Setup::new(&workspace, &config, &fs, &sources, true);
        let r = ModuleResolver::new(fs, &workspace, &setup);
        let before = counting.reads.load(std::sync::atomic::Ordering::SeqCst);

        let src = root.join("packages/a/src");
        for (importer, specifier, found) in [
            ("a.ts", "./b", "b.ts"),
            ("a.ts", "./c.js", "c.ts"),
            ("b.ts", "./c", "c.ts"),
            ("c.ts", "./deep/d", "deep/d.ts"),
            ("deep/d.ts", "../a", "a.ts"),
        ] {
            assert_eq!(
                r.resolve(&src.join(importer), specifier).target,
                Resolved::File(src.join(found)),
                "{specifier} from {importer}"
            );
        }
        // Once by the first resolution that fails on it and once to record the failure, instead of once per importer.
        let reads = counting.reads.load(std::sync::atomic::Ordering::SeqCst) - before;
        assert_eq!(reads, 2, "the tsconfig was read {reads} times");
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
