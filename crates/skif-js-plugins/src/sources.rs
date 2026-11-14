//! The JavaScript and TypeScript source files of a package, listed and read once for every plugin that scans them.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, PoisonError, RwLock};

use rayon::prelude::*;
use skif_core::{HashMap, HashSet};

use crate::plugin::PluginContext;

/// The JavaScript and TypeScript source files below a package directory, declaration files left out, each read on
/// first use. Several plugins (`pino`, `zx`, `execa`, `nano-spawn`, `bun`, ...) search the sources of a package for
/// the calls they know; with one listing for the package, each file is listed and read once for all of them.
///
/// The resolvers ([`resolve_plugins`](crate::resolve_plugins) and its variants) use one for each package and scope
/// they resolve. A listing serves only a context of its own directory: a context of another directory lists and reads
/// its sources itself.
///
/// Listings made [`within`](Self::within) one another form a group that shares the texts it reads, by absolute path:
/// the sources of a package below the root package, or of an Nx project below its package, are also sources of the
/// enclosing directory, and a file one of them read is not read again for another (two listings that want it at the
/// same time on different threads may both read it). The texts are a snapshot of the
/// [`FileSystem`](skif_core::FileSystem) taken during one run and live as long as the longest-lived listing of the
/// group: a listing must not outlive the run, or the state of the file system, it read, and the listings of a group
/// share texts correctly only when all of them read through the same file system.
///
/// The texts a search wants are read side by side on the current rayon pool, unless the listing is
/// [`sequential`](Self::sequential). A search of the package's own sources, which leaves out the directories of the
/// run's other packages below it, does not list those directories either, unless a search of every source listed
/// them already.
#[derive(Debug)]
pub struct PackageSources {
    dir: PathBuf,
    files: OnceLock<Vec<SourceFile>>,
    own: OnceLock<OwnFiles>,
    texts: Arc<Texts>,
    sequential: bool,
}

/// The source files outside the directories of `members`, listed without walking them.
#[derive(Debug)]
struct OwnFiles {
    members: Vec<String>,
    files: Vec<SourceFile>,
}

/// A source file, relative to the package directory, and its text once read; `None` when it cannot be read. The cell
/// is the listing's own so that it remembers a failed read, which the group's [`Texts`] does not keep.
#[derive(Debug)]
struct SourceFile {
    path: String,
    text: OnceLock<Option<Arc<str>>>,
}

/// The texts read by a group of listings, by absolute path. A file that cannot be read is not kept, so the next
/// listing that wants it tries again, as it would on its own.
#[derive(Debug, Default)]
struct Texts(RwLock<HashMap<PathBuf, Arc<str>>>);

impl Texts {
    /// The text of `path`, read with `read` unless a listing of the group read it already.
    fn get_or_read(&self, path: &Path, read: impl FnOnce() -> Option<Arc<str>>) -> Option<Arc<str>> {
        if let Some(text) = self.0.read().unwrap_or_else(PoisonError::into_inner).get(path) {
            return Some(Arc::clone(text));
        }
        let text = read()?;
        let mut texts = self.0.write().unwrap_or_else(PoisonError::into_inner);
        texts.insert(path.to_path_buf(), Arc::clone(&text));
        Some(text)
    }
}

impl SourceFile {
    fn listed(paths: Vec<String>) -> Vec<Self> {
        paths
            .into_iter()
            .map(|path| Self {
                path,
                text: OnceLock::new(),
            })
            .collect()
    }
}

impl PackageSources {
    /// An empty listing of the sources below `dir`, a package directory.
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            files: OnceLock::new(),
            own: OnceLock::new(),
            texts: Arc::default(),
            sequential: false,
        }
    }

    /// This listing, reading the texts a search wants one after the other on the calling thread with `sequential`
    /// instead of side by side on the current rayon pool, as do the listings made [`within`](Self::within) it.
    #[must_use]
    pub fn sequential(self, sequential: bool) -> Self {
        Self { sequential, ..self }
    }

    /// An empty listing of the sources below `dir` that takes the text of a file from `enclosing`, and from every
    /// other listing made within it, when one of them read the file already, and shares what it reads with them.
    ///
    /// `dir` need not lie below the directory of `enclosing`: texts are shared by absolute path, so a listing of any
    /// directory can join the group. All of them must read through the same file system during the same run. It
    /// reads one text after the other when `enclosing` does ([`sequential`](Self::sequential)).
    #[must_use]
    pub fn within(dir: impl Into<PathBuf>, enclosing: &Self) -> Self {
        Self {
            dir: dir.into(),
            files: OnceLock::new(),
            own: OnceLock::new(),
            texts: Arc::clone(&enclosing.texts),
            sequential: enclosing.sequential,
        }
    }

    /// The listing of `cx`, when it has one of its own directory.
    pub(crate) fn of<'a>(cx: &PluginContext<'a>) -> Option<&'a Self> {
        cx.sources.filter(|sources| sources.dir.as_path() == cx.dir)
    }

    /// The source files below none of the `members` directories (relative to the listing's directory and
    /// `/`-separated; every file when there are none), in path order, with their text when `keep` accepts it.
    ///
    /// `list` lists the files below no directory of the members it is given, and is asked for every file (no members)
    /// or for the files outside `members` on first use; the files outside `members` are taken from the listing of
    /// every file instead when that is made already, or when an earlier search asked for the files outside other
    /// members. `read` reads a file's text on its first use, unless a listing sharing this one's texts read it
    /// already.
    pub(crate) fn texts(
        &self,
        members: &[String],
        list: impl Fn(&[String]) -> Vec<String>,
        read: impl Fn(&str) -> Option<Arc<str>> + Sync,
        keep: impl Fn(&str) -> bool + Sync,
    ) -> Vec<(String, Arc<str>)> {
        let (files, excluded) = self.files_outside(members, &list);
        let excluded: HashSet<&str> = excluded.iter().map(String::as_str).collect();
        let found = |file: &SourceFile| {
            if in_member(&excluded, &file.path) {
                return None;
            }
            let text = file
                .text
                .get_or_init(|| {
                    self.texts
                        .get_or_read(&self.dir.join(&file.path), || read(file.path.as_str()))
                })
                .as_ref()?;
            keep(text).then(|| (file.path.clone(), Arc::clone(text)))
        };
        if self.sequential {
            files.iter().filter_map(found).collect()
        } else {
            files.par_iter().filter_map(found).collect()
        }
    }

    /// The listed files to search for those outside `members`, and the members whose files are still to be left out
    /// of them.
    fn files_outside<'m>(
        &self,
        members: &'m [String],
        list: &impl Fn(&[String]) -> Vec<String>,
    ) -> (&[SourceFile], &'m [String]) {
        let every = || self.files.get_or_init(|| SourceFile::listed(list(&[]))).as_slice();
        if members.is_empty() {
            return (every(), &[]);
        }
        if let Some(files) = self.files.get() {
            return (files.as_slice(), members);
        }
        let own = self.own.get_or_init(|| OwnFiles {
            members: members.to_vec(),
            files: SourceFile::listed(list(members)),
        });
        if own.members == members {
            (own.files.as_slice(), &[])
        } else {
            (every(), members)
        }
    }
}

/// Whether `path`, relative to a listing's directory, lies below one of the `members` directories.
fn in_member(members: &HashSet<&str>, path: &str) -> bool {
    !members.is_empty() && path.match_indices('/').any(|(end, _)| members.contains(&path[..end]))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use indexmap::IndexMap;
    use skif_core::MemoryFs;

    use super::*;
    use crate::files::GlobCache;
    use crate::files::tests::ListingFs;
    use crate::support::{own_sources_where, sources_containing};

    fn context<'a>(
        fs: &'a dyn skif_core::FileSystem,
        dir: &'a str,
        globs: &'a GlobCache,
        sources: Option<&'a PackageSources>,
        members: &'a [String],
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
            members,
        }
    }

    fn found(found: &[(String, Arc<str>)]) -> Vec<(&str, &str)> {
        found.iter().map(|(path, text)| (path.as_str(), &**text)).collect()
    }

    /// A listing answers for the directory it was made for, as a context without one does, and a context of another
    /// directory that is handed it finds its own sources. The own sources leave out those of a member, `nested`, and
    /// keep those below a `package.json` that is not one.
    #[test]
    fn a_listing_serves_only_the_directory_it_lists() {
        let fs = MemoryFs::new()
            .with_file("/a/x.ts", "pino()")
            .with_file("/a/types.d.ts", "pino()")
            .with_file("/a/nested/package.json", "{}")
            .with_file("/a/nested/y.ts", "pino()")
            .with_file("/a/nested/deep/q.ts", "other()")
            .with_file("/a/esm/package.json", r#"{"type":"module"}"#)
            .with_file("/a/esm/v.mjs", "other()")
            .with_file("/a/z.js", "other()")
            .with_file("/b/w.ts", "pino()");
        let globs = GlobCache::new();
        let members = ["nested".to_owned()];
        let listing = PackageSources::new("/a");
        let with = context(&fs, "/a", &globs, Some(&listing), &members);
        let without = context(&fs, "/a", &globs, None, &members);
        assert_eq!(
            found(&sources_containing(&with, "pino")),
            [("nested/y.ts", "pino()"), ("x.ts", "pino()")]
        );
        assert_eq!(
            found(&sources_containing(&with, "pino")),
            found(&sources_containing(&without, "pino"))
        );
        assert_eq!(
            found(&own_sources_where(&with, |_| true)),
            [("esm/v.mjs", "other()"), ("x.ts", "pino()"), ("z.js", "other()")]
        );
        assert_eq!(
            found(&own_sources_where(&with, |_| true)),
            found(&own_sources_where(&without, |_| true))
        );
        let other = context(&fs, "/b", &globs, Some(&listing), &members);
        assert_eq!(found(&sources_containing(&other, "pino")), [("w.ts", "pino()")]);
    }

    /// A search of the own sources lists no directory of a member, and one leaving out other members lists every
    /// file and leaves theirs out; a search of every source then finds the members' files too. Searched in the other
    /// order, the own sources are taken from the listing of every file without listing again. Both orders find the
    /// same files.
    #[test]
    fn own_sources_are_listed_without_walking_the_members() {
        let memory = || {
            MemoryFs::new()
                .with_file("/a/x.ts", "pino()")
                .with_file("/a/nested/package.json", "{}")
                .with_file("/a/nested/y.ts", "pino()")
                .with_file("/a/nested/deep/q.ts", "pino()")
                .with_file("/a/esm/v.mjs", "pino()")
        };
        let members = ["nested".to_owned()];
        let globs = GlobCache::new();
        let own = [("esm/v.mjs", "pino()"), ("x.ts", "pino()")];
        let every = [
            ("esm/v.mjs", "pino()"),
            ("nested/deep/q.ts", "pino()"),
            ("nested/y.ts", "pino()"),
            ("x.ts", "pino()"),
        ];

        let fs = ListingFs::new(memory());
        let listing = PackageSources::new("/a");
        let cx = context(&fs, "/a", &globs, Some(&listing), &members);
        assert_eq!(found(&own_sources_where(&cx, |_| true)), own);
        assert_eq!(fs.listed(), ["/a", "/a/esm"].map(Path::new));
        let others = ["esm".to_owned()];
        let other_cx = context(&fs, "/a", &globs, Some(&listing), &others);
        assert_eq!(found(&own_sources_where(&other_cx, |_| true)), &every[1..]);
        assert_eq!(found(&sources_containing(&cx, "pino")), every);
        assert_eq!(found(&own_sources_where(&cx, |_| true)), own);

        let fs = ListingFs::new(memory());
        let listing = PackageSources::new("/a");
        let cx = context(&fs, "/a", &globs, Some(&listing), &members);
        assert_eq!(found(&sources_containing(&cx, "pino")), every);
        let listed = fs.listed();
        assert_eq!(listed, ["/a", "/a/esm", "/a/nested", "/a/nested/deep"].map(Path::new));
        assert_eq!(found(&own_sources_where(&cx, |_| true)), own);
        assert_eq!(
            fs.listed(),
            listed,
            "the own sources are taken from the listing of every file"
        );
    }

    /// Texts read side by side come in path order, as those read one after the other on the calling thread do.
    #[test]
    fn a_search_finds_the_sources_in_path_order_on_any_thread() {
        let mut fs = MemoryFs::new();
        for i in 0..300 {
            fs.insert(
                format!("/a/src/{i:03}.ts"),
                if i % 3 == 0 { "pino()" } else { "other()" },
            );
        }
        let globs = GlobCache::new();
        let expected: Vec<String> = (0..300).step_by(3).map(|i| format!("src/{i:03}.ts")).collect();
        for listing in [PackageSources::new("/a"), PackageSources::new("/a").sequential(true)] {
            let cx = context(&fs, "/a", &globs, Some(&listing), &[]);
            let paths: Vec<String> = sources_containing(&cx, "pino")
                .into_iter()
                .map(|(path, _)| path)
                .collect();
            assert_eq!(paths, expected);
        }
    }
}
