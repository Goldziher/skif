//! Lints fixture workspaces with the JS provider and checks the diagnostics.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use skif_config::Config;
use skif_core::{Diagnostic, FileSystem, Fix, FixOp, FixSafety, JsonAction, MemoryFs, PathSeg, apply_text_edits};
use skif_js::{InputRequest, InputSource, JsProvider, PluginSource, WorkspaceInputs};
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
        Self::at(case, "", extra_toml)
    }

    /// The fixture with the root at `root`, a directory of the fixture (`""` for the fixture itself).
    fn at(case: &str, root: &str, extra_toml: &str) -> Self {
        let base = fixture_dir(case);
        let mut memory = MemoryFs::new();
        let mut file_config = None;
        load(&base, &base, &mut memory, &mut file_config);
        let toml = format!("{}\n{extra_toml}", file_config.unwrap_or_default());
        let config = Config::from_toml_str(&toml).unwrap();
        let workspace = Workspace::discover_with_fs(&Path::new(ROOT).join(root), &config, &memory).unwrap();
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

    /// Lints with the plugin table, the way the CLI does.
    fn lint_with_plugins(&self) -> Vec<Diagnostic> {
        let plugins = PluginSource::new(self.config.js.plugins.clone());
        self.lint_with(JsProvider::new(Arc::clone(&self.fs)).with_input_source(Arc::new(plugins)))
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

fn none() -> Vec<String> {
    Vec::new()
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
fn typeof_import_snapshot() {
    let diagnostics = Run::new("typeof-import", "").lint();
    insta::assert_json_snapshot!("typeof_import", diagnostics);
}

#[test]
fn unresolved_imports_snapshot() {
    let diagnostics = Run::new("unresolved-imports", "").lint_with_plugins();
    insta::assert_json_snapshot!("unresolved_imports", diagnostics);
}

#[test]
fn cycles_snapshot() {
    let diagnostics = Run::new("cycles", "").lint();
    insta::assert_json_snapshot!("cycles", diagnostics);
}

/// A repository whose `build` and `remote` directories hold projects of their own, linted as roots of their own
/// (vscode's layout): the outer scripts run files of `build`, and `remote` only lists what it installs.
fn nested_roots() -> Vec<(&'static str, Vec<Diagnostic>)> {
    ["", "build", "remote"]
        .into_iter()
        .map(|root| (root, Run::at("nested-roots", root, "").lint()))
        .collect()
}

#[test]
fn nested_roots_snapshot() {
    insta::assert_json_snapshot!("nested_roots", nested_roots());
}

fn nested_root_findings(root: &str, rule: &str) -> Vec<String> {
    found(&Run::at("nested-roots", root, "").lint(), rule)
}

#[test]
fn a_file_of_a_nested_root_is_judged_by_its_own_manifest() {
    // The outer root runs `build/hygiene.ts`, whose `vinyl` the manifest of `build` lists.
    assert_eq!(nested_root_findings("", "unlisted-dependencies"), none());
}

#[test]
fn a_file_of_a_nested_root_does_not_use_the_dependencies_of_the_outer_root() {
    // Only `build/hygiene.ts`, a file of the nested root, imports `kleur`; the outer root lists it too.
    assert_eq!(nested_root_findings("", "unused-dependencies"), ["package.json: kleur"]);
}

#[test]
fn files_a_script_above_a_nested_root_runs_are_its_entries() {
    // `node build/npm/postinstall.ts` and `node build/hygiene.ts` in the outer manifest; nothing runs the orphan.
    assert_eq!(
        nested_root_findings("build", "unused-files"),
        ["lib/orphan.ts: lib/orphan.ts"]
    );
    assert_eq!(nested_root_findings("build", "unused-dev-dependencies"), none());
}

#[test]
fn a_root_without_source_files_does_not_judge_its_dependencies() {
    // `remote` only lists what the code of the outer project installs it for.
    assert_eq!(nested_root_findings("remote", "unused-dependencies"), none());
}

#[test]
fn production_mode_takes_no_entries_from_scripts_above_a_nested_root() {
    // In production mode only the `start` script gives entries, and the scripts of the outer manifest give none.
    let diagnostics = Run::at("nested-roots", "build", "[js]\nproduction = true").lint();
    assert_eq!(
        found(&diagnostics, "unused-files"),
        [
            "hygiene.ts: hygiene.ts",
            "lib/orphan.ts: lib/orphan.ts",
            "npm/helper.ts: npm/helper.ts",
            "npm/postinstall.ts: npm/postinstall.ts"
        ]
    );
}

/// A member excluded with `ignore-packages` whose file the root imports: the file belongs to the member, a project of
/// its own that the run does not analyse, so nothing in it is reported, neither in the member nor in the root.
#[test]
fn ignored_member_reached_from_the_root_snapshot() {
    insta::assert_json_snapshot!("ignored_member", Run::new("ignored-member", "").lint());
}

/// mastra's `factory-ui`: `vite.config.ts` sets `root: "src"`, so Vite serves `src/index.html`, whose `/boot.tsx` is
/// `src/boot.tsx`. Only `src/orphan.ts` is unused.
#[test]
fn vite_root_locates_index_html_and_its_scripts() {
    let diagnostics = Run::new("vite-root", "").lint_with_plugins();
    assert_eq!(found(&diagnostics, "unused-files"), ["src/orphan.ts: src/orphan.ts"]);
    insta::assert_json_snapshot!("vite_root", diagnostics);
}

/// n8n's `ui.store.registration.spec.ts`: a glob or specifier that reads a file's text (`?raw`, legacy `as: 'raw'`)
/// does not load it as a module, so its matches stay unused; a plain glob and the queries that still load the module
/// (`?inline`, `?url`, `raw=true`) keep theirs.
#[test]
fn raw_globs_do_not_keep_files_used() {
    let diagnostics = Run::new("raw-globs", "").lint();
    assert_eq!(
        found(&diagnostics, "unused-files"),
        [
            "src/components/Card.ts: src/components/Card.ts",
            "src/legacy/old.ts: src/legacy/old.ts",
            "src/objects/one.ts: src/objects/one.ts",
            "src/text/plain.ts: src/text/plain.ts",
        ]
    );
    insta::assert_json_snapshot!("raw_globs", diagnostics);
}

#[test]
fn n8n_nodes_and_credentials_are_entries() {
    let diagnostics = Run::new("n8n-nodes", "").lint();
    assert_eq!(
        found(&diagnostics, "unused-files"),
        ["nodes/Example/Orphan.ts: nodes/Example/Orphan.ts"]
    );
    insta::assert_json_snapshot!("n8n_nodes", diagnostics);
}

#[test]
fn imported_base_configs_are_followed() {
    let diagnostics = Run::new("imported-base-configs", "").lint_with_plugins();
    // The base configs of a workspace package and of a relative module are read, so the packages using them are
    // judged and their dead files reported. The config that imports itself back stays unknown, but a test config can
    // only hide tests, so the other files of its package are judged too (ADR 0065).
    assert_eq!(
        found(&diagnostics, "unused-files"),
        [
            "packages/app/src/orphan.ts: packages/app/src/orphan.ts",
            "packages/cyclic/src/hidden.ts: packages/cyclic/src/hidden.ts",
            "packages/web/src/dead.ts: packages/web/src/dead.ts",
        ]
    );
    insta::assert_json_snapshot!("imported_base_configs", diagnostics);
}

/// A config skif cannot fully read leaves only the files its plugin could make entries unjudged (ADR 0065): Vite's
/// `index`/`main` module, Vitest's and Playwright's tests, setup modules and test directories, and the Next.js
/// router directories. Every other file is judged. A computed `import()` or an unparsable config keeps the whole
/// package unjudged.
#[test]
fn incomplete_configs_narrow_what_is_not_judged() {
    let diagnostics = Run::new("incomplete-scopes", "").lint_with_plugins();
    assert_eq!(
        found(&diagnostics, "unused-files"),
        [
            "packages/e2e/src/dead.ts: packages/e2e/src/dead.ts",
            "packages/next-app/lib/cropImage.ts: packages/next-app/lib/cropImage.ts",
            "packages/vite-app/src/dead.ts: packages/vite-app/src/dead.ts",
            "packages/vitest-lib/src/dead.ts: packages/vitest-lib/src/dead.ts",
        ]
    );
    insta::assert_json_snapshot!("incomplete_scopes", diagnostics);
}

/// The `playwright-computed-reporters` fixture: `e2e` spreads a reporter helper whose two returns differ and a
/// reporter list a condition adds, so every reporter either names is credited and its stale reporter is reported;
/// `opaque` spreads reporters it parses from the environment, which may name any file, so no file of it is judged;
/// `smoke` lists its reporters, an unknown option of one included, so its stale reporter is reported.
#[test]
fn computed_playwright_reporter_lists_credit_every_branch() {
    let diagnostics = Run::new("playwright-computed-reporters", "").lint_with_plugins();
    assert_eq!(
        found(&diagnostics, "unused-files"),
        [
            "packages/e2e/reporters/stale-reporter.ts: packages/e2e/reporters/stale-reporter.ts",
            "packages/smoke/reporters/stale-reporter.ts: packages/smoke/reporters/stale-reporter.ts",
        ]
    );
}

/// The `storybook-addon-helpers` fixture: `ui` wraps its addons and framework in the ESM `getAbsolutePath` helper the
/// Storybook CLI writes, which names the packages, so its addons are used and its dead file is judged; `docs` picks an
/// addon at run time, so its dependencies are not judged, but an addon only hides files of its `.storybook`
/// directory, so its dead file is judged too.
#[test]
fn storybook_addons_behind_get_absolute_path_are_used() {
    let diagnostics = Run::new("storybook-addon-helpers", "").lint_with_plugins();
    assert_eq!(found(&diagnostics, "unused-dev-dependencies"), none());
    assert_eq!(
        found(&diagnostics, "unused-files"),
        [
            "packages/docs/src/dead.ts: packages/docs/src/dead.ts",
            "packages/ui/src/dead.ts: packages/ui/src/dead.ts",
        ]
    );
}

/// The stories of the `storybook-addon-helpers` fixture are development entries: production mode leaves them out of
/// the project, so only the dead files are reported, as in a normal run.
#[test]
fn production_mode_leaves_stories_out_of_the_project() {
    let diagnostics = Run::new("storybook-addon-helpers", "[js]\nproduction = true").lint_with_plugins();
    assert_eq!(
        found(&diagnostics, "unused-files"),
        [
            "packages/docs/src/dead.ts: packages/docs/src/dead.ts",
            "packages/ui/src/dead.ts: packages/ui/src/dead.ts",
        ]
    );
}

/// The `incomplete-judged` fixture: `narrowed` merges an unknown base into its Vitest config, so its unknown entries
/// lie in the test scope, whose files are entries; `whole` has a Vitest config unknown as a whole. The exports, types
/// and enum members outside the narrowed scope are judged (ADR 0081), those of the scope's setup module and of the
/// package unjudged as a whole are not.
#[test]
fn exports_outside_a_narrowed_incomplete_scope_are_judged() {
    let diagnostics = Run::new("incomplete-judged", "").lint_with_plugins();
    assert_eq!(
        found(&diagnostics, "unused-exports"),
        [
            "packages/narrowed/src/seed.ts: unusedSeed",
            "packages/narrowed/src/util.ts: stale",
        ]
    );
    assert_eq!(
        found(&diagnostics, "unused-types"),
        ["packages/narrowed/src/util.ts: Stale"]
    );
    assert_eq!(
        found(&diagnostics, "unused-enum-members"),
        ["packages/narrowed/src/util.ts: Color.Green"]
    );
    assert_eq!(found(&diagnostics, "unused-files"), none());
}

/// A file of the narrowed scope is an entry even when the package's `project` globs leave it out: the setup module
/// under `test/` still counts as importing `seed`.
#[test]
fn a_scope_file_outside_the_project_globs_still_imports() {
    let diagnostics = Run::new(
        "incomplete-judged",
        "[js.workspaces.\"packages/narrowed\"]\nproject = [\"src/**\"]\n",
    )
    .lint_with_plugins();
    assert_eq!(
        found(&diagnostics, "unused-exports"),
        [
            "packages/narrowed/src/seed.ts: unusedSeed",
            "packages/narrowed/src/util.ts: stale",
        ]
    );
}

/// With `include-entry-exports`, the exports of the files of a narrowed scope stay unjudged: such a file may be an
/// entry its configs hide, whose exports the tool uses. Exports outside the scope are still judged.
#[test]
fn exports_in_a_narrowed_scope_stay_unjudged_with_include_entry_exports() {
    let diagnostics = Run::new("incomplete-judged", "[js]\ninclude-entry-exports = true\n").lint_with_plugins();
    let unused = found(&diagnostics, "unused-exports");
    assert!(
        unused
            .iter()
            .any(|export| export == "packages/narrowed/src/util.ts: stale"),
        "{unused:?}"
    );
    assert!(
        !unused
            .iter()
            .any(|export| export.starts_with("packages/narrowed/test/")),
        "{unused:?}"
    );
}

/// The narrowed scope of a root test config also matches the files of the packages nested below it: the setup module
/// of a nested package is an entry of the root, so what it imports from the root is used.
#[test]
fn a_root_scope_counts_the_imports_of_nested_packages() {
    let diagnostics = Run::new("incomplete-root-scope", "").lint_with_plugins();
    assert_eq!(found(&diagnostics, "unused-exports"), ["src/seed.ts: unusedSeed"]);
    assert_eq!(found(&diagnostics, "unused-files"), none());
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
            "src/side.ts: sideEffect",
        ]
    );
    assert_eq!(
        found(&diagnostics, "unused-namespace-exports"),
        ["src/namespace.ts: beta"]
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
    for kept in [
        "src/lazy.ts",
        "src/plugins/one.ts",
        "src/plugins/two.ts",
        "src/reporters/audit.ts",
    ] {
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
        let mut inputs = WorkspaceInputs::default();
        match request.package.name.as_deref() {
            Some("@fx/lib") => inputs.entry.push("src/unused.ts".to_owned()),
            Some("fx-root") => inputs.dependencies.push("left-over".to_owned()),
            _ => {}
        }
        inputs
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
    assert!(defaults.contains(&("stale-js-config", true)));
    assert!(defaults.contains(&("stale-mocks", true)));
    assert!(defaults.contains(&("unused-subpath-imports", true)));
    assert!(defaults.contains(&("unused-subpath-exports", false)));
    assert_eq!(defaults.len(), 20);
}

#[test]
fn root_tsconfig_replaces_tsconfig_json_for_the_root_package() {
    let default = Run::new("tsconfig-flags", "").lint();
    assert_eq!(found(&default, "unresolved-imports").len(), 1, "{default:#?}");
    let configured = Run::new("tsconfig-flags", "[js]\ntsconfig = \"tsconfig.app.json\"").lint();
    assert_eq!(found(&configured, "unresolved-imports"), Vec::<String>::new());
    let mut unused = found(&configured, "unused-files");
    unused.sort();
    assert_eq!(
        unused,
        [
            "packages/a/lib/unused.ts: packages/a/lib/unused.ts",
            "packages/a/other/stray.ts: packages/a/other/stray.ts",
            "scripts/tool.ts: scripts/tool.ts",
            "src/orphan.ts: src/orphan.ts",
        ]
    );
}

#[test]
fn use_tsconfig_files_takes_the_project_from_the_tsconfig() {
    let diagnostics = Run::new(
        "tsconfig-flags",
        "[js]\ntsconfig = \"tsconfig.app.json\"\nuse-tsconfig-files = true",
    )
    .lint();
    insta::assert_json_snapshot!("tsconfig_flags", diagnostics);
}

/// Test-runner mocks keep local files reachable but are not imports of dependencies; module names in strings are
/// not mocks (#16).
#[test]
fn test_mocks_snapshot() {
    let diagnostics = Run::new("test-mocks", "").lint_with_plugins();
    insta::assert_json_snapshot!("test_mocks", diagnostics);
}

/// A test-runner mock of a module that does not exist is a `stale-mocks` error, not an unresolved import: for the
/// `vi` and `jest` calls, Vitest's `vi.mock(import("x"))`, relative and aliased specifiers, and a `?react` specifier
/// with its query dropped. A mock of a
/// module that exists, a virtual mock and a mock of a package are not reported (#111).
#[test]
fn stale_mocks_snapshot() {
    let diagnostics = Run::new("stale-mocks", "").lint_with_plugins();
    insta::assert_json_snapshot!("stale_mocks", diagnostics);
}

#[test]
fn a_mock_of_a_missing_module_is_a_stale_mock_not_an_unresolved_import() {
    let diagnostics = Run::new("stale-mocks", "").lint_with_plugins();
    assert_eq!(
        found(&diagnostics, "stale-mocks"),
        [
            "src/a.test.ts: vi.mock('./gone')",
            "src/legacy.test.ts: jest.mock('./gone-jest')",
            "src/legacy.test.ts: jest.doMock('./gone-jest-do-mock')",
            "src/legacy.test.ts: jest.requireActual('./gone-required')",
            "src/variants.test.ts: vi.doMock('./gone-do-mock')",
            "src/variants.test.ts: vi.unmock('./gone-unmock')",
            "src/variants.test.ts: vi.mock('./missing-icon.svg?react')",
            "src/variants.test.ts: vi.mock('@/lib/gone')",
            "src/variants.test.ts: vi.importActual('./gone-actual')",
            "src/variants.test.ts: vi.mock('./gone-typed')",
        ],
        "{diagnostics:#?}"
    );
    assert_eq!(
        found(&diagnostics, "unresolved-imports"),
        ["src/variants.test.ts: ./missing-import"],
        "an import that resolves to nothing is still an unresolved import"
    );
    let repro = only(&diagnostics, "stale-mocks", "vi.mock('./gone')");
    assert_eq!(repro.message, "`vi.mock('./gone')` names a module that does not exist");
    assert_eq!(
        repro.help.as_deref(),
        Some("remove the call, or point it at the module's new path")
    );
    assert_eq!(repro.severity, skif_core::Severity::Error);
    let actual = only(&diagnostics, "stale-mocks", "jest.requireActual('./gone-required')");
    assert_eq!(
        actual.message, "`jest.requireActual('./gone-required')` names a module that does not exist",
        "a call that loads the real module does not mock it"
    );
    assert_eq!(
        found(&diagnostics, "unlisted-dependencies"),
        none(),
        "a mocked package is not unlisted"
    );
    assert_eq!(
        found(&diagnostics, "unused-files"),
        none(),
        "a mocked module stays reachable"
    );
}

#[test]
fn ignore_unresolved_silences_a_stale_mock() {
    let diagnostics =
        Run::new("stale-mocks", "[js]\nignore-unresolved = [\"./gone\", \"@/lib/gone\"]").lint_with_plugins();
    let stale = found(&diagnostics, "stale-mocks");
    assert!(!stale.iter().any(|d| d.ends_with("vi.mock('./gone')")), "{stale:?}");
    assert!(!stale.iter().any(|d| d.contains("@/lib/gone")), "{stale:?}");
    assert_eq!(stale.len(), 8, "{stale:?}");
    assert_eq!(found(&diagnostics, "stale-js-config"), none(), "both entries matched");
}

#[test]
fn stale_mocks_can_be_turned_off_without_bringing_the_mocks_back_as_unresolved_imports() {
    let diagnostics = Run::new("stale-mocks", "[rules]\nstale-mocks = \"off\"").lint_with_plugins();
    assert_eq!(found(&diagnostics, "stale-mocks"), none());
    assert_eq!(
        found(&diagnostics, "unresolved-imports"),
        ["src/variants.test.ts: ./missing-import"]
    );
}

/// With `stale-mocks` off, `stale-js-config` still judges the ignore lists: an `ignore-unresolved` entry that only
/// silences a mock is used, and entries that match nothing are stale (#111).
#[test]
fn stale_js_config_judges_the_ignore_lists_with_stale_mocks_off() {
    let diagnostics = Run::new(
        "stale-mocks",
        "[rules]\nstale-mocks = \"off\"\n[js]\nignore-unresolved = [\"./gone\", \"./never-written\"]\n\
         ignore-dependencies = [\"nothing-here\"]",
    )
    .lint_with_plugins();
    let mut stale: Vec<&str> = diagnostics
        .iter()
        .filter(|d| d.rule.as_str() == "stale-js-config")
        .map(|d| d.message.as_str())
        .collect();
    stale.sort_unstable();
    assert_eq!(
        stale,
        [
            "`./never-written` in `js.ignore-unresolved` matched nothing",
            "`nothing-here` in `js.ignore-dependencies` matched nothing",
        ]
    );
}

/// Imports inside `declare module` bodies use the packages they name (#20).
#[test]
fn ambient_module_imports_snapshot() {
    let diagnostics = Run::new("ambient-module-imports", "").lint();
    insta::assert_json_snapshot!("ambient_module_imports", diagnostics);
}

#[test]
fn fixture_dirs_snapshot() {
    let diagnostics = Run::new("fixture-dirs", "").lint();
    insta::assert_json_snapshot!("fixture_dirs", diagnostics);
}

#[test]
fn files_in_fixture_directories_are_not_unused_by_default() {
    let diagnostics = Run::new("fixture-dirs", "").lint();
    assert_eq!(found(&diagnostics, "unused-files"), ["src/orphan.ts: src/orphan.ts"]);
    assert_eq!(found(&diagnostics, "unused-exports"), ["src/helper.ts: unusedHelper"]);
    assert_eq!(found(&diagnostics, "unused-types"), Vec::<String>::new());
    assert_eq!(found(&diagnostics, "unused-enum-members"), Vec::<String>::new());
}

#[test]
fn include_fixtures_reports_fixture_directories_like_any_other() {
    let diagnostics = Run::new("fixture-dirs", "[js]\ninclude-fixtures = true\n").lint();
    assert_eq!(
        found(&diagnostics, "unused-files"),
        [
            "fixtures/broken/input.ts: fixtures/broken/input.ts",
            "src/orphan.ts: src/orphan.ts"
        ]
    );
    assert_eq!(
        found(&diagnostics, "unused-exports"),
        [
            "src/__fixtures__/sample.ts: unusedSample",
            "src/__fixtures__/sample.ts: SampleKind",
            "src/helper.ts: unusedHelper"
        ]
    );
    assert_eq!(
        found(&diagnostics, "unused-types"),
        ["src/__fixtures__/sample.ts: SampleShape"]
    );
}
