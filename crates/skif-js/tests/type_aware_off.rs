//! With type awareness turned off no part of the checker runs (ADR 0043). A test binary of its own: the arena counter
//! is process-wide, so a typed analysis on another test thread would move it.

#![cfg(feature = "type-aware")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::Arc;

use skif_config::{Config, ConfigOverrides};
use skif_core::{FileSystem, MemoryFs, TypeAwarePackage, TypeAwareState, TypeAwareSummary};
use skif_js::{JsProvider, PluginSource};
use skif_lint::{Registry, RunOptions, run};
use skif_workspace::Workspace;

const ROOT: &str = "/ws";

const FILES: [(&str, &str); 4] = [
    (
        "package.json",
        r#"{"name":"app","private":true,"main":"src/index.ts"}"#,
    ),
    (
        "tsconfig.json",
        r#"{ "compilerOptions": { "strict": true, "noEmit": true }, "include": ["src"] }"#,
    ),
    (
        "src/widgets.ts",
        "export class Widget {\n  render(): void {}\n}\n",
    ),
    (
        "src/index.ts",
        "import { Widget } from './widgets';\nexport const widget = new Widget();\n",
    ),
];

fn type_aware_summary(config: &Config) -> Option<TypeAwareSummary> {
    let memory = FILES.iter().fold(MemoryFs::new(), |memory, (path, text)| {
        memory.with_file(Path::new(ROOT).join(path), text)
    });
    let workspace = Workspace::discover_with_fs(Path::new(ROOT), config, &memory).unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(memory);
    let provider = JsProvider::new(fs).with_input_source(Arc::new(PluginSource::new(config.js.plugins.clone())));
    let registry = Registry::new(vec![Box::new(provider)]).unwrap();
    run(&registry, &workspace, config, &RunOptions::default())
        .unwrap()
        .type_aware
}

#[test]
fn no_type_aware_runs_no_checker() {
    let before = skif_js_types::arena_bytes_in_use();
    let mut overridden = Config::from_toml_str("").unwrap();
    overridden
        .apply(&ConfigOverrides::default().with_no_type_aware(true))
        .unwrap();
    for config in [Config::from_toml_str("[js]\ntype-aware = false\n").unwrap(), overridden] {
        assert_eq!(
            type_aware_summary(&config),
            Some(TypeAwareSummary::new(
                false,
                vec![
                    TypeAwarePackage::new("", "app", TypeAwareState::Untyped)
                        .with_reason("disabled")
                        .with_untyped_files(2)
                ],
            ))
        );
    }
    assert_eq!(skif_js_types::arena_bytes_in_use(), before, "the checker reserved arena memory");
}
