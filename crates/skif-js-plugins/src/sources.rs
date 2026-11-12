//! The JavaScript and TypeScript source files of a package, listed and read once for every plugin that scans them.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use crate::plugin::PluginContext;

/// The JavaScript and TypeScript source files below a package directory, declaration files left out, each read on
/// first use. Several plugins (`pino`, `zx`, `execa`, `nano-spawn`, `bun`, ...) search the sources of a package for
/// the calls they know; with one listing for the package, each file is listed and read once for all of them.
///
/// The resolvers ([`resolve_plugins`](crate::resolve_plugins) and its variants) keep one for each package and scope
/// they resolve, and drop it when they are done. A listing serves only a context of its own directory: a context of
/// another directory lists and reads its sources itself.
#[derive(Debug)]
pub struct PackageSources {
    dir: PathBuf,
    files: OnceLock<Vec<SourceFile>>,
}

/// A source file, relative to the package directory, and its text once read; `None` when it cannot be read.
#[derive(Debug)]
struct SourceFile {
    path: String,
    text: OnceLock<Option<Arc<str>>>,
}

impl PackageSources {
    /// An empty listing of the sources below `dir`, a package directory.
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            files: OnceLock::new(),
        }
    }

    /// The listing of `cx`, when it has one of its own directory.
    pub(crate) fn of<'a>(cx: &PluginContext<'a>) -> Option<&'a Self> {
        cx.sources.filter(|sources| sources.dir.as_path() == cx.dir)
    }

    /// The source files `wanted` accepts by path, in path order, with their text when `keep` accepts it. `list`
    /// lists the files on first use and `read` reads a file's text on its first use.
    pub(crate) fn texts(
        &self,
        list: impl FnOnce() -> Vec<String>,
        read: impl Fn(&str) -> Option<Arc<str>>,
        mut wanted: impl FnMut(&str) -> bool,
        keep: impl Fn(&str) -> bool,
    ) -> Vec<(String, Arc<str>)> {
        let files = self.files.get_or_init(|| {
            list()
                .into_iter()
                .map(|path| SourceFile {
                    path,
                    text: OnceLock::new(),
                })
                .collect()
        });
        files
            .iter()
            .filter(|file| wanted(file.path.as_str()))
            .filter_map(|file| {
                let text = file.text.get_or_init(|| read(file.path.as_str())).as_ref()?;
                keep(&**text).then(|| (file.path.clone(), Arc::clone(text)))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use indexmap::IndexMap;
    use skif_core::MemoryFs;

    use super::*;
    use crate::files::GlobCache;
    use crate::support::{own_sources_where, sources_containing};

    fn context<'a>(
        fs: &'a MemoryFs,
        dir: &'a str,
        globs: &'a GlobCache,
        sources: Option<&'a PackageSources>,
    ) -> PluginContext<'a> {
        static SCRIPTS: std::sync::LazyLock<IndexMap<String, String>> = std::sync::LazyLock::new(IndexMap::new);
        static PLUGINS: std::sync::LazyLock<IndexMap<String, skif_config::PluginConfig>> =
            std::sync::LazyLock::new(IndexMap::new);
        PluginContext {
            dir: Path::new(dir),
            is_root: false,
            package_name: None,
            dependencies: &[],
            inherited_dependencies: &[],
            scripts: &SCRIPTS,
            fs,
            plugins: &PLUGINS,
            modules: None,
            nx: None,
            globs,
            sources,
        }
    }

    fn found(found: &[(String, Arc<str>)]) -> Vec<(&str, &str)> {
        found.iter().map(|(path, text)| (path.as_str(), &**text)).collect()
    }

    /// A listing answers for the directory it was made for, as a context without one does, and a context of another
    /// directory that is handed it finds its own sources.
    #[test]
    fn a_listing_serves_only_the_directory_it_lists() {
        let fs = MemoryFs::new()
            .with_file("/a/x.ts", "pino()")
            .with_file("/a/types.d.ts", "pino()")
            .with_file("/a/nested/package.json", "{}")
            .with_file("/a/nested/y.ts", "pino()")
            .with_file("/a/z.js", "other()")
            .with_file("/b/w.ts", "pino()");
        let globs = GlobCache::new();
        let listing = PackageSources::new("/a");
        let with = context(&fs, "/a", &globs, Some(&listing));
        let without = context(&fs, "/a", &globs, None);
        assert_eq!(
            found(&sources_containing(&with, "pino")),
            [("nested/y.ts", "pino()"), ("x.ts", "pino()")]
        );
        assert_eq!(
            found(&sources_containing(&with, "pino")),
            found(&sources_containing(&without, "pino"))
        );
        assert_eq!(found(&own_sources_where(&with, |_| true)), [("x.ts", "pino()"), ("z.js", "other()")]);
        assert_eq!(
            found(&own_sources_where(&with, |_| true)),
            found(&own_sources_where(&without, |_| true))
        );
        let other = context(&fs, "/b", &globs, Some(&listing));
        assert_eq!(found(&sources_containing(&other, "pino")), [("w.ts", "pino()")]);
    }
}
