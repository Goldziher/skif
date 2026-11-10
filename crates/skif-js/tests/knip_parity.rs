//! Cases where the corpus runs showed skif and knip disagreeing and skif was wrong: members read in their own file,
//! types in destructured exports, `new URL` targets, `require` assertions, imports awaited together with
//! `Promise.all`, spies on a namespace member, enums read through a namespace, parameters that shadow an import,
//! fixture directories, wildcard `exports`, same-name pairs, types named by a same-name type and duplicate exports.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::Arc;

use skif_config::Config;
use skif_core::{Diagnostic, FileSystem, MemoryFs};
use skif_js::{InputSource, JsAnalysis, JsProvider, PluginSource};
use skif_lint::{Registry, RunOptions, run_unsorted};
use skif_workspace::Workspace;

const ROOT: &str = "/ws";

const PACKAGE: (&str, &str) = ("package.json", r#"{"name":"app","private":true,"main":"index.ts"}"#);

fn memory(files: &[(&str, &str)]) -> MemoryFs {
    files.iter().fold(MemoryFs::new(), |memory, (path, text)| {
        memory.with_file(Path::new(ROOT).join(path), text)
    })
}

fn lint_files(files: &[(&str, &str)]) -> Vec<Diagnostic> {
    let memory = memory(files);
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

fn found(diagnostics: &[Diagnostic], rule: &str) -> Vec<String> {
    diagnostics
        .iter()
        .filter(|d| d.rule.as_str() == rule)
        .map(|d| {
            let start = d.message.find('`').map_or(0, |i| i + 1);
            let end = d.message[start..].find('`').map_or(d.message.len(), |i| start + i);
            format!("{}: {}", d.location.path.display(), &d.message[start..end])
        })
        .collect()
}

fn none() -> Vec<String> {
    Vec::new()
}

fn analysis_text(files: &[(&str, &str)], file: &str, export: Option<&str>) -> String {
    let memory = memory(files);
    let config = Config::from_toml_str("").unwrap();
    let workspace = Workspace::discover_with_fs(Path::new(ROOT), &config, &memory).unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(memory);
    let sources: Vec<Arc<dyn InputSource>> = Vec::new();
    JsAnalysis::build(&workspace, &config, &fs, &sources, true)
        .explain_usage(Path::new(file), export)
        .render()
}

#[test]
fn a_member_read_in_its_own_file_counts_but_the_enum_being_referenced_does_not_use_every_member() {
    let diagnostics = lint_files(&[
        PACKAGE,
        (
            "index.ts",
            "import { Color, Mode, Flag, first, describe, modes, flags, flag, pick } from './enums';\n\
             console.log(Color.Green, Mode.A, Flag.On, first, describe, modes, flags, flag, pick);\n",
        ),
        (
            "enums.ts",
            "export enum Color { Red, Green, Blue }\nexport const first = Color.Red;\n\
             export function describe(c: Color): string { return String(c); }\n\
             export enum Mode { A, B }\nexport const modes = Object.values(Mode);\n\
             export enum Flag { On, Off }\nexport const flags = { ...Flag };\nexport const flag = Flag;\n\
             export const pick = (k: string) => Flag[k];\n",
        ),
    ]);
    assert_eq!(
        found(&diagnostics, "unused-enum-members"),
        ["enums.ts: Color.Blue", "enums.ts: Flag.Off", "enums.ts: Mode.B"],
        "Red is read in its file, Green by the importer; whole-enum uses in the file (`Object.values`, spread, bare, `E[k]`) credit no member"
    );
}

#[test]
fn the_types_in_a_destructured_export_belong_to_it() {
    let diagnostics = lint_files(&[
        PACKAGE,
        ("index.ts", "import { useCtx } from './ctx';\nconsole.log(useCtx);\n"),
        (
            "ctx.ts",
            "export type Props = { value: string };\n\
             function make(options: { init: (props: any) => unknown }) { return { use: options.init, provider: options.init }; }\n\
             export const { use: useCtx, provider: Provider } = make({ init: (props: Props) => props });\n",
        ),
    ]);
    assert_eq!(found(&diagnostics, "unused-types"), none());
}

#[test]
fn a_re_export_of_a_binding_in_the_signature_of_a_used_export_is_used() {
    let diagnostics = lint_files(&[
        PACKAGE,
        ("index.ts", "import { Service } from './service';\nnew Service();\n"),
        (
            "options.ts",
            "export interface Options { a: string }\nexport interface Other { b: string }\n",
        ),
        (
            "service.ts",
            "import type { Options } from './options';\nexport type { Options, Other } from './options';\n\
             export class Service { run(options: Options): void {} }\n",
        ),
    ]);
    assert_eq!(
        found(&diagnostics, "unused-types"),
        ["options.ts: Other", "service.ts: Other"],
        "`Options` names the parameter type of the used `Service`"
    );
}

#[test]
fn a_new_url_target_is_used_as_a_whole() {
    let diagnostics = lint_files(&[
        PACKAGE,
        (
            "index.ts",
            "export const asset = new URL('./generated.ts', import.meta.url);\n",
        ),
        (
            "generated.ts",
            "export type A = { a: 1 };\nexport interface B { b: 1 }\nexport const c = 1;\n",
        ),
    ]);
    assert_eq!(found(&diagnostics, "unused-exports"), none());
    assert_eq!(found(&diagnostics, "unused-types"), none());
    assert_eq!(found(&diagnostics, "unused-files"), none());
}

#[test]
fn a_require_asserted_with_typeof_import_uses_only_what_it_destructures() {
    let diagnostics = lint_files(&[
        PACKAGE,
        (
            "index.ts",
            "const { used } = require('./lazy') as typeof import('./lazy');\nused();\n",
        ),
        ("lazy.ts", "export function used() {}\nexport function unusedOne() {}\n"),
    ]);
    assert_eq!(found(&diagnostics, "unused-exports"), ["lazy.ts: unusedOne"]);
}

#[test]
fn imports_awaited_together_use_only_what_their_bindings_read() {
    let diagnostics = lint_files(&[
        PACKAGE,
        (
            "index.ts",
            "export async function load() {\n\
               const [{ used }, helpers] = await Promise.all([import('./named'), import('./spaced')]);\n\
               return [used(), helpers.read()];\n\
             }\n",
        ),
        (
            "named.ts",
            "export function used() {}\nexport function unusedNamed() {}\n",
        ),
        (
            "spaced.ts",
            "export function read() {}\nexport function unusedSpaced() {}\n",
        ),
    ]);
    assert_eq!(found(&diagnostics, "unused-exports"), ["named.ts: unusedNamed"]);
    assert_eq!(
        found(&diagnostics, "unused-namespace-exports"),
        ["spaced.ts: unusedSpaced"]
    );
}

#[test]
fn a_spy_on_a_namespace_uses_only_the_member_it_spies_on() {
    let diagnostics = lint_files(&[
        PACKAGE,
        (
            "index.ts",
            "import * as utils from './utils';\nexport const stub = (vi: any) => vi.spyOn(utils, 'spied');\n",
        ),
        ("utils.ts", "export function spied() {}\nexport function unspied() {}\n"),
    ]);
    assert_eq!(found(&diagnostics, "unused-namespace-exports"), ["utils.ts: unspied"]);
    assert_eq!(found(&diagnostics, "unused-exports"), none());
}

#[test]
fn an_enum_read_through_a_namespace_uses_only_the_members_read_on_it() {
    let diagnostics = lint_files(&[
        PACKAGE,
        (
            "index.ts",
            "import * as C from './constants';\nimport * as R from './reexports';\n\
             console.log(C.Ids.Open, C.Ids['Close'], C.Kinds, C.Grouped.Inner, R.Levels.High);\n\
             export let mode: C.Modes.On | undefined;\n",
        ),
        (
            "constants.ts",
            "export const enum Ids { Open = 'open', Close = 'close', Unused = 'unused' }\n\
             export enum Kinds { A, B }\nexport const enum Modes { On, Off }\n\
             export namespace Grouped { export const Inner = 1; export const Other = 2; }\n",
        ),
        ("levels.ts", "export enum Levels { High, Low }\n"),
        ("reexports.ts", "export { Levels } from './levels';\n"),
    ]);
    assert_eq!(
        found(&diagnostics, "unused-enum-members"),
        [
            "constants.ts: Ids.Unused",
            "constants.ts: Modes.Off",
            "levels.ts: Levels.Low"
        ],
        "`C.Kinds` is used as a whole; `R.Levels` follows the re-export"
    );
    assert_eq!(
        found(&diagnostics, "unused-namespace-members"),
        ["constants.ts: Grouped.Other"]
    );
}

#[test]
fn a_parameter_that_shadows_a_namespace_import_does_not_use_the_namespace() {
    let diagnostics = lint_files(&[
        PACKAGE,
        ("index.ts", "import { scan } from './lexer';\nscan('x');\n"),
        (
            "lexer.ts",
            "import * as chars from './chars';\n\
             export function scan(input: string) { return chars.isDigit(input.length) && attempt(input); }\n\
             function attempt(chars: string) { return chars.length > 0 && check(chars); }\n\
             function check(_: string) { return true; }\n",
        ),
        (
            "chars.ts",
            "export const isDigit = (c: number) => c > 0;\nexport const $PIPE = 124;\n",
        ),
    ]);
    assert_eq!(
        found(&diagnostics, "unused-namespace-exports"),
        ["chars.ts: $PIPE"],
        "the parameter `chars` is passed around, not the namespace"
    );
}

#[test]
fn a_type_that_shares_its_name_with_a_value_export_credits_what_it_names() {
    let diagnostics = lint_files(&[
        PACKAGE,
        (
            "index.ts",
            "import type { Service } from './client';\nimport { IStore } from './store';\n\
             export let client: Service | undefined;\nconsole.log(IStore);\n",
        ),
        (
            "client.ts",
            "export interface Interface { run(): void }\n\
             export class ClientService { constructor(readonly impl: Interface) {} }\n\
             export const Service = ClientService;\nexport type Service = ClientService;\nexport class Unrelated {}\n",
        ),
        (
            "store.ts",
            "declare function createDecorator<T>(id: string): T;\nexport interface Base { close(): void }\n\
             export const IStore = createDecorator<IStore>('store');\n\
             export interface IStore extends Base { read(options: ReadOptions): Entry }\n\
             export interface ReadOptions { key: string }\nexport type Entry = string;\n\
             export interface Orphan { x: number }\n",
        ),
    ]);
    assert_eq!(
        found(&diagnostics, "unused-exports"),
        ["client.ts: Unrelated"],
        "the type `Service` names `ClientService`, used as a type"
    );
    assert_eq!(
        found(&diagnostics, "unused-types"),
        ["store.ts: Orphan"],
        "the interface `IStore` shares its name with the used value"
    );
}

#[test]
fn a_value_and_a_type_with_one_name_are_reported_once() {
    let diagnostics = lint_files(&[
        PACKAGE,
        ("index.ts", "import './pair';\n"),
        ("pair.ts", "export const Token = 1;\nexport type Token = number;\n"),
    ]);
    assert_eq!(found(&diagnostics, "unused-exports"), ["pair.ts: Token"]);
    assert_eq!(found(&diagnostics, "unused-types"), none());
}

#[test]
fn duplicate_exports_follow_initialisers_and_ignore_export_clauses() {
    let diagnostics = lint_files(&[
        PACKAGE,
        ("index.ts", "import * as lib from './lib';\nconsole.log(lib);\n"),
        (
            "lib.ts",
            "export class A {}\nexport { A as B };\nexport function f() {}\nexport const g = f;\n\
             export const h = 1;\nexport default h;\n",
        ),
    ]);
    assert_eq!(
        found(&diagnostics, "duplicate-exports"),
        ["lib.ts: f", "lib.ts: h"],
        "`f` and `g` share a binding through the initialiser, `h` is also the default; `A as B` is a deliberate alias"
    );
}

#[test]
fn an_export_initialised_with_an_alias_export_duplicates_it() {
    let diagnostics = lint_files(&[
        PACKAGE,
        ("index.ts", "import * as lib from './lib';\nconsole.log(lib);\n"),
        (
            "lib.ts",
            "const base = () => 1;\nexport const first = base;\nexport const second = first;\n",
        ),
    ]);
    assert_eq!(
        found(&diagnostics, "duplicate-exports"),
        ["lib.ts: first"],
        "`second` is `first`, itself initialised with the unexported `base`"
    );
}

#[test]
fn only_fixtures_and_dunder_fixtures_directories_hold_test_data() {
    let diagnostics = lint_files(&[
        PACKAGE,
        (
            "index.ts",
            "import './fixtures/a';\nimport './colorize-fixtures/b';\nimport './__testfixtures__/c';\n",
        ),
        ("fixtures/a.ts", "import 'not-installed-a';\n"),
        ("colorize-fixtures/b.ts", "import 'not-installed-b';\n"),
        ("__testfixtures__/c.ts", "import 'not-installed-c';\n"),
    ]);
    let mut unlisted = found(&diagnostics, "unlisted-dependencies");
    unlisted.sort();
    assert_eq!(
        unlisted,
        [
            "__testfixtures__/c.ts: not-installed-c",
            "colorize-fixtures/b.ts: not-installed-b"
        ]
    );
}

#[test]
fn a_spec_file_with_an_underscore_is_not_a_test_entry() {
    let diagnostics = lint_files(&[
        PACKAGE,
        ("index.ts", "export const main = 1;\n"),
        ("foo.ts", "export function helper() {}\n"),
        ("foo_spec.ts", "import { helper } from './foo';\nhelper();\n"),
    ]);
    assert_eq!(paths(&diagnostics, "unused-files"), ["foo.ts", "foo_spec.ts"]);
}

#[test]
fn a_wildcard_in_exports_matches_across_directories() {
    let diagnostics = lint_files(&[
        (
            "package.json",
            r#"{"name":"app","private":true,"exports":{"./*":"./src/*.ts"}}"#,
        ),
        ("src/promise/agent.ts", "export interface AgentEditor { id: string }\n"),
        ("src/top.ts", "export const top = 1;\n"),
    ]);
    assert_eq!(found(&diagnostics, "unused-types"), none());
    assert_eq!(found(&diagnostics, "unused-exports"), none());
    assert_eq!(found(&diagnostics, "unused-files"), none());
}

fn paths(diagnostics: &[Diagnostic], rule: &str) -> Vec<String> {
    diagnostics
        .iter()
        .filter(|d| d.rule.as_str() == rule)
        .map(|d| d.location.path.display().to_string())
        .collect()
}

#[test]
fn the_catch_all_export_does_not_make_every_file_an_entry() {
    let diagnostics = lint_files(&[
        (
            "package.json",
            r#"{"name":"app","private":true,"exports":{"./*":"./*"}}"#,
        ),
        ("top.ts", "export const top = 1;\n"),
        ("sub/deep.ts", "export const deep = 1;\n"),
    ]);
    assert_eq!(paths(&diagnostics, "unused-files"), ["sub/deep.ts", "top.ts"]);
}

#[test]
fn bin_and_main_globs_are_not_widened_across_directories() {
    let diagnostics = lint_files(&[
        (
            "package.json",
            r#"{"name":"app","private":true,"bin":"bin/*.js","main":"lib/*.js"}"#,
        ),
        ("bin/tool.ts", "export const tool = 1;\n"),
        ("bin/nested/inner.ts", "export const inner = 1;\n"),
        ("lib/nested/inner.ts", "export const other = 1;\n"),
    ]);
    let unused = paths(&diagnostics, "unused-files");
    assert!(unused.contains(&"bin/nested/inner.ts".to_owned()), "{unused:?}");
    assert!(unused.contains(&"lib/nested/inner.ts".to_owned()), "{unused:?}");
    assert!(!unused.contains(&"bin/tool.ts".to_owned()), "{unused:?}");
}

const EXPLAIN_FILES: [(&str, &str); 5] = [
    PACKAGE,
    ("index.ts", "import { go } from './barrel';\ngo();\n"),
    ("barrel.ts", "export { run as go } from './lib';\n"),
    ("lib.ts", "export function run() {}\nexport function idle() {}\n"),
    ("orphan.ts", "export const lonely = 1;\n"),
];

#[test]
fn the_explanation_of_a_file_names_its_importers_and_the_chain_from_an_entry() {
    let text = analysis_text(&EXPLAIN_FILES, "index.ts", None);
    assert!(text.contains("entry file: "), "{text}");
    let text = analysis_text(&EXPLAIN_FILES, "lib.ts", None);
    assert!(text.contains("barrel.ts:1"), "{text}");
    assert!(
        text.contains("reached from an entry through: index.ts -> barrel.ts -> lib.ts"),
        "{text}"
    );
}

#[test]
fn the_explanation_of_a_script_file_names_the_script_whatever_the_path_spelling() {
    let files = [
        (
            "package.json",
            r#"{"name":"root","private":true,"workspaces":["packages/*"],"scripts":{"gen":"tsx ./scripts/gen.ts"}}"#,
        ),
        (
            "packages/db/package.json",
            r#"{"name":"db","private":true,"scripts":{"seed":"tsx ../../scripts/seed.ts"}}"#,
        ),
        ("scripts/gen.ts", "export {};\n"),
        ("scripts/seed.ts", "export {};\n"),
    ];
    for file in ["scripts/gen.ts", "scripts/seed.ts"] {
        let text = analysis_text(&files, file, None);
        assert!(text.contains("entry file: run by a script of package.json"), "{text}");
    }
}

#[test]
fn the_explanation_of_an_export_follows_re_exports_to_the_user() {
    let used = analysis_text(&EXPLAIN_FILES, "lib.ts", Some("run"));
    assert!(used.contains("re-exported by barrel.ts as `go`"), "{used}");
    assert!(used.contains("imported by index.ts:1"), "{used}");
    assert!(used.contains("=> used by another module"), "{used}");
    let idle = analysis_text(&EXPLAIN_FILES, "lib.ts", Some("idle"));
    assert!(idle.contains("no module imports it by name or as a whole"), "{idle}");
    assert!(idle.contains("=> not used by another module"), "{idle}");
}

#[test]
fn the_explanation_of_an_unreached_file_says_so() {
    let text = analysis_text(&EXPLAIN_FILES, "orphan.ts", None);
    assert!(text.contains("`unused-files`"), "{text}");
}
