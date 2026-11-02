//! Module resolution on `oxc_resolver`, reading through the host's [`FileSystem`].

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use oxc_resolver::{FileMetadata, ResolveError, ResolveOptions, ResolverGeneric, TsconfigDiscovery};
use serde_json::Value;
use skif_core::{FileSystem, OsFs};
use skif_workspace::Workspace;

use crate::manifest_paths::{ENTRY_FIELDS, map_to_source, resolve_exports};
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
}

/// Resolves specifiers for the whole workspace. Cheap to share between threads.
pub(crate) struct ModuleResolver {
    with_tsconfig: ResolverGeneric<ResolverFs>,
    plain: ResolverGeneric<ResolverFs>,
    fs: Arc<dyn FileSystem>,
    root: PathBuf,
    members: BTreeMap<String, Member>,
}

fn options(tsconfig: bool) -> ResolveOptions {
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
        condition_names: owned(&["import", "require", "node", "default", "types"]),
        main_fields: owned(&["module", "main"]),
        symlinks: false,
        node_path: false,
        ..ResolveOptions::default()
    }
}

impl ModuleResolver {
    pub(crate) fn new(fs: Arc<dyn FileSystem>, workspace: &Workspace) -> Self {
        let make =
            |tsconfig| ResolverGeneric::new_with_file_system(ResolverFs { fs: Arc::clone(&fs) }, options(tsconfig));
        let members = workspace
            .all_packages()
            .filter_map(|package| {
                let name = package.name.clone()?;
                Some((
                    name,
                    Member {
                        dir: workspace.root.join(&package.dir),
                        manifest: package.manifest.value.clone(),
                    },
                ))
            })
            .collect();
        Self {
            with_tsconfig: make(true),
            plain: make(false),
            fs,
            root: workspace.root.clone(),
            members,
        }
    }

    fn classify(&self, path: PathBuf) -> Resolved {
        if path.starts_with(&self.root) && is_code_file(&path) {
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
        match self.attempt(importer, specifier) {
            Ok(path) => {
                if let Some(installed) = node_modules_package(&path) {
                    let package = name.unwrap_or(installed);
                    return self.package_target(&package, specifier);
                }
                outcome(self.classify(path), member_name)
            }
            Err(_) => match name {
                Some(name) => self.package_target(&name, specifier),
                None => outcome(Resolved::Unresolved, None),
            },
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

    /// The source file a workspace package exposes at `subpath`, or `None` when its build output is all there is.
    fn member_file(&self, member: &Member, subpath: Option<&str>) -> Option<PathBuf> {
        if let Some(exports) = member.manifest.get("exports").filter(|value| !value.is_null()) {
            let key = subpath.map_or_else(|| ".".to_owned(), |sub| format!("./{sub}"));
            let target = resolve_exports(exports, &key)?;
            return map_to_source(self.fs.as_ref(), &member.dir, &target);
        }
        match subpath {
            None => ENTRY_FIELDS
                .iter()
                .filter_map(|field| member.manifest.get(*field)?.as_str())
                .find_map(|target| map_to_source(self.fs.as_ref(), &member.dir, target))
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

    fn resolver(files: &[(&str, &str)]) -> ModuleResolver {
        let fs = files.iter().fold(MemoryFs::new(), |fs, (path, text)| {
            fs.with_file(Path::new("/ws").join(path), text)
        });
        let config = Config::from_toml_str("").unwrap();
        let workspace = Workspace::discover_with_fs(Path::new("/ws"), &config, &fs).unwrap();
        ModuleResolver::new(Arc::new(fs), &workspace)
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
}
