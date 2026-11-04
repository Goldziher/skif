//! Lints fixture workspaces with the JS provider and checks the diagnostics.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use skif_config::Config;
use skif_core::{Diagnostic, FileSystem, Fix, FixOp, FixSafety, JsonAction, MemoryFs, PathSeg, apply_text_edits};
use skif_js::{InputRequest, InputSource, JsProvider, WorkspaceInputs};
use skif_lint::{Registry, RunOptions, run_unsorted};
use skif_workspace::Workspace;

// The resolver only applies a `tsconfig` found below an absolute path, and `/ws` has no drive on Windows.
const ROOT: &str = if cfg!(windows) { "C:/ws" } else { "/ws" };

fn fixture_dir(case: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(case)
}

fn load(dir: &Path, base: &Path, fs_out: &mut MemoryFs, config: &mut Option<String>) {
    let mut entries: Vec<_> = fs::read_dir(dir).unwrap().map(|entry| entry.unwrap().path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            load(&path, base, fs_out, config);
            continue;
        }
        let rel = path.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/");
        let text = fs::read_to_string(&path).unwrap();
        if rel == "skif.toml" {
            *config = Some(text);
        } else {
            fs_out.insert(Path::new(ROOT).join(&rel), text);
        }
    }
}

struct Run {
    fs: Arc<dyn FileSystem>,
    workspace: Workspace,
    config: Config,
}

impl Run {
    fn new(case: &str, extra_toml: &str) -> Self {
        let base = fixture_dir(case);
        let mut memory = MemoryFs::new();
        let mut file_config = None;
        load(&base, &base, &mut memory, &mut file_config);
        let toml = format!("{}\n{extra_toml}", file_config.unwrap_or_default());
        let config = Config::from_toml_str(&toml).unwrap();
        let workspace = Workspace::discover_with_fs(Path::new(ROOT), &config, &memory).unwrap();
        Self {
            fs: Arc::new(memory),
            workspace,
            config,
        }
    }

    fn lint_with(&self, provider: JsProvider) -> Vec<Diagnostic> {
        let registry = Registry::new(vec![Box::new(provider)]).unwrap();
        let mut diagnostics = run_unsorted(&registry, &self.workspace, &self.config, &RunOptions::default()).unwrap();
        diagnostics.sort_by(|a, b| {
            (
                a.rule.as_str(),
                &a.location.path,
                a.location.span.map(|s| s.start),
                &a.message,
            )
                .cmp(&(
                    b.rule.as_str(),
                    &b.location.path,
                    b.location.span.map(|s| s.start),
                    &b.message,
                ))
        });
        diagnostics
    }

    fn lint(&self) -> Vec<Diagnostic> {
        self.lint_with(JsProvider::new(Arc::clone(&self.fs)))
    }
}

fn subject(diagnostic: &Diagnostic) -> String {
    let name = diagnostic.dependency.clone().unwrap_or_else(|| {
        let message = &diagnostic.message;
        let start = message.find('`').map_or(0, |i| i + 1);
        let end = message[start..].find('`').map_or(message.len(), |i| start + i);
        message[start..end].to_owned()
    });
    format!("{}: {name}", diagnostic.location.path.display())
}

fn found(diagnostics: &[Diagnostic], rule: &str) -> Vec<String> {
    diagnostics
        .iter()
        .filter(|d| d.rule.as_str() == rule)
        .map(subject)
        .collect()
}

fn only<'a>(diagnostics: &'a [Diagnostic], rule: &str, name: &str) -> &'a Diagnostic {
    let mut matching = diagnostics
        .iter()
        .filter(|d| d.rule.as_str() == rule && subject(d).ends_with(name));
    let first = matching
        .next()
        .unwrap_or_else(|| panic!("no {rule} diagnostic for {name}"));
    assert!(matching.next().is_none(), "several {rule} diagnostics for {name}");
    first
}

fn fix_of(diagnostic: &Diagnostic) -> &Fix {
    diagnostic.fixes.first().expect("a fix")
}

#[test]
fn monorepo_snapshot() {
    let diagnostics = Run::new("monorepo", "").lint();
    insta::assert_json_snapshot!("monorepo", diagnostics);
}

#[test]
fn reexports_snapshot() {
    let diagnostics = Run::new("reexports", "").lint();
    insta::assert_json_snapshot!("reexports", diagnostics);
}

#[test]
fn cycles_snapshot() {
    let diagnostics = Run::new("cycles", "").lint();
    insta::assert_json_snapshot!("cycles", diagnostics);
}

#[test]
fn monorepo_dependencies() {
    let diagnostics = Run::new("monorepo", "").lint();
    assert_eq!(
        found(&diagnostics, "unused-dependencies"),
        ["packages/app/package.json: unused-dep"]
    );
    assert_eq!(
        found(&diagnostics, "unused-dev-dependencies"),
        [
            "package.json: left-over",
            "packages/app/package.json: @types/unused-thing"
        ]
    );
    assert_eq!(
        found(&diagnostics, "unlisted-dependencies"),
        ["packages/app/src/main.ts: chalk"]
    );
    assert_eq!(found(&diagnostics, "unlisted-binaries"), ["package.json: eslint"]);
    assert_eq!(
        found(&diagnostics, "unresolved-imports"),
        ["packages/app/src/main.ts: ./missing"]
    );
}

#[test]
fn monorepo_files_and_exports() {
    let diagnostics = Run::new("monorepo", "").lint();
    assert_eq!(
        found(&diagnostics, "unused-files"),
        ["packages/lib/src/unused.ts: packages/lib/src/unused.ts"]
    );
    assert_eq!(
        found(&diagnostics, "unused-exports"),
        ["packages/app/src/format.ts: unusedFormat"]
    );
    assert_eq!(
        found(&diagnostics, "unused-types"),
        ["packages/app/src/format.ts: FormatOptions"]
    );
}

#[test]
fn removing_a_dependency_is_an_unsafe_json_edit() {
    let diagnostics = Run::new("monorepo", "").lint();
    let diagnostic = only(&diagnostics, "unused-dependencies", "unused-dep");
    let fix = fix_of(diagnostic);
    assert_eq!(fix.safety, FixSafety::Unsafe);
    assert_eq!(
        fix.ops,
        [FixOp::json(
            "packages/app/package.json",
            vec![PathSeg::Key("dependencies".into()), PathSeg::Key("unused-dep".into())],
            JsonAction::Remove,
        )]
    );
}

#[test]
fn removing_an_unused_export_keeps_the_declaration() {
    let diagnostics = Run::new("monorepo", "").lint();
    let diagnostic = only(&diagnostics, "unused-exports", "unusedFormat");
    let fix = fix_of(diagnostic);
    assert_eq!(fix.safety, FixSafety::Unsafe);
    let [FixOp::Text { path, edit, .. }] = fix.ops.as_slice() else {
        panic!("one text edit expected, got {:?}", fix.ops);
    };
    let source = fs::read_to_string(fixture_dir("monorepo").join(path)).unwrap();
    let fixed = apply_text_edits(&source, std::slice::from_ref(edit)).unwrap();
    assert!(fixed.contains("\nconst unusedFormat = 1;\n"), "{fixed}");
    assert!(fixed.contains("export const apiFormat = 2;"), "{fixed}");
}

#[test]
fn public_tag_exempts_an_export() {
    let diagnostics = Run::new("monorepo", "").lint();
    assert!(
        !found(&diagnostics, "unused-exports")
            .iter()
            .any(|s| s.contains("apiFormat"))
    );
}

#[test]
fn reexport_chains_and_member_tracking() {
    let diagnostics = Run::new("reexports", "").lint();
    assert_eq!(
        found(&diagnostics, "unused-exports"),
        [
            "src/chain/a.ts: a2",
            "src/chain/b.ts: b2",
            "src/chain/c.ts: c",
            "src/chain/index.ts: c",
            "src/namespace.ts: beta",
            "src/side.ts: sideEffect",
        ]
    );
    assert_eq!(found(&diagnostics, "unused-types"), ["src/shape.ts: Unused2"]);
    assert_eq!(
        found(&diagnostics, "unused-enum-members"),
        ["src/color.ts: Color.Green"]
    );
    assert_eq!(found(&diagnostics, "duplicate-exports"), ["src/dupe.ts: x"]);
    assert!(found(&diagnostics, "unused-files").is_empty(), "{diagnostics:#?}");
}

#[test]
fn dynamic_imports_and_globs_keep_files_and_exports_alive() {
    let diagnostics = Run::new("reexports", "").lint();
    for kept in ["src/lazy.ts", "src/plugins/one.ts", "src/plugins/two.ts"] {
        assert!(
            !diagnostics.iter().any(|d| d.location.path == Path::new(kept)),
            "{kept} should be used: {diagnostics:#?}"
        );
    }
}

#[test]
fn circular_imports_are_off_by_default_and_ignore_type_only_edges() {
    let diagnostics = Run::new("cycles", "").lint();
    let cycles: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.rule.as_str() == "circular-imports")
        .collect();
    assert_eq!(cycles.len(), 1, "{diagnostics:#?}");
    assert_eq!(cycles[0].location.path, Path::new("src/a.ts"));
    assert_eq!(cycles[0].related.len(), 1);

    let mut off = Run::new("cycles", "");
    off.config = Config::from_toml_str("").unwrap();
    let diagnostics = off.lint();
    assert_eq!(found(&diagnostics, "circular-imports"), [] as [std::string::String; 0]);
}

#[test]
fn js_can_be_disabled() {
    let diagnostics = Run::new("monorepo", "[js]\nenabled = false").lint();
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
}

#[test]
fn production_mode_skips_dev_dependencies() {
    let diagnostics = Run::new("monorepo", "[js]\nproduction = true").lint();
    assert_eq!(
        found(&diagnostics, "unused-dev-dependencies"),
        [] as [std::string::String; 0]
    );
    assert_eq!(
        found(&diagnostics, "unused-dependencies"),
        ["packages/app/package.json: unused-dep"]
    );
}

#[test]
fn ignore_lists_silence_dependency_rules() {
    let root_level = Run::new("monorepo", "[js]\nignore-dependencies = [\"unused-dep\", \"@types/*\"]").lint();
    assert_eq!(
        found(&root_level, "unused-dependencies"),
        [] as [std::string::String; 0]
    );
    assert_eq!(
        found(&root_level, "unused-dev-dependencies"),
        ["package.json: left-over"]
    );

    let per_workspace = Run::new(
        "monorepo",
        "[js.workspaces.\"packages/app\"]\nignore-dependencies = [\"unused-dep\"]",
    )
    .lint();
    assert_eq!(
        found(&per_workspace, "unused-dependencies"),
        [] as [std::string::String; 0]
    );
    assert_eq!(found(&per_workspace, "unused-dev-dependencies").len(), 2);

    let binaries = Run::new("monorepo", "[js]\nignore-binaries = [\"eslint\"]").lint();
    assert_eq!(found(&binaries, "unlisted-binaries"), [] as [std::string::String; 0]);

    let files = Run::new("monorepo", "[js]\nignore = [\"packages/lib/src/unused.ts\"]").lint();
    assert_eq!(found(&files, "unused-files"), [] as [std::string::String; 0]);
}

#[test]
fn configured_entry_replaces_the_default_entry() {
    let diagnostics = Run::new(
        "monorepo",
        "[js.workspaces.\"packages/lib\"]\nentry = [\"src/unused.ts\"]",
    )
    .lint();
    assert!(found(&diagnostics, "unused-files").is_empty(), "{diagnostics:#?}");
}

struct Seam;

impl InputSource for Seam {
    fn inputs(&self, request: &InputRequest<'_>) -> WorkspaceInputs {
        match request.package.name.as_deref() {
            Some("@fx/lib") => WorkspaceInputs {
                entry: vec!["src/unused.ts".to_owned()],
                ..WorkspaceInputs::default()
            },
            Some("fx-root") => WorkspaceInputs {
                dependencies: vec!["left-over".to_owned()],
                ..WorkspaceInputs::default()
            },
            _ => WorkspaceInputs::default(),
        }
    }
}

#[test]
fn input_sources_add_entries_and_references() {
    let run = Run::new("monorepo", "");
    let provider = JsProvider::new(Arc::clone(&run.fs)).with_input_source(Arc::new(Seam));
    let diagnostics = run.lint_with(provider);
    assert!(found(&diagnostics, "unused-files").is_empty(), "{diagnostics:#?}");
    assert_eq!(
        found(&diagnostics, "unused-dev-dependencies"),
        ["packages/app/package.json: @types/unused-thing"]
    );
}

#[test]
fn rule_metadata_is_stable() {
    let provider = JsProvider::new(Arc::new(MemoryFs::new()));
    let registry = Registry::new(vec![Box::new(provider)]).unwrap();
    let defaults: Vec<(&str, bool)> = registry.rules().map(|meta| (meta.id, meta.default.is_some())).collect();
    assert!(defaults.contains(&("unused-files", true)));
    assert!(defaults.contains(&("circular-imports", false)));
    assert_eq!(defaults.len(), 14);
}
