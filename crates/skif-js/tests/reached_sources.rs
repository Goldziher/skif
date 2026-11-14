//! The plugins that search source files for the commands they run (`execa`, `zx`, `nano-spawn`) read the files the
//! module graph reaches, as knip does, and no other: a fixture below a member that inherits the root's `execa` is
//! never read.
//!
//! Every case is a minimal synthetic workspace.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use skif_config::Config;
use skif_core::{Diagnostic, DirEntry, FileSystem, MemoryFs, PathKind};
use skif_js::{JsProvider, PluginSource};
use skif_lint::{Registry, RunOptions, run_unsorted};
use skif_workspace::Workspace;

const ROOT: &str = "/ws";

/// A file system that counts the reads of each file.
struct CountingReads {
    inner: MemoryFs,
    reads: Mutex<BTreeMap<PathBuf, usize>>,
}

impl FileSystem for CountingReads {
    fn read_to_string(&self, path: &Path) -> io::Result<Arc<str>> {
        *self.reads.lock().unwrap().entry(path.to_path_buf()).or_default() += 1;
        self.inner.read_to_string(path)
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
        self.inner.read_dir(path)
    }

    fn is_file(&self, path: &Path) -> bool {
        self.inner.is_file(path)
    }

    fn is_dir(&self, path: &Path) -> bool {
        self.inner.is_dir(path)
    }

    fn kind(&self, path: &Path) -> PathKind {
        self.inner.kind(path)
    }

    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        self.inner.canonicalize(path)
    }
}

/// Lints `files` with the JS provider and the plugin table, the way the CLI does, and returns the diagnostics sorted
/// with the number of reads of each file.
fn lint_counting(files: &[(&str, &str)]) -> (Vec<Diagnostic>, BTreeMap<PathBuf, usize>) {
    let memory = files.iter().fold(MemoryFs::new(), |memory, (path, text)| {
        memory.with_file(Path::new(ROOT).join(path), text)
    });
    let config = Config::from_toml_str("").unwrap();
    let workspace = Workspace::discover_with_fs(Path::new(ROOT), &config, &memory).unwrap();
    let counting = Arc::new(CountingReads {
        inner: memory,
        reads: Mutex::default(),
    });
    let fs: Arc<dyn FileSystem> = Arc::clone(&counting);
    let provider = JsProvider::new(fs).with_input_source(Arc::new(PluginSource::new(config.js.plugins.clone())));
    let registry = Registry::new(vec![Box::new(provider)]).unwrap();
    let mut diagnostics = run_unsorted(&registry, &workspace, &config, &RunOptions::default()).unwrap();
    diagnostics.sort_by(|a, b| {
        (a.rule.as_str(), &a.location.path, &a.message).cmp(&(b.rule.as_str(), &b.location.path, &b.message))
    });
    let reads = counting.reads.lock().unwrap().clone();
    (diagnostics, reads)
}

/// `path: name` of each diagnostic of `rule`, the name being the dependency or the first quoted word of the message.
fn found(diagnostics: &[Diagnostic], rule: &str) -> Vec<String> {
    diagnostics
        .iter()
        .filter(|d| d.rule.as_str() == rule)
        .map(|d| {
            let name = d.dependency.clone().unwrap_or_else(|| {
                let start = d.message.find('`').map_or(0, |i| i + 1);
                let end = d.message[start..].find('`').map_or(d.message.len(), |i| start + i);
                d.message[start..end].to_owned()
            });
            format!("{}: {name}", d.location.path.display())
        })
        .collect()
}

/// Only the root lists execa, which its member inherits. The member's entry runs `mytool`; the root's entry runs
/// `scripts/next.mjs` with `node`, which makes it an entry whose own `execa` call runs `chainedtool`, so the graph is
/// built again to reach it. A fixture of the member calls `execa("fixturetool")` but nothing reaches it: it is never
/// read and runs nothing. Each reached file is read once, by the graph, whose texts the scans and the second graph
/// take.
#[test]
fn execa_calls_are_read_from_the_files_the_graph_reaches() {
    let files = [
        (
            "package.json",
            r#"{"name":"root","private":true,"packageManager":"pnpm@9.0.0","main":"scripts/release.mjs","devDependencies":{"execa":"9.0.0"}}"#,
        ),
        ("pnpm-workspace.yaml", "packages:\n  - packages/*\n"),
        (
            "scripts/release.mjs",
            "import { execa } from 'execa';\nawait execa('node', ['scripts/next.mjs']);\n",
        ),
        (
            "scripts/next.mjs",
            "import { execa } from 'execa';\nawait execa('chainedtool', ['run']);\n",
        ),
        (
            "packages/app/package.json",
            r#"{"name":"app","version":"1.0.0","main":"src/index.mjs"}"#,
        ),
        (
            "packages/app/src/index.mjs",
            "import { execa } from 'execa';\nawait execa('mytool', ['build']);\n",
        ),
        (
            "packages/app/test/fixtures/run.mjs",
            "import { execa } from 'execa';\nawait execa('fixturetool', ['x']);\n",
        ),
    ];
    let (diagnostics, reads) = lint_counting(&files);
    assert_eq!(
        found(&diagnostics, "unlisted-binaries"),
        ["packages/app/src/index.mjs: mytool", "scripts/next.mjs: chainedtool"],
        "{diagnostics:#?}"
    );
    assert!(
        !found(&diagnostics, "unused-files")
            .iter()
            .any(|file| file.starts_with("scripts/next.mjs")),
        "{diagnostics:#?}"
    );
    let reads_of = |path: &str| reads.get(&Path::new(ROOT).join(path)).copied();
    assert_eq!(reads_of("packages/app/test/fixtures/run.mjs"), None, "{reads:?}");
    for reached in ["scripts/release.mjs", "scripts/next.mjs", "packages/app/src/index.mjs"] {
        assert_eq!(reads_of(reached), Some(1), "{reached}: {reads:?}");
    }
}
