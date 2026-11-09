//! Imports that resolve only through the tsconfig `paths` of another workspace package: still unlisted for the
//! importing package, but reported as a missing mapping rather than a missing npm package.
//!
//! Every case is a minimal synthetic workspace.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use skif_config::Config;
use skif_core::{Diagnostic, FileSystem, MemoryFs};
use skif_js::{JsProvider, PluginSource};
use skif_lint::{Registry, RunOptions, run_unsorted};
use skif_workspace::Workspace;

const ROOT: &str = "/ws";
const RULE: &str = "unlisted-dependencies";
const DETAIL: &str = "ForeignTsconfigPaths";

const ROOT_MANIFEST: &str = r#"{"name":"root","private":true,"workspaces":["packages/*"]}"#;
const APP_MANIFEST: &str = r#"{"name":"app","version":"1.0.0","main":"src/index.ts"}"#;
const LIB_MANIFEST: &str = r#"{"name":"lib","version":"1.0.0","main":"src/index.ts"}"#;
const APP_TSCONFIG: &str = r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "shared/*": ["../lib/src/*"] } } }"#;
const VALUE: &str = "export const s = 1;\n";
const LIB_INDEX: &str = "import { s } from 'shared/value';\nexport const t = s;\n";

fn lint(files: &[(&str, &str)]) -> Vec<Diagnostic> {
    let memory = files.iter().fold(MemoryFs::new(), |memory, (path, text)| {
        memory.with_file(Path::new(ROOT).join(path), text)
    });
    let config = Config::from_toml_str("").unwrap();
    let workspace = Workspace::discover_with_fs(Path::new(ROOT), &config, &memory).unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(memory);
    let provider = JsProvider::new(fs).with_input_source(Arc::new(PluginSource::new(config.js.plugins.clone())));
    let registry = Registry::new(vec![Box::new(provider)]).unwrap();
    let mut diagnostics = run_unsorted(&registry, &workspace, &config, &RunOptions::default()).unwrap();
    diagnostics.sort_by(|a, b| {
        (a.rule.as_str(), &a.location.path, &a.message).cmp(&(b.rule.as_str(), &b.location.path, &b.message))
    });
    diagnostics
}

fn of_rule<'d>(diagnostics: &'d [Diagnostic], rule: &str) -> Vec<&'d Diagnostic> {
    diagnostics.iter().filter(|d| d.rule.as_str() == rule).collect()
}

/// The repro: `lib` imports `shared/value`, which only `app`'s tsconfig maps.
fn repro() -> Vec<(&'static str, &'static str)> {
    vec![
        ("package.json", ROOT_MANIFEST),
        ("packages/app/package.json", APP_MANIFEST),
        ("packages/app/tsconfig.json", APP_TSCONFIG),
        ("packages/app/src/index.ts", "export const app = 1;\n"),
        ("packages/lib/package.json", LIB_MANIFEST),
        ("packages/lib/src/index.ts", LIB_INDEX),
        ("packages/lib/src/value.ts", VALUE),
    ]
}

#[test]
fn an_import_mapped_only_by_another_package_names_its_tsconfig() {
    let diagnostics = lint(&repro());
    let unlisted = of_rule(&diagnostics, RULE);
    assert_eq!(unlisted.len(), 1, "{diagnostics:#?}");
    let finding = unlisted[0];
    assert_eq!(finding.location.path, PathBuf::from("packages/lib/src/index.ts"));
    assert_eq!(finding.detail.as_deref(), Some(DETAIL));
    assert_eq!(finding.dependency.as_deref(), Some("shared"));
    assert_eq!(finding.package.as_deref(), Some("lib"));
    assert_eq!(
        finding.message,
        "`shared/value` resolves only through the `paths` of `packages/app/tsconfig.json`; add the mapping to this \
         package's tsconfig or import it by package name"
    );
    let help = finding.help.as_deref().unwrap();
    assert!(!help.contains("package.json"), "{help}");
    assert!(!help.contains("dependencies"), "{help}");
    assert!(finding.fixes.is_empty(), "{:#?}", finding.fixes);
    assert_eq!(finding.related.len(), 1, "{:#?}", finding.related);
    assert_eq!(
        finding.related[0].location.path,
        PathBuf::from("packages/app/tsconfig.json")
    );
}

#[test]
fn foreign_tsconfig_paths_finding_json() {
    let diagnostics = lint(&repro());
    insta::assert_json_snapshot!(of_rule(&diagnostics, RULE));
}

#[test]
fn the_importers_own_paths_still_resolve() {
    let mut files = repro();
    files.retain(|(path, _)| *path != "packages/lib/src/index.ts");
    files.push(("packages/lib/src/index.ts", "export const t = 1;\n"));
    files.retain(|(path, _)| *path != "packages/app/src/index.ts");
    files.push((
        "packages/app/src/index.ts",
        "import { s } from 'shared/value';\nexport const app = s;\n",
    ));
    let diagnostics = lint(&files);
    assert!(of_rule(&diagnostics, RULE).is_empty(), "{diagnostics:#?}");
    assert!(
        of_rule(&diagnostics, "unresolved-imports").is_empty(),
        "{diagnostics:#?}"
    );
}

#[test]
fn an_import_no_tsconfig_maps_keeps_the_package_json_advice() {
    let mut files = repro();
    files.retain(|(path, _)| *path != "packages/lib/src/index.ts");
    files.push((
        "packages/lib/src/index.ts",
        "import { x } from 'left-pad';\nexport const t = x;\n",
    ));
    let diagnostics = lint(&files);
    let unlisted = of_rule(&diagnostics, RULE);
    assert_eq!(unlisted.len(), 1, "{diagnostics:#?}");
    assert_eq!(unlisted[0].detail, None);
    assert_eq!(
        unlisted[0].message,
        "`left-pad` is imported but not listed in `package.json`"
    );
    assert!(unlisted[0].related.is_empty(), "{:#?}", unlisted[0].related);
}

#[test]
fn a_catch_all_pattern_of_another_package_does_not_count() {
    let mut files = repro();
    files.retain(|(path, _)| *path != "packages/app/tsconfig.json");
    files.push((
        "packages/app/tsconfig.json",
        r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "*": ["./types/*"] } } }"#,
    ));
    let diagnostics = lint(&files);
    let unlisted = of_rule(&diagnostics, RULE);
    assert_eq!(unlisted.len(), 1, "{diagnostics:#?}");
    assert_eq!(unlisted[0].detail, None, "{:#?}", unlisted[0]);
}

#[test]
fn the_most_specific_foreign_pattern_wins() {
    let mut files = repro();
    files.extend([
        (
            "packages/web/package.json",
            r#"{"name":"web","version":"1.0.0","main":"src/index.ts"}"#,
        ),
        (
            "packages/web/tsconfig.json",
            r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "shared/value": ["../lib/src/value.ts"] } } }"#,
        ),
        ("packages/web/src/index.ts", "export const web = 1;\n"),
    ]);
    let diagnostics = lint(&files);
    let unlisted = of_rule(&diagnostics, RULE);
    assert_eq!(unlisted.len(), 1, "{diagnostics:#?}");
    assert_eq!(unlisted[0].detail.as_deref(), Some(DETAIL));
    assert!(
        unlisted[0].message.contains("`packages/web/tsconfig.json`"),
        "{}",
        unlisted[0].message
    );
}

#[test]
fn equally_specific_foreign_patterns_name_the_first_config_by_path() {
    let mut files = repro();
    // `zeta` sorts after `app`, whichever order the packages are discovered in.
    files.extend([
        (
            "packages/zeta/package.json",
            r#"{"name":"zeta","version":"1.0.0","main":"src/index.ts"}"#,
        ),
        ("packages/zeta/tsconfig.json", APP_TSCONFIG),
        ("packages/zeta/src/index.ts", "export const zeta = 1;\n"),
    ]);
    files.reverse();
    let diagnostics = lint(&files);
    let unlisted = of_rule(&diagnostics, RULE);
    assert_eq!(unlisted.len(), 1, "{diagnostics:#?}");
    assert!(
        unlisted[0].message.contains("`packages/app/tsconfig.json`"),
        "{}",
        unlisted[0].message
    );
}

#[test]
fn a_root_tsconfig_the_package_does_not_follow_is_foreign() {
    let files = [
        ("package.json", ROOT_MANIFEST),
        (
            "tsconfig.json",
            r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "shared/*": ["packages/lib/src/*"] } } }"#,
        ),
        ("packages/lib/package.json", LIB_MANIFEST),
        // The package's own `paths` replace the root's, so the root mapping does not apply to it.
        (
            "packages/lib/tsconfig.json",
            r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "lib/*": ["./src/*"] } } }"#,
        ),
        ("packages/lib/src/index.ts", LIB_INDEX),
        ("packages/lib/src/value.ts", VALUE),
    ];
    let diagnostics = lint(&files);
    let unlisted = of_rule(&diagnostics, RULE);
    assert_eq!(unlisted.len(), 1, "{diagnostics:#?}");
    assert_eq!(unlisted[0].detail.as_deref(), Some(DETAIL));
    assert!(
        unlisted[0].message.contains("the `paths` of `tsconfig.json`"),
        "{}",
        unlisted[0].message
    );
}

/// The repro with `app`'s tsconfig and `lib`'s index replaced.
fn repro_with(app_tsconfig: &'static str, lib_index: &'static str) -> Vec<(&'static str, &'static str)> {
    let mut files = repro();
    files.retain(|(path, _)| !matches!(*path, "packages/app/tsconfig.json" | "packages/lib/src/index.ts"));
    files.extend([
        ("packages/app/tsconfig.json", app_tsconfig),
        ("packages/lib/src/index.ts", lib_index),
    ]);
    files
}

/// The only `unlisted-dependencies` finding, which must keep the `package.json` advice for `name`.
fn assert_package_json_advice(diagnostics: &[Diagnostic], name: &str) {
    let unlisted = of_rule(diagnostics, RULE);
    assert_eq!(unlisted.len(), 1, "{diagnostics:#?}");
    assert_eq!(unlisted[0].detail, None, "{:#?}", unlisted[0]);
    assert_eq!(
        unlisted[0].message,
        format!("`{name}` is imported but not listed in `package.json`")
    );
    assert!(unlisted[0].related.is_empty(), "{:#?}", unlisted[0].related);
}

#[test]
fn a_workspace_package_another_package_maps_keeps_the_package_json_advice() {
    let files = [
        ("package.json", ROOT_MANIFEST),
        ("packages/app/package.json", APP_MANIFEST),
        (
            "packages/app/tsconfig.json",
            r#"{ "compilerOptions": { "paths": { "@acme/lib": ["../lib/src/index.ts"] } } }"#,
        ),
        ("packages/app/src/index.ts", "export const app = 1;\n"),
        (
            "packages/lib/package.json",
            r#"{"name":"@acme/lib","version":"1.0.0","main":"src/index.ts"}"#,
        ),
        ("packages/lib/src/index.ts", VALUE),
        (
            "packages/web/package.json",
            r#"{"name":"web","version":"1.0.0","main":"src/index.ts"}"#,
        ),
        (
            "packages/web/src/index.ts",
            "import { s } from '@acme/lib';\nexport const web = s;\n",
        ),
    ];
    assert_package_json_advice(&lint(&files), "@acme/lib");
}

#[test]
fn a_package_another_manifest_lists_keeps_the_package_json_advice() {
    let mut files = repro_with(
        r#"{ "compilerOptions": { "paths": { "left-pad": ["./shims/left-pad.ts"] } } }"#,
        "import { x } from 'left-pad';\nexport const t = x;\n",
    );
    files.retain(|(path, _)| *path != "packages/app/package.json");
    files.extend([
        (
            "packages/app/package.json",
            r#"{"name":"app","version":"1.0.0","main":"src/index.ts","dependencies":{"left-pad":"1.0.0"}}"#,
        ),
        ("packages/app/shims/left-pad.ts", "export const x = 1;\n"),
    ]);
    assert_package_json_advice(&lint(&files), "left-pad");
}

#[test]
fn an_import_that_resolves_through_hoisted_node_modules_keeps_the_package_json_advice() {
    let mut files = repro_with(
        r#"{ "compilerOptions": { "paths": { "lodash/*": ["./shims/lodash/*"] } } }"#,
        "import merge from 'lodash/merge';\nexport const t = merge;\n",
    );
    files.extend([
        ("packages/app/shims/lodash/merge.ts", "export default 1;\n"),
        (
            "node_modules/lodash/package.json",
            r#"{"name":"lodash","version":"4.17.21","main":"lodash.js"}"#,
        ),
        ("node_modules/lodash/lodash.js", "module.exports = {};\n"),
        (
            "node_modules/lodash/merge.js",
            "module.exports = function merge() {};\n",
        ),
    ]);
    assert_package_json_advice(&lint(&files), "lodash");
}

#[test]
fn a_foreign_pattern_into_node_modules_keeps_the_package_json_advice() {
    let mut files = repro_with(
        r#"{ "compilerOptions": { "paths": { "react": ["./node_modules/@types/react"] } } }"#,
        "import { useState } from 'react';\nexport const t = useState;\n",
    );
    files.extend([
        (
            "packages/app/node_modules/@types/react/package.json",
            r#"{"name":"@types/react","version":"18.0.0","types":"index.d.ts"}"#,
        ),
        (
            "packages/app/node_modules/@types/react/index.d.ts",
            "export declare function useState(): void;\n",
        ),
    ]);
    assert_package_json_advice(&lint(&files), "react");
}

#[test]
fn a_foreign_pattern_whose_targets_are_missing_keeps_the_package_json_advice() {
    let files = repro_with(
        r#"{ "compilerOptions": { "paths": { "shared/*": ["../missing/*"] } } }"#,
        LIB_INDEX,
    );
    assert_package_json_advice(&lint(&files), "shared");
}

#[test]
fn inherited_paths_name_the_config_that_declares_them() {
    let mut files = repro_with(r#"{ "extends": "../../tsconfig.base.json" }"#, LIB_INDEX);
    files.push((
        "tsconfig.base.json",
        r#"{ "compilerOptions": { "paths": { "shared/*": ["./packages/lib/src/*"] } } }"#,
    ));
    let diagnostics = lint(&files);
    let unlisted = of_rule(&diagnostics, RULE);
    assert_eq!(unlisted.len(), 1, "{diagnostics:#?}");
    assert_eq!(unlisted[0].detail.as_deref(), Some(DETAIL));
    assert!(
        unlisted[0].message.contains("the `paths` of `tsconfig.base.json`"),
        "{}",
        unlisted[0].message
    );
    assert_eq!(
        unlisted[0].related[0].location.path,
        PathBuf::from("tsconfig.base.json")
    );
}
