//! Module resolution on `oxc_resolver`, reading through the host's [`FileSystem`].

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use oxc_resolver::{FileMetadata, ResolveError, ResolveOptions, ResolverGeneric, TsconfigDiscovery};
use serde_json::Value;
use skif_core::{FileSystem, OsFs};
use skif_js_syntax::is_stylesheet;
use skif_workspace::Workspace;

use crate::manifest_paths::{ENTRY_FIELDS, map_to_source, resolve_exports};
use crate::setup::Setup;
use crate::specifier::{has_foreign_scheme, is_builtin, node_modules_package, package_name, subpath};

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

/// Resolves specifiers for the whole workspace. Cheap to share between threads.
pub(crate) struct ModuleResolver {
    with_tsconfig: ResolverGeneric<ResolverFs>,
    plain: ResolverGeneric<ResolverFs>,
    style_with_tsconfig: ResolverGeneric<ResolverFs>,
    style_plain: ResolverGeneric<ResolverFs>,
    fs: Arc<dyn FileSystem>,
    root: PathBuf,
    members: BTreeMap<String, Member>,
    scopes: Vec<AliasScope>,
    /// `exports` conditions that select source files, besides the built-in `source`.
    conditions: Vec<String>,
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

fn options(tsconfig: bool, conditions: &[String]) -> ResolveOptions {
    let owned = |items: &[&str]| items.iter().map(|item| (*item).to_owned()).collect::<Vec<_>>();
    ResolveOptions {
        tsconfig: tsconfig.then_some(TsconfigDiscovery::Auto),
        extensions: owned(&[
            ".ts", ".tsx", ".mts", ".cts", ".d.ts", ".js", ".jsx", ".mjs", ".cjs", ".json", ".vue", ".svelte", ".astro",
        ]),
        extension_alias: vec![
            (".js".to_owned(), owned(&[".ts", ".tsx", ".d.ts", ".js"])),
            (".jsx".to_owned(), owned(&[".tsx", ".jsx"])),
            (".mjs".to_owned(), owned(&[".mts", ".d.mts", ".mjs"])),
            (".cjs".to_owned(), owned(&[".cts", ".d.cts", ".cjs"])),
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
        let make = |tsconfig| {
            ResolverGeneric::new_with_file_system(ResolverFs { fs: Arc::clone(&fs) }, options(tsconfig, &conditions))
        };
        let make_style = |tsconfig| {
            ResolverGeneric::new_with_file_system(ResolverFs { fs: Arc::clone(&fs) }, style_options(tsconfig))
        };
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
                AliasScope {
                    dir: package.dir.clone(),
                    aliases,
                }
            })
            .collect();
        scopes.sort_by_key(|scope| std::cmp::Reverse(scope.dir.components().count()));
        Self {
            with_tsconfig: make(true),
            plain: make(false),
            style_with_tsconfig: make_style(true),
            style_plain: make_style(false),
            fs,
            root: workspace.root.clone(),
            members,
            scopes,
            conditions,
        }
    }

    fn classify(&self, path: PathBuf) -> Resolved {
        if path.starts_with(&self.root) && (is_code_file(&path) || is_stylesheet(&path)) {
            Resolved::File(path)
        } else {
            Resolved::Asset
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

    /// Resolves `specifier` as written in the file `importer`.
    pub(crate) fn resolve(&self, importer: &Path, specifier: &str) -> Outcome {
        let outcome = |target, package| Outcome { target, package };
        if is_builtin(specifier) {
            return outcome(Resolved::Builtin, None);
        }
        if has_foreign_scheme(specifier) {
            return outcome(Resolved::Ignored, None);
        }
        let name = package_name(specifier);
        let member_name = name.clone().filter(|name| self.members.contains_key(name));
        if let Ok(path) = self.attempt(importer, specifier) {
            if let Some(installed) = node_modules_package(&path) {
                let package = name.unwrap_or(installed);
                return self.package_target(&package, specifier);
            }
            outcome(self.classify(path), member_name)
        } else {
            if let Some(aliased) = self.through_alias(importer, specifier) {
                return outcome(aliased, None);
            }
            match name {
                Some(name) => self.package_target(&name, specifier),
                None => outcome(Resolved::Unresolved, None),
            }
        }
    }

    fn style_attempt(&self, importer: &Path, specifier: &str) -> Result<PathBuf, ResolveError> {
        match self.style_with_tsconfig.resolve_file(importer, specifier) {
            Ok(resolution) => Ok(resolution.path().to_path_buf()),
            Err(error @ (ResolveError::NotFound(_) | ResolveError::MatchedAliasNotFound(..))) => Err(error),
            Err(_) => self
                .style_plain
                .resolve_file(importer, specifier)
                .map(|resolution| resolution.path().to_path_buf()),
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
        .find_map(|candidate| self.style_attempt(importer, candidate).ok())
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
        let relative = specifier.starts_with(['.', '/']);
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
                return outcome(Resolved::Unresolved, None);
            }
        }
        let name = package_name(specifier);
        let member_name = name.clone().filter(|name| self.members.contains_key(name));
        if let Ok(path) = self.style_attempt(importer, specifier) {
            if let Some(installed) = node_modules_package(&path) {
                let package = name.unwrap_or(installed);
                return self.package_target(&package, specifier);
            }
            return outcome(self.classify(path), member_name);
        }
        if let Some(aliased) = self.through_alias(importer, specifier) {
            return outcome(aliased, None);
        }
        match name {
            Some(name) => self.package_target(&name, specifier),
            None => outcome(Resolved::Unresolved, None),
        }
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
                if let Ok(resolution) = self.plain.resolve(directory, &path.to_string_lossy()) {
                    return Some(self.classify(resolution.path().to_path_buf()));
                }
            }
        }
        match (matched, computed) {
            (false, _) => None,
            (true, true) => Some(Resolved::Ignored),
            (true, false) => Some(Resolved::Unresolved),
        }
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
                    self.plain
                        .resolve(&member.dir, ".")
                        .ok()
                        .map(|resolution| resolution.path().to_path_buf())
                })
                .or_else(|| map_to_source(self.fs.as_ref(), &member.dir, "src/index")),
            Some(sub) => self
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
        let fs = files.iter().fold(MemoryFs::new(), |fs, (path, text)| {
            fs.with_file(Path::new("/ws").join(path), text)
        });
        let config = Config::from_toml_str("").unwrap();
        let workspace = Workspace::discover_with_fs(Path::new("/ws"), &config, &fs).unwrap();
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
        let r = resolver(&[
            ("package.json", ROOT),
            ("packages/a/package.json", r#"{"name":"@x/a"}"#),
            (
                "packages/a/tsconfig.json",
                r#"{"compilerOptions":{"baseUrl":".","paths":{"@app/*":["src/lib/*"]}}}"#,
            ),
            ("packages/a/src/a.ts", ""),
            ("packages/a/src/lib/thing.ts", ""),
        ]);
        let importer = Path::new("/ws/packages/a/src/a.ts");
        assert_eq!(
            r.resolve(importer, "@app/thing").target,
            Resolved::File(PathBuf::from("/ws/packages/a/src/lib/thing.ts"))
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
}
