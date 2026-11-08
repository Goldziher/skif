//! A run on several threads reports exactly what a sequential run reports, on a generated workspace large enough for
//! the policy analysis, its findings and workspace discovery to split their work (#29).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::Arc;

use skif::{Config, FileSystem, Linter, MemoryFs, Report, RunOptions};
use skif_bench::generate::{self, Options, PackageManager};

const ROOT: &str = "/repo";

/// Configurations that send the instances through different version-group policies.
const CONFIGS: [&str; 3] = [
    "",
    "[[semver-groups]]\ndependencies = [\"@types/*\"]\nrange = \"^\"\n\n[[semver-groups]]\nrange = \"~\"\n",
    "[[version-groups]]\nlabel = \"same-range\"\npolicy = \"same-range\"\n\n[[version-groups]]\nlabel = \"lowest\"\nprefer = \"lowest\"\n",
];

fn workspace_fs() -> MemoryFs {
    let generated = generate::build(&Options {
        packages: 300,
        seed: 7,
        package_manager: PackageManager::Pnpm,
        catalog: false,
        mismatch_rate: 0.2,
    })
    .unwrap();
    let mut fs = MemoryFs::new();
    for (path, contents) in generated.files {
        fs.insert(Path::new(ROOT).join(path), contents);
    }
    fs
}

/// The report of a run on `threads` threads.
fn report(fs: &Arc<MemoryFs>, config: &Config, threads: usize) -> Report {
    let linter = Linter::with_fs(config, Arc::clone(fs) as Arc<dyn FileSystem>).unwrap();
    let options = RunOptions::default().with_threads(NonZeroUsize::new(threads).unwrap());
    let workspace = linter.discover_with_fs(Path::new(ROOT), fs.as_ref(), &options).unwrap();
    linter.run_with(&workspace, &options).unwrap()
}

#[test]
fn threads_do_not_change_the_report() {
    let fs = Arc::new(workspace_fs());
    for toml in CONFIGS {
        let config = Config::from_toml_str(toml).unwrap();
        let sequential = report(&fs, &config, 1);
        let mismatches = sequential
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.rule.as_str() == "version-mismatch")
            .count();
        assert!(
            mismatches > 100,
            "the workspace must give the policy analysis work to split"
        );
        for threads in [2, 4] {
            assert!(
                report(&fs, &config, threads) == sequential,
                "{threads} threads changed the report under {toml:?}"
            );
        }
    }
}
