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
    let found = rules(&ConfigOverrides {
        no_js: true,
        ..ConfigOverrides::default()
    });
    assert!(!found.contains("unused-dependencies"), "{found:?}");
    assert!(!found.contains("unlisted-dependencies"), "{found:?}");
}
