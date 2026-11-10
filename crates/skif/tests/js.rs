#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use skif::{ConfigOverrides, Linter, MemoryFs, Workspace, load_config};

fn fs() -> MemoryFs {
    MemoryFs::new()
        .with_file(
            "/ws/package.json",
            r#"{"name": "root", "private": true, "packageManager": "pnpm@10.0.0", "workspaces": ["packages/*"]}"#,
        )
        .with_file(
            "/ws/packages/app/package.json",
            r#"{"name": "app", "version": "1.0.0", "private": true, "main": "index.js", "dependencies": {"is-number": "7.0.0"}}"#,
        )
        .with_file("/ws/packages/app/index.js", "import 'left-pad';\n")
}

fn rules(overrides: &ConfigOverrides) -> BTreeSet<String> {
    let fs = Arc::new(fs());
    let config = load_config(Path::new("/ws"), &*fs, overrides).unwrap();
    let workspace = Workspace::discover_with_fs(Path::new("/ws"), &config, &*fs).unwrap();
    let linter = Linter::with_fs(&config, fs).unwrap();
    linter
        .run(&workspace)
        .unwrap()
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.rule.as_str().to_owned())
        .collect()
}

#[test]
fn builtin_rules_include_the_js_analysis_over_the_given_file_system() {
    let found = rules(&ConfigOverrides::default());
    assert!(found.contains("unused-dependencies"), "{found:?}");
    assert!(found.contains("unlisted-dependencies"), "{found:?}");
}

#[test]
fn no_js_turns_the_analysis_off() {
    let found = rules(&ConfigOverrides::default().with_no_js(true));
    assert!(!found.contains("unused-dependencies"), "{found:?}");
    assert!(!found.contains("unlisted-dependencies"), "{found:?}");
}

/// A project at `/r` with a standalone package nested in `tools/sub`, both configured by `/etc/skif.toml` alone.
fn shared_config_fs() -> MemoryFs {
    let manifest = |name: &str| {
        format!(r#"{{"name": "{name}", "version": "1.0.0", "private": true, "packageManager": "pnpm@10.0.0", "main": "src/main.js"}}"#)
    };
    MemoryFs::new()
        .with_file("/r/package.json", manifest("app"))
        .with_file("/r/src/main.js", "module.exports = 1;\n")
        .with_file("/r/tools/sub/package.json", manifest("sub"))
        .with_file("/r/tools/sub/src/main.js", "module.exports = 2;\n")
        .with_file("/etc/skif.toml", "[js]\nignore-dependencies = [\"left-pad\"]\n")
}

/// Lints every root below `/r` with the shared file and returns each root with its report, in path order.
fn lint_shared_config_roots() -> Vec<(std::path::PathBuf, skif::Report)> {
    let fs = Arc::new(shared_config_fs());
    let overrides = ConfigOverrides::default().with_config_file(Some("/etc/skif.toml".into()));
    let discovery = skif::discover_targets(Path::new("/r"), &*fs, &overrides).unwrap();
    discovery
        .targets
        .iter()
        .map(|target| {
            let linter = Linter::with_fs(&target.config, Arc::clone(&fs) as Arc<dyn skif::FileSystem>).unwrap();
            let config_path = target.config_path().unwrap();
            let options = skif::RunOptions::default().with_config_path(&config_path);
            (target.root.clone(), linter.run_with(&target.workspace, &options).unwrap())
        })
        .collect()
}

/// The `stale-js-config` messages of each report.
fn stale_messages(roots: &[(std::path::PathBuf, skif::Report)]) -> Vec<Vec<String>> {
    roots
        .iter()
        .map(|(_, report)| {
            report
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.rule.as_str() == "stale-js-config")
                .map(|diagnostic| diagnostic.message.clone())
                .collect()
        })
        .collect()
}

#[test]
fn settling_the_reports_of_roots_that_share_a_file_reports_a_stale_entry_once() {
    let stale = "`left-pad` in `js.ignore-dependencies` matched nothing".to_owned();
    let mut roots = lint_shared_config_roots();
    let paths: Vec<&Path> = roots.iter().map(|(root, _)| root.as_path()).collect();
    assert_eq!(paths, [Path::new("/r"), Path::new("/r/tools/sub")]);
    // Each root judges the shared file on its own.
    assert_eq!(stale_messages(&roots), [vec![stale.clone()], vec![stale.clone()]]);
    skif::settle_shared_config(roots.iter_mut().map(|(root, report)| (root.as_path(), report)));
    assert_eq!(stale_messages(&roots), [vec![stale], vec![]]);
    assert_eq!(roots[1].1.summary.warnings, 0);
}
