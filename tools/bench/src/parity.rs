//! Findings parity with knip: runs skif and a pinned knip on each corpus repository and diffs the findings.
//!
//! Both tools' findings are reduced to a key per finding, `<file>|<name>` within a knip issue type. The two sides
//! disagree where the tools model things differently, so the numbers are a measure of drift, not a verdict: the
//! committed baselines say what the agreement was, and `--check` fails when it moves.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Argument, ArrayExpression, ArrayExpressionElement, AssignmentExpression, AssignmentOperator, AssignmentTarget,
    BindingPattern, CallExpression, ComputedMemberExpression, Expression, ImportExpression, ObjectPropertyKind,
    PropertyKey, StaticMemberExpression, TSImportType, TSImportTypeQualifier, TSTypeName, TSTypeQuery, TSTypeReference,
    VariableDeclarator,
};
use oxc_ast_visit::{Visit, walk};
use oxc_parser::Parser;
use oxc_span::SourceType;
use serde_json::{Value, json};
use skif_js::{IncompleteScopeGlobs, is_out_dir, n8n_load_paths, source_candidates};
use skif_js_plugins::plugins::heft;
use skif_js_plugins::scripts::{IGNORED_BINARIES, is_machine_binary, is_shell_builtin};
use skif_js_plugins::{GlobCache, PluginContext, PluginId, PluginOutput, resolve_plugins};
use skif_js_syntax::{
    Export, ExportKind, Import, ImportKind, ImportedKind, MockCall, ModuleInfo, ParseOptions, parse_module,
};
use skif_workspace::{FileSystem, OsFs, declared_member_dirs};

use crate::corpus::{self, RUSH_KNIP_ROOT, Repo, SCRATCH_ROOT_MARK, Tools, is_scratch_root};
use crate::diff::{
    self, BASELINE_SCHEMA, Baseline, COMPARED, CategorySet, Counts, Deviation, Entry, Gate, Keys, Samples, Side,
};
use crate::proc::{Captured, capture, find_on_path};
use crate::provenance;
use crate::run::table_cell;
use crate::type_aware;

/// skif rule to knip issue type.
const RULE_CATEGORIES: [(&str, &str); 14] = [
    ("unused-dependencies", "dependencies"),
    ("unused-dev-dependencies", "devDependencies"),
    ("unused-optional-peer-dependencies", "optionalPeerDependencies"),
    ("unlisted-dependencies", "unlisted"),
    ("unlisted-binaries", "binaries"),
    ("unresolved-imports", "unresolved"),
    ("unused-files", "files"),
    ("unused-exports", "exports"),
    ("unused-types", "types"),
    ("unused-namespace-exports", "nsExports"),
    ("unused-namespace-types", "nsTypes"),
    ("unused-enum-members", "enumMembers"),
    ("unused-namespace-members", "namespaceMembers"),
    ("duplicate-exports", "duplicates"),
];

/// knip issue type (as named in its JSON) to the common category. `nsExports` and `nsTypes` are their own categories
/// (the counterparts of `unused-namespace-exports` and `unused-namespace-types`, ADR 0018), not names of
/// `namespaceMembers`; knip's default run reports a namespace-only module's unused exports and types under `exports`
/// and `types` with a `namespace` field, which [`knip_keys`] moves into them. `classMembers` has no skif rule and is
/// left out of the comparison.
const KNIP_CATEGORIES: [(&str, &str); 14] = [
    ("dependencies", "dependencies"),
    ("devDependencies", "devDependencies"),
    ("optionalPeerDependencies", "optionalPeerDependencies"),
    ("unlisted", "unlisted"),
    ("binaries", "binaries"),
    ("unresolved", "unresolved"),
    ("files", "files"),
    ("exports", "exports"),
    ("types", "types"),
    ("enumMembers", "enumMembers"),
    ("namespaceMembers", "namespaceMembers"),
    ("nsExports", "nsExports"),
    ("nsTypes", "nsTypes"),
    ("duplicates", "duplicates"),
];

pub const NON_CODE_FILE: &str = "non-code-file";
pub const TEST_FILE: &str = "test-file";
pub const FIXTURE_FILE: &str = "fixture-file";
pub const KNIP_UNREAD: &str = "knip-unread";
pub const NOT_JUDGED: &str = "not-judged-by-skif";
pub const KNIP_CONFIG_ERROR: &str = "knip-config-error";
pub const FOREIGN_TSCONFIG_PATHS: &str = "foreign-tsconfig-paths";

/// The `detail` of an `unlisted-dependencies` finding whose import only another package's tsconfig `paths` map
/// (ADR 0019).
const FOREIGN_TSCONFIG_PATHS_DETAIL: &str = "ForeignTsconfigPaths";
pub const KNIP_UNJUDGED_MANIFEST: &str = "knip-unjudged-manifest";
pub const NON_MEMBER_PROJECT: &str = "non-member-project";
pub const SKIPPED_DIRECTORY: &str = "skipped-directory";
pub const PUBLISHED_SOURCE: &str = "published-source";
pub const IN_KNIP_UNUSED_FILE: &str = "in-knip-unused-file";
pub const IN_SKIF_UNUSED_FILE: &str = "in-skif-unused-file";
pub const USED_IN_KNIP_UNUSED_FILE: &str = "used-in-knip-unused-file";
pub const USED_IN_SKIF_UNUSED_FILE: &str = "used-in-skif-unused-file";
pub const RUSH_ROOT_FILE: &str = "rush-root-file";
pub const IMPORTED_BY_KNIP_UNUSED_FILE: &str = "imported-by-knip-unused-file";
pub const IMPORTED_BY_SKIF_UNUSED_FILE: &str = "imported-by-skif-unused-file";
pub const ENTRY_MODULE_DUPLICATE: &str = "entry-module-duplicate";
pub const BAZEL_BUILD_FILE: &str = "bazel-build-file";
pub const DECLARATION_FILE: &str = "declaration-file";
pub const SIGNATURE_TYPE: &str = "signature-type";
pub const RUSH_PROVIDED_COMMAND: &str = "rush-provided-command";
pub const GLOBAL_BINARY: &str = "global-binary";
pub const SHELL_BUILTIN: &str = "shell-builtin";
pub const DATA_URI: &str = "data-uri";
pub const REPLACED_KNIP_CONFIG: &str = "replaced-knip-config";
pub const AMBIENT_MODULE: &str = "ambient-module";
pub const OWN_TSCONFIG_PATHS: &str = "own-tsconfig-paths";
pub const PATHS_BEFORE_PACKAGES: &str = "paths-before-packages";
pub const WHOLE_ENUM_USE: &str = "whole-enum-use";
pub const PUBLISHED_FILE: &str = "published-file";
pub const UNINSTALLED_PROJECT: &str = "uninstalled-project";
pub const TYPE_RESOLVED: &str = "type-resolved";
pub const KNIP_DOT_IGNORE: &str = "knip-dot-ignore";
pub const REEXPORT_ALIAS: &str = "re-export-alias";
pub const TSCONFIG_DECLARATION_FILE: &str = "tsconfig-declaration-file";
pub const MODULE_OBJECT_TYPES: &str = "module-object-types";
pub const HEFT_CONFIG: &str = "heft-config";
pub const TYPE_ASSERTED_REQUIRE: &str = "type-asserted-require";
pub const TEMPLATE_LITERAL_IMPORT: &str = "template-literal-import";
pub const COMMONJS_DEFAULT_IMPORT: &str = "commonjs-default-import";
pub const INSTALLED_MAIN: &str = "installed-main";
pub const TYPE_REFERENCED_MEMBERS: &str = "type-referenced-members";
pub const MODULE_OBJECT_VALUES: &str = "module-object-values";
pub const KNIP_NODE_MODULES_NAME: &str = "knip-node-modules-name";
pub const RAW_GLOB_READ: &str = "raw-glob-read";
pub const TYPESCRIPT_TWIN: &str = "typescript-twin";
pub const OPAQUE_DYNAMIC_IMPORT: &str = "opaque-dynamic-import";
pub const DYNAMIC_IMPORT_DEFAULT: &str = "dynamic-import-default";
pub const JEST_REQUIRE_ACTUAL: &str = "jest-require-actual";
pub const SPIED_NAMESPACE: &str = "spied-namespace";
pub const ENUMERATED_NAMESPACE: &str = "enumerated-namespace";
pub const NAMESPACE_REEXPORT: &str = "namespace-re-export";
pub const TYPESCRIPT_COMMONJS_EXPORT: &str = "typescript-commonjs-export";
pub const NODE_TEST_ENTRY: &str = "node-test-entry";
pub const NG_PACKAGE_ENTRY: &str = "ng-package-entry";
pub const IMPORT_TYPE_QUALIFIER: &str = "import-type-qualifier";
pub const MODULE_AUGMENTATION: &str = "module-augmentation";
pub const JEST_MOCK_SPECIFIER: &str = "jest-mock-specifier";
pub const NEXTRA_ALIAS: &str = "nextra-resolve-alias";
pub const EXPO_UPDATES: &str = "expo-updates";
pub const OPTIONAL_PEER_HOST: &str = "optional-peer-host";
pub const SHARED_CONFIG_REFERENCE: &str = "shared-config-reference";
pub const SHELL_TEMPLATE_DIRECTORY: &str = "shell-template-directory";
pub const NESTED_VITEST_SETUP: &str = "nested-vitest-setup";
pub const GITIGNORED_IMPORT: &str = "gitignored-import";
pub const COMMAND_DIRECTORY: &str = "command-directory";
pub const PASSTHROUGH_COMMAND: &str = "passthrough-command";
pub const BUN_FILE_ARGUMENT: &str = "bun-file-argument";
pub const REQUIRED_CHILD_PROCESS: &str = "required-child-process";

/// The name of a file of git ignore patterns.
const GITIGNORE: &str = ".gitignore";

/// The file that makes a checkout a Rush repository (`RUSH_JSON` in `crates/skif-js/src/deps.rs`).
const RUSH_JSON: &str = "rush.json";

/// The commands Rush itself provides to the scripts of a Rush repository (`RUSH_COMMANDS` in
/// `crates/skif-js/src/deps.rs`, ADR 0012).
const RUSH_COMMANDS: [&str; 3] = ["rush", "rushx", "rush-pnpm"];

/// The names of knip's configuration file, which the harness replaces with an empty one.
const KNIP_CONFIG_FILES: [&str; 14] = [
    "knip.json",
    "knip.jsonc",
    ".knip.json",
    ".knip.jsonc",
    "knip.ts",
    "knip.js",
    "knip.mts",
    "knip.mjs",
    "knip.cts",
    "knip.cjs",
    "knip.config.ts",
    "knip.config.js",
    "knip.config.mts",
    "knip.config.mjs",
];

/// The dependency categories skif does not judge anywhere in a package whose entry files are not all known (ADR 0022),
/// whatever the package's `scope`.
const NOT_JUDGED_DEPENDENCY_CATEGORIES: [&str; 2] = ["dependencies", "devDependencies"];

/// The categories skif does not judge in the files of a package whose entry files are not all known: files (ADR 0022),
/// and exports, types and members, which skif stopped reporting there with #48 (ADR 0021). Where the package's `scope`
/// is narrowed to globs (`{"within": [...]}`), skif judges them in every file outside the globs (ADR 0016).
const NOT_JUDGED_FILE_CATEGORIES: [&str; 7] = [
    "files",
    "exports",
    "types",
    "nsExports",
    "nsTypes",
    "enumMembers",
    "namespaceMembers",
];

/// The categories skif does not report in a fixture directory (ADR 0014): files, exports, types and members unless
/// `[js] include-fixtures` is set, which the harness never sets, and unresolved imports and unlisted dependencies at
/// all.
const FIXTURE_CATEGORIES: [&str; 9] = [
    "files",
    "exports",
    "types",
    "nsExports",
    "nsTypes",
    "enumMembers",
    "namespaceMembers",
    "unresolved",
    "unlisted",
];

/// The categories `non-member-project` leaves compared in a project that skif lints as a package of a root of a
/// recursive run (ADR 0006): skif judges the exports, types, members and duplicate exports of its files as knip does
/// from the enclosing workspace. Unused files, unresolved imports and unlisted dependencies there depend on the
/// project's own entry files, plugins and manifest, which knip does not read, so they stay set apart.
const ROOT_COMPARED_CATEGORIES: [&str; 7] = [
    "exports",
    "types",
    "nsExports",
    "nsTypes",
    "enumMembers",
    "namespaceMembers",
    "duplicates",
];

/// The categories whose findings name exports of a file, `<file>|<export>` or, for duplicates, `<file>|<names>`: what an
/// import of the file uses.
const IMPORTED_EXPORT_CATEGORIES: [&str; 5] = ["exports", "types", "nsExports", "nsTypes", "duplicates"];

/// The categories whose findings name a member of an enum or namespace export, `<file>|<member>`.
const MEMBER_CATEGORIES: [&str; 2] = ["enumMembers", "namespaceMembers"];

/// The categories whose findings name a value export of a file, `<file>|<export>`: what a `require` or a use of a module
/// object reads, which holds no type.
const VALUE_EXPORT_CATEGORIES: [&str; 2] = ["exports", "nsExports"];

/// The categories whose findings name a type export of a file, `<file>|<type>`.
const TYPE_CATEGORIES: [&str; 2] = ["types", "nsTypes"];

/// The categories whose findings name an import of a file that a tool reads: `<file>|<specifier>`.
const IMPORT_CATEGORIES: [&str; 2] = ["unlisted", "unresolved"];

/// The extensions of a declaration file, as knip's `IS_DTS` matches them.
const DECLARATION_EXTENSIONS: [&str; 3] = [".d.ts", ".d.mts", ".d.cts"];

/// The text a file holds when skif's parser can find a whole use of a module object in it (`import()`, `require` or a
/// namespace import); the files without any are not parsed for [`module_object_credits`].
const WHOLE_USE_MARKERS: [&str; 3] = ["import(", "require(", "* as "];

/// The name that stands for every export of a file among the exports an import uses ([`unused_file_credits`]): a
/// namespace used as a whole, `import()` or `require` of the module, or a star re-export of it.
const WHOLE_MODULE: &str = "*";

/// The categories of a finding only skif reports that `non-member-project` sets apart below a project that is not one of
/// knip's workspaces: they follow from the project's own entry files, plugins, configs and manifest, which a recursive
/// skif run lints the project with and knip does not read there.
const NON_MEMBER_SKIF_CATEGORIES: [&str; 3] = ["files", "unlisted", "binaries"];

/// The files that declare a Bazel package (`BUILD_FILES` of `skif_js_plugins::bazel`).
const BUILD_FILES: [&str; 2] = ["BUILD.bazel", "BUILD"];

/// The files that mark the root of a Bazel repository (`ROOT_FILES` of `skif_js_plugins::bazel`).
const BAZEL_ROOT_FILES: [&str; 4] = ["MODULE.bazel", "REPO.bazel", "WORKSPACE.bazel", "WORKSPACE"];

/// What a root file of a Bazel repository that uses `rules_js` holds; skif's Bazel plugin reads no other repository.
const RULES_JS: &str = "aspect_rules_js";

/// The manifest categories of a listed package that an import makes used.
const IMPORTED_CATEGORIES: [&str; 3] = ["dependencies", "devDependencies", "optionalPeerDependencies"];

/// The categories whose findings sit on a `package.json`: its dependencies, and the binaries its scripts run.
const MANIFEST_CATEGORIES: [&str; 4] = [
    "dependencies",
    "devDependencies",
    "optionalPeerDependencies",
    "binaries",
];

/// The dependency fields of a manifest that list a package.
const DEPENDENCY_FIELDS: [&str; 4] = [
    "dependencies",
    "devDependencies",
    "peerDependencies",
    "optionalDependencies",
];

/// Categories that name the same thing, `<file>|<export>`, and differ only in how the tools label it: knip counts a
/// `const enum` as a type and files an export under `nsExports` when any importer uses a namespace, skif when all do.
const EXPORT_LABELS: [&str; 4] = ["exports", "types", "nsExports", "nsTypes"];

/// The known differences between skif and knip.
pub const KNIP_DEVIATIONS: [Deviation; 71] = [
    Deviation {
        id: NON_CODE_FILE,
        reason: "knip lists stylesheets, markup, data and docs as unused files; skif's project set is code files only",
    },
    Deviation {
        id: DECLARATION_FILE,
        reason: "knip reports these declaration files (`.d.ts`) as unused; skif never reports a `.d.ts` file as unused, since one holds ambient declarations or the typings a package publishes, which no import needs to name. skif's exemption covers `.d.ts` only, so a `.d.mts` or `.d.cts` file stays compared",
    },
    Deviation {
        id: TEST_FILE,
        reason: "knip reports these test files and files below test directories as unused and skif does not: its test-runner plugins make test files entries. The other files below test directories are test helpers, setup files and, in next-js (thousands), the app files of test fixtures, which are not test entries; the harness sets them apart by path without establishing why skif does not report each one",
    },
    Deviation {
        id: FIXTURE_FILE,
        reason: "knip reports these findings in files below a `fixtures` or `__fixtures__` directory, where skif reports no unused file, export, type or member unless `[js] include-fixtures` is set, and no unresolved import or unlisted dependency at all (ADR 0014). Other names of test data (`fixture`, `testdata`, `*-fixtures`) are judged like any other directory by skif and are not counted here",
    },
    Deviation {
        id: KNIP_UNREAD,
        reason: "knip reads no files below these paths (a deny-all .gitignore hides them), so it reports every dependency there as unused and has no view of their files",
    },
    Deviation {
        id: NOT_JUDGED,
        reason: "skif lists the package under `incomplete-packages`: a tool config it cannot read leaves its entry files unknown, so it does not judge the package's dependencies, nor its files, exports, types and members where the config could make entries: the whole package for a `scope` of `\"package\"`, only the files its globs match (relative to the package) for a `scope` of `{\"within\": [...]}`, among them the files of the packages nested below it, which skif makes entries (ADR 0016)",
    },
    Deviation {
        id: KNIP_CONFIG_ERROR,
        reason: "knip could not load a configuration file of the package (it prints `ERROR: Error loading <file>` and exits with 2), so it ran without the entry files that configuration names: one-sided findings of either tool in that package are set apart. The errors are listed in `parity.json` under `knip_errors`. The files of other packages that only those entries would reach stay compared: the harness evaluates no configuration, so it cannot tell which files a failed one names (next-js's root `jest.config.js` has `roots` in `packages/next` and `packages/font`), and skif's report does not say what reaches a file",
    },
    Deviation {
        id: FOREIGN_TSCONFIG_PATHS,
        reason: "skif reports an import that only the tsconfig `paths` of another package map as `unlisted-dependencies` with the detail `ForeignTsconfigPaths`; knip merges the `paths` of all workspaces and resolves it (ADR 0019)",
    },
    Deviation {
        id: KNIP_UNJUDGED_MANIFEST,
        reason: "skif reports these unused dependencies, and binaries the scripts run, in a `package.json` that is not one of knip's workspaces: not the root manifest, not selected by the root's workspace patterns (`pnpm-workspace.yaml` or the `workspaces` of `package.json`, as skif's workspace discovery reads them) and not a file knip reports anything for. knip judges the dependencies and scripts of its workspaces only; a recursive skif run lints such a project as a root of its own (ADR 0006)",
    },
    Deviation {
        id: NON_MEMBER_PROJECT,
        reason: "knip reports these findings in a file (unused files, exports, types, members, duplicates and unresolved imports, and unlisted dependencies that a `package.json` at or above the file and below the workspace knip charges them to lists, or any unlisted dependency when a workspace member encloses the project, since no skif run reads the imports of a project inside a member) below a directory with its own `package.json` (one with fields besides `type`, `sideEffects` and `imports`) that is not one of knip's workspaces (as in `knip-unjudged-manifest`). knip counts such files to an enclosing workspace, without the entry files, plugins and dependencies of their own manifest; skif's analysis of the enclosing workspace skips the directory, and a recursive run lints it as a root of its own when no workspace member encloses it (ADR 0006), where its exports, types, members and duplicates stay compared, but for the exports and types of the project's entry files and those such a file re-exports through `export … from` and `export *` (followed through the files in between), which are public in skif's run of the project and which knip reads without the project's entries: its default entry files (`index`, `cli` or `main` at the top or in `src`), and the entry files and code configuration files of skif's plugins there (the plugins skif's plugin resolver, which the harness runs on the project's manifest and files, enables for the project, such as the GraphQL Code Generator outputs of a `codegen.ts` or Mastra's tools and workflows), since knip runs no plugin in a project that is not one of its workspaces. On skif's side, an unused file, unlisted dependency or unlisted binary only skif reports below such a project is set apart too: a recursive run lints the project with its own entry files, plugins, configs and manifest (a package its `.eslintrc.json` or `project.json` names), which knip does not read there; a binary on a `package.json` that knip reports anything on stays compared; so is an export, type or duplicate only skif reports in a file of knip's workspaces that a file below such a project imports (the import scan of `imported-by-knip-unused-file`), and an enum or namespace member only skif reports whose enum or namespace such a file reads the member of or uses as a whole: skif's analysis of the workspace does not read that importer, and knip does. The harness checks the manifests, not why skif has no finding for each one",
    },
    Deviation {
        id: SKIPPED_DIRECTORY,
        reason: "knip reports these findings in files below a directory that skif's file discovery skips by name, unless the file lies in a workspace member at or below that directory: `node_modules`, `.git`, `dist`, `.next`, `.turbo`, `.svelte-kit`, `storybook-static`, `.yarn`, `.vercel` and `.nuxt`; `out`, `target` and `coverage` directly in the root or a package directory; and every hidden directory but `.github`, `.storybook`, `.rnstorybook`, `.config`, `.vitepress`, `.vuepress`, `.husky` and `.changeset`. skif reads no file there. On skif's side, an export, type or duplicate only skif reports in a file of knip's workspaces that a file below such a directory imports, and an enum or namespace member only skif reports whose enum or namespace such a file reads the member of or uses as a whole, is set apart too (the import scan of `imported-by-knip-unused-file` over the hidden directories, the harness reading none of the build output ones): skif does not read the importer, and knip does",
    },
    Deviation {
        id: PUBLISHED_SOURCE,
        reason: "knip reports these files as unused, and each is either a TypeScript file whose package publishes its path with a built extension (`.js`, `.mjs`, `.cjs`, `.jsx` or a `.d.ts` variant) or without one in `main`, `module`, `source`, `types`, `typings`, `browser`, `bin`, an `exports` leaf or, in an n8n node package, `n8n.nodes` and `n8n.credentials`, where `*` matches any part of the path and a path below a build directory (`dist`, `lib`, `out`, `esm` and the like, also `lib-commonjs`) is also the path below `src` (for the n8n fields, the path without the build directory), or a file such a source imports by relative path, directly or through other files only knip reports unused. A published `bin` stub without a code extension whose first line is a `#!` line naming `node` loads the build output its relative `require`s and imports name, which maps to source the same way, as skif reads such a stub: that source and the files it reaches are published sources too. So are the sources that a JavaScript or declaration file the manifest publishes as it is (`published-file`, named without `*`) loads by a relative specifier into build output (`module.exports = require('./dist/client/index')`, `export * from './dist/client/index'`), mapped the same way, and the files they reach: skif makes such a file an entry and resolves a relative specifier into unbuilt output to the source it is built from. A published path without `*` is first mapped through the build directories of the package, as skif's TypeScript plugin reads them: below the `outDir` of a `tsconfig.json`, `tsconfig.*.json` or `jsconfig.json` of the package directory is the same path below its `rootDir` (without one, the directory its `include` and `files` share). The exports and types (also as namespace exports and types) only knip reports in a published source, or in a file whose exports a published source, or a declaration or JavaScript file the manifest publishes as it is, re-exports through `export … from` and `export *` (followed through the files in between, a specifier of a workspace package's subpath into build output resolved to its source, next.js's `server.d.ts` re-exporting `next/dist/server/web/types`), are `published-source` too: skif makes the source and the published file entries, whose exports are public. skif makes the source of a published path an entry and uses what it imports; knip maps a published path to source only through a tsconfig `outDir` it resolves itself and does not read the n8n fields. The harness checks the published path and the relative imports, not why skif has no finding for each file; enum and namespace members of these files, and the exports of the files they reach, stay compared, and their duplicates are `entry-module-duplicate`",
    },
    Deviation {
        id: ENTRY_MODULE_DUPLICATE,
        reason: "knip reports these duplicate exports (one value exported under several names, `export const x …; export default x`) in a file skif makes an entry from the manifest and the defaults alone: a default entry file of its project (`index`, `cli` or `main` at the top or in `src`) or the source of a path its manifest publishes (as in `published-source`). skif's `duplicate-exports` skips entry modules, whose names are all public API; knip reports them. Entries that only skif's plugins or the configuration make are not known to the harness, so duplicates there stay compared",
    },
    Deviation {
        id: SIGNATURE_TYPE,
        reason: "knip reports these exports and types as unused, and skif's parser finds each in the signature of another export of the same file that skif does not report as unused (a parameter, return or member type of an exported function, class or constant), directly or through other exports in such signatures. ADR 0018 counts an export that a used export's signature mentions as used; knip has no such rule. The harness takes an export skif does not report for a used one. A type that only an exported `interface` or `type` alias mentions is not exempt in skif either and stays compared",
    },
    Deviation {
        id: BAZEL_BUILD_FILE,
        reason: "skif's Bazel plugin reads the `BUILD.bazel` and `BUILD` files of a repository that uses `rules_js`, which knip has no plugin for: skif reports these unlisted dependencies at a `BUILD` file that names a package by label, and credits these dependencies knip reports as unused through a label of a `BUILD` file in the package's directory: a `node_modules` target of the package's Bazel package (`\":node_modules/zod\"`, `\"//:node_modules/typescript\"`) or the `package_json.bzl` of another repository (`\"@npm//packages/app:esbuild/package_json.bzl\"`), and a peer dependency of such a dependency that the package lists (one its installed `package.json` names in `peerDependencies`, optional or not, or the package an `@types/` one types), which skif credits as a peer of a used package, the harness scanning the string literals of every `BUILD` file below a Bazel root that names `aspect_rules_js` as skif does. An unused file only knip reports is `bazel-build-file` too when a literal of the `srcs` or `data` of a `js_library`, `ts_project` or `ng_project`, or of the `entry_point` or `data` of a `js_binary`, `js_run_devserver` or `js_test`, in a top-level call of a `BUILD` file (not inside a nested `glob` or `select`) names it in the `BUILD` file's package, when an include pattern of a `glob` (also `native.glob`) that the `srcs` or `data` of a test target (`js_test`, or one of these rules with `testonly = True`) add matches it below the `BUILD` file's directory, its `exclude` patterns not applied as skif applies none, or when such a file reaches it by relative imports through other files only knip reports unused: skif uses the files a rule builds from and what they import. Their exports stay compared: skif judges the exports of `srcs` and `data` files, and only an `entry_point` is an entry whose exports are public. An unresolved relative import only knip reports is `bazel-build-file` too when a literal of the `outs`, `out` or `stdout` of a top-level call of such a `BUILD` file names the missing file, also with another extension or none (angular's `./dist/index.js` beside a `genrule` with `outs = [\"dist/index.d.ts\"]`): skif takes it for generated code (ADR 0031)",
    },
    Deviation {
        id: IN_KNIP_UNUSED_FILE,
        reason: "skif reports these findings (anything but unused files and manifest findings) in a file that knip lists as unused. knip does not analyse an unused file, so it reports nothing inside one; the disagreement is the file, which `files` already counts once",
    },
    Deviation {
        id: IN_SKIF_UNUSED_FILE,
        reason: "knip reports these findings (anything but unused files and manifest findings) in a file that skif reports as unused, so skif reports nothing inside it; the disagreement is the file, which `files` already counts once",
    },
    Deviation {
        id: RUSH_ROOT_FILE,
        reason: "knip reports these findings in files of a Rush repository's root that lie in no project (`.prettierrc.js`, `common/scripts`, `common/config`): the root `package.json` the harness writes for knip (`knip-root = \"rush\"`) makes the root a workspace, while a Rush root has no `package.json`, so skif analyses no file there",
    },
    Deviation {
        id: USED_IN_KNIP_UNUSED_FILE,
        reason: "knip reports these dependencies as unused, and a file of the same workspace that knip lists as unused and skif does not imports the package (an import scan of those files with skif's parser; an `@types/` package counts as imported with its runtime package). knip does not analyse an unused file, so it never sees the import skif credits; the disagreement is the file, which `files` already counts once. A dependency the manifest lists is set apart too when the installed manifest of a package so imported names it in `peerDependencies`, as skif and knip credit the peers of a used package. The harness does not check that the scanned import is the one skif credits",
    },
    Deviation {
        id: USED_IN_SKIF_UNUSED_FILE,
        reason: "skif reports these dependencies as unused, and a file of the same workspace that skif reports as unused and knip does not imports the package (the same import scan; an `@types/` package counts as imported with its runtime package). skif credits no import of a file it calls unused, while knip analyses the file and credits the import; the disagreement is the file, which `files` already counts once. The harness does not check that the scanned import is the one knip credits",
    },
    Deviation {
        id: IMPORTED_BY_KNIP_UNUSED_FILE,
        reason: "knip reports these exports, types and duplicate exports as unused, and a file that knip lists as unused and skif does not imports them by relative path, by a specifier its own tsconfig `paths` map (the nearest `tsconfig.json` or `jsconfig.json` with its `extends` and `baseUrl`) or by the name of a workspace package (mapped to the source of what its manifest publishes): by name, through a namespace whose members it reads, or as a whole module (a namespace used as a whole, `import()`, `require`), directly or through the `export … from` and `export *` re-exports of the files in between (an import scan of those files with skif's parser). knip does not analyse an unused file, so it never sees the import skif credits; the disagreement is the importing file, which `files` already counts once. The same holds for these enum and namespace members knip reports unused that such a file reads (`BrevoNode.INTERCEPTORS`), through a named or default import or a namespace member, or uses as a whole. The in-file counterpart of `used-in-knip-unused-file`; the harness does not check that skif credits the export through that import",
    },
    Deviation {
        id: IMPORTED_BY_SKIF_UNUSED_FILE,
        reason: "skif reports these exports, types and duplicate exports as unused, and a file that skif reports as unused and knip does not imports them (the same import scan). skif credits no import of a file it calls unused, while knip analyses the file and credits the import; the disagreement is the importing file, which `files` already counts once. The in-file counterpart of `used-in-skif-unused-file`",
    },
    Deviation {
        id: RUSH_PROVIDED_COMMAND,
        reason: "knip reports `rush`, `rushx` and `rush-pnpm` as unlisted binaries of a script in a repository with a `rush.json`; Rush provides these commands to every script from its install, so skif never reports them there (ADR 0012)",
    },
    Deviation {
        id: SHELL_BUILTIN,
        reason: "knip reports these shell builtins (`read`, `break`, `ulimit`, ...) as unlisted binaries; a shell runs them itself, so skif never reports one (`SHELL_BUILTINS` of `skif-js-plugins`, ADR 0017)",
    },
    Deviation {
        id: GLOBAL_BINARY,
        reason: "knip reports these binaries as unlisted, and skif's list of system and platform tools that are installed on the machine names them (`IGNORED_BINARIES` and `MACHINE_BINARIES` of `skif-js-plugins`, read by the harness, ADR 0017); skif extends knip's list of global binaries and reports none of them while `ignore-global-binaries` is on, as it is by default",
    },
    Deviation {
        id: DATA_URI,
        reason: "knip reports a `data:` specifier (`import x from 'data:text/javascript,...'`) as an unlisted dependency, named as written or, cut at its first colon (`sanitizeSpecifier` in its `util/modules.js`), as `data` in a file that holds a quoted `data:` URI; it names an inline module, no package, and skif never takes it for one",
    },
    Deviation {
        id: AMBIENT_MODULE,
        reason: "knip reports these imports as unlisted dependencies, and a script file of the checkout declares the module with `declare module 'name' { … }` (the specifier, its package, or a pattern with one `*` that matches it, as skif matches them); skif counts an import of a declared module for no manifest (ADR 0019). The harness reads every code file of the checkout that holds `declare module` with skif's parser, while skif reads those of the program, so a declaration outside it counts here too",
    },
    Deviation {
        id: OWN_TSCONFIG_PATHS,
        reason: "knip reports these imports as unlisted dependencies, and the `paths` of the importing file's own TypeScript configuration (the nearest `tsconfig.json` or `jsconfig.json` at or above it, with its `extends` and `baseUrl`) map the specifier to a file of the checkout outside `node_modules`, or to a target outside `node_modules` that does not exist and that git ignores (with a code extension or as the `index` file of a directory, as `git check-ignore` reads the checkout's `.gitignore` files: vscode's `electron` mapped to `.build/typings/electron.d.ts`); skif resolves the import to that workspace source, or takes the ignored target for generated code, and counts it for no manifest (ADR 0019). The mirror of `foreign-tsconfig-paths`",
    },
    Deviation {
        id: PATHS_BEFORE_PACKAGES,
        reason: "skif reports these dependencies as unused, or knip reports these files as unused, where a file of the workspace imports the dependency's package by a specifier that the `paths` of the importer's TypeScript configuration map to a file of the same workspace, which exists: skif follows TypeScript, which resolves the import through `paths` before it looks for the package, so the import uses that file and not the dependency (ADR 0013); knip credits the dependency and does not use the file. A mapping into another workspace member or into `node_modules` is an import of that package and stays compared. The harness finds such imports in the files whose configuration sets `paths` and whose workspace manifest lists the package",
    },
    Deviation {
        id: WHOLE_ENUM_USE,
        reason: "knip reports these members of an enum as unused, and skif's parser finds the enum used as a whole value, which ADR 0018 counts as a use of every member: in its own file (`Object.keys(E)`, `E[key]`, `typeof E`, passed, stored or spread), or in any code file of the checkout through a named import that is used so (passed, put in an object literal, given to a decorator), a namespace import whose member `ns.E` is used so or that is used as a whole itself, `import()` or `require`, directly or through the `export … from` and `export *` re-exports of the files in between, resolved as the import scan of `imported-by-knip-unused-file` resolves them. knip reports the members anyway. The harness scans only the files that name the enum",
    },
    Deviation {
        id: PUBLISHED_FILE,
        reason: "knip reports these exports and types of a declaration file, and these unused files and exports of a JavaScript file (`.js`, `.mjs`, `.cjs`, `.jsx`), and the manifest of the file's project publishes the file as it is: a published field, `bin` or `exports` leaf names its path (`*` matching any part of it), or an entry of `files` names it or matches it with `*`. A published file is an entry of skif, whose exports are public; a directory listed in `files` is not taken to publish its files",
    },
    Deviation {
        id: UNINSTALLED_PROJECT,
        reason: "one tool reports these dependencies, binaries, unlisted dependencies and unresolved imports in a project that has no installed packages at or above it (no `node_modules` directory or `.pnp.cjs`) in a checkout the harness installed, such as a repository root the corpus install leaves out: without the packages neither tool sees their manifests and binaries, and each resolves the imports in its own way",
    },
    Deviation {
        id: TYPE_RESOLVED,
        reason: "skif with `--type-aware` (the run compared here) and skif with `--no-type-aware` disagree on these findings: skif reports them only with types, or knip reports them and only skif without types does too. Types settle which declaration a read reaches, so a typed read credits only the members TypeScript treats as the same (ADR 0034), while knip credits by name. Counted only where no other deviation explains the finding and only with the untyped pass (`--no-untyped-pass` turns it off); the per-rule counts of both modes are in `type-aware.json`",
    },
    Deviation {
        id: KNIP_DOT_IGNORE,
        reason: "skif reports these findings (anything but manifest findings) in a file that knip does not read because of a `./` pattern of a `.gitignore` below the checkout root (`./storage/` in `packages/core/.gitignore`): git matches no path with such a pattern, since no path has a `.` segment, while knip drops the `./` when it joins the pattern to the `.gitignore`'s directory and ignores every file and directory of that name at any depth below it. knip neither reports such a file unused nor analyses what it exports or imports. On knip's side, these dependencies knip reports unused that such a file of the same workspace imports (`lru-cache` of mastra's `packages/core/src/processors/memory`), and these exports and types knip reports unused that such a file imports, by the import scan of `imported-by-knip-unused-file`; a file skif reports unused credits nothing. The harness reads the `.gitignore` files outside the directories its file scan skips, and only patterns without wildcards",
    },
    Deviation {
        id: REEXPORT_ALIAS,
        reason: "skif reports these duplicate exports, in which every alias (`export const b = a`) names an export of the group that is itself initialised with an imported binding (`export const a = importedA`), and no name is `default`: knip takes such an export for a re-export of the import and records no alias of a re-export, so it reports no duplicate, while skif's `duplicate-exports` counts every name that holds the one value. The harness reads the aliases with skif's parser",
    },
    Deviation {
        id: TSCONFIG_DECLARATION_FILE,
        reason: "knip makes every declaration file (`.d.ts`, `.d.mts`, `.d.cts`) that the `tsconfig.json` of one of its workspaces compiles a production entry: the files its `files` lists and its `include` globs select (everything below the configuration's directory when it sets neither), less its `exclude` globs, `node_modules` and the `outDir`, as knip reads the configuration and those it `extends`. The exports of an entry are public and its imports are read. skif judges a module declaration file like any module, an entry only when something else makes it one (ADR 0013). Set apart: the exports, types and members skif reports in such a declaration file, and the exports and types skif reports that such a file re-exports through `export … from` and `export *`, followed through the files in between (next.js's `route-modules/*/module.compiled.d.ts`, `export * from './module'`), which are public for knip as re-exports of an entry; the unlisted dependencies and unresolved imports knip reports in one that no code file of the checkout imports (relative, through the importer's tsconfig `paths` or as a subpath of a workspace package, by its stem with or without a built extension), whose imports skif never reads; and the unused files only skif reports that such a declaration file reaches by relative imports, directly or through other files only skif reports unused, which knip reaches from the entry",
    },
    Deviation {
        id: MODULE_OBJECT_TYPES,
        reason: "skif reports these types, and an import uses their file as a whole module object (`import()` or `require` of the module, or a namespace import used as a whole, directly or through the `export … from` and `export *` re-exports of the files in between, as skif's parser reads it in any code file of the checkout): knip records such a use as opaque and credits every export of the module, types included, while skif's whole use of a module object counts its value exports only (ADR 0018). Only the files that hold `import(`, `require(` or `* as ` are parsed",
    },
    Deviation {
        id: HEFT_CONFIG,
        reason: "knip reports these dependencies as unused, and skif's `heft` plugin credits them (ADR 0015): a Heft config of the project (`config/heft.json`, `config/rig.json`, `config/jest.config.json`, ...) or of a rig's profile (`profiles/<profile>/` of the rig itself or of the installed rig `config/rig.json` names) names them, or they are peers of a package so named. knip has no Heft plugin. The harness runs skif's plugin resolver for each project that owns such a finding and takes what the `heft` plugin references, and what any plugin references from a file of a rig profile (the `tslib` an `importHelpers` of a profile's `tsconfig-base.json` needs). An unresolved import of a package only knip reports in a file of a rig profile (`profiles/<profile>/…` below its project) is `heft-config` too: knip resolves it from the rig, while skif charges what a profile names to the projects that use the rig",
    },
    Deviation {
        id: TYPE_ASSERTED_REQUIRE,
        reason: "knip reports these exports and namespace exports as unused, and a code file of the checkout loads their file with a `require` whose result a TypeScript `as` asserts a type of (`require('./x') as typeof import('./x')`, the specifier quoted or a template literal without a substitution), directly or through the `export … from` and `export *` re-exports of the files in between: knip does not follow a `require` wrapped in `as`, skif reads the names it takes. The harness finds such a `require` in the source text and resolves its specifier as the import scan of `imported-by-knip-unused-file` does; it does not check which names skif credits through it",
    },
    Deviation {
        id: TEMPLATE_LITERAL_IMPORT,
        reason: "knip reports these files as unused, and a `require` or `import()` of a template literal with a relative start (`require(`./commands/${c}`)` in npm's `lib/npm.js`) matches them, or such a file reaches them by relative imports through other files only knip reports unused: skif reads the template literal as a glob, each substitution a `*` within one path segment (`./commands/*`), knip resolves no computed specifier. The harness takes the globs from skif's parser and matches them as skif's graph does, an extensionless last segment with any code extension",
    },
    Deviation {
        id: COMMONJS_DEFAULT_IMPORT,
        reason: "knip reports these exports and namespace exports as unused, and a code file of the checkout default-imports their CommonJS file (`module.exports`, no `default` export) and reads them as properties of the binding (`import routing from './ci-routing.cjs'; const { a } = routing;`): skif binds the default import of a CommonJS module to its `module.exports` object and reads its properties like the members of a namespace, knip takes the import for a use of `default` only. The harness reads the imports and member reads with skif's parser",
    },
    Deviation {
        id: INSTALLED_MAIN,
        reason: "knip reports these files as unused, and the installed file that the `main` of their project's manifest names below `node_modules` (`node_modules/expo/AppEntry.js` of an Expo app) imports them by relative path, or such a file reaches them by relative imports through other files only knip reports unused: skif makes the `main` an entry and follows its relative imports back into the project, knip reads nothing in `node_modules`",
    },
    Deviation {
        id: TYPE_REFERENCED_MEMBERS,
        reason: "skif reports these enum and namespace members as unused, and a code file of the checkout imports their enum, or their namespace with exported members (an interface or type merged with a namespace), by name from the member's file without an alias and refers to it as a type (`x: E`, `typeof E`): knip skips the member findings of an export that an importer refers to so while `nsTypes` is not reported (`getMemberIssues` in its `graph/analyze.js`), a type reference names the enum type and reads no member for skif. The harness reads the imports with skif's parser and the type references with oxc",
    },
    Deviation {
        id: MODULE_OBJECT_VALUES,
        reason: "knip reports these exports and namespace exports as unused, and an import in a code file of the checkout uses their file as a whole module object, as skif's parser reads it (a namespace spread or put in an object literal, passed to a call such as `vi.mocked(ns)`, a `require` a callback returns, or a named import of a namespace re-export used so, `methods = { listSearch }` after `import { listSearch } from './methods'` where `methods/index.ts` has `export * as listSearch from './listSearch'`), directly or through the `export … from` and `export *` re-exports of the files in between: skif credits every value export of a module used as a whole (ADR 0018), knip does not take these uses for one. Only the files that hold `import(`, `require(` or `* as ` are parsed",
    },
    Deviation {
        id: KNIP_NODE_MODULES_NAME,
        reason: "skif reports these findings (anything but manifest findings) in a file whose path holds `node_modules` inside a name and below no `node_modules` directory (`node_modules_in_sandbox.test.mjs`): knip skips every path that contains the text (`isInNodeModules` in its `util/path.js`), so it neither reports such a file unused nor analyses it",
    },
    Deviation {
        id: RAW_GLOB_READ,
        reason: "skif reports these files as unused, and an `import.meta.glob` that reads the matched files as text (`query: '?raw'`) matches them, or such a file reaches them by relative imports through other files only skif reports unused: knip counts the glob as a use of its files, skif reads their text and keeps none of them in use. The harness matches the globs from skif's parser as skif's graph does",
    },
    Deviation {
        id: TYPESCRIPT_TWIN,
        reason: "skif reports these JavaScript files (`.js`, `.jsx`, `.mjs`, `.cjs`) as unused, a TypeScript file of the same stem lies beside each, and a code file of the checkout names it by a relative specifier with its extension (`require('../sum.js')` beside `sum.ts`): skif resolves the specifier in TypeScript's order, to the TypeScript twin, knip to the file it names",
    },
    Deviation {
        id: OPAQUE_DYNAMIC_IMPORT,
        reason: "skif reports these exports and types (also as namespace exports and types), and a code file of the checkout loads their file with an `import()` of a literal specifier that knip reads as opaque, directly or through the `export … from` and `export *` re-exports of the files in between: knip binds an `import()` to names only as the initialiser of a declarator (`const { a } = await import('./x')`, or one name when awaited), the first parameter of a `.then` callback, an element of an awaited `Promise.all([...])` given to an array pattern, the body of an arrow callback argument (a loader) or the module of a `vi.mock(import('./x'), () => ({ … }))` its Vitest plugin reads, and credits every export of the module, types included, for any other (`(await import('./x')).a`, `(await import('./x')) as any`, `vi.mock(import('./x'), async (original) => …)`, `handleImportExpression` in its `typescript/visitors/imports.js`). skif reads the names taken from the module. The harness reads the `import()` expressions with oxc, as knip's parser keeps parentheses, and resolves their specifiers as the import scan of `imported-by-knip-unused-file` does",
    },
    Deviation {
        id: DYNAMIC_IMPORT_DEFAULT,
        reason: "skif reports these `default` exports, and a code file of the checkout binds an `import()` of their file to one name (`const m = await import('./x')`, `import('./x').then(m => m.register())`, an element of an awaited `Promise.all([...])` given to a name), directly or through re-exports: knip records such a binding as an import of `default` besides the members read through the name (`handleDynamicImportBinding` in its `typescript/visitors/imports.js`), so it never reports that file's `default` unused; skif reads the members only. The harness reads the bindings with oxc",
    },
    Deviation {
        id: JEST_REQUIRE_ACTUAL,
        reason: "skif reports these exports and types (also as namespace exports and types), and a code file of the checkout loads their file with `jest.requireActual` (the global `jest`, or `jest` imported from `@jest/globals`) of a literal specifier, directly or through re-exports: knip's Jest plugin records `requireActual` as an opaque import that credits every export, types included (`plugins/jest/visitors/mock.js`), skif reads the names taken from the module. The harness reads the calls with oxc",
    },
    Deviation {
        id: SPIED_NAMESPACE,
        reason: "skif reports these exports and types (also as namespace exports and types), and spy calls (`vi.spyOn(ns, 'a')`, `jest.spyOn`, Jasmine's `spyOn`) are the only reads of a namespace import of their file, with no namespace import of the file read through a member otherwise, directly or through re-exports: for skif a spy call reads the member it names, for knip it references the namespace as a whole, and knip skips the exports and types of a module whose namespace imports are all referenced as a whole while `nsExports` and `nsTypes` are not reported (`hasStrictlyNsReferences` in its `graph/analyze.js`). The harness reads the namespace imports and the members read through them with skif's parser and the spy calls and member expressions with oxc",
    },
    Deviation {
        id: ENUMERATED_NAMESPACE,
        reason: "skif reports these exports and types (also as namespace exports and types), and a code file of the checkout that imports their file as a namespace (`import * as path`) holds an identifier of that name in an array literal or passes it to `Object.keys` (`values`, `entries`, `getOwnPropertyNames`), directly or through re-exports: knip's `ArrayExpression` visitor and its `Object.keys` branch mark the namespace import opaque, which credits every export of the module, types included, without checking whether a parameter or variable of that name shadows the import (vscode's `['ls-tree', path]` with a parameter `path` beside `import * as path`); skif resolves the name in its scope, and a whole use of the namespace credits its values only (ADR 0018). The harness reads the identifiers with oxc and the namespace imports with skif's parser",
    },
    Deviation {
        id: NAMESPACE_REEXPORT,
        reason: "skif reports these exports and namespace exports, and their file exports the name as a namespace re-export (`export * as Data from './data.js'`, of the file itself or of another): knip records an `export … from` as a re-export of the source module and never lists it among the file's own exports (`staticExports` with a `moduleRequest` in its `typescript/get-imports-and-exports.js`), so it never reports one unused; skif judges the name like any export. The harness reads the re-exports with skif's parser",
    },
    Deviation {
        id: TYPESCRIPT_COMMONJS_EXPORT,
        reason: "skif reports these exports, and a `CommonJS` assignment of their file, which is no JavaScript file (`.ts`, `.tsx`, `.mts`, `.cts`), exports the name (`exports.x = …`, `module.exports.x = …`, `module.exports = { a, b }`): knip records `CommonJS` exports in `.js`, `.mjs`, `.cjs` and `.jsx` files only (`isJS` in its `typescript/visitors/walk.js`), skif's parser in every code file. The harness reads the assignments with oxc",
    },
    Deviation {
        id: NODE_TEST_ENTRY,
        reason: "skif reports these exports and types (also as namespace exports and types) in a file that matches a pattern of knip's `node` plugin below its workspace (`**/*{.,-,_}test.<ext>`, `**/test-*.<ext>`, `**/test.<ext>`, `**/test/**/*.<ext>`, each extension one of `cjs`, `mjs`, `js`, `cts`, `mts` and `ts`), where a script of the root's manifest or of the workspace's own runs `node --test` (also `nub --test`): knip's `node` plugin makes those files entries in every workspace then (`hasNodeTest` in its `plugins/node/index.js`), and knip judges no export of an entry; skif makes only its own test-runner files entries. The harness reads the scripts as knip's plugin splits their commands",
    },
    Deviation {
        id: NG_PACKAGE_ENTRY,
        reason: "knip reports these exports and types (also as namespace exports and types), and their file is an entry point of an `ng_package` call of a `BUILD.bazel` or `BUILD` file (the `index.ts` of each Bazel package its `deps` name inside the project that owns the `BUILD` file, angular's `@angular/core/primitives/signals`), or such an entry point re-exports them through `export … from` and `export *`, followed through the files in between: skif's Bazel plugin makes each such `index.ts` a production entry, whose exports are public, and knip has no Bazel plugin. The manifest does not publish these secondary entry points (Bazel's `ng_package` writes their `exports` at build time), so `published-source` does not take them. The harness reads the `BUILD` files as skif's plugin does, only in a Bazel repository that uses `rules_js`",
    },
    Deviation {
        id: IMPORT_TYPE_QUALIFIER,
        reason: "knip reports these exports and types (also as namespace exports and types), and a code file of the checkout names them by a qualified import type (`type ILoadOptions = import('./window.js').ILoadOptions`, also `import('./x').A.B` for `A`), directly or through re-exports: knip's `TSImportType` visitor records the module as a type-only import of no name (its `typescript/visitors/walk.js`), skif's parser reads the export the qualifier names. The harness reads the import types with oxc and resolves their specifiers as the import scan of `imported-by-knip-unused-file` does",
    },
    Deviation {
        id: MODULE_AUGMENTATION,
        reason: "knip reports these unlisted dependencies, and their file is a module that augments the package with `declare module 'name' { … }`: knip records an augmentation in a module file as a type-only import of the module (`TSModuleDeclaration` in its `typescript/visitors/walk.js`), skif's parser as an augmentation, which needs no manifest. The harness reads the augmentations with skif's parser",
    },
    Deviation {
        id: JEST_MOCK_SPECIFIER,
        reason: "knip reports these unlisted dependencies, and their file names the package only in a Jest mock call (`jest.mock`, `doMock`, `unmock`, `dontMock`, `requireActual`, `requireMock` or `createMockFromModule`): knip's Jest plugin records each such call as an import of the module (`plugins/jest/visitors/mock.js`), skif's parser as a mock, which uses no package. The harness reads the calls with skif's parser",
    },
    Deviation {
        id: NEXTRA_ALIAS,
        reason: "knip reports these unlisted dependencies and unresolved imports in a Next.js configuration (`next.config.*`) that imports `nextra` and does not hold the specifier: knip executes the configuration and reads the targets of its `turbopack.resolveAlias` as specifiers (`plugins/next/index.js`), and nextra's wrapper adds aliases of its own (`./content/*`, `@vercel/turbopack-next`); skif reads the configuration's text",
    },
    Deviation {
        id: EXPO_UPDATES,
        reason: "knip reports `expo-updates` as an unlisted dependency of an Expo app configuration (`app.json`, `app.config.*`): its Expo plugin names the package for every app whose `updates.enabled` is not `false` (`plugins/expo/helpers.js`), skif's Expo plugin does not name it",
    },
    Deviation {
        id: OPTIONAL_PEER_HOST,
        reason: "skif reports these unlisted dependencies, and the manifest of the importing file's workspace, or of a workspace enclosing it, lists a package whose installed `package.json` (the nearest `node_modules/<name>` at or above that workspace) names the dependency in `peerDependencies` and marks it optional in `peerDependenciesMeta`: knip takes a peer of a listed host for present, optional or not (`DependencyDeputy.js`, which ignores `isPeerOptional`), skif does not take an optional peer for provided (n8n's `@vitest/coverage-v8`)",
    },
    Deviation {
        id: SHARED_CONFIG_REFERENCE,
        reason: "knip reports these unresolved imports of a package in a file that does not hold the specifier, and the manifest of a package the file imports (a workspace package by its `name`, else the installed `node_modules/<name>`) lists it: knip executes a tool configuration and resolves the names it gets from a shared configuration package (`...baseConfig` of n8n's `@n8n/stylelint-config/base`, with its `plugins` and `customSyntax`) from the consumer, while the shared package lists and resolves them itself; skif reads the consumer's text",
    },
    Deviation {
        id: SHELL_TEMPLATE_DIRECTORY,
        reason: "knip reports these unresolved relative imports in a file whose Bun shell template (`` $`bun ./script/x.ts` ``) names the specifier, which does not resolve from the file's directory and resolves from the directory of its project: knip resolves a command's file argument from the directory of the file that holds the command, the command runs in the working directory of the process, the project's directory, where skif resolves it",
    },
    Deviation {
        id: NESTED_VITEST_SETUP,
        reason: "knip reports these unresolved imports in a Vitest or Vite configuration (`vitest.config.*`, `vite.config.*`) whose specifier does not resolve from the configuration's directory, and a configuration of the same names below that directory holds the specifier quoted and resolves it from its own directory: knip executes the root configuration, which loads the nested projects, and resolves their `setupFiles` and `globalSetup` against the root configuration instead of each project's `root` (mastra's `src/vitest-setup.ts`)",
    },
    Deviation {
        id: GITIGNORED_IMPORT,
        reason: "knip reports these unresolved relative imports, and the file they name does not exist and git ignores it (the path, with a code extension or as the `index` file of a directory, as `git check-ignore` reads the checkout's `.gitignore` files): skif takes an import of ignored code that does not exist for generated code, which a build writes (ADR 0019, cal-com's `./types` of `packages/kysely`)",
    },
    Deviation {
        id: COMMAND_DIRECTORY,
        reason: "knip reports these unlisted binaries of a command that runs in another project than the workspace knip judges it in: a `cd <dir>` before it, a `pnpm -C <dir>`, `--dir <dir>` or `--dir=<dir>` of its command, the `working-directory` of its GitHub workflow step, or the `defaults.run.working-directory` of its job or workflow, each relative to the one before (the workflow at the checkout root, a script at its manifest's directory), name a directory whose project is not that workspace, or a directory the command computes (`${{ … }}`, `${dir}`). knip judges the command where its file starts it (a workflow at the checkout root, a script at its manifest's directory), and a step's `working-directory` in the workspace that holds it; skif judges a binary in the project the command runs in and none in a computed directory (ADR 0017). The harness reads the commands and the workflow's indentation from the text",
    },
    Deviation {
        id: PASSTHROUGH_COMMAND,
        reason: "knip reports these commands as unlisted binaries, and skif reads each as a command that runs the command in its arguments (`watch`, `timeout`, `nohup`, `xargs`, ..., `PASSTHROUGH` of skif's script tables): skif judges the command it runs and not the command itself",
    },
    Deviation {
        id: BUN_FILE_ARGUMENT,
        reason: "knip reports these names with a code extension (`imports.mjs`) as unlisted binaries, and a command of their file runs them with `bun`: `bun <file>` runs a script, which knip takes for a binary when it does not resolve the file from the directory it assumes, skif reads it as a file",
    },
    Deviation {
        id: REQUIRED_CHILD_PROCESS,
        reason: "skif reports these unlisted binaries in a code file that loads `child_process` (or `node:child_process`) with `require` and imports it with no `import` declaration: knip reads the commands of `child_process` calls only in a file that imports the module through an `import` declaration (`staticImports` in its `typescript/get-imports-and-exports.js`), skif in every file that loads it (cal-com's `sentry-cli`, vscode's `sqlite3`)",
    },
    Deviation {
        id: REPLACED_KNIP_CONFIG,
        reason: "knip reports the repository's own knip configuration (`knip.ts`, `knip.config.js`, ... at the root of a workspace) as an unused file; the harness replaces it with an empty configuration through `--config`, so knip does not load it, while skif's knip plugin makes it an entry",
    },
];

/// Extensions skif parses and follows; every other file is outside its project set.
const CODE_EXTENSIONS: [&str; 11] = [
    "js", "mjs", "cjs", "jsx", "ts", "mts", "cts", "tsx", "vue", "svelte", "astro",
];

const TEST_DIRECTORIES: [&str; 6] = ["test", "tests", "__tests__", "e2e", "cypress", "__mocks__"];
/// The directories skif's JS analysis treats as fixtures (`skif_config::FIXTURE_DIRECTORIES`, ADR 0014).
const FIXTURE_DIRECTORIES: [&str; 2] = ["fixtures", "__fixtures__"];

/// The default entry files of a workspace, the same in knip and skif. A configured `entry` replaces them in both, so a
/// corpus-level `entry` is given with these in front.
const DEFAULT_ENTRY: [&str; 2] = [
    "{index,cli,main}.{js,mjs,cjs,jsx,ts,tsx,mts,cts}",
    "src/{index,cli,main}.{js,mjs,cjs,jsx,ts,tsx,mts,cts}",
];

/// Marker written into a checkout once its dependencies are installed, holding the pinned commit.
const INSTALLED_MARKER: &str = ".skif-bench-installed";

// Independent command-line switches, one per flag.
#[allow(clippy::struct_excessive_bools)]
pub struct Options {
    pub skif: PathBuf,
    pub corpus_file: PathBuf,
    pub corpus_dir: PathBuf,
    pub out: PathBuf,
    pub only: Vec<String>,
    pub install: bool,
    pub recursive: bool,
    /// Run knip with the repository's own configuration instead of an empty one.
    pub repo_knip_config: bool,
    /// Count test and fixture files in the knip-only `files` instead of reporting them as known deviations.
    pub include_test_files: bool,
    pub timeout: Duration,
    pub gate: Gate,
    /// Run skif a second time with `--no-type-aware` on each repository, for the `type-resolved` deviation and the
    /// type-aware baseline.
    pub untyped_pass: bool,
    /// The baseline of the typed and untyped counts per rule; recorded and checked with [`Options::gate`]'s flags.
    pub type_aware_baseline: PathBuf,
    /// Sample findings the full report keeps per category, known deviation and side (`usize::MAX` for all).
    pub samples: usize,
}

/// Makes a path relative to the checkout with `/` separators.
pub fn normalize(raw: &str, root: &Path) -> String {
    let unified = raw.replace('\\', "/");
    let prefix = format!("{}/", root.to_string_lossy().replace('\\', "/").trim_end_matches('/'));
    let relative = unified.strip_prefix(&prefix).unwrap_or(&unified);
    relative.trim_start_matches("./").to_owned()
}

/// The backtick-quoted names in a diagnostic message.
fn backticked(message: &str) -> Vec<&str> {
    message.split('`').skip(1).step_by(2).collect()
}

/// The name of an enum or namespace member without its `Enum.` or `ns.` qualifier: skif names `Enum.Member`, knip
/// names `Member`.
fn member_name(category: &str, name: &str) -> String {
    if matches!(category, "enumMembers" | "namespaceMembers") {
        name.rsplit('.').next().unwrap_or(name).to_owned()
    } else {
        name.to_owned()
    }
}

fn joined_sorted(mut names: Vec<String>) -> String {
    names.sort();
    names.join("|")
}

/// The findings of a skif JSON document, by knip issue type.
pub fn skif_keys(document: &Value, root: &Path) -> Keys {
    let mut keys = Keys::new();
    let Some(diagnostics) = document.get("diagnostics").and_then(Value::as_array) else {
        return keys;
    };
    for diagnostic in diagnostics {
        let Some(rule) = diagnostic.get("rule").and_then(Value::as_str) else {
            continue;
        };
        let Some((_, category)) = RULE_CATEGORIES.iter().find(|(id, _)| *id == rule) else {
            continue;
        };
        let key = skif_key(diagnostic, category, root);
        keys.entry((*category).to_owned()).or_default().insert(key);
    }
    keys
}

/// The key of a skif diagnostic in `category`: `<file>` for files, `<file>|<names>` for duplicates and `<file>|<name>`
/// otherwise.
fn skif_key(diagnostic: &Value, category: &str, root: &Path) -> String {
    let path = normalize(
        diagnostic
            .pointer("/location/path")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        root,
    );
    let message = diagnostic.get("message").and_then(Value::as_str).unwrap_or_default();
    let names = backticked(message);
    match category {
        "files" => path,
        "duplicates" => format!(
            "{path}|{}",
            joined_sorted(names.iter().skip(1).map(|n| (*n).to_owned()).collect())
        ),
        _ => {
            let name = diagnostic
                .get("dependency")
                .and_then(Value::as_str)
                .or_else(|| names.first().copied())
                .unwrap_or_default();
            format!("{path}|{}", member_name(category, name))
        }
    }
}

/// The keys of the `unlisted-dependencies` findings with the detail `ForeignTsconfigPaths`: imports that only the
/// tsconfig `paths` of another package map, which knip resolves through the merged `paths` of all workspaces.
pub fn foreign_tsconfig_paths_keys(document: &Value, root: &Path) -> BTreeSet<String> {
    document
        .get("diagnostics")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|diagnostic| {
            diagnostic.get("rule").and_then(Value::as_str) == Some("unlisted-dependencies")
                && diagnostic.get("detail").and_then(Value::as_str) == Some(FOREIGN_TSCONFIG_PATHS_DETAIL)
        })
        .map(|diagnostic| skif_key(diagnostic, "unlisted", root))
        .collect()
}

fn name_of(value: &Value) -> Option<String> {
    value
        .get("name")
        .or_else(|| value.get("symbol"))
        .and_then(Value::as_str)
        .or_else(|| value.as_str())
        .map(str::to_owned)
}

/// The names of one item of a knip issue type: a group of items (duplicates) is one joined name.
fn item_names(item: &Value) -> Vec<String> {
    match item {
        Value::Array(group) => vec![joined_sorted(group.iter().filter_map(name_of).collect())],
        other => name_of(other).into_iter().collect(),
    }
}

/// The names under one knip issue type: a list of items, a list of lists (duplicates) or a map of lists (enums).
fn knip_names(value: &Value) -> Vec<String> {
    match value {
        Value::Array(items) => items.iter().flat_map(item_names).collect(),
        Value::Object(map) => map.values().flat_map(knip_names).collect(),
        _ => Vec::new(),
    }
}

/// The category a knip item of `exports` or `types` that carries a `namespace` belongs to: knip names the namespace
/// import that reads the module (`parentSymbol`) on the findings of a module that is only read through one.
fn namespaced_category(type_name: &str, item: &Value) -> Option<&'static str> {
    let namespaced = item.get("namespace").is_some_and(Value::is_string);
    match type_name {
        "exports" if namespaced => Some("nsExports"),
        "types" if namespaced => Some("nsTypes"),
        _ => None,
    }
}

/// The findings of a knip JSON report, by issue type.
pub fn knip_keys(document: &Value, root: &Path) -> Keys {
    let mut keys = Keys::new();
    if let Some(files) = document.get("files").and_then(Value::as_array) {
        keys.entry("files".to_owned())
            .or_default()
            .extend(files.iter().filter_map(Value::as_str).map(|f| normalize(f, root)));
    }
    let Some(issues) = document.get("issues").and_then(Value::as_array) else {
        return keys;
    };
    for issue in issues {
        let file = normalize(issue.get("file").and_then(Value::as_str).unwrap_or_default(), root);
        for (type_name, category) in KNIP_CATEGORIES {
            let Some(value) = issue.get(type_name) else {
                continue;
            };
            let routed: Vec<(&str, String)> = match value {
                Value::Array(items) => items
                    .iter()
                    .flat_map(|item| {
                        let target = namespaced_category(type_name, item).unwrap_or(category);
                        item_names(item).into_iter().map(move |name| (target, name))
                    })
                    .collect(),
                other => knip_names(other).into_iter().map(|name| (category, name)).collect(),
            };
            for (target, name) in routed {
                let set = keys.entry(target.to_owned()).or_default();
                if target == "files" {
                    set.insert(file.clone());
                } else {
                    set.insert(format!("{file}|{}", member_name(target, &name)));
                }
            }
        }
    }
    keys
}

/// Moves each export finding of `skif` that knip has under another label to that label, so the same `<file>|<export>`
/// counts as agreement.
fn align_export_labels(skif: &mut Keys, knip: &Keys) {
    for category in EXPORT_LABELS {
        let Some(keys) = skif.get(category) else { continue };
        let moved: Vec<(String, &str)> = keys
            .iter()
            .filter(|key| !knip.get(category).is_some_and(|own| own.contains(*key)))
            .filter_map(|key| {
                EXPORT_LABELS
                    .iter()
                    .find(|other| knip.get(**other).is_some_and(|theirs| theirs.contains(key)))
                    .map(|other| (key.clone(), *other))
            })
            .collect();
        for (key, label) in moved {
            if let Some(keys) = skif.get_mut(category) {
                keys.remove(&key);
            }
            skif.entry(label.to_owned()).or_default().insert(key);
        }
    }
}

/// Moves each finding only skif reports in `unlisted` or `unresolved` that knip reports in the other of the two, on the
/// same file and package, to knip's category and key, so it counts as agreement: both tools report the import (a
/// tsconfig `types` entry, `importHelpers`), skif as a package no manifest lists and knip as a module it cannot
/// resolve, or the reverse. A specifier with a subpath (`vite/client`) pairs with its package (`vite`); each knip
/// finding pairs with one skif finding at most.
fn align_unlisted_unresolved(skif: &mut Keys, knip: &Keys) {
    let empty = BTreeSet::new();
    let mut paired: BTreeSet<(&str, String)> = BTreeSet::new();
    let mut moves: Vec<(&str, String, &str, String)> = Vec::new();
    for (own, other) in [("unlisted", "unresolved"), ("unresolved", "unlisted")] {
        let knip_own = knip.get(own).unwrap_or(&empty);
        let knip_other = knip.get(other).unwrap_or(&empty);
        let skif_other = skif.get(other).unwrap_or(&empty);
        for key in skif.get(own).unwrap_or(&empty).difference(knip_own) {
            let Some((path, name)) = key.split_once('|') else {
                continue;
            };
            let Some(package) = bare_package(name) else {
                continue;
            };
            let found = knip_other.iter().find(|candidate| {
                !skif_other.contains(*candidate)
                    && !paired.contains(&(other, (*candidate).clone()))
                    && candidate
                        .split_once('|')
                        .is_some_and(|(file, theirs)| file == path && bare_package(theirs) == Some(package))
            });
            if let Some(found) = found {
                paired.insert((other, found.clone()));
                moves.push((own, key.clone(), other, found.clone()));
            }
        }
    }
    for (own, key, other, found) in moves {
        if let Some(keys) = skif.get_mut(own) {
            keys.remove(&key);
        }
        skif.entry(other.to_owned()).or_default().insert(found);
    }
}

/// The `<file>|<specifier>` keys of skif's `stale-mocks` findings: the module a test-runner mock (`jest.requireActual`,
/// `vi.mock`) names that does not exist, which skif's message quotes in the call (`` `jest.mock('./x')` ``).
pub fn stale_mock_keys(document: &Value, root: &Path) -> BTreeSet<String> {
    document
        .get("diagnostics")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|diagnostic| diagnostic.get("rule").and_then(Value::as_str) == Some("stale-mocks"))
        .filter_map(|diagnostic| {
            let path = normalize(
                diagnostic
                    .pointer("/location/path")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                root,
            );
            let message = diagnostic.get("message").and_then(Value::as_str)?;
            let call = backticked(message).into_iter().next()?;
            mock_specifier(call).map(|specifier| format!("{path}|{specifier}"))
        })
        .collect()
}

/// The specifier a mock call quotes: `./x` of `jest.mock('./x')`.
fn mock_specifier(call: &str) -> Option<&str> {
    call.split_once("('")?.1.strip_suffix("')")
}

/// Adds to skif's `unresolved` each finding only knip reports there that skif reports as `stale-mocks` on the same
/// file and specifier ([`stale_mock_keys`]), so it counts as agreement: skif reports a test-runner mock of a module
/// that does not exist under its own rule (ADR 0019), and knip reports the specifier as unresolved.
fn align_stale_mocks(skif: &mut Keys, knip: &Keys, stale: &BTreeSet<String>) {
    let Some(theirs) = knip.get("unresolved") else {
        return;
    };
    let paired: Vec<String> = theirs
        .iter()
        .filter(|key| stale.contains(*key) && !skif.get("unresolved").is_some_and(|own| own.contains(*key)))
        .cloned()
        .collect();
    if !paired.is_empty() {
        skif.entry("unresolved".to_owned()).or_default().extend(paired);
    }
}

/// What skif left unjudged in a package it lists under `incomplete-packages`, from the entry's `scope`.
#[derive(Debug, Clone)]
pub enum NotJudgedScope {
    /// `"package"` (or no `scope`): no file of the package is judged.
    Package,
    /// `{"within": [globs]}`: only the files these globs match, relative to the package directory, are not judged.
    /// The globs are compiled once, as skif's analysis compiles them.
    Within(IncompleteScopeGlobs),
}

impl PartialEq for NotJudgedScope {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Package, Self::Package) => true,
            (Self::Within(ours), Self::Within(theirs)) => ours.patterns() == theirs.patterns(),
            _ => false,
        }
    }
}

impl Eq for NotJudgedScope {}

impl NotJudgedScope {
    /// A scope narrowed to `globs`.
    fn within(globs: Vec<String>) -> Self {
        Self::Within(IncompleteScopeGlobs::new(globs))
    }

    fn read(value: Option<&Value>) -> Self {
        let globs = value.and_then(|scope| scope.get("within")).and_then(Value::as_array);
        globs.map_or(Self::Package, |globs| {
            Self::within(globs.iter().filter_map(Value::as_str).map(str::to_owned).collect())
        })
    }

    /// The scope covering both: the whole package when either is (as `skif_core::IncompleteScope::union`).
    fn union(self, other: Self) -> Self {
        match (self, other) {
            (Self::Within(ours), Self::Within(more)) => {
                let mut globs = ours.patterns().to_vec();
                for glob in more.patterns() {
                    if !globs.contains(glob) {
                        globs.push(glob.clone());
                    }
                }
                Self::within(globs)
            }
            _ => Self::Package,
        }
    }
}

/// The packages skif did not judge (its JSON `incomplete-packages`) with what it left unjudged in each: by directory,
/// relative to the checkout with `/` separators, the root package as the empty string. A directory listed twice (by
/// two roots of a recursive run) gets the union of its scopes.
pub fn not_judged_dirs(document: &Value) -> BTreeMap<String, NotJudgedScope> {
    let mut packages: BTreeMap<String, NotJudgedScope> = BTreeMap::new();
    for package in document
        .get("incomplete-packages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(dir) = package.get("dir").and_then(Value::as_str) else {
            continue;
        };
        let dir = if dir == "." {
            String::new()
        } else {
            dir.trim_end_matches('/').to_owned()
        };
        let scope = NotJudgedScope::read(package.get("scope"));
        let merged = match packages.remove(&dir) {
            Some(known) => known.union(scope),
            None => scope,
        };
        packages.insert(dir, merged);
    }
    packages
}

/// The package directories of every root of a recursive skif run (the `packages` of its JSON `roots`, ADR 0006),
/// relative to the checkout with `/` separators, the root package as the empty string. Empty for a single-workspace
/// run, whose report has no `roots`.
pub fn skif_root_packages(document: &Value) -> BTreeSet<String> {
    document
        .get("roots")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|root| root.get("packages").and_then(Value::as_array))
        .flatten()
        .filter_map(Value::as_str)
        .map(report_dir)
        .collect()
}

/// The directories of the roots of a recursive skif run (the `path` of each of its JSON `roots`), relative to the
/// checkout as [`skif_root_packages`] gives them. Empty for a single-workspace run.
fn skif_root_dirs(document: &Value) -> BTreeSet<String> {
    document
        .get("roots")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|root| root.get("path").and_then(Value::as_str))
        .map(report_dir)
        .collect()
}

/// A directory of skif's JSON report relative to the checkout with `/` separators, the checkout itself as the empty
/// string.
fn report_dir(dir: &str) -> String {
    match dir.replace('\\', "/").trim_end_matches('/') {
        "." => String::new(),
        dir => dir.trim_start_matches("./").to_owned(),
    }
}

/// The package that owns a finding key (`<path>` or `<path>|<name>`): the deepest directory above the path that
/// `is_package` accepts (for the classification, [`Layout::is_project`], so a marker-only manifest owns nothing), the
/// root (`""`) at the latest.
fn owner_dir(key: &str, is_package: &dyn Fn(&str) -> bool) -> String {
    let path = key.split_once('|').map_or(key, |(path, _)| path);
    let mut dir = path.rsplit_once('/').map_or("", |(dir, _)| dir);
    while !dir.is_empty() && !is_package(dir) {
        dir = dir.rsplit_once('/').map_or("", |(parent, _)| parent);
    }
    dir.to_owned()
}

/// Whether a one-sided finding is in a category skif does not judge where it lies: a dependency of a package skif
/// lists as incomplete, or a file, export, type or member in its files, all of them or only those its narrowed scope
/// matches. A narrowed scope also reaches the files of the packages nested below the incomplete package, matched
/// relative to the incomplete package, as skif makes every file it matches an entry (ADR 0016).
fn not_judged(
    category: &str,
    key: &str,
    packages: &BTreeMap<String, NotJudgedScope>,
    is_package: &dyn Fn(&str) -> bool,
) -> bool {
    let in_files = NOT_JUDGED_FILE_CATEGORIES.contains(&category);
    if packages.is_empty() || !(in_files || NOT_JUDGED_DEPENDENCY_CATEGORIES.contains(&category)) {
        return false;
    }
    let dir = owner_dir(key, is_package);
    let path = key_path(key);
    let own = packages.get(&dir).is_some_and(|scope| {
        !in_files
            || match scope {
                NotJudgedScope::Package => true,
                NotJudgedScope::Within(globs) => globs.is_match(below(&dir, path)),
            }
    });
    own || (in_files && in_enclosing_scope(&dir, path, packages))
}

/// `path` relative to the directory `dir` at or above it (`""` is the root, and `dir` itself).
fn below<'p>(dir: &str, path: &'p str) -> &'p str {
    if dir.is_empty() {
        return path;
    }
    path.strip_prefix(dir)
        .and_then(|rest| {
            if rest.is_empty() {
                Some(rest)
            } else {
                rest.strip_prefix('/')
            }
        })
        .unwrap_or(path)
}

/// `name` joined to the directory `dir`, both relative to the checkout; either may be empty.
fn joined(dir: &str, name: &str) -> String {
    match (dir, name) {
        ("", name) => name.to_owned(),
        (dir, "") => dir.to_owned(),
        (dir, name) => format!("{dir}/{name}"),
    }
}

/// Whether the narrowed scope of an incomplete package above the package directory `dir` matches `path`, relative to
/// that package (`scope_entries` in `skif-js`).
fn in_enclosing_scope(dir: &str, path: &str, packages: &BTreeMap<String, NotJudgedScope>) -> bool {
    let mut above = dir;
    while !above.is_empty() {
        above = parent_dir(above);
        if let Some(NotJudgedScope::Within(globs)) = packages.get(above)
            && globs.is_match(below(above, path))
        {
            return true;
        }
    }
    false
}

/// The known deviation a finding only skif reports belongs to: an unlisted import that only another package's tsconfig
/// `paths` map (`foreign`, from [`foreign_tsconfig_paths_keys`]), which knip resolves through the merged `paths`.
fn skif_only_deviation(category: &str, key: &str, foreign: &BTreeSet<String>) -> Option<&'static str> {
    (category == "unlisted" && foreign.contains(key)).then_some(FOREIGN_TSCONFIG_PATHS)
}

/// Whether a knip-reported unused file is a known deviation: outside skif's project set (not code), a declaration file
/// skif never reports (`.d.ts`, as `unused-files` filters them), in a directory skif treats as fixtures, a test file or
/// any code file below a test directory (such as the apps of an end-to-end suite). Fixtures and tests only count when
/// `include_test_files` is off.
fn classify_file(path: &str, include_test_files: bool) -> Option<&'static str> {
    let normalized = path.replace('\\', "/");
    let (directories, name) = normalized.rsplit_once('/').unwrap_or(("", normalized.as_str()));
    let extension = name
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase());
    if !extension.is_some_and(|e| CODE_EXTENSIONS.contains(&e.as_str())) {
        return Some(NON_CODE_FILE);
    }
    if name.ends_with(".d.ts") {
        return Some(DECLARATION_FILE);
    }
    if include_test_files {
        return None;
    }
    if in_fixture_directory(&normalized) {
        return Some(FIXTURE_FILE);
    }
    let in_test_directory = directories.split('/').any(|p| TEST_DIRECTORIES.contains(&p));
    let test_name = [".test.", ".spec.", "_spec.", "_test."]
        .iter()
        .any(|marker| name.contains(marker));
    (in_test_directory || test_name).then_some(TEST_FILE)
}

/// The file of a finding key (`<path>` or `<path>|<name>`).
fn key_path(key: &str) -> &str {
    key.split_once('|').map_or(key, |(path, _)| path)
}

/// Whether a directory of `path` (not its file name) is one skif treats as fixtures.
fn in_fixture_directory(path: &str) -> bool {
    path.rsplit_once('/')
        .is_some_and(|(directories, _)| directories.split('/').any(|part| FIXTURE_DIRECTORIES.contains(&part)))
}

/// The package a bare specifier names: `@scope/name` or `name`, without a subpath.
fn package_name(specifier: &str) -> &str {
    let segments = if specifier.starts_with('@') { 2 } else { 1 };
    specifier
        .match_indices('/')
        .nth(segments - 1)
        .map_or(specifier, |(at, _)| &specifier[..at])
}

/// Directory names skif's file discovery never enters unless the directory is a workspace member (`SKIPPED_DIRS` in
/// `crates/skif-js/src/discover.rs`).
const SKIPPED_DIRECTORIES: [&str; 13] = [
    "node_modules",
    ".git",
    "dist",
    "coverage",
    ".next",
    "out",
    ".turbo",
    ".svelte-kit",
    "storybook-static",
    ".yarn",
    ".vercel",
    ".nuxt",
    "target",
];

/// The names of [`SKIPPED_DIRECTORIES`] that are skipped only directly inside a package directory
/// (`PACKAGE_ROOT_ONLY_DIRS` in `discover.rs`).
const PACKAGE_ROOT_ONLY_DIRECTORIES: [&str; 3] = ["out", "target", "coverage"];

/// The hidden directories skif's file discovery enters (`HIDDEN_DIRS` in `discover.rs`); it skips every other one.
const TOOL_DIRECTORIES: [&str; 8] = [
    ".storybook",
    ".rnstorybook",
    ".config",
    ".vitepress",
    ".vuepress",
    ".husky",
    ".github",
    ".changeset",
];

/// Manifest fields that only mark a directory: a `package.json` with nothing else is not a project
/// (`MARKER_FIELDS` in `discover.rs`).
const MARKER_FIELDS: [&str; 3] = ["type", "sideEffects", "imports"];

/// Manifest fields that publish one file each.
const PUBLISHED_FIELDS: [&str; 6] = ["main", "module", "source", "types", "typings", "browser"];

/// The extensions of a published (built) path and the TypeScript source extensions skif maps each one back to
/// (`EXTENSION_PAIRS` in `crates/skif-js/src/manifest_paths.rs`, the TypeScript sources only).
const PUBLISHED_EXTENSIONS: [(&str, &[&str]); 7] = [
    (".d.mts", &["mts"]),
    (".d.cts", &["cts"]),
    (".d.ts", &["ts", "tsx"]),
    (".mjs", &["mts", "ts", "tsx", "cts"]),
    (".cjs", &["cts", "ts", "tsx", "mts"]),
    (".jsx", &["tsx"]),
    (".js", &["ts", "tsx", "mts", "cts"]),
];

/// The TypeScript source extensions; an extensionless published path (`./out/main`) may be the twin of any of them.
const TYPESCRIPT_EXTENSIONS: &[&str] = &["ts", "tsx", "mts", "cts"];

/// The stem and extension of a TypeScript source file (`.ts`, `.tsx`, `.mts`, `.cts`, not a declaration file).
fn typescript_source(path: &str) -> Option<(&str, &str)> {
    if is_declaration(path) {
        return None;
    }
    let (stem, extension) = path.rsplit_once('.')?;
    TYPESCRIPT_EXTENSIONS.contains(&extension).then_some((stem, extension))
}

/// Whether `text` matches `pattern`, where each `*` matches one or more characters, `/` included, as in the
/// subpath patterns of `exports`.
fn wildcard_match(pattern: &str, text: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == text,
        Some((head, tail)) => text.strip_prefix(head).is_some_and(|rest| {
            rest.char_indices()
                .map(|(at, character)| at + character.len_utf8())
                .any(|end| wildcard_match(tail, &rest[end..]))
        }),
    }
}

/// The published path without its built extension and the source extensions it maps back to: those of
/// [`PUBLISHED_EXTENSIONS`], or every TypeScript one for a path whose file name has no extension (`./out/main`, which
/// Node resolves with `.js`). `None` for any other extension (`.css`, `.json`).
fn published_stem(clean: &str) -> Option<(&str, &[&str])> {
    PUBLISHED_EXTENSIONS
        .iter()
        .find_map(|(suffix, sources)| clean.strip_suffix(suffix).map(|published| (published, *sources)))
        .or_else(|| {
            let name = clean.rsplit('/').next().unwrap_or(clean);
            (!name.contains('.')).then_some((clean, TYPESCRIPT_EXTENSIONS))
        })
}

/// Whether the source file `stem.extension` (relative to its package) is a twin of the published path pattern
/// `target` (one with `*`): the same path, or for a path below a build directory (`is_out_dir` of `skif-js`: `out`,
/// `lib-commonjs`, ...) the same path below `src`. skif makes every file such a pattern matches an entry; a published
/// path without `*` names one source only ([`Layout::source_of`]).
fn is_published_twin(target: &str, stem: &str, extension: &str) -> bool {
    let clean = target.trim_start_matches("./");
    let Some((published, sources)) = published_stem(clean) else {
        return false;
    };
    if !sources.contains(&extension) {
        return false;
    }
    let below_src = published
        .split_once('/')
        .filter(|(first, _)| is_out_dir(first))
        .map(|(_, rest)| format!("src/{rest}"));
    wildcard_match(published, stem) || below_src.is_some_and(|source| wildcard_match(&source, stem))
}

fn collect_strings(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(text) if !text.is_empty() => out.push(text.clone()),
        Value::Array(items) => items.iter().for_each(|item| collect_strings(item, out)),
        Value::Object(map) => map.values().for_each(|item| collect_strings(item, out)),
        _ => {}
    }
}

/// The file paths a manifest publishes: [`PUBLISHED_FIELDS`], every `bin`, every `exports` leaf but the catch-all `./*`
/// and the node and credential files of an n8n node package as skif reads them (`skif_js::n8n_load_paths`).
fn published_paths(manifest: &Value) -> Vec<String> {
    let mut paths: Vec<String> = PUBLISHED_FIELDS
        .iter()
        .filter_map(|field| manifest.get(*field).and_then(Value::as_str))
        .map(str::to_owned)
        .collect();
    if let Some(bin) = manifest.get("bin") {
        collect_strings(bin, &mut paths);
    }
    if let Some(exports) = manifest.get("exports") {
        let mut leaves = Vec::new();
        collect_strings(exports, &mut leaves);
        paths.extend(
            leaves
                .into_iter()
                .filter(|leaf| leaf.starts_with("./") && leaf != "./*"),
        );
    }
    paths.extend(n8n_load_paths(manifest));
    paths
}

/// The extensions a relative specifier without one resolves with, in the order skif's resolver tries them.
const RESOLVED_EXTENSIONS: [&str; 8] = [".ts", ".tsx", ".mts", ".cts", ".js", ".jsx", ".mjs", ".cjs"];

/// The JavaScript extensions a relative specifier may name for its TypeScript twin (`./a.js` for `a.ts`).
const BUILT_EXTENSIONS: [&str; 4] = [".js", ".jsx", ".mjs", ".cjs"];

/// `specifier` (`./a`, `../b/c.js`) joined to the directory `dir`, both relative to the checkout; `None` when it leaves
/// the checkout.
fn join_relative(dir: &str, specifier: &str) -> Option<String> {
    let mut parts: Vec<&str> = dir.split('/').filter(|part| !part.is_empty()).collect();
    for part in specifier.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            name => parts.push(name),
        }
    }
    Some(parts.join("/"))
}

/// The file a relative specifier in a file of `dir` names: the path itself, the path with one of
/// [`RESOLVED_EXTENSIONS`] (a [`BUILT_EXTENSIONS`] one also replaced by them), or the `index` file of the directory it
/// names.
fn resolve_relative(dir: &str, specifier: &str, is_file: &FileProbe<'_>) -> Option<String> {
    let path = join_relative(dir, specifier)?;
    let stem = BUILT_EXTENSIONS
        .iter()
        .find_map(|extension| path.strip_suffix(extension))
        .unwrap_or(&path);
    let index = if path.is_empty() {
        "index".to_owned()
    } else {
        format!("{path}/index")
    };
    std::iter::once(path.clone())
        .chain(RESOLVED_EXTENSIONS.iter().map(|extension| format!("{stem}{extension}")))
        .chain(
            RESOLVED_EXTENSIONS
                .iter()
                .map(|extension| format!("{index}{extension}")),
        )
        .find(|candidate| is_file(candidate))
}

/// Whether a module specifier is relative (`.`, `..`, `./a`, `../b`) and has no query (`?raw`), which names no module.
fn is_relative_specifier(specifier: &str) -> bool {
    !specifier.contains('?')
        && (matches!(specifier, "." | "..") || specifier.starts_with("./") || specifier.starts_with("../"))
}

/// The directory of a path relative to the checkout (`""` for a file of the root).
fn parent_dir(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

/// The imports of the parsed file `path` that name a file by relative specifier, with that file: its imports,
/// requires, re-exports, dynamic imports and resolve calls (`require.resolve`, `import.meta.resolve`); globs,
/// test-runner mocks and specifiers with a query are left out.
fn relative_targets<'m>(path: &str, module: &'m ModuleInfo, is_file: &FileProbe<'_>) -> Vec<(&'m Import, String)> {
    module_targets(path, module, &Resolver::relative(is_file))
}

/// The imports of the parsed file `path` that `resolver` resolves to a file of the checkout, with that file: its
/// imports, requires, re-exports, dynamic imports and resolve calls; globs and test-runner mocks are left out.
fn module_targets<'m>(path: &str, module: &'m ModuleInfo, resolver: &Resolver<'_>) -> Vec<(&'m Import, String)> {
    module
        .imports
        .iter()
        .filter(|import| {
            !matches!(
                import.kind,
                ImportKind::ImportMetaGlob | ImportKind::SourceGlob | ImportKind::Mock
            )
        })
        .filter_map(|import| resolver.resolve(path, &import.specifier).map(|target| (import, target)))
        .collect()
}

/// The files a source file imports by relative specifier, as skif's parser reads its imports, requires, re-exports and
/// dynamic imports ([`relative_targets`]). Empty for a file it cannot parse.
fn relative_imports(path: &str, source: &str, is_file: &FileProbe<'_>) -> Vec<String> {
    let options = ParseOptions {
        in_file_references: false,
        member_accesses: false,
    };
    let Ok(module) = parse_module(Path::new(path), source, &options) else {
        return Vec::new();
    };
    relative_targets(path, &module, is_file)
        .into_iter()
        .map(|(_, target)| target)
        .collect()
}

/// A file's source by its path relative to the checkout; `None` when it cannot be read.
type SourceReader<'a> = dyn Fn(&str) -> Option<String> + 'a;

/// A [`SourceReader`] that reads nothing.
fn no_source(_: &str) -> Option<String> {
    None
}

/// How many `extends` links of a TypeScript configuration are followed (`MAX_EXTENDS` in
/// `crates/skif-js/src/tsconfig_paths.rs`).
const MAX_EXTENDS: usize = 8;

/// The placeholder TypeScript 5.5 replaces with the directory of the configuration that is being resolved.
const CONFIG_DIR: &str = "${configDir}";

/// The configuration files whose `paths` apply to the files below them, as skif's resolver reads them: the nearest
/// `tsconfig.json` or `jsconfig.json` at or above a file, a `tsconfig.json` first.
const PATHS_CONFIGS: [&str; 2] = ["tsconfig.json", "jsconfig.json"];

/// The `paths` of a TypeScript configuration: each pattern with its targets, relative to the checkout.
#[derive(Debug, Default, PartialEq, Eq)]
struct PathMap {
    patterns: Vec<(String, Vec<String>)>,
}

impl PathMap {
    /// The targets of the pattern TypeScript picks for `specifier`, each `*` replaced by what the pattern's `*`
    /// matched: a pattern without `*` that equals it, else the matching pattern with the longest start. Empty when no
    /// pattern matches.
    fn targets(&self, specifier: &str) -> Vec<String> {
        let mut best: Option<(usize, &str, &[String])> = None;
        for (pattern, targets) in &self.patterns {
            let matched = match pattern.split_once('*') {
                None => (pattern == specifier).then_some((usize::MAX, "")),
                Some((prefix, suffix)) => (specifier.len() >= prefix.len() + suffix.len()
                    && specifier.starts_with(prefix)
                    && specifier.ends_with(suffix))
                .then(|| (prefix.len(), &specifier[prefix.len()..specifier.len() - suffix.len()])),
            };
            if let Some((rank, captured)) = matched
                && best.is_none_or(|(known, _, _)| rank > known)
            {
                best = Some((rank, captured, targets.as_slice()));
            }
        }
        best.map_or_else(Vec::new, |(_, captured, targets)| {
            targets.iter().map(|target| target.replacen('*', captured, 1)).collect()
        })
    }

    /// The literal start of each pattern: the text an import that the pattern matches begins with.
    fn prefixes(&self) -> impl Iterator<Item = &str> {
        self.patterns
            .iter()
            .map(|(pattern, _)| pattern.split('*').next().unwrap_or(pattern))
    }
}

/// The `baseUrl` and `paths` a TypeScript configuration sets itself or through the configurations it extends, relative
/// to the checkout: `paths` with the directory of the configuration that sets it, which its targets are relative to
/// when no `baseUrl` is set.
#[derive(Debug, Default)]
struct PathOptions {
    base_url: Option<String>,
    paths: Option<(String, PathMappings)>,
}

/// Each `paths` pattern with its targets, in the order the configuration lists them.
type PathMappings = Vec<(String, Vec<String>)>;

/// `value`, a path of the configuration in `dir`, relative to the checkout: below `dir`, or for a value that starts
/// with `${configDir}` below `config_dir`, the directory of the configuration that is being resolved.
fn config_path(dir: &str, config_dir: &str, value: &str) -> Option<String> {
    match value.strip_prefix(CONFIG_DIR) {
        Some(rest) => join_relative(config_dir, rest.trim_start_matches('/')),
        None => join_relative(dir, value),
    }
}

/// The configuration file an `extends` entry of a configuration in `dir` names: a relative path (with `.json` added,
/// or a directory's `tsconfig.json`), or a package's file below the nearest `node_modules` at or above `dir`.
fn extended_config(dir: &str, entry: &str, is_file: &FileProbe<'_>) -> Option<String> {
    let candidates = |path: String| {
        let json = if is_json_file(&path) {
            path.clone()
        } else {
            format!("{path}.json")
        };
        let nested = format!("{path}/tsconfig.json");
        [path, json, nested]
    };
    if entry.starts_with("./") || entry.starts_with("../") || entry.starts_with('/') {
        return candidates(join_relative(dir, entry)?)
            .into_iter()
            .find(|candidate| is_file(candidate));
    }
    let mut above = dir;
    loop {
        let found = candidates(joined(above, &format!("node_modules/{entry}")))
            .into_iter()
            .find(|candidate| is_file(candidate));
        if found.is_some() || above.is_empty() {
            return found;
        }
        above = parent_dir(above);
    }
}

/// What the configuration file `config` sets of `baseUrl` and `paths`, itself or through the configurations it
/// `extends` (a later entry of an `extends` list over an earlier one, the configuration itself over all of them), up
/// to [`MAX_EXTENDS`] links; `config_dir` is the directory of the configuration being resolved. Nothing for a file that
/// cannot be read or is not JSON with comments.
fn path_options(
    config: &str,
    config_dir: &str,
    (read, is_file): (&SourceReader<'_>, &FileProbe<'_>),
    depth: usize,
) -> PathOptions {
    let Some(value) = read(config).and_then(|text| serde_json::from_str::<Value>(&strip_jsonc(&text)).ok()) else {
        return PathOptions::default();
    };
    let dir = parent_dir(config);
    let mut options = PathOptions::default();
    let mut extends = Vec::new();
    if let Some(entries) = value.get("extends") {
        collect_strings(entries, &mut extends);
    }
    if depth < MAX_EXTENDS {
        for entry in &extends {
            if let Some(file) = extended_config(dir, entry, is_file) {
                let inherited = path_options(&file, config_dir, (read, is_file), depth + 1);
                options.base_url = inherited.base_url.or(options.base_url);
                options.paths = inherited.paths.or(options.paths);
            }
        }
    }
    let compiler = value.get("compilerOptions");
    if let Some(base_url) = compiler
        .and_then(|compiler| compiler.get("baseUrl"))
        .and_then(Value::as_str)
        .and_then(|base_url| config_path(dir, config_dir, base_url))
    {
        options.base_url = Some(base_url);
    }
    if let Some(paths) = compiler
        .and_then(|compiler| compiler.get("paths"))
        .and_then(Value::as_object)
    {
        let entries = paths
            .iter()
            .map(|(pattern, targets)| {
                let mut found = Vec::new();
                collect_strings(targets, &mut found);
                (pattern.clone(), found)
            })
            .collect();
        options.paths = Some((dir.to_owned(), entries));
    }
    options
}

/// The `paths` of the configuration file `config` ([`path_options`]) with their targets relative to the checkout:
/// below its `baseUrl` when one is set, else below the directory of the configuration that sets `paths`. `None`
/// without `paths`.
fn path_map(config: &str, read: &SourceReader<'_>, is_file: &FileProbe<'_>) -> Option<PathMap> {
    let config_dir = parent_dir(config);
    let options = path_options(config, config_dir, (read, is_file), 0);
    let (paths_dir, entries) = options.paths?;
    let base = options.base_url.unwrap_or(paths_dir);
    let patterns = entries
        .into_iter()
        .map(|(pattern, targets)| {
            let targets = targets
                .iter()
                .filter_map(|target| config_path(&base, config_dir, target))
                .collect();
            (pattern, targets)
        })
        .collect();
    Some(PathMap { patterns })
}

/// Resolves the module specifiers of a file to files of the checkout, as skif's resolver does in the cases the harness
/// models: a relative specifier ([`resolve_relative`]), and, built with [`Resolver::new`], a bare one that the `paths`
/// of the importing file's TypeScript configuration map to a file outside `node_modules` ([`Resolver::paths_target`])
/// or that names a workspace package, mapped to the source of what its manifest publishes
/// ([`Resolver::workspace_target`]).
struct Resolver<'a> {
    is_file: &'a FileProbe<'a>,
    read: &'a SourceReader<'a>,
    layout: Option<&'a Layout<'a>>,
    /// The directory of each workspace package (the root and the members) by the `name` of its manifest.
    packages: BTreeMap<String, String>,
    /// The `paths` that apply to the files of each directory ([`Resolver::paths_of`]), read once.
    configs: RefCell<BTreeMap<String, Option<Rc<PathMap>>>>,
}

impl<'a> Resolver<'a> {
    /// A resolver of relative specifiers only.
    fn relative(is_file: &'a FileProbe<'a>) -> Self {
        Self {
            is_file,
            read: &no_source,
            layout: None,
            packages: BTreeMap::new(),
            configs: RefCell::default(),
        }
    }

    /// A resolver of relative specifiers, tsconfig `paths` and workspace packages in the checkout `layout` describes,
    /// whose files `read` gives.
    fn new(layout: &'a Layout<'a>, read: &'a SourceReader<'a>) -> Self {
        let packages = std::iter::once(String::new())
            .chain(layout.members.iter().cloned())
            .filter_map(|dir| {
                let name = (layout.manifest)(&dir)?.get("name")?.as_str()?.to_owned();
                Some((name, dir))
            })
            .collect();
        Self {
            is_file: layout.is_file,
            read,
            layout: Some(layout),
            packages,
            configs: RefCell::default(),
        }
    }

    /// The file `specifier` names in the file `importer`; `None` for one the resolver does not model or that names no
    /// file of the checkout.
    fn resolve(&self, importer: &str, specifier: &str) -> Option<String> {
        if is_relative_specifier(specifier) {
            return resolve_relative(parent_dir(importer), specifier, self.is_file);
        }
        if self.layout.is_none() || specifier.contains('?') || bare_package(specifier).is_none() {
            return None;
        }
        self.paths_target(importer, specifier)
            .or_else(|| self.workspace_target(specifier))
    }

    /// The `paths` that apply to the files of `dir`: those of the nearest of [`PATHS_CONFIGS`] at or above it
    /// ([`path_map`]); `None` when that configuration sets none.
    fn paths_of(&self, dir: &str) -> Option<Rc<PathMap>> {
        if let Some(known) = self.configs.borrow().get(dir) {
            return known.clone();
        }
        let config = PATHS_CONFIGS
            .iter()
            .map(|name| joined(dir, name))
            .find(|path| (self.is_file)(path));
        let found = match config {
            Some(config) => path_map(&config, self.read, self.is_file).map(Rc::new),
            None if dir.is_empty() => None,
            None => self.paths_of(parent_dir(dir)),
        };
        self.configs.borrow_mut().insert(dir.to_owned(), found.clone());
        found
    }

    /// The file outside `node_modules` that the `paths` of the configuration of `importer` map `specifier` to: the
    /// first target of the pattern TypeScript picks ([`PathMap::targets`]) that names a file, with an extension, a
    /// TypeScript twin or an `index` file as a relative specifier does.
    fn paths_target(&self, importer: &str, specifier: &str) -> Option<String> {
        let map = self.paths_of(parent_dir(importer))?;
        map.targets(specifier)
            .iter()
            .find_map(|target| resolve_relative("", target, self.is_file))
            .filter(|file| !file.split('/').any(|part| part == "node_modules"))
    }

    /// Whether the `paths` of the configuration of `importer` map `specifier` to generated code: a target of the
    /// pattern TypeScript picks ([`PathMap::targets`]) outside `node_modules` that names no file, as a relative
    /// specifier does, and that `is_ignored` accepts ([`ignored_candidate`]). skif takes an alias into ignored code
    /// that does not exist for generated code (`is_generated` in `crates/skif-js/src/resolve.rs`).
    fn generated_paths_target(&self, importer: &str, specifier: &str, is_ignored: &FileProbe<'_>) -> bool {
        let Some(map) = self.paths_of(parent_dir(importer)) else {
            return false;
        };
        map.targets(specifier)
            .iter()
            .filter(|target| !target.split('/').any(|part| part == "node_modules"))
            .filter_map(|target| join_relative("", target))
            .any(|target| {
                resolve_relative("", &target, self.is_file).is_none() && ignored_candidate(is_ignored, &target)
            })
    }

    /// The source file of the workspace package `specifier` names (`name` or `name/subpath`): the source behind the
    /// `exports` entry of the subpath (for the package itself also `main`, `module` and the other published fields),
    /// mapped as skif maps a published path ([`Layout::source_of`]), else the subpath itself or its `src` twin (the
    /// package's `src/index` or `index` for no subpath), else the source a subpath into build output that was never
    /// built is built from, mapped the same way (`next/dist/server/web/types` names `src/server/web/types.ts` of
    /// `next`), as skif resolves a specifier into unbuilt output (`through_build_output` in
    /// `crates/skif-js/src/resolve.rs`).
    fn workspace_target(&self, specifier: &str) -> Option<String> {
        let layout = self.layout?;
        let name = bare_package(specifier)?;
        let dir = self.packages.get(name)?;
        let subpath = specifier[name.len()..].trim_start_matches('/');
        let manifest = (layout.manifest)(dir)?;
        let key = if subpath.is_empty() {
            ".".to_owned()
        } else {
            format!("./{subpath}")
        };
        let mut targets = Vec::new();
        match manifest.get("exports") {
            Some(Value::Object(map)) if map.keys().any(|condition| condition.starts_with('.')) => {
                if let Some(value) = map.get(&key) {
                    collect_strings(value, &mut targets);
                }
            }
            Some(exports) if subpath.is_empty() => collect_strings(exports, &mut targets),
            _ => {}
        }
        if subpath.is_empty() {
            targets.extend(
                PUBLISHED_FIELDS
                    .iter()
                    .filter_map(|field| manifest.get(*field).and_then(Value::as_str))
                    .map(str::to_owned),
            );
        }
        let local = if subpath.is_empty() {
            ["./src/index".to_owned(), "./index".to_owned()]
        } else {
            [format!("./{subpath}"), format!("./src/{subpath}")]
        };
        targets
            .iter()
            .filter(|target| !target.contains('*'))
            .find_map(|target| layout.source_of(dir, target))
            .map(|source| joined(dir, &source))
            .or_else(|| local.iter().find_map(|path| resolve_relative(dir, path, self.is_file)))
            .or_else(|| {
                (!subpath.is_empty())
                    .then(|| layout.source_of(dir, &format!("./{subpath}")))
                    .flatten()
                    .map(|source| joined(dir, &source))
            })
    }
}

/// The exports of other files that the parsed file `path` imports by a specifier `resolver` resolves, as `(file,
/// export)`: a named import or re-export by its name, a default import as `default`, a namespace by the members read
/// through it, and a namespace used as a whole, `export * from`, `import()`, `require` and a resolve call
/// (`require.resolve`, `import.meta.resolve`) of the module as [`WHOLE_MODULE`]. A side-effect import uses no export.
fn imported_exports(path: &str, module: &ModuleInfo, resolver: &Resolver<'_>) -> Vec<(String, String)> {
    let mut found = Vec::new();
    for (import, target) in module_targets(path, module, resolver) {
        if import.names.is_empty() {
            if matches!(
                import.kind,
                ImportKind::Dynamic | ImportKind::Require | ImportKind::Resolve
            ) {
                found.push((target, WHOLE_MODULE.to_owned()));
            }
            continue;
        }
        for name in &import.names {
            match &name.kind {
                ImportedKind::Named(export) => found.push((target.clone(), export.clone())),
                ImportedKind::Default => found.push((target.clone(), "default".to_owned())),
                ImportedKind::Namespace if name.local.is_some() && !name.escapes => found.extend(
                    name.accessed_members
                        .iter()
                        .map(|member| (target.clone(), member.clone())),
                ),
                ImportedKind::Namespace => found.push((target.clone(), WHOLE_MODULE.to_owned())),
            }
        }
    }
    found
}

/// Where the export `name` (or, for [`WHOLE_MODULE`], every export) of the parsed file `path` comes from when the file
/// re-exports it, as `(file, export)`: the source of `export { name } from` (and of `import { x } from`; `export { x }`)
/// under the name taken from it, and the source of an `export * from` for a name the file does not export itself; the
/// source is the file `resolver` resolves the specifier to. A member of an export (`E.A`, [`member_uses`]) follows its
/// export and keeps its member part.
fn reexported(path: &str, module: &ModuleInfo, name: &str, resolver: &Resolver<'_>) -> Vec<(String, String)> {
    let declares = |wanted: &str| module.exports.iter().any(|export| export.name == wanted);
    let (head, member) = name
        .split_once('.')
        .map_or((name, String::new()), |(head, member)| (head, format!(".{member}")));
    module
        .exports
        .iter()
        .filter_map(|export| {
            let specifier = export.from.as_deref()?;
            let followed = if export.kind == ExportKind::StarReExport && export.name == WHOLE_MODULE {
                (name == WHOLE_MODULE || !declares(head)).then(|| name.to_owned())
            } else if name == WHOLE_MODULE {
                Some(export.imported_name.clone().unwrap_or_else(|| export.name.clone()))
            } else {
                (export.name == head).then(|| {
                    let imported = export.imported_name.as_deref().unwrap_or(&export.name);
                    match member.strip_prefix('.') {
                        Some(rest) if imported == WHOLE_MODULE => rest.to_owned(),
                        _ => format!("{imported}{member}"),
                    }
                })
            };
            followed.and_then(|followed| resolver.resolve(path, specifier).map(|target| (target, followed)))
        })
        .collect()
}

/// `<file>|<export>` (`<file>|*` for [`WHOLE_MODULE`]) for every `(file, export)` of `seeds` and for the exports those
/// re-export from other files, followed through every file in between ([`reexported`]). `read` gives a file's source;
/// a file it cannot read or parse re-exports nothing.
fn follow_reexports(
    seeds: Vec<(String, String)>,
    resolver: &Resolver<'_>,
    read: &SourceReader<'_>,
) -> BTreeSet<String> {
    let options = ParseOptions {
        in_file_references: false,
        member_accesses: false,
    };
    let mut pending = seeds;
    let mut modules: BTreeMap<String, Option<ModuleInfo>> = BTreeMap::new();
    let mut credits = BTreeSet::new();
    while let Some((file, name)) = pending.pop() {
        if !credits.insert(format!("{file}|{name}")) {
            continue;
        }
        let module = modules
            .entry(file.clone())
            .or_insert_with(|| read(&file).and_then(|source| parse_module(Path::new(&file), &source, &options).ok()));
        if let Some(module) = module {
            pending.extend(reexported(&file, module, &name, resolver));
        }
    }
    credits
}

/// `<file>|<export>` for every export the `importers` import ([`imported_exports`]), followed through re-exports
/// ([`follow_reexports`]). A file `read` cannot read or parse imports nothing.
fn import_credits<'f>(
    importers: impl IntoIterator<Item = &'f str>,
    resolver: &Resolver<'_>,
    read: &SourceReader<'_>,
) -> BTreeSet<String> {
    let options = ParseOptions {
        in_file_references: false,
        member_accesses: true,
    };
    let seeds = importers
        .into_iter()
        .filter_map(|file| {
            read(file)
                .and_then(|source| parse_module(Path::new(file), &source, &options).ok())
                .map(|module| imported_exports(file, &module, resolver))
        })
        .flatten()
        .collect();
    follow_reexports(seeds, resolver, read)
}

/// `<file>|<export>` (`<file>|*` for [`WHOLE_MODULE`]) for every export that one of the files only one tool lists as
/// unused (`unused` but not `other_unused`) imports by a specifier `resolver` resolves (a relative one, through the
/// importer's tsconfig `paths`, or a workspace package), and for the exports those re-export from other files,
/// followed through every file in between ([`import_credits`]): what that tool would have credited had it analysed the
/// file. `read` gives a file's source; a file it cannot read or parse imports nothing and re-exports nothing.
fn unused_file_credits(
    unused: &BTreeSet<String>,
    other_unused: &BTreeSet<String>,
    resolver: &Resolver<'_>,
    read: &SourceReader<'_>,
) -> BTreeSet<String> {
    import_credits(unused.difference(other_unused).map(String::as_str), resolver, read)
}

/// Whether a finding names an export of its file (or, for duplicates, one of its names) that `credits` (from
/// [`unused_file_credits`]) holds, or the file is credited as a whole.
fn imported_by_unused_file(credits: &BTreeSet<String>, category: &str, key: &str) -> bool {
    if !IMPORTED_EXPORT_CATEGORIES.contains(&category) {
        return false;
    }
    credited(credits, key)
}

/// Whether `credits` (`<file>|<export>`, `<file>|*` for a whole file) holds the export a finding key names
/// (`<file>|<name>`, or for duplicates any of `<file>|<names>`), or its file as a whole.
fn credited(credits: &BTreeSet<String>, key: &str) -> bool {
    let Some((path, names)) = key.split_once('|') else {
        return false;
    };
    credits.contains(&format!("{path}|{WHOLE_MODULE}"))
        || names.split('|').any(|name| credits.contains(&format!("{path}|{name}")))
}

/// The entry modules among `files` of the projects that own one of `keys`, as skif makes them without the repository's
/// configuration: the sources of the paths a project publishes ([`Layout::publishes_source_of`]), and the entry files of
/// the projects that are not knip workspaces and that a recursive skif run lints as a package of one of its roots
/// (`root_packages`): their default entry files ([`Layout::is_default_entry`]) and the files `entries_of` gives for the
/// project's directory, read once per project.
fn entry_modules(
    keys: &BTreeSet<String>,
    files: &[String],
    layout: &Layout<'_>,
    (root_packages, entries_of): (&BTreeSet<String>, &EntriesOf<'_>),
) -> (Vec<String>, Vec<String>) {
    let is_project = |dir: &str| layout.is_project(dir);
    let projects: BTreeSet<String> = keys.iter().map(|key| owner_dir(key, &is_project)).collect();
    let mut published = Vec::new();
    let mut defaults = Vec::new();
    if projects.is_empty() {
        return (published, defaults);
    }
    let mut by_project: BTreeMap<String, PluginEntries> = BTreeMap::new();
    for file in files {
        if !projects.contains(&owner_dir(file, &is_project)) {
            continue;
        }
        if layout.publishes_source_of(file) {
            published.push(file.clone());
        }
        let Some(dir) = layout
            .non_member_project(file)
            .filter(|dir| root_packages.contains(dir))
        else {
            continue;
        };
        let relative = below(&dir, file).to_owned();
        if layout.is_default_entry(file)
            || by_project
                .entry(dir)
                .or_insert_with_key(|dir| entries_of(dir.as_str()))
                .contains(&relative)
        {
            defaults.push(file.clone());
        }
    }
    (published, defaults)
}

/// The files skif's plugins make entries of a project: the entry globs of the plugins its manifest and files enable,
/// and the code configuration files they read, relative to the project directory.
/// The plugin entries skif's resolver gives the project of a directory.
type EntriesOf<'a> = dyn Fn(&str) -> PluginEntries + 'a;

struct PluginEntries {
    globs: IncompleteScopeGlobs,
    configs: BTreeSet<String>,
}

impl PluginEntries {
    /// No entry: a project whose plugins are not read.
    fn none() -> Self {
        Self {
            globs: IncompleteScopeGlobs::new(Vec::new()),
            configs: BTreeSet::new(),
        }
    }

    /// Whether `relative`, a path below the project directory with `/` separators, is one of the entries.
    fn contains(&self, relative: &str) -> bool {
        self.configs.contains(relative) || self.globs.is_match(relative)
    }
}

/// What skif's plugin resolver (`skif_js_plugins::resolve_plugins`) gives for the project in `dir` of the checkout at
/// `root`, as skif's run of the project as a package of a root of its own resolves it: with the dependencies, scripts
/// and name of its `manifest`, no dependency of an enclosing workspace, the default plugin configuration, `is_root`
/// when the project is the root of its run, and `members`, the directories of the other packages of that run below it
/// (relative to it), whose sources are theirs.
fn project_plugins(
    fs: &dyn FileSystem,
    root: &Path,
    (dir, is_root): (&str, bool),
    manifest: &Value,
    members: &[String],
) -> Vec<(PluginId, PluginOutput)> {
    let abs_dir = root.join(dir);
    let dependencies: Vec<String> = DEPENDENCY_FIELDS
        .iter()
        .filter_map(|field| manifest.get(*field).and_then(Value::as_object))
        .flat_map(|listed| listed.keys().cloned())
        .collect();
    let scripts = manifest
        .get("scripts")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(name, command)| Some((name.clone(), command.as_str()?.to_owned())))
        .collect();
    let plugins = indexmap::IndexMap::default();
    let globs = GlobCache::new();
    let ctx = PluginContext {
        dir: &abs_dir,
        is_root,
        package_name: manifest.get("name").and_then(Value::as_str),
        dependencies: &dependencies,
        inherited_dependencies: &[],
        scripts: &scripts,
        fs,
        plugins: &plugins,
        modules: None,
        nx: None,
        globs: &globs,
        sources: None,
        members,
    };
    resolve_plugins(&ctx)
}

/// The entries of the project in `dir` that skif's plugins give ([`project_plugins`]): their entry globs, and the code
/// files among the configs they read, relative to the project.
fn plugin_entries(
    fs: &dyn FileSystem,
    root: &Path,
    (dir, is_root): (&str, bool),
    manifest: &Value,
    members: &[String],
) -> PluginEntries {
    let abs_dir = root.join(dir);
    let mut patterns = Vec::new();
    let mut configs = BTreeSet::new();
    for (_, output) in project_plugins(fs, root, (dir, is_root), manifest, members) {
        patterns.extend(output.entries);
        configs.extend(output.config_files.iter().filter_map(|path| {
            let relative = path.strip_prefix(&abs_dir).ok()?.to_str()?.replace('\\', "/");
            has_code_extension(&relative).then_some(relative)
        }));
    }
    PluginEntries {
        globs: IncompleteScopeGlobs::new(patterns),
        configs,
    }
}

/// The directories of `packages` strictly below `dir`, relative to it.
fn packages_below(packages: &BTreeSet<String>, dir: &str) -> Vec<String> {
    packages
        .iter()
        .filter(|package| package.as_str() != dir)
        .filter_map(|package| {
            if dir.is_empty() {
                return Some(package.clone());
            }
            package
                .strip_prefix(dir)
                .and_then(|rest| rest.strip_prefix('/'))
                .map(str::to_owned)
        })
        .collect()
}

/// `<file>|<export>` for every export that one of `entries` re-exports through `export … from` and `export *`,
/// followed through the files in between ([`follow_reexports`]), with `<entry>|*` for each entry: an entry's exports
/// are public in skif, the ones it re-exports from other files among them.
fn entry_reexports(entries: Vec<String>, resolver: &Resolver<'_>, read: &SourceReader<'_>) -> BTreeSet<String> {
    let seeds = entries
        .into_iter()
        .map(|entry| (entry, WHOLE_MODULE.to_owned()))
        .collect();
    follow_reexports(seeds, resolver, read)
}

/// What the projects that own one of `keys` publish as it is, as entries skif makes of them: `(sources, files)`, the
/// sources their published JavaScript and declaration files and their `bin` stubs load into build output
/// ([`published_file_sources`], [`bin_stub_sources`]), and the published files themselves among `files`
/// ([`Layout::publishes_file`]).
fn published_file_entries(
    keys: &BTreeSet<String>,
    files: &[String],
    layout: &Layout<'_>,
    read: &dyn Fn(&str) -> Option<String>,
) -> (Vec<String>, Vec<String>) {
    let is_project = |dir: &str| layout.is_project(dir);
    let projects: BTreeSet<String> = keys.iter().map(|key| owner_dir(key, &is_project)).collect();
    let mut sources: Vec<String> = projects
        .iter()
        .flat_map(|dir| {
            let mut loaded = bin_stub_sources(layout, dir, read);
            loaded.extend(published_file_sources(layout, dir, read));
            loaded
        })
        .collect();
    sources.sort();
    sources.dedup();
    let published = files
        .iter()
        .filter(|file| projects.contains(&owner_dir(file, &is_project)) && layout.publishes_file(file))
        .cloned()
        .collect();
    (sources, published)
}

/// `<file>|<export>` for every export that one of the published files `published` re-exports from another file,
/// followed through the files in between ([`entry_reexports`]), without the published files themselves, whose own
/// findings `published-file` takes.
fn published_file_reexports(
    published: Vec<String>,
    resolver: &Resolver<'_>,
    read: &SourceReader<'_>,
) -> BTreeSet<String> {
    let own: Vec<String> = published.iter().map(|file| format!("{file}|{WHOLE_MODULE}")).collect();
    let mut credits = entry_reexports(published, resolver, read);
    for key in &own {
        credits.remove(key);
    }
    credits
}

/// `<file>|<export>` for every export that the files of `files` import ([`import_credits`]) from where skif's analysis
/// of a workspace reads no file: `(non_member, skipped)`, the files below a project that is not one of knip's
/// workspaces (and in no skipped directory), and the files below a directory skif's file discovery skips.
fn outside_credits(
    files: &[String],
    layout: &Layout<'_>,
    resolver: &Resolver<'_>,
    read: &SourceReader<'_>,
) -> (BTreeSet<String>, BTreeSet<String>) {
    let (non_member, skipped) = outside_importers(files, layout);
    (
        import_credits(non_member, resolver, read),
        import_credits(skipped, resolver, read),
    )
}

/// The files of `files` where skif's analysis of a workspace reads no file: `(non_member, skipped)`, those below a
/// project that is not one of knip's workspaces (and in no skipped directory), and those below a directory skif's file
/// discovery skips.
fn outside_importers<'f>(files: &'f [String], layout: &Layout<'_>) -> (Vec<&'f str>, Vec<&'f str>) {
    let (skipped, rest): (Vec<&str>, Vec<&str>) = files
        .iter()
        .map(String::as_str)
        .partition(|file| layout.in_skipped_directory(file));
    let non_member = rest
        .into_iter()
        .filter(|file| layout.non_member_project(file).is_some())
        .collect();
    (non_member, skipped)
}

/// The categories of a finding that depend on the packages installed for its project: its dependencies and binaries,
/// and the imports a tool resolves through them.
const INSTALLED_CATEGORIES: [&str; 6] = [
    "dependencies",
    "devDependencies",
    "optionalPeerDependencies",
    "binaries",
    "unlisted",
    "unresolved",
];

/// The projects (`Layout::is_project` directories) that own a finding only one tool reports in
/// [`INSTALLED_CATEGORIES`] and that have no installed packages at or above them: no directory there that
/// `has_packages` accepts (one with a `node_modules` directory or Yarn's `.pnp.cjs`). Both tools resolve the imports
/// and binaries of such a project without its packages, each in its own way.
fn uninstalled_projects(
    (skif, knip): (&Keys, &Keys),
    is_project: &dyn Fn(&str) -> bool,
    has_packages: &FileProbe<'_>,
) -> BTreeSet<String> {
    let mut checked = BTreeSet::new();
    let mut found = BTreeSet::new();
    for (own, other) in [(skif, knip), (knip, skif)] {
        for category in INSTALLED_CATEGORIES {
            for key in one_sided(own, other, category) {
                let dir = owner_dir(&key, is_project);
                if checked.insert(dir.clone()) && !installed_at(&dir, has_packages) {
                    found.insert(dir);
                }
            }
        }
    }
    found
}

/// Whether `has_packages` accepts `dir` or a directory above it, up to the checkout root.
fn installed_at(dir: &str, has_packages: &FileProbe<'_>) -> bool {
    let mut candidate = dir;
    loop {
        if has_packages(candidate) {
            return true;
        }
        if candidate.is_empty() {
            return false;
        }
        candidate = parent_dir(candidate);
    }
}

/// The files only knip reports as unused (`unused` but not `other_unused`) that one of them whose path its package
/// publishes ([`Layout::publishes_source_of`]) reaches by relative imports ([`relative_imports`]), directly or through
/// other such files; the published sources themselves are left out. skif makes a published source an entry, so it
/// uses every file the entry imports; knip, which reads no entry there, reports the whole chain. The sources a published
/// bin stub of the package of such a file loads ([`bin_stub_sources`]), and those a file it publishes as it is loads
/// from build output ([`published_file_sources`]), are entries too: they are among the files, with what they reach. `read` gives a file's source; a file it cannot read imports nothing.
fn published_reach(
    unused: &BTreeSet<String>,
    other_unused: &BTreeSet<String>,
    layout: &Layout<'_>,
    read: &dyn Fn(&str) -> Option<String>,
) -> BTreeSet<String> {
    let only: BTreeSet<&str> = unused.difference(other_unused).map(String::as_str).collect();
    let projects: BTreeSet<String> = only
        .iter()
        .map(|file| owner_dir(file, &|dir: &str| layout.is_project(dir)))
        .collect();
    let stubs: BTreeSet<&str> = projects
        .iter()
        .flat_map(|dir| {
            let mut loaded = bin_stub_sources(layout, dir, read);
            loaded.extend(published_file_sources(layout, dir, read));
            loaded
        })
        .filter_map(|source| only.get(source.as_str()).copied())
        .collect();
    let seeds = only
        .iter()
        .copied()
        .filter(|file| layout.publishes_source_of(file))
        .chain(stubs.iter().copied())
        .collect();
    let mut reached = reach(seeds, &only, layout.is_file, read);
    reached.extend(stubs.iter().map(|stub| (*stub).to_owned()));
    reached
}

/// The files of `only` that the files of `seeds` reach by relative imports ([`relative_imports`]), directly or through
/// other files of `only`; the seeds themselves are left out unless another seed reaches them. `read` gives a file's
/// source; a file it cannot read imports nothing.
fn reach<'u>(
    seeds: Vec<&'u str>,
    only: &BTreeSet<&'u str>,
    is_file: &FileProbe<'_>,
    read: &dyn Fn(&str) -> Option<String>,
) -> BTreeSet<String> {
    let mut pending = seeds;
    let mut seen: BTreeSet<&str> = pending.iter().copied().collect();
    let mut reached = BTreeSet::new();
    while let Some(file) = pending.pop() {
        let Some(source) = read(file) else { continue };
        for target in relative_imports(file, &source, is_file) {
            if let Some(&found) = only.get(target.as_str())
                && seen.insert(found)
            {
                reached.insert(found.to_owned());
                pending.push(found);
            }
        }
    }
    reached
}

/// The `./` and `../` string literals that follow `require(`, `import(`, `import` or `from` in `text`, sorted and
/// without duplicates (`relative_specifiers` in `crates/skif-js/src/manifest_paths.rs`).
fn stub_specifiers(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    for marker in ["require(", "import(", "import ", "from "] {
        for (at, _) in text.match_indices(marker) {
            let rest = text[at + marker.len()..].trim_start();
            let Some(quote) = rest.chars().next().filter(|c| matches!(*c, '\'' | '"' | '`')) else {
                continue;
            };
            let body = &rest[1..];
            if let Some(end) = body.find(quote) {
                let specifier = &body[..end];
                if specifier.starts_with("./") || specifier.starts_with("../") {
                    found.push(specifier.to_owned());
                }
            }
        }
    }
    found.sort();
    found.dedup();
    found
}

/// The sources, relative to the checkout, that the published bin stubs of the project in `dir` load, as skif reads
/// them (`bin_stub_sources` in `crates/skif-js/src/manifest_paths.rs`): a `bin` file without a code extension whose
/// first line is a `#!` line naming `node` is no module, so the relative specifiers it requires or imports
/// ([`stub_specifiers`]) are mapped to source like a published path ([`loaded_sources`]).
fn bin_stub_sources(layout: &Layout<'_>, dir: &str, read: &dyn Fn(&str) -> Option<String>) -> Vec<String> {
    let Some(manifest) = (layout.manifest)(dir) else {
        return Vec::new();
    };
    let mut targets = Vec::new();
    if let Some(bin) = manifest.get("bin") {
        collect_strings(bin, &mut targets);
    }
    let mut sources = Vec::new();
    for target in &targets {
        let Some(stub) = join_relative(dir, target) else {
            continue;
        };
        if has_code_extension(&stub) {
            continue;
        }
        let Some(text) = read(&stub) else { continue };
        if !text.starts_with("#!") || !text.lines().next().is_some_and(|line| line.contains("node")) {
            continue;
        }
        sources.extend(loaded_sources(layout, dir, &stub, &text, false));
    }
    sources
}

/// The sources, relative to the checkout, that the files the project in `dir` publishes as they are load: a JavaScript
/// or declaration file that a published field, `bin`, an `exports` leaf or an entry of `files` names without `*`
/// ([`Layout::publishes_file`]), whose relative specifiers into build output ([`stub_specifiers`],
/// `require('./dist/client/index')`, `export * from './dist/client/index'`) are mapped to source like a published path
/// ([`loaded_sources`]), sorted and without duplicates. skif makes such a file an entry and resolves a relative
/// specifier into build output that was never built to the source it is built from (`through_build_output` in
/// `crates/skif-js/src/resolve.rs`).
fn published_file_sources(layout: &Layout<'_>, dir: &str, read: &dyn Fn(&str) -> Option<String>) -> Vec<String> {
    let Some(manifest) = (layout.manifest)(dir) else {
        return Vec::new();
    };
    let mut targets = published_paths(&manifest);
    if let Some(files) = manifest.get("files") {
        collect_strings(files, &mut targets);
    }
    let published: BTreeSet<String> = targets
        .iter()
        .filter(|target| !target.contains('*'))
        .filter_map(|target| join_relative(dir, target))
        .filter(|path| layout.publishes_file(path))
        .collect();
    let mut sources = Vec::new();
    for file in &published {
        let Some(text) = read(file) else { continue };
        sources.extend(loaded_sources(layout, dir, file, &text, true));
    }
    sources.sort();
    sources.dedup();
    sources
}

/// The sources, relative to the checkout, behind the relative specifiers ([`stub_specifiers`]) of `text`, the source
/// of the file `loader` of the project in `dir`: each specifier joined to the loader's directory, inside the project,
/// and with `into_output` only one into build output ([`is_build_output`]) from a loader outside it, mapped to source
/// like a published path ([`Layout::source_of`]) or as the directory's `index`. A specifier that leaves the project
/// names nothing.
fn loaded_sources(layout: &Layout<'_>, dir: &str, loader: &str, text: &str, into_output: bool) -> Vec<String> {
    let mut sources = Vec::new();
    if into_output && is_build_output(layout, dir, below(dir, loader)) {
        return sources;
    }
    for specifier in stub_specifiers(text) {
        let Some(target) = join_relative(parent_dir(loader), &specifier) else {
            continue;
        };
        if !dir.is_empty() && !target.starts_with(&format!("{dir}/")) {
            continue;
        }
        let published = below(dir, &target);
        if into_output && !is_build_output(layout, dir, published) {
            continue;
        }
        let source = layout
            .source_of(dir, published)
            .or_else(|| layout.source_of(dir, &format!("{published}/index")));
        sources.extend(source.map(|source| joined(dir, &source)));
    }
    sources
}

/// Whether `relative`, a path below the project directory `dir`, lies in build output: below an `outDir` of the
/// project's build directories, or in a first directory named like build output (`skif_js::is_out_dir`).
fn is_build_output(layout: &Layout<'_>, dir: &str, relative: &str) -> bool {
    let first = relative.split('/').next().unwrap_or(relative);
    (relative.contains('/') && is_out_dir(first))
        || (layout.build_dirs)(dir).iter().any(|(out_dir, _)| {
            relative
                .strip_prefix(out_dir.as_str())
                .is_some_and(|rest| rest.starts_with('/'))
        })
}

/// The parsed `package.json` of a directory of the checkout (`""` is the root): `None` when there is none, `Null` when
/// it cannot be read or parsed.
type ManifestReader<'a> = dyn Fn(&str) -> Option<Rc<Value>> + 'a;

/// Whether a file exists, by its path relative to the checkout with `/` separators.
type FileProbe<'a> = dyn Fn(&str) -> bool + 'a;

/// The build directories of a package directory of the checkout (`""` is the root), `(out_dir, root_dir)` relative to
/// it, in the order skif reads them ([`build_dir_reader`]).
type BuildDirReader<'a> = dyn Fn(&str) -> Vec<(String, String)> + 'a;

/// A [`BuildDirReader`] for a checkout whose packages declare no build directory.
fn no_build_dirs(_: &str) -> Vec<(String, String)> {
    Vec::new()
}

/// What the classification reads of a checkout besides the two reports: knip's workspaces, the manifests, the build
/// directories and which files exist.
struct Layout<'a> {
    /// The workspace member directories, relative with `/` separators; the root is a workspace without being listed.
    members: BTreeSet<String>,
    manifest: &'a ManifestReader<'a>,
    is_file: &'a FileProbe<'a>,
    build_dirs: &'a BuildDirReader<'a>,
    /// [`DEFAULT_ENTRY`], compiled as skif compiles package-relative globs.
    default_entry: IncompleteScopeGlobs,
}

impl<'a> Layout<'a> {
    fn new(members: BTreeSet<String>, manifest: &'a ManifestReader<'a>, is_file: &'a FileProbe<'a>) -> Self {
        Self {
            members,
            manifest,
            is_file,
            build_dirs: &no_build_dirs,
            default_entry: IncompleteScopeGlobs::new(DEFAULT_ENTRY.iter().map(|glob| (*glob).to_owned()).collect()),
        }
    }

    /// Whether `path` is a default entry file of its project ([`DEFAULT_ENTRY`], relative to the project directory).
    fn is_default_entry(&self, path: &str) -> bool {
        let dir = owner_dir(path, &|dir: &str| self.is_project(dir));
        self.default_entry.is_match(below(&dir, path))
    }

    /// Whether skif makes `path` an entry of its project from the manifest and the defaults alone: a default entry file
    /// or the source of a published path ([`Self::publishes_source_of`]). Entries from plugins and configuration are
    /// not known here.
    fn is_entry_module(&self, path: &str) -> bool {
        self.is_default_entry(path) || self.publishes_source_of(path)
    }

    /// The same layout, with the build directories of each package from `build_dirs`.
    fn with_build_dirs(self, build_dirs: &'a BuildDirReader<'a>) -> Self {
        Self { build_dirs, ..self }
    }

    /// Whether `dir` holds a project manifest, as skif's file discovery decides it: an unreadable manifest or one with
    /// a field besides [`MARKER_FIELDS`]. A marker-only manifest (`{"type": "module"}`) belongs to the package above
    /// it, so it is never the owner of a finding.
    fn is_project(&self, dir: &str) -> bool {
        (self.manifest)(dir).is_some_and(|manifest| {
            manifest
                .as_object()
                .is_none_or(|fields| fields.keys().any(|key| !MARKER_FIELDS.contains(&key.as_str())))
        })
    }

    /// Whether `dir` is one of knip's workspaces: the root or a member.
    fn is_workspace(&self, dir: &str) -> bool {
        dir.is_empty() || self.members.contains(dir)
    }

    /// The directory of the deepest project manifest above `path` when it is not one of knip's workspaces.
    fn non_member_project(&self, path: &str) -> Option<String> {
        let dir = owner_dir(path, &|dir: &str| self.is_project(dir));
        (!self.is_workspace(&dir)).then_some(dir)
    }

    /// Whether the manifest of `dir` lists `name` in one of its [`DEPENDENCY_FIELDS`].
    fn lists(&self, dir: &str, name: &str) -> bool {
        (self.manifest)(dir).is_some_and(|manifest| lists_package(&manifest, name))
    }

    /// Whether a project manifest at or above `path` and below the workspace knip charges the file's imports to (the
    /// deepest workspace above it) lists `name`.
    fn listed_below_workspace(&self, path: &str, name: &str) -> bool {
        let is_project = |dir: &str| self.is_project(dir);
        let mut dir = owner_dir(path, &is_project);
        while !self.is_workspace(&dir) {
            if self.lists(&dir, name) {
                return true;
            }
            dir = owner_dir(&dir, &is_project);
        }
        false
    }

    /// Whether the directory `dir` lies below a workspace member.
    fn in_member(&self, dir: &str) -> bool {
        self.members.iter().any(|member| dir.starts_with(&format!("{member}/")))
    }

    /// Whether a directory of `path` is one skif's file discovery skips by name: [`SKIPPED_DIRECTORIES`] (those of
    /// [`PACKAGE_ROOT_ONLY_DIRECTORIES`] directly in the root or a package directory) and hidden directories other
    /// than [`TOOL_DIRECTORIES`], unless the directory is a workspace member. A skipped directory that holds members
    /// (`.vscode/extensions/*`) is walked for those members only, so a file there counts as skipped unless it lies
    /// in one of them.
    fn in_skipped_directory(&self, path: &str) -> bool {
        let Some((directories, _)) = path.rsplit_once('/') else {
            return false;
        };
        let mut parent = "";
        let mut start = 0;
        for end in directories
            .match_indices('/')
            .map(|(at, _)| at)
            .chain([directories.len()])
        {
            let name = &directories[start..end];
            let dir = &directories[..end];
            let by_name =
                SKIPPED_DIRECTORIES.contains(&name) || (name.starts_with('.') && !TOOL_DIRECTORIES.contains(&name));
            let here = !PACKAGE_ROOT_ONLY_DIRECTORIES.contains(&name) || parent.is_empty() || self.is_project(parent);
            if by_name && here && !self.members.contains(dir) && !self.in_member_below(dir, path) {
                return true;
            }
            parent = dir;
            start = end + 1;
        }
        false
    }

    /// Whether `path` lies in a workspace member below the directory `dir`.
    fn in_member_below(&self, dir: &str, path: &str) -> bool {
        let prefix = format!("{dir}/");
        self.members
            .iter()
            .any(|member| member.starts_with(&prefix) && path.starts_with(&format!("{member}/")))
    }

    /// Whether `path` is a TypeScript file whose package publishes its path with a built extension: a published path
    /// with `*` whose pattern it matches ([`is_published_twin`]), or one without `*` that skif maps to it.
    fn publishes_source_of(&self, path: &str) -> bool {
        let Some((stem, extension)) = typescript_source(path) else {
            return false;
        };
        let dir = owner_dir(path, &|dir: &str| self.is_project(dir));
        let Some(manifest) = (self.manifest)(&dir) else {
            return false;
        };
        let relative = if dir.is_empty() {
            Some(stem)
        } else {
            stem.strip_prefix(dir.as_str()).and_then(|rest| rest.strip_prefix('/'))
        };
        relative.is_some_and(|stem| {
            let source = format!("{stem}.{extension}");
            published_paths(&manifest).iter().any(|target| {
                if target.contains('*') {
                    is_published_twin(target, stem, extension)
                } else {
                    self.source_of(&dir, target).is_some_and(|found| found == source)
                }
            })
        })
    }

    /// Whether `path`, a declaration file or a code file that is no TypeScript source (`.js`, `.mjs`, `.cjs`, `.jsx`),
    /// is published as it is by the manifest of its project: a path of [`published_paths`] that names it (`*`
    /// matching any part of the path), or an entry of `files` that names it or matches it with `*`. A directory that
    /// `files` lists publishes nothing here.
    fn publishes_file(&self, path: &str) -> bool {
        if typescript_source(path).is_some() || !has_code_extension(path) {
            return false;
        }
        let dir = owner_dir(path, &|dir: &str| self.is_project(dir));
        let Some(manifest) = (self.manifest)(&dir) else {
            return false;
        };
        let relative = below(&dir, path);
        let mut targets = published_paths(&manifest);
        targets.extend(
            manifest
                .get("files")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned),
        );
        targets
            .iter()
            .filter_map(|target| join_relative("", target))
            .any(|target| {
                if target.contains('*') {
                    wildcard_match(&target, relative)
                } else {
                    target == relative
                }
            })
    }

    /// The source, relative to the package directory `dir`, that skif maps the published path `target` (without `*`)
    /// to, as `map_published` in `skif-js` picks it: through the package's build directories first (a path below an
    /// `outDir` names the same path below its `rootDir`, [`build_source`]), then by the `dist` to `src` convention,
    /// each time the first of `skif_js::source_candidates` that exists.
    fn source_of(&self, dir: &str, target: &str) -> Option<String> {
        let first = |published: &str| {
            source_candidates(published).into_iter().find(|candidate| {
                let path = if dir.is_empty() {
                    candidate.clone()
                } else {
                    format!("{dir}/{candidate}")
                };
                (self.is_file)(&path)
            })
        };
        let clean = join_relative("", target).unwrap_or_default();
        (self.build_dirs)(dir)
            .iter()
            .filter_map(|(out_dir, root_dir)| build_source(out_dir, root_dir, &clean))
            .find_map(|source| first(&source))
            .or_else(|| first(target))
    }
}

/// The source path behind `rel`, a package-relative path inside the build output directory `out_dir`, below its
/// `root_dir` (`source_paths` in `skif-js`); `None` for a path outside `out_dir`.
fn build_source(out_dir: &str, root_dir: &str, rel: &str) -> Option<String> {
    if out_dir.is_empty() {
        return None;
    }
    let rest = rel.strip_prefix(out_dir)?;
    if !rest.is_empty() && !rest.starts_with('/') {
        return None;
    }
    Some(match (root_dir, rest.trim_start_matches('/')) {
        ("", rest) => rest.to_owned(),
        (root, "") => root.to_owned(),
        (root, rest) => format!("{root}/{rest}"),
    })
}

/// Whether `path` names a declaration file (`.d.ts`, `.d.mts`, `.d.cts`).
fn is_declaration(path: &str) -> bool {
    declaration_stem(path).is_some()
}

/// The path of a declaration file without its declaration extension ([`DECLARATION_EXTENSIONS`]); `None` for another
/// file.
fn declaration_stem(path: &str) -> Option<&str> {
    DECLARATION_EXTENSIONS
        .iter()
        .find_map(|extension| path.strip_suffix(extension))
}

/// Whether the last segment of an `include` pattern names a file: it has a stem and a code extension.
fn has_input_extension(segment: &str) -> bool {
    segment.rsplit_once('.').is_some_and(|(stem, extension)| {
        !stem.is_empty() && CODE_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
    })
}

/// The `rootDir` TypeScript uses when a `tsconfig` sets none, as skif's TypeScript plugin approximates it: the longest
/// common directory of the `include` patterns (each up to its first wildcard, a file pattern without its name) and the
/// directories of the `files`, declaration files left out. `None` without either, or when one leaves the directory.
fn inferred_root_dir(config: &Value) -> Option<String> {
    let strings = |field: &str| {
        let mut found = Vec::new();
        if let Some(value) = config.get(field) {
            collect_strings(value, &mut found);
        }
        found
    };
    let mut roots = Vec::new();
    for pattern in strings("include").iter().filter(|pattern| !is_declaration(pattern)) {
        let segments: Vec<&str> = pattern.split('/').collect();
        let directory = match segments
            .iter()
            .position(|segment| segment.contains(['*', '?', '{', '[']))
        {
            Some(at) => &segments[..at],
            None if segments.last().is_some_and(|last| has_input_extension(last)) => &segments[..segments.len() - 1],
            None => &segments[..],
        };
        roots.push(join_relative("", &directory.join("/"))?);
    }
    for file in strings("files").iter().filter(|file| !is_declaration(file)) {
        roots.push(join_relative("", parent_dir(file))?);
    }
    let (first, rest) = roots.split_first()?;
    let mut common: Vec<&str> = first.split('/').filter(|segment| !segment.is_empty()).collect();
    for root in rest {
        let shared = common
            .iter()
            .zip(root.split('/').filter(|segment| !segment.is_empty()))
            .take_while(|&(ours, theirs)| *ours == theirs)
            .count();
        common.truncate(shared);
    }
    Some(common.join("/"))
}

/// The build directory a `tsconfig` declares, `(out_dir, root_dir)` relative to its directory, as skif's TypeScript
/// plugin reads it: `compilerOptions.outDir` with `rootDir`, or without one the root [`inferred_root_dir`] gives.
/// `None` without `outDir`, without a root, for a path that leaves the directory, or for a file that is not JSON with
/// comments.
fn tsconfig_build_dir(text: &str) -> Option<(String, String)> {
    let config: Value = serde_json::from_str(&strip_jsonc(text)).ok()?;
    let options = config.get("compilerOptions")?;
    let out_dir = join_relative("", options.get("outDir")?.as_str()?)?;
    let root_dir = match options.get("rootDir").and_then(Value::as_str) {
        Some(root_dir) => join_relative("", root_dir)?,
        None => inferred_root_dir(&config)?,
    };
    (!out_dir.is_empty()).then_some((out_dir, root_dir))
}

/// Reads the build directories of the package directories of a checkout, each once: those of the `tsconfig.json`,
/// `tsconfig.*.json` and `jsconfig.json` files directly in the directory ([`tsconfig_build_dir`]), in file name order.
/// A configuration another one extends from elsewhere is not read.
fn build_dir_reader(root: &Path) -> impl Fn(&str) -> Vec<(String, String)> + '_ {
    let cache: RefCell<BTreeMap<String, Vec<(String, String)>>> = RefCell::default();
    move |dir: &str| {
        if let Some(known) = cache.borrow().get(dir) {
            return known.clone();
        }
        let package = root.join(dir);
        let names: BTreeSet<String> = fs::read_dir(&package)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| name == "jsconfig.json" || (name.starts_with("tsconfig.") && is_json_file(name)))
            .collect();
        let found: Vec<(String, String)> = names
            .iter()
            .filter_map(|name| fs::read_to_string(package.join(name)).ok())
            .filter_map(|text| tsconfig_build_dir(&text))
            .collect();
        cache.borrow_mut().insert(dir.to_owned(), found.clone());
        found
    }
}

/// A `files`, `include` or `exclude` list of a TypeScript configuration, with the directory of the configuration that
/// sets it, which its entries are relative to.
type LocatedEntries = (String, Vec<String>);

/// What a TypeScript configuration sets of the files of its program, itself or through the configurations it
/// `extends`: each list with the directory of the configuration that sets it, and the `outDir`, relative to the
/// checkout.
#[derive(Debug, Default)]
struct ProgramSelection {
    files: Option<LocatedEntries>,
    include: Option<LocatedEntries>,
    exclude: Option<LocatedEntries>,
    out_dir: Option<String>,
}

/// What the configuration file `config` sets of `files`, `include`, `exclude` and `outDir`, itself or through the
/// configurations it `extends` (a later entry of an `extends` list over an earlier one, the configuration itself over
/// all of them), up to [`MAX_EXTENDS`] links, as knip reads them with `get-tsconfig`; `config_dir` is the directory of
/// the configuration being resolved. Nothing for a file that cannot be read or is not JSON with comments.
fn program_selection(
    config: &str,
    config_dir: &str,
    (read, is_file): (&SourceReader<'_>, &FileProbe<'_>),
    depth: usize,
) -> ProgramSelection {
    let Some(value) = read(config).and_then(|text| serde_json::from_str::<Value>(&strip_jsonc(&text)).ok()) else {
        return ProgramSelection::default();
    };
    let dir = parent_dir(config);
    let mut selection = ProgramSelection::default();
    let mut extends = Vec::new();
    if let Some(entries) = value.get("extends") {
        collect_strings(entries, &mut extends);
    }
    if depth < MAX_EXTENDS {
        for entry in &extends {
            if let Some(file) = extended_config(dir, entry, is_file) {
                let inherited = program_selection(&file, config_dir, (read, is_file), depth + 1);
                selection.files = inherited.files.or(selection.files);
                selection.include = inherited.include.or(selection.include);
                selection.exclude = inherited.exclude.or(selection.exclude);
                selection.out_dir = inherited.out_dir.or(selection.out_dir);
            }
        }
    }
    let located = |key: &str| {
        value.get(key).map(|entries| {
            let mut found = Vec::new();
            collect_strings(entries, &mut found);
            (dir.to_owned(), found)
        })
    };
    selection.files = located("files").or(selection.files);
    selection.include = located("include").or(selection.include);
    selection.exclude = located("exclude").or(selection.exclude);
    if let Some(out_dir) = value
        .get("compilerOptions")
        .and_then(|options| options.get("outDir"))
        .and_then(Value::as_str)
        .and_then(|out_dir| config_path(dir, config_dir, out_dir))
    {
        selection.out_dir = Some(out_dir);
    }
    selection
}

/// An entry of a `files`, `include` or `exclude` list of the configuration in `dir` as a path or glob relative to the
/// checkout, as knip's `loadTSConfig` resolves it: with `expand_dirs`, an entry without a wildcard whose last segment
/// has no extension names a directory and stands for everything below it. `None` for an absolute entry or one that
/// leaves the checkout.
fn program_entry(dir: &str, config_dir: &str, entry: &str, expand_dirs: bool) -> Option<String> {
    if entry.starts_with('/') {
        return None;
    }
    let path = config_path(dir, config_dir, entry)?;
    let trimmed = entry.trim_end_matches('/');
    let last = trimmed.rsplit('/').next().unwrap_or(trimmed);
    let names_directory = !entry.contains(['*', '?']) && (last == "." || last == ".." || !last.contains('.'));
    Some(if expand_dirs && names_directory {
        joined(&path, "**/*")
    } else {
        path
    })
}

/// The declaration files among `files` that the `tsconfig.json` in the directory `dir` compiles, as knip's
/// `loadTSConfig` selects them ([`program_selection`]): the paths `files` lists, and the files the `include` globs
/// match (`**/*` below `dir` when neither `include` nor `files` is set) that no `exclude` glob, `node_modules` below
/// `dir` or the `outDir` matches, and that have no hidden segment, which knip's globs skip. Empty without a
/// `tsconfig.json`.
fn program_declarations(
    dir: &str,
    files: &[String],
    (read, is_file): (&SourceReader<'_>, &FileProbe<'_>),
) -> Vec<String> {
    let config = joined(dir, "tsconfig.json");
    if !is_file(&config) {
        return Vec::new();
    }
    let selection = program_selection(&config, dir, (read, is_file), 0);
    let resolved = |located: &LocatedEntries, expand_dirs: bool| -> Vec<String> {
        let (base, entries) = located;
        entries
            .iter()
            .filter_map(|entry| program_entry(base, dir, entry, expand_dirs))
            .collect()
    };
    let listed: BTreeSet<String> = selection
        .files
        .as_ref()
        .map_or_default(|located| resolved(located, false).into_iter().collect());
    let include = match (&selection.include, &selection.files) {
        (Some(located), _) => resolved(located, true),
        (None, None) => vec![joined(dir, "**/*")],
        (None, Some(_)) => Vec::new(),
    };
    let exclude = selection
        .exclude
        .as_ref()
        .map_or_default(|located| resolved(located, true))
        .into_iter()
        .chain(std::iter::once(joined(dir, "node_modules/**")))
        .chain(selection.out_dir.iter().map(|out_dir| joined(out_dir, "**")))
        .map(|glob| format!("!{glob}"));
    let globs = IncompleteScopeGlobs::new(include.into_iter().chain(exclude).collect());
    files
        .iter()
        .filter(|file| is_declaration(file))
        .filter(|file| {
            listed.contains(*file) || (!file.split('/').any(|part| part.starts_with('.')) && globs.is_match(file))
        })
        .cloned()
        .collect()
}

/// The declaration files among `files` that the `tsconfig.json` of one of knip's workspaces (the root and the members
/// with a manifest, as knip analyses them) compiles ([`program_declarations`]), which knip makes production entries.
fn workspace_declarations(files: &[String], layout: &Layout<'_>, read: &SourceReader<'_>) -> BTreeSet<String> {
    std::iter::once(String::new())
        .chain(layout.members.iter().cloned())
        .filter(|dir| (layout.manifest)(dir).is_some())
        .flat_map(|dir| program_declarations(&dir, files, (read, layout.is_file)))
        .collect()
}

/// The paths a module specifier of the file `importer` may name, relative to the checkout and without the extension
/// resolution adds: a relative specifier joined to the importer's directory, each target that the `paths` of the
/// importer's TypeScript configuration map a bare one to, and the subpath of a workspace package it names.
fn specifier_paths(resolver: &Resolver<'_>, importer: &str, specifier: &str) -> Vec<String> {
    if is_relative_specifier(specifier) {
        return join_relative(parent_dir(importer), specifier).into_iter().collect();
    }
    let Some(name) = bare_package(specifier).filter(|_| !specifier.contains('?')) else {
        return Vec::new();
    };
    let mut found: Vec<String> = resolver.paths_of(parent_dir(importer)).map_or_default(|map| {
        map.targets(specifier)
            .iter()
            .filter_map(|target| join_relative("", target))
            .collect()
    });
    if let Some(dir) = resolver.packages.get(name) {
        found.push(joined(dir, specifier[name.len()..].trim_start_matches('/')));
    }
    found
}

/// Whether a specifier that names `path` ([`specifier_paths`]) imports the declaration file `declaration`, as
/// TypeScript resolves it: the path itself, the path without a built extension or as it is (`./api.js` and `./api` for
/// `api.d.ts`), or the `index` of the directory it names.
fn names_declaration(path: &str, declaration: &str) -> bool {
    let Some(stem) = declaration_stem(declaration) else {
        return false;
    };
    let without_extension = BUILT_EXTENSIONS
        .iter()
        .find_map(|extension| path.strip_suffix(extension))
        .unwrap_or(path);
    path == declaration || without_extension == stem || joined(path, "index") == stem
}

/// The declaration files of `declarations` that no code file of `files` but themselves imports: a specifier the
/// resolver resolves names the file it resolves to, which TypeScript prefers to a declaration file beside it
/// (`index.ts` over `index.d.ts`), and one it does not resolve names the paths of [`specifier_paths`] as
/// [`names_declaration`] matches them. Only the files whose text names the file name of a declaration file without its extension,
/// or its directory's name for an `index` file, are parsed; `read` gives a file's source, and a file it cannot read or
/// parse imports nothing.
fn unimported_declarations(
    declarations: &BTreeSet<String>,
    files: &[String],
    resolver: &Resolver<'_>,
    read: &SourceReader<'_>,
) -> BTreeSet<String> {
    let names: BTreeSet<&str> = declarations
        .iter()
        .filter_map(|declaration| {
            let stem = declaration_stem(declaration)?;
            let name = stem.rsplit('/').next().unwrap_or(stem);
            Some(if name == "index" {
                parent_dir(stem).rsplit('/').next().unwrap_or("")
            } else {
                name
            })
        })
        .filter(|name| !name.is_empty())
        .collect();
    let options = ParseOptions {
        in_file_references: false,
        member_accesses: false,
    };
    let mut imported = BTreeSet::new();
    if !names.is_empty() {
        for file in files {
            let Some(source) = read(file) else { continue };
            if !names.iter().any(|name| source.contains(name)) {
                continue;
            }
            let Ok(module) = parse_module(Path::new(file), &source, &options) else {
                continue;
            };
            for import in &module.imports {
                let paths = match resolver.resolve(file, &import.specifier) {
                    Some(target) => vec![target],
                    None => specifier_paths(resolver, file, &import.specifier),
                };
                for path in paths {
                    imported.extend(
                        declarations
                            .iter()
                            .filter(|declaration| *declaration != file && names_declaration(&path, declaration)),
                    );
                }
            }
        }
    }
    declarations
        .iter()
        .filter(|declaration| !imported.contains(declaration))
        .cloned()
        .collect()
}

/// Decides which known deviation, if any, a one-sided finding is.
struct Classifier<'a> {
    knip_unread: &'a [String],
    include_test_files: bool,
    /// The packages skif did not judge and what it left unjudged in each (`not_judged_dirs`).
    not_judged: &'a BTreeMap<String, NotJudgedScope>,
    /// The packages holding a configuration knip could not load.
    knip_failed: &'a BTreeSet<String>,
    /// The `package.json` files knip reports any finding on.
    knip_manifests: &'a BTreeSet<String>,
    /// The `unlisted` keys whose import only another package's tsconfig `paths` map (ADR 0019).
    foreign_paths: &'a BTreeSet<String>,
    /// The files knip reports as unused, before any are set apart.
    knip_unused_files: &'a BTreeSet<String>,
    /// The files skif reports as unused.
    skif_unused_files: &'a BTreeSet<String>,
    /// `<manifest>|<package>` for every package a file only knip lists as unused imports, with the manifest of the
    /// file's workspace (`unused_file_imports`).
    knip_unused_imports: &'a BTreeSet<String>,
    /// The same for the files only skif reports as unused.
    skif_unused_imports: &'a BTreeSet<String>,
    /// The package directories of the roots of a recursive skif run (`skif_root_packages`).
    skif_root_packages: &'a BTreeSet<String>,
    /// The files only knip reports as unused that a published source reaches (`published_reach`).
    published_reach: &'a BTreeSet<String>,
    /// `<file>|<export>` for every export that a file only knip lists as unused imports (`unused_file_credits`).
    knip_unused_credits: &'a BTreeSet<String>,
    /// The same for the files only skif reports as unused.
    skif_unused_credits: &'a BTreeSet<String>,
    /// `<manifest>|<package>` for every dependency a `BUILD` file credits, with their peers (`bazel_credits`,
    /// `with_peer_credits`).
    bazel_credits: &'a BTreeSet<String>,
    /// The `<file>|<export>` keys only knip reports whose export a used export's signature names (`signature_types`).
    signature_types: &'a BTreeSet<String>,
    /// The `unlisted` keys only knip reports whose import a `declare module` of the checkout declares
    /// (`ambient_module_keys`).
    ambient_modules: &'a BTreeSet<String>,
    /// The `unlisted` keys only knip reports whose import the importer's own tsconfig `paths` map (`own_paths_keys`).
    own_paths: &'a BTreeSet<String>,
    /// `<manifest>|<package>` and the files of the imports tsconfig `paths` map before the package
    /// (`paths_before_packages`).
    paths_before_packages: &'a BTreeSet<String>,
    /// The `enumMembers` keys only knip reports whose enum is used as a whole (`whole_enum_members`).
    whole_enum_members: &'a BTreeSet<String>,
    /// `<file>|<export>` for every export a published source re-exports (`entry_reexports`).
    published_reexports: &'a BTreeSet<String>,
    /// The same for the entry files (default and plugin ones, [`entry_modules`]) of the projects that are not knip
    /// workspaces and that a recursive skif run lints as root packages.
    root_entry_reexports: &'a BTreeSet<String>,
    /// The files only knip reports unused that are Bazel build sources or that those reach (`reach_from_sources`).
    bazel_sources: &'a BTreeSet<String>,
    /// `<file>|<export>` for every export the files below a project that is not a knip workspace import
    /// (`import_credits`).
    non_member_credits: &'a BTreeSet<String>,
    /// The same for the files below a directory skif's file discovery skips.
    skipped_credits: &'a BTreeSet<String>,
    /// The enum and namespace member keys only skif reports that the files below a project that is not a knip
    /// workspace use (`outside_members`).
    non_member_members: &'a BTreeSet<String>,
    /// The same for the files below a directory skif's file discovery skips.
    skipped_members: &'a BTreeSet<String>,
    /// The directories below which knip reads no file for a `./` pattern of a nested `.gitignore`, each with the path
    /// it ignores at any depth (`dot_slash_ignores`).
    knip_dot_ignores: &'a [(String, Vec<String>)],
    /// The `duplicates` keys only skif reports whose aliases knip takes for re-exports (`reexport_alias_duplicates`).
    reexport_aliases: &'a BTreeSet<String>,
    /// The declaration files the `tsconfig.json` of one of knip's workspaces compiles (`tsconfig_declarations`).
    tsconfig_declarations: &'a BTreeSet<String>,
    /// Those of them that hold an unlisted dependency or unresolved import only knip reports and that no code file
    /// imports (`unimported_declarations`).
    unimported_declarations: &'a BTreeSet<String>,
    /// The files only skif reports unused that one of `tsconfig_declarations` reaches by relative imports (`reach`).
    declaration_reach: &'a BTreeSet<String>,
    /// The `types` and `nsTypes` keys only skif reports whose file an import uses as a whole module object
    /// (`module_objects`).
    module_object_types: &'a BTreeSet<String>,
    /// The projects without installed packages that own a one-sided finding (`uninstalled_projects`).
    uninstalled_projects: &'a BTreeSet<String>,
    /// The enum and namespace member keys only knip reports that a file only knip reports unused uses
    /// (`members_used_by`).
    knip_unused_members: &'a BTreeSet<String>,
    /// `<manifest>|<package>` for every package a file that a `./` pattern of a nested `.gitignore` hides from knip
    /// imports, with their peers (`unused_file_imports`, `with_peer_credits`).
    knip_dot_ignore_imports: &'a BTreeSet<String>,
    /// `<file>|<export>` for every export such a file imports (`unused_file_credits`).
    knip_dot_ignore_credits: &'a BTreeSet<String>,
    /// `<manifest>|<package>` for every dependency skif's Heft support credits (`heft_credits`).
    heft_credits: &'a BTreeSet<String>,
    /// `<file>|*` for every file a `require` wrapped in `as` loads (`type_asserted_require_credits`).
    type_asserted_requires: &'a BTreeSet<String>,
    /// The `exports` and `nsExports` keys only knip reports whose file an import uses as a whole module object
    /// (`module_objects`).
    module_object_values: &'a BTreeSet<String>,
    /// The `exports` and `nsExports` keys only knip reports that a default import of their `CommonJS` file reads
    /// (`commonjs_default_members`).
    commonjs_default_members: &'a BTreeSet<String>,
    /// The files only knip reports unused that a template literal `require` or `import()` matches or that those reach
    /// (`template_import_targets`, `reach_from_sources`).
    template_import_reach: &'a BTreeSet<String>,
    /// The files only knip reports unused that an installed `main` imports or that those reach
    /// (`installed_main_imports`, `reach_from_sources`).
    installed_main_reach: &'a BTreeSet<String>,
    /// The enum and namespace member keys only skif reports whose export an importer refers to as a type
    /// (`type_referenced_members`).
    type_referenced_members: &'a BTreeSet<String>,
    /// The files only skif reports unused that an `import.meta.glob` reading text matches or that those reach
    /// (`raw_glob_targets`, `reach_from_sources`).
    raw_glob_reach: &'a BTreeSet<String>,
    /// The JavaScript files only skif reports unused that an import names beside a TypeScript twin
    /// (`typescript_twin_files`).
    typescript_twins: &'a BTreeSet<String>,
    /// The facts of the deviations that follow from how knip reads a module ([`KnipReadSets`]).
    knip_reads: &'a KnipReadSets,
    /// The deviations that follow from how knip reads dependencies, imports and commands ([`DependencyReads`]).
    dependency_reads: &'a DependencyReads,
    /// knip ran with the scratch root manifest of a Rush repository (`knip-root = "rush"`).
    rush_root: bool,
    /// knip ran with the harness's empty configuration instead of the repository's own.
    replaced_knip_config: bool,
    layout: &'a Layout<'a>,
}

impl Classifier<'_> {
    fn classify(&self, side: Side, category: &str, key: &str) -> Option<&'static str> {
        if self.knip_unread.iter().any(|prefix| key.starts_with(prefix.as_str())) {
            return Some(KNIP_UNREAD);
        }
        let path = key_path(key);
        let is_package = |dir: &str| self.layout.is_project(dir);
        // A configuration knip could not load changes what it reads in the whole package, so both sides are affected.
        let config_error = || {
            self.knip_failed
                .contains(&owner_dir(key, &is_package))
                .then_some(KNIP_CONFIG_ERROR)
        };
        if side == Side::Skif {
            return skif_only_deviation(category, key, self.foreign_paths)
                .or_else(|| self.unjudged_manifest(category, path).then_some(KNIP_UNJUDGED_MANIFEST))
                .or_else(|| {
                    (!MANIFEST_CATEGORIES.contains(&category) && in_dot_slash_ignore(self.knip_dot_ignores, path))
                        .then_some(KNIP_DOT_IGNORE)
                })
                .or_else(|| {
                    (!MANIFEST_CATEGORIES.contains(&category) && named_like_node_modules(path))
                        .then_some(KNIP_NODE_MODULES_NAME)
                })
                .or_else(|| (category == "duplicates" && self.reexport_aliases.contains(key)).then_some(REEXPORT_ALIAS))
                .or_else(|| {
                    self.skif_non_member_project(category, path)
                        .then_some(NON_MEMBER_PROJECT)
                })
                .or_else(|| (category == "unlisted" && is_build_file(path)).then_some(BAZEL_BUILD_FILE))
                .or_else(|| self.dependency_reads.deviation(side, category, key))
                .or_else(|| in_unused_file(category, path, self.knip_unused_files).then_some(IN_KNIP_UNUSED_FILE))
                .or_else(|| {
                    used_in_unused_file(self.skif_unused_imports, category, key).then_some(USED_IN_SKIF_UNUSED_FILE)
                })
                .or_else(|| {
                    imported_by_unused_file(self.skif_unused_credits, category, key)
                        .then_some(IMPORTED_BY_SKIF_UNUSED_FILE)
                })
                .or_else(|| {
                    (IMPORTED_CATEGORIES.contains(&category) && self.paths_before_packages.contains(key))
                        .then_some(PATHS_BEFORE_PACKAGES)
                })
                .or_else(|| self.outside_use(category, key))
                .or_else(|| {
                    self.tsconfig_declaration(side, category, key)
                        .then_some(TSCONFIG_DECLARATION_FILE)
                })
                .or_else(|| {
                    (TYPE_CATEGORIES.contains(&category) && self.module_object_types.contains(key))
                        .then_some(MODULE_OBJECT_TYPES)
                })
                .or_else(|| self.knip_read_credit(category, key))
                .or_else(|| {
                    (MEMBER_CATEGORIES.contains(&category) && self.type_referenced_members.contains(key))
                        .then_some(TYPE_REFERENCED_MEMBERS)
                })
                .or_else(|| (category == "files" && self.raw_glob_reach.contains(key)).then_some(RAW_GLOB_READ))
                .or_else(|| (category == "files" && self.typescript_twins.contains(key)).then_some(TYPESCRIPT_TWIN))
                .or_else(|| self.uninstalled(category, key).then_some(UNINSTALLED_PROJECT))
                .or_else(config_error);
        }
        (category == "files")
            .then(|| classify_file(key, self.include_test_files))
            .flatten()
            .or_else(|| self.replaced_knip_config(category, key).then_some(REPLACED_KNIP_CONFIG))
            .or_else(|| {
                (category != "files" && FIXTURE_CATEGORIES.contains(&category) && in_fixture_directory(path))
                    .then_some(FIXTURE_FILE)
            })
            .or_else(|| (self.rush_root && owner_dir(key, &is_package).is_empty()).then_some(RUSH_ROOT_FILE))
            .or_else(|| self.binary_deviation(category, key))
            .or_else(|| self.unlisted_deviation(category, key))
            .or_else(|| self.dependency_reads.deviation(side, category, key))
            .or_else(|| not_judged(category, key, self.not_judged, &is_package).then_some(NOT_JUDGED))
            .or_else(|| self.layout.in_skipped_directory(path).then_some(SKIPPED_DIRECTORY))
            .or_else(|| self.non_member_project(category, key).then_some(NON_MEMBER_PROJECT))
            .or_else(|| self.published_source(category, key).then_some(PUBLISHED_SOURCE))
            .or_else(|| self.published_file(category, path).then_some(PUBLISHED_FILE))
            .or_else(|| {
                (EXPORT_LABELS.contains(&category) && credited(&self.knip_reads.ng_package_reexports, key))
                    .then_some(NG_PACKAGE_ENTRY)
            })
            .or_else(|| {
                (category == "duplicates" && self.layout.is_entry_module(path)).then_some(ENTRY_MODULE_DUPLICATE)
            })
            .or_else(|| in_unused_file(category, path, self.skif_unused_files).then_some(IN_SKIF_UNUSED_FILE))
            .or_else(|| {
                used_in_unused_file(self.knip_unused_imports, category, key).then_some(USED_IN_KNIP_UNUSED_FILE)
            })
            .or_else(|| {
                (category == "files" && self.paths_before_packages.contains(key)).then_some(PATHS_BEFORE_PACKAGES)
            })
            .or_else(|| {
                let credited = IMPORTED_CATEGORIES.contains(&category) && self.bazel_credits.contains(key);
                let built = category == "files" && self.bazel_sources.contains(key);
                (credited || built).then_some(BAZEL_BUILD_FILE)
            })
            .or_else(|| {
                let member = MEMBER_CATEGORIES.contains(&category) && self.knip_unused_members.contains(key);
                (member || imported_by_unused_file(self.knip_unused_credits, category, key))
                    .then_some(IMPORTED_BY_KNIP_UNUSED_FILE)
            })
            .or_else(|| {
                (used_in_unused_file(self.knip_dot_ignore_imports, category, key)
                    || imported_by_unused_file(self.knip_dot_ignore_credits, category, key))
                .then_some(KNIP_DOT_IGNORE)
            })
            .or_else(|| {
                (IMPORTED_CATEGORIES.contains(&category) && self.heft_credits.contains(key)).then_some(HEFT_CONFIG)
            })
            .or_else(|| self.unread_load(category, key))
            .or_else(|| {
                (EXPORT_LABELS.contains(&category) && credited(&self.knip_reads.import_types, key))
                    .then_some(IMPORT_TYPE_QUALIFIER)
            })
            .or_else(|| {
                (EXPORT_LABELS.contains(&category) && self.signature_types.contains(key)).then_some(SIGNATURE_TYPE)
            })
            .or_else(|| (category == "enumMembers" && self.whole_enum_members.contains(key)).then_some(WHOLE_ENUM_USE))
            .or_else(|| {
                self.tsconfig_declaration(side, category, key)
                    .then_some(TSCONFIG_DECLARATION_FILE)
            })
            .or_else(|| self.uninstalled(category, key).then_some(UNINSTALLED_PROJECT))
            .or_else(config_error)
    }

    /// The deviation an export or type only skif reports is when knip credits it through a read skif makes otherwise
    /// ([`KnipReadSets`]): an opaque or bound `import()`, a `jest.requireActual`, namespace imports only spy calls
    /// read, a namespace import's name held by an array literal or `Object.keys`, a namespace re-export, a `CommonJS`
    /// export of a file that is no JavaScript file, or a file knip's `node` plugin makes an entry.
    fn knip_read_credit(&self, category: &str, key: &str) -> Option<&'static str> {
        if !EXPORT_LABELS.contains(&category) {
            return None;
        }
        let reads = self.knip_reads;
        let value = VALUE_EXPORT_CATEGORIES.contains(&category);
        [
            (&reads.opaque_imports, OPAQUE_DYNAMIC_IMPORT),
            (&reads.import_defaults, DYNAMIC_IMPORT_DEFAULT),
            (&reads.required_actuals, JEST_REQUIRE_ACTUAL),
            (&reads.spied_namespaces, SPIED_NAMESPACE),
            (&reads.enumerated_namespaces, ENUMERATED_NAMESPACE),
        ]
        .into_iter()
        .find_map(|(credits, id)| credited(credits, key).then_some(id))
        .or_else(|| (value && reads.namespace_reexports.contains(key)).then_some(NAMESPACE_REEXPORT))
        .or_else(|| (value && reads.typescript_commonjs.contains(key)).then_some(TYPESCRIPT_COMMONJS_EXPORT))
        .or_else(|| reads.node_test_files.contains(key_path(key)).then_some(NODE_TEST_ENTRY))
    }

    /// The deviation a finding only knip reports is when skif loads its file, or reads its export, through code knip
    /// reads no import from: a `require` wrapped in `as`, a whole use of the module object, a default import of a
    /// `CommonJS` module, a template literal `require` or `import()`, or an installed `main`.
    fn unread_load(&self, category: &str, key: &str) -> Option<&'static str> {
        if category == "files" {
            if self.template_import_reach.contains(key) {
                return Some(TEMPLATE_LITERAL_IMPORT);
            }
            return self.installed_main_reach.contains(key).then_some(INSTALLED_MAIN);
        }
        if !VALUE_EXPORT_CATEGORIES.contains(&category) {
            return None;
        }
        if credited(self.type_asserted_requires, key) {
            return Some(TYPE_ASSERTED_REQUIRE);
        }
        if self.commonjs_default_members.contains(key) {
            return Some(COMMONJS_DEFAULT_IMPORT);
        }
        self.module_object_values.contains(key).then_some(MODULE_OBJECT_VALUES)
    }

    /// Whether a one-sided finding follows from knip making the declaration files a workspace's `tsconfig.json`
    /// compiles production entries: on skif's side an export, type or member in such a file, or an unused file one of
    /// them reaches; on knip's side an unlisted dependency or unresolved import in one that no code file imports.
    fn tsconfig_declaration(&self, side: Side, category: &str, key: &str) -> bool {
        let path = key_path(key);
        match side {
            Side::Skif if category == "files" => self.declaration_reach.contains(path),
            Side::Skif => {
                ((EXPORT_LABELS.contains(&category) || MEMBER_CATEGORIES.contains(&category))
                    && self.tsconfig_declarations.contains(path))
                    || (EXPORT_LABELS.contains(&category) && credited(&self.knip_reads.declaration_reexports, key))
            }
            Side::Tool => IMPORT_CATEGORIES.contains(&category) && self.unimported_declarations.contains(path),
        }
    }

    /// The deviation a binary only knip reports on a `package.json` (`<manifest>|<binary>`) is: a command Rush provides
    /// in a Rush repository, a shell builtin, or a tool skif takes for installed on the machine.
    fn binary_deviation(&self, category: &str, key: &str) -> Option<&'static str> {
        if category != "binaries" {
            return None;
        }
        let (_, binary) = key.split_once('|')?;
        if RUSH_COMMANDS.contains(&binary) && (self.layout.is_file)(RUSH_JSON) {
            return Some(RUSH_PROVIDED_COMMAND);
        }
        if is_shell_builtin(binary) {
            return Some(SHELL_BUILTIN);
        }
        (IGNORED_BINARIES.contains(&binary) || is_machine_binary(binary)).then_some(GLOBAL_BINARY)
    }

    /// The deviation an unlisted dependency only knip reports (`<file>|<name>`) is: a `data:` specifier as written (cut
    /// at its colon, it is one of the [`DependencyReads`]), an import a `declare module` declares, or one the
    /// importer's own tsconfig `paths` map.
    fn unlisted_deviation(&self, category: &str, key: &str) -> Option<&'static str> {
        if category != "unlisted" {
            return None;
        }
        let (_, name) = key.split_once('|')?;
        if name.starts_with("data:") {
            return Some(DATA_URI);
        }
        if self.ambient_modules.contains(key) {
            return Some(AMBIENT_MODULE);
        }
        self.own_paths.contains(key).then_some(OWN_TSCONFIG_PATHS)
    }

    /// Whether an unused file only knip reports is a knip configuration file at the root of one of knip's workspaces,
    /// while knip ran with the harness's configuration instead.
    fn replaced_knip_config(&self, category: &str, key: &str) -> bool {
        let (dir, name) = key.rsplit_once('/').unwrap_or(("", key));
        category == "files"
            && self.replaced_knip_config
            && KNIP_CONFIG_FILES.contains(&name)
            && self.layout.is_workspace(dir)
    }

    /// Whether a finding only knip reports follows from a published source being an entry of skif: an unused file that
    /// is a published source or one a published source reaches, or an export or type ([`EXPORT_LABELS`]) of a
    /// published source or that one re-exports, which is public as an entry's export.
    fn published_source(&self, category: &str, key: &str) -> bool {
        let path = key_path(key);
        if category == "files" {
            return self.layout.publishes_source_of(path) || self.published_reach.contains(path);
        }
        EXPORT_LABELS.contains(&category)
            && (self.layout.publishes_source_of(path) || credited(self.published_reexports, key))
    }

    /// Whether an unused file, export or type only knip reports lies in a declaration or JavaScript file that its
    /// project's manifest publishes as it is ([`Layout::publishes_file`]).
    fn published_file(&self, category: &str, path: &str) -> bool {
        (category == "files" || EXPORT_LABELS.contains(&category)) && self.layout.publishes_file(path)
    }

    /// The deviation an export, type, duplicate or enum or namespace member only skif reports is when its only users
    /// lie where skif reads no file of the enclosing workspace: `non-member-project` for an import by a file below a
    /// project that is not one of knip's workspaces, `skipped-directory` for one by a file below a directory skif's
    /// discovery skips. A finding that lies in such a place itself stays compared.
    fn outside_use(&self, category: &str, key: &str) -> Option<&'static str> {
        let (non_member, skipped) = if IMPORTED_EXPORT_CATEGORIES.contains(&category) {
            (
                credited(self.non_member_credits, key),
                credited(self.skipped_credits, key),
            )
        } else if MEMBER_CATEGORIES.contains(&category) {
            (
                self.non_member_members.contains(key),
                self.skipped_members.contains(key),
            )
        } else {
            return None;
        };
        let path = key_path(key);
        if self.layout.non_member_project(path).is_some() || self.layout.in_skipped_directory(path) {
            return None;
        }
        if non_member {
            return Some(NON_MEMBER_PROJECT);
        }
        skipped.then_some(SKIPPED_DIRECTORY)
    }

    /// Whether a finding in [`INSTALLED_CATEGORIES`] lies in a project without installed packages
    /// ([`uninstalled_projects`]).
    fn uninstalled(&self, category: &str, key: &str) -> bool {
        INSTALLED_CATEGORIES.contains(&category)
            && self
                .uninstalled_projects
                .contains(&owner_dir(key, &|dir: &str| self.layout.is_project(dir)))
    }

    /// Whether a finding only skif reports sits on a `package.json` that is not one of knip's workspaces.
    fn unjudged_manifest(&self, category: &str, path: &str) -> bool {
        MANIFEST_CATEGORIES.contains(&category)
            && path
                .strip_suffix("/package.json")
                .is_some_and(|dir| !self.layout.is_workspace(dir))
            && !self.knip_manifests.contains(path)
    }

    /// Whether a finding only skif reports in [`NON_MEMBER_SKIF_CATEGORIES`] lies below a project directory that is not
    /// one of knip's workspaces; a binary on a `package.json` that knip reports anything on stays compared.
    fn skif_non_member_project(&self, category: &str, path: &str) -> bool {
        NON_MEMBER_SKIF_CATEGORIES.contains(&category)
            && !self.knip_manifests.contains(path)
            && self.layout.non_member_project(path).is_some()
    }

    /// Whether a finding only knip reports in a file (any category but the manifest ones) lies below a project
    /// directory that is not one of knip's workspaces; an unlisted dependency only when a project manifest between the
    /// file and the workspace knip charges it to lists it, since skif judges the project's imports against its
    /// manifests, or when a workspace member encloses the project, whose imports no skif run reads. In a project that
    /// a recursive skif run lints as a package of one of its roots, [`ROOT_COMPARED_CATEGORIES`] stay compared, except
    /// the exports and types of the project's entry files (its default entry files and those of skif's plugins there,
    /// [`entry_modules`]) and what they re-export, which are public there while knip reads the files without the
    /// project's entries.
    fn non_member_project(&self, category: &str, key: &str) -> bool {
        if MANIFEST_CATEGORIES.contains(&category) {
            return false;
        }
        let Some(dir) = self.layout.non_member_project(key_path(key)) else {
            return false;
        };
        if ROOT_COMPARED_CATEGORIES.contains(&category) && self.skif_root_packages.contains(&dir) {
            return EXPORT_LABELS.contains(&category)
                && (self.layout.is_default_entry(key_path(key)) || credited(self.root_entry_reexports, key));
        }
        category != "unlisted"
            || self.layout.in_member(&dir)
            || key
                .split_once('|')
                .is_some_and(|(path, name)| self.layout.listed_below_workspace(path, package_name(name)))
    }
}

/// Whether a finding inside a file (any category but `files` and the manifest ones) sits in a file the other tool
/// reports as unused: a tool analyses nothing inside a file it calls unused, so such a finding follows from the `files`
/// disagreement, which is counted once already. A dependency knip reports unused because only such a file imports it is
/// `used-in-knip-unused-file` (or `used-in-skif-unused-file`) instead.
fn in_unused_file(category: &str, path: &str, unused: &BTreeSet<String>) -> bool {
    category != "files" && !MANIFEST_CATEGORIES.contains(&category) && unused.contains(path)
}

/// Whether a dependency finding (`<manifest>|<package>`) names a package that `imports` (from [`unused_file_imports`])
/// holds for that manifest: one a file of its workspace that only one tool calls unused imports; an `@types/` package
/// also when that file imports its runtime package.
fn used_in_unused_file(imports: &BTreeSet<String>, category: &str, key: &str) -> bool {
    if !IMPORTED_CATEGORIES.contains(&category) {
        return false;
    }
    let Some((manifest, name)) = key.split_once('|') else {
        return false;
    };
    imports.contains(key)
        || name
            .strip_prefix("@types/")
            .is_some_and(|typed| imports.contains(&format!("{manifest}|{}", typed_package(typed))))
}

/// The package an `@types/` package types: `node` for `@types/node`, `@scope/name` for `@types/scope__name`.
fn typed_package(typed: &str) -> String {
    typed
        .split_once("__")
        .map_or_else(|| typed.to_owned(), |(scope, name)| format!("@{scope}/{name}"))
}

/// The package a module specifier names when it is a bare specifier: not relative, absolute, a subpath import (`#x`)
/// or a URL or protocol (`node:fs`, `data:`, `https://`).
fn bare_package(specifier: &str) -> Option<&str> {
    let first = specifier.split('/').next().unwrap_or(specifier);
    let local = specifier.is_empty() || specifier.starts_with(['.', '/', '#', '\\']) || first.contains(':');
    (!local).then(|| package_name(specifier))
}

/// The packages a source file imports by bare specifier, as skif's parser reads its imports, requires, re-exports,
/// dynamic imports and resolve calls (`require.resolve`, `import.meta.resolve`), which use the package they name;
/// globs and test-runner mocks are not imports of a package. Empty for a file it cannot parse.
fn imported_packages(path: &str, source: &str) -> BTreeSet<String> {
    bare_specifiers(path, source)
        .iter()
        .filter_map(|specifier| bare_package(specifier))
        .map(str::to_owned)
        .collect()
}

/// The bare specifiers of a source file ([`bare_package`]), in source order, as skif's parser reads its imports,
/// requires, re-exports, dynamic imports and resolve calls; globs and test-runner mocks are left out. Empty for a file
/// it cannot parse.
fn bare_specifiers(path: &str, source: &str) -> Vec<String> {
    let options = ParseOptions {
        in_file_references: false,
        member_accesses: false,
    };
    let Ok(module) = parse_module(Path::new(path), source, &options) else {
        return Vec::new();
    };
    module
        .imports
        .into_iter()
        .filter(|import| {
            !matches!(
                import.kind,
                ImportKind::ImportMetaGlob | ImportKind::SourceGlob | ImportKind::Mock
            ) && bare_package(&import.specifier).is_some()
        })
        .map(|import| import.specifier)
        .collect()
}

/// `<manifest>|<package>` for every package that one of the files only one tool lists as unused (`unused` but not
/// `other_unused`) imports, where `<manifest>` is the `package.json` of the file's workspace (`workspace_of`): what
/// that tool would have credited had it analysed the file, and the other tool did credit. A file both tools call
/// unused credits nothing on either side, so a dependency only it imports is a plain disagreement. `read` gives a
/// file's source; a file it cannot read imports nothing.
fn unused_file_imports(
    unused: &BTreeSet<String>,
    other_unused: &BTreeSet<String>,
    read: &dyn Fn(&str) -> Option<String>,
    workspace_of: &dyn Fn(&str) -> String,
) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    for file in unused.difference(other_unused) {
        let Some(source) = read(file) else { continue };
        let workspace = workspace_of(file);
        let manifest = if workspace.is_empty() {
            "package.json".to_owned()
        } else {
            format!("{workspace}/package.json")
        };
        keys.extend(
            imported_packages(file, &source)
                .into_iter()
                .map(|package| format!("{manifest}|{package}")),
        );
    }
    keys
}

/// Whether the file name of `path` has one of [`CODE_EXTENSIONS`].
fn has_code_extension(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.rsplit_once('.')
        .is_some_and(|(_, extension)| CODE_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str()))
}

/// The files of a checkout whose name `keep` accepts, by path relative to it with `/` separators, sorted: in every
/// directory but those of [`SKIPPED_DIRECTORIES`] (at any depth) and Bazel's `bazel-*` output trees in the root,
/// hidden directories included, never through a symbolic link. The harness scans the code files among them for the
/// imports and declarations elsewhere in the checkout that explain a finding, and the `.gitignore` files for the
/// patterns knip reads otherwise than git.
fn checkout_files(root: &Path, keep: &dyn Fn(&str) -> bool) -> Vec<String> {
    let mut found = Vec::new();
    let mut pending = vec![String::new()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = fs::read_dir(root.join(&dir)) else {
            continue;
        };
        for entry in entries.flatten() {
            let (Ok(name), Ok(kind)) = (entry.file_name().into_string(), entry.file_type()) else {
                continue;
            };
            let path = joined(&dir, &name);
            if kind.is_dir() {
                let skipped =
                    SKIPPED_DIRECTORIES.contains(&name.as_str()) || (dir.is_empty() && name.starts_with("bazel-"));
                if !skipped {
                    pending.push(path);
                }
            } else if kind.is_file() && keep(name.as_str()) {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// The modules the script files among `files` declare with `declare module 'name' { … }` (`ambient_modules` of skif's
/// parser): skif collects them from every file of the program and from the declaration files nothing imports; the
/// harness reads every file that holds `declare module`.
fn ambient_modules(files: &[String], read: &SourceReader<'_>) -> BTreeSet<String> {
    let options = ParseOptions {
        in_file_references: false,
        member_accesses: false,
    };
    files
        .iter()
        .filter_map(|file| {
            read(file)
                .filter(|source| source.contains("declare module"))
                .and_then(|source| parse_module(Path::new(file), &source, &options).ok())
        })
        .flat_map(|module| module.ambient_modules)
        .collect()
}

/// Whether the modules in `declared` ([`ambient_modules`]) declare the module `specifier` imports, as skif judges it
/// (`declares_module` in `crates/skif-js/src/analysis.rs`): the specifier itself, the package it names, or a pattern
/// with one `*` and a literal start that matches it.
fn declares_module(declared: &BTreeSet<String>, specifier: &str) -> bool {
    declared.contains(specifier)
        || bare_package(specifier).is_some_and(|package| declared.contains(package))
        || declared.iter().any(|module| {
            module.split_once('*').is_some_and(|(prefix, suffix)| {
                !prefix.is_empty()
                    && !suffix.contains('*')
                    && specifier.len() >= prefix.len() + suffix.len()
                    && specifier.starts_with(prefix)
                    && specifier.ends_with(suffix)
            })
        })
}

/// The specifiers by which the file of an unlisted-dependency key (`<file>|<name>`) imports `name`: those of its
/// [`bare_specifiers`] that equal it or name the same package. The name itself when the file cannot be read or holds
/// none.
fn unlisted_specifiers(key: &str, read: &SourceReader<'_>) -> Vec<String> {
    let Some((path, name)) = key.split_once('|') else {
        return Vec::new();
    };
    let package = bare_package(name);
    let found: Vec<String> = read(path)
        .map_or_default(|source| bare_specifiers(path, &source))
        .into_iter()
        .filter(|specifier| specifier == name || (package.is_some() && bare_package(specifier) == package))
        .collect();
    if found.is_empty() { vec![name.to_owned()] } else { found }
}

/// The keys of `unlisted` (`<file>|<name>`) whose import of the package a `declare module` among `files` declares
/// ([`ambient_modules`], [`declares_module`]): skif counts such an import for no manifest (ADR 0019). The files are
/// read only when `unlisted` holds a key.
fn ambient_module_keys(unlisted: &BTreeSet<String>, files: &[String], read: &SourceReader<'_>) -> BTreeSet<String> {
    if unlisted.is_empty() {
        return BTreeSet::new();
    }
    let declared = ambient_modules(files, read);
    if declared.is_empty() {
        return BTreeSet::new();
    }
    unlisted
        .iter()
        .filter(|key| {
            unlisted_specifiers(key, read)
                .iter()
                .any(|specifier| declares_module(&declared, specifier))
        })
        .cloned()
        .collect()
}

/// The keys of `unlisted` (`<file>|<name>`) whose import of the package the `paths` of the importing file's own
/// TypeScript configuration map to a file of the checkout outside `node_modules` ([`Resolver::paths_target`]), or to
/// generated code `is_ignored` accepts ([`Resolver::generated_paths_target`]): skif resolves such an import to the
/// workspace source, or takes the target for generated code, and counts it for no manifest (ADR 0019).
fn own_paths_keys(
    unlisted: &BTreeSet<String>,
    resolver: &Resolver<'_>,
    read: &SourceReader<'_>,
    is_ignored: &FileProbe<'_>,
) -> BTreeSet<String> {
    unlisted
        .iter()
        .filter(|key| {
            let path = key_path(key);
            unlisted_specifiers(key, read).iter().any(|specifier| {
                resolver.paths_target(path, specifier).is_some()
                    || resolver.generated_paths_target(path, specifier, is_ignored)
            })
        })
        .cloned()
        .collect()
}

/// `<manifest>|<package>` for each dependency that a file of `files` imports by a specifier the `paths` of its
/// TypeScript configuration map to a file of the same workspace ([`Resolver::paths_target`]), where `<manifest>` is
/// the `package.json` of that workspace and lists the package, and that file: TypeScript resolves such an import
/// through `paths` before it looks for the package, so skif counts it as a use of the file and not of the dependency
/// (ADR 0013). A mapping into another workspace is an import of that package. Only files whose configuration sets
/// `paths` and whose text holds the literal start of one of its patterns are parsed.
fn paths_before_packages(
    files: &[String],
    resolver: &Resolver<'_>,
    layout: &Layout<'_>,
    read: &SourceReader<'_>,
) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for file in files {
        let Some(map) = resolver.paths_of(parent_dir(file)) else {
            continue;
        };
        let Some(source) = read(file) else { continue };
        if !map.prefixes().any(|prefix| source.contains(prefix)) {
            continue;
        }
        let workspace = owner_dir(file, &|dir: &str| layout.is_workspace(dir));
        for specifier in bare_specifiers(file, &source) {
            let Some(package) = bare_package(&specifier) else {
                continue;
            };
            if !layout.lists(&workspace, package) {
                continue;
            }
            let Some(target) = resolver.paths_target(file, &specifier) else {
                continue;
            };
            if owner_dir(&target, &|dir: &str| layout.is_workspace(dir)) == workspace {
                found.insert(format!("{}|{package}", joined(&workspace, "package.json")));
                found.insert(target);
            }
        }
    }
    found
}

/// The exported names each file has findings for in [`EXPORT_LABELS`], by file.
fn export_names(keys: &Keys) -> BTreeMap<&str, BTreeSet<&str>> {
    let mut names: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for category in EXPORT_LABELS {
        for key in keys.get(category).into_iter().flatten() {
            if let Some((file, name)) = key.split_once('|') {
                names.entry(file).or_default().insert(name);
            }
        }
    }
    names
}

/// Whether an export of `module` named `name` appears in the signature of another of its exports that `used` accepts,
/// directly or through exports in the signatures of such ones (`referenced_in`, as `signature_use` in `skif-js` follows
/// it), skipping the names in `visited`.
fn in_used_signature<'m>(
    module: &'m ModuleInfo,
    name: &str,
    used: &dyn Fn(&str) -> bool,
    visited: &mut BTreeSet<&'m str>,
) -> bool {
    module
        .exports
        .iter()
        .filter(|export| export.name == name)
        .flat_map(|export| &export.referenced_in)
        .any(|container| {
            used(container)
                || (visited.insert(container.as_str()) && in_used_signature(module, container, used, visited))
        })
}

/// `<file>|<export>` for every export only knip reports in [`EXPORT_LABELS`] that the signature of another export of
/// the file names when skif does not report that one as unused ([`in_used_signature`]): skif counts it as used
/// (ADR 0018). Only the files with such findings are parsed; `read` gives a file's source, and a file it cannot read
/// or parse yields nothing.
fn signature_types(skif: &Keys, knip: &Keys, read: &dyn Fn(&str) -> Option<String>) -> BTreeSet<String> {
    let options = ParseOptions {
        in_file_references: true,
        member_accesses: false,
    };
    let skif_names = export_names(skif);
    let none = BTreeSet::new();
    let mut found = BTreeSet::new();
    for (file, names) in export_names(knip) {
        let reported = skif_names.get(file).unwrap_or(&none);
        let only: Vec<&str> = names.difference(reported).copied().collect();
        if only.is_empty() {
            continue;
        }
        let Some(module) = read(file).and_then(|source| parse_module(Path::new(file), &source, &options).ok()) else {
            continue;
        };
        let used = |container: &str| !reported.contains(container);
        for name in only {
            if in_used_signature(&module, name, &used, &mut BTreeSet::new()) {
                found.insert(format!("{file}|{name}"));
            }
        }
    }
    found
}

/// The paths knip ignores below the directory of a nested `.gitignore` while git ignores nothing for them, from each
/// `(path, text)` of `gitignores`: a line `./name` or `./name/` (several `/`-separated segments allowed, no wildcard)
/// is a path git matches nowhere, since no path has a `.` segment, and that knip, which joins every pattern of a
/// nested `.gitignore` to its directory, ignores as `name` at any depth below the directory. Each entry is the
/// directory and the segments of the ignored path. A `.gitignore` in the checkout root, a negation and a pattern with a
/// wildcard name nothing.
fn dot_slash_ignores(gitignores: &[(String, String)]) -> Vec<(String, Vec<String>)> {
    let mut found = Vec::new();
    for (path, text) in gitignores {
        let Some((dir, _)) = path.rsplit_once('/') else {
            continue;
        };
        for line in text.lines() {
            let Some(pattern) = line.trim().strip_prefix("./") else {
                continue;
            };
            let pattern = pattern.trim_end_matches('/');
            if pattern.is_empty() || pattern.contains(['*', '?', '[', '\\']) {
                continue;
            }
            let segments: Vec<String> = pattern.split('/').map(str::to_owned).collect();
            if segments
                .iter()
                .any(|segment| segment.is_empty() || segment == "." || segment == "..")
            {
                continue;
            }
            found.push((dir.to_owned(), segments));
        }
    }
    found
}

/// Whether one of `ignores` ([`dot_slash_ignores`]) hides `path` from knip: the path lies below the entry's directory
/// and its segments there hold the ignored segments in a row, as a directory above the file or as the file itself.
fn in_dot_slash_ignore(ignores: &[(String, Vec<String>)], path: &str) -> bool {
    ignores.iter().any(|(dir, segments)| {
        let Some(rest) = path.strip_prefix(dir.as_str()).and_then(|rest| rest.strip_prefix('/')) else {
            return false;
        };
        let parts: Vec<&str> = rest.split('/').collect();
        parts
            .windows(segments.len())
            .any(|window| window.iter().zip(segments).all(|(part, segment)| *part == segment))
    })
}

/// The keys of `duplicates` (`<file>|<names>`, the duplicates only skif reports) whose group knip records no alias for
/// ([`knip_records_no_alias`]), read with skif's parser; a file it cannot read or parse is left out.
fn reexport_alias_duplicates(duplicates: &BTreeSet<String>, read: &SourceReader<'_>) -> BTreeSet<String> {
    let options = ParseOptions {
        in_file_references: false,
        member_accesses: false,
    };
    duplicates
        .iter()
        .filter(|key| {
            let Some((file, names)) = key.split_once('|') else {
                return false;
            };
            let names: BTreeSet<&str> = names.split('|').collect();
            read(file)
                .and_then(|source| parse_module(Path::new(file), &source, &options).ok())
                .is_some_and(|module| knip_records_no_alias(&module, &names))
        })
        .cloned()
        .collect()
}

/// Whether knip sees no duplicate among the exports `names` of `module`: none is `default`, at least one is an alias
/// (`export const b = a`) of another of them, and every such alias names an export initialised with a binding the
/// module imports (`export const a = importedA`), which knip takes for a re-export and records no alias of.
fn knip_records_no_alias(module: &ModuleInfo, names: &BTreeSet<&str>) -> bool {
    if names.contains("default") {
        return false;
    }
    let imported: BTreeSet<&str> = module
        .imports
        .iter()
        .flat_map(|import| &import.names)
        .filter_map(|name| name.local.as_deref())
        .collect();
    let group: BTreeMap<&str, &Export> = module
        .exports
        .iter()
        .filter(|export| names.contains(export.name.as_str()))
        .map(|export| (export.name.as_str(), export))
        .collect();
    let mut aliases = 0;
    for export in group.values() {
        let Some(target) = export.alias_of.as_deref().and_then(|name| group.get(name)) else {
            continue;
        };
        if !target.alias_of.as_deref().is_some_and(|name| imported.contains(name)) {
            return false;
        }
        aliases += 1;
    }
    aliases > 0
}

/// The `(file, export)` pairs that the parsed file `path` uses as a whole value through the imports `resolver`
/// resolves, as skif credits every member of such a use (`ExportUsage` in `crates/skif-js/src/usage.rs`): a named or
/// default import that escapes (passed, stored, put in an object literal, given to a decorator), a member of a
/// namespace import read in a way that is not a read of its own members (`{ E: ns.E }`, `typeof ns.E`), and as
/// [`WHOLE_MODULE`] a namespace used as a whole, `import()` and `require`. A re-export uses nothing itself.
fn whole_uses(path: &str, module: &ModuleInfo, resolver: &Resolver<'_>) -> Vec<(String, String)> {
    let mut found = Vec::new();
    for (import, target) in module_targets(path, module, resolver) {
        if matches!(import.kind, ImportKind::ExportFrom) {
            continue;
        }
        if import.names.is_empty() {
            if matches!(import.kind, ImportKind::Dynamic | ImportKind::Require) {
                found.push((target, WHOLE_MODULE.to_owned()));
            }
            continue;
        }
        for name in &import.names {
            match &name.kind {
                ImportedKind::Named(export) if name.escapes => found.push((target.clone(), export.clone())),
                ImportedKind::Default if name.escapes => found.push((target.clone(), "default".to_owned())),
                ImportedKind::Namespace if name.local.is_some() && !name.escapes => found.extend(
                    name.accessed_members
                        .iter()
                        .filter(|member| !name.nested_members.contains_key(*member))
                        .map(|member| (target.clone(), member.clone())),
                ),
                ImportedKind::Namespace => found.push((target.clone(), WHOLE_MODULE.to_owned())),
                ImportedKind::Named(_) | ImportedKind::Default => {}
            }
        }
    }
    found
}

/// The `(file, use)` pairs of the members of other files' exports that the parsed file `path` uses through the imports
/// `resolver` resolves: the whole uses of [`whole_uses`], and as `<export>.<member>` a member read through a named or
/// default import that does not escape (`E.A`) or through a member of a namespace import (`ns.E.A`).
fn member_uses(path: &str, module: &ModuleInfo, resolver: &Resolver<'_>) -> Vec<(String, String)> {
    let mut found = whole_uses(path, module, resolver);
    for (import, target) in module_targets(path, module, resolver) {
        if matches!(import.kind, ImportKind::ExportFrom) {
            continue;
        }
        for name in import.names.iter().filter(|name| name.local.is_some() && !name.escapes) {
            let reads: Vec<(&str, &String)> = match &name.kind {
                ImportedKind::Named(export) => name
                    .accessed_members
                    .iter()
                    .map(|member| (export.as_str(), member))
                    .collect(),
                ImportedKind::Default => name.accessed_members.iter().map(|member| ("default", member)).collect(),
                ImportedKind::Namespace => name
                    .nested_members
                    .iter()
                    .flat_map(|(export, members)| members.iter().map(move |member| (export.as_str(), member)))
                    .collect(),
            };
            found.extend(
                reads
                    .into_iter()
                    .map(|(export, member)| (target.clone(), format!("{export}.{member}"))),
            );
        }
    }
    found
}

/// The keys of `members` (`<file>|<member>`, enum and namespace members only skif reports) that `credits` (from
/// [`member_uses`], followed through re-exports) uses: the file as a whole, or an enum or namespace export of the file
/// that has the member, used as a whole or through that member. `read` gives a file's source; a file it cannot read or
/// parse has no member.
fn credited_members(
    members: &BTreeSet<String>,
    credits: &BTreeSet<String>,
    read: &SourceReader<'_>,
) -> BTreeSet<String> {
    let options = ParseOptions {
        in_file_references: false,
        member_accesses: false,
    };
    let mut modules: BTreeMap<&str, Option<ModuleInfo>> = BTreeMap::new();
    let mut found = BTreeSet::new();
    for key in members {
        let Some((path, member)) = key.split_once('|') else {
            continue;
        };
        let module = modules
            .entry(path)
            .or_insert_with(|| read(path).and_then(|source| parse_module(Path::new(path), &source, &options).ok()));
        let used = credits.contains(&format!("{path}|{WHOLE_MODULE}"))
            || module.as_ref().is_some_and(|module| {
                module.exports.iter().any(|export| {
                    export
                        .enum_members
                        .iter()
                        .chain(&export.namespace_members)
                        .any(|known| known.name == member)
                        && (credits.contains(&format!("{path}|{}", export.name))
                            || credits.contains(&format!("{path}|{}.{member}", export.name)))
                })
            });
        if used {
            found.insert(key.clone());
        }
    }
    found
}

/// The keys of `members` (enum and namespace members only skif reports) whose members the files of `files` use
/// ([`member_uses`], followed through re-exports) from where skif's analysis of a workspace reads no file:
/// `(non_member, skipped)` as in [`outside_credits`]. A file `read` cannot read or parse uses nothing.
fn outside_members(
    members: &BTreeSet<String>,
    files: &[String],
    layout: &Layout<'_>,
    resolver: &Resolver<'_>,
    read: &SourceReader<'_>,
) -> (BTreeSet<String>, BTreeSet<String>) {
    let (non_member, skipped) = outside_importers(files, layout);
    (
        members_used_by(members, non_member, resolver, read),
        members_used_by(members, skipped, resolver, read),
    )
}

/// The keys of `members` (`<file>|<member>`, enum and namespace members) that the files of `importers` use
/// ([`member_uses`], followed through re-exports, [`credited_members`]). A file `read` cannot read or parse uses
/// nothing.
fn members_used_by<'f>(
    members: &BTreeSet<String>,
    importers: impl IntoIterator<Item = &'f str>,
    resolver: &Resolver<'_>,
    read: &SourceReader<'_>,
) -> BTreeSet<String> {
    let options = ParseOptions {
        in_file_references: false,
        member_accesses: true,
    };
    let seeds = importers
        .into_iter()
        .filter_map(|file| {
            read(file)
                .and_then(|source| parse_module(Path::new(file), &source, &options).ok())
                .map(|module| member_uses(file, &module, resolver))
        })
        .flatten()
        .collect();
    credited_members(members, &follow_reexports(seeds, resolver, read), read)
}

/// The enum exports of a file: the name, the member names, and whether the file uses the enum as a whole itself
/// (`used_whole_in_file`).
type FileEnums = Vec<(String, Vec<String>, bool)>;

/// The keys of `members` (`<file>|<member>`, enum members only knip reports) whose enum skif's parser finds used as a
/// whole value, which ADR 0018 counts as a use of every member: in its own file (`used_whole_in_file`), or by a file of
/// `files` ([`whole_uses`]), directly or through the re-exports of the files in between ([`follow_reexports`]). Only
/// the files whose text names one of the enums are parsed for their imports; `read` gives a file's source.
fn whole_enum_members(
    members: &BTreeSet<String>,
    files: &[String],
    resolver: &Resolver<'_>,
    read: &SourceReader<'_>,
) -> BTreeSet<String> {
    let mut enums: BTreeMap<&str, FileEnums> = BTreeMap::new();
    for key in members {
        let path = key_path(key);
        if enums.contains_key(path) {
            continue;
        }
        let found: FileEnums = read(path)
            .and_then(|source| parse_module(Path::new(path), &source, &ParseOptions::default()).ok())
            .map_or_default(|module| {
                module
                    .exports
                    .iter()
                    .filter(|export| export.kind == ExportKind::Enum)
                    .map(|export| {
                        let names = export.enum_members.iter().map(|member| member.name.clone()).collect();
                        (export.name.clone(), names, export.used_whole_in_file)
                    })
                    .collect()
            });
        enums.insert(path, found);
    }
    let wanted: BTreeSet<&str> = enums
        .values()
        .flatten()
        .filter(|(_, _, whole)| !whole)
        .map(|(name, _, _)| name.as_str())
        .collect();
    let options = ParseOptions {
        in_file_references: false,
        member_accesses: true,
    };
    let mut seeds = Vec::new();
    if !wanted.is_empty() {
        for file in files {
            let Some(source) = read(file) else { continue };
            if !wanted.iter().any(|name| source.contains(name)) {
                continue;
            }
            if let Ok(module) = parse_module(Path::new(file), &source, &options) {
                seeds.extend(whole_uses(file, &module, resolver));
            }
        }
    }
    let credits = follow_reexports(seeds, resolver, read);
    members
        .iter()
        .filter(|key| {
            let Some((path, member)) = key.split_once('|') else {
                return false;
            };
            enums.get(path).is_some_and(|found| {
                found.iter().any(|(name, names, whole)| {
                    names.iter().any(|known| known == member)
                        && (*whole
                            || credits.contains(&format!("{path}|{name}"))
                            || credits.contains(&format!("{path}|{WHOLE_MODULE}")))
                })
            })
        })
        .cloned()
        .collect()
}

/// `<file>|*` for every file that a code file of `files` uses as a whole module object ([`whole_uses`]: `import()`,
/// `require`, a namespace used as a whole), and for the exports such a file re-exports, followed through the files in
/// between ([`follow_reexports`]). Only the files holding one of [`WHOLE_USE_MARKERS`] are parsed; `read` gives a file's
/// source.
fn module_object_credits(files: &[String], resolver: &Resolver<'_>, read: &SourceReader<'_>) -> BTreeSet<String> {
    let options = ParseOptions {
        in_file_references: false,
        member_accesses: true,
    };
    let mut seeds = Vec::new();
    for file in files {
        let Some(source) = read(file) else { continue };
        if !WHOLE_USE_MARKERS.iter().any(|marker| source.contains(marker)) {
            continue;
        }
        if let Ok(module) = parse_module(Path::new(file), &source, &options) {
            seeds.extend(
                whole_uses(file, &module, resolver)
                    .into_iter()
                    .filter(|(_, name)| name == WHOLE_MODULE),
            );
        }
    }
    follow_reexports(seeds, resolver, read)
}

/// The keys of `keys` (`<file>|<export>`) that `credits` ([`module_object_credits`]) holds: their file is used as a
/// whole module object. On skif's side these are the types only skif reports, since knip credits every export of such a
/// module, types included, while skif's whole use reaches its values only (ADR 0018); on knip's side the value exports
/// only knip reports, a whole use knip does not record as one.
fn module_objects(keys: &BTreeSet<String>, credits: &BTreeSet<String>) -> BTreeSet<String> {
    keys.iter().filter(|key| credited(credits, key)).cloned().collect()
}

/// `<file>|*` for every file that a code file of `files` uses as a whole through a named or default import of a
/// namespace re-export that escapes ([`whole_uses`]: `export * as listSearch from './listSearch'` in `methods/index.ts`
/// and `methods = { listSearch }` after `import { listSearch } from './methods'`), and for the exports such a file
/// re-exports, followed through the files in between ([`follow_reexports`]). Only the files holding `export * as ` are
/// parsed for the re-exported names, and only the files whose text holds one of those names for their uses; `read`
/// gives a file's source.
fn namespace_reexport_object_credits(
    files: &[String],
    resolver: &Resolver<'_>,
    read: &SourceReader<'_>,
) -> BTreeSet<String> {
    let options = ParseOptions {
        in_file_references: false,
        member_accesses: true,
    };
    let parsed = |file: &str, marker: &dyn Fn(&str) -> bool| {
        read(file)
            .filter(|source| marker(source.as_str()))
            .and_then(|source| parse_module(Path::new(file), &source, &options).ok())
    };
    let names: BTreeSet<String> = files
        .iter()
        .filter_map(|file| parsed(file, &|source: &str| source.contains("export * as ")))
        .flat_map(|module| {
            module
                .exports
                .into_iter()
                .filter(|export| export.kind == ExportKind::StarReExport && export.name != WHOLE_MODULE)
                .map(|export| export.name)
        })
        .collect();
    if names.is_empty() {
        return BTreeSet::new();
    }
    let mentions_a_name = |source: &str| names.iter().any(|name| source.contains(name.as_str()));
    let seeds: Vec<(String, String)> = files
        .iter()
        .filter_map(|file| parsed(file, &mentions_a_name).map(|module| whole_uses(file, &module, resolver)))
        .flatten()
        .filter(|(_, name)| names.contains(name))
        .collect();
    follow_reexports(seeds, resolver, read)
        .into_iter()
        .filter(|credit| credit.ends_with(&format!("|{WHOLE_MODULE}")))
        .collect()
}

/// `<manifest>|<package>` for every dependency of the projects in `dirs` that skif's Heft support credits (ADR 0015),
/// with the peers of those ([`with_peer_credits`]): in a project where the `heft` plugin is enabled, what it references
/// ([`project_plugins`]), and what any plugin references from a file of a rig profile ([`in_rig_profile`]), such as the
/// `tslib` an `importHelpers` of a profile's `tsconfig-base.json` needs. `manifest` reads the `package.json` of a
/// directory; a directory without one credits nothing.
fn heft_credits<'d>(
    dirs: impl IntoIterator<Item = &'d str>,
    fs: &dyn FileSystem,
    root: &Path,
    manifest: &ManifestReader<'_>,
) -> BTreeSet<String> {
    let mut credits = BTreeSet::new();
    for dir in dirs {
        let Some(listed) = manifest(dir) else { continue };
        let abs_dir = root.join(dir);
        let listing = joined(dir, "package.json");
        let outputs = project_plugins(fs, root, (dir, dir.is_empty()), &listed, &[]);
        if !outputs.iter().any(|(id, _)| *id == heft::ID) {
            continue;
        }
        for (id, output) in outputs {
            for reference in output.referenced_dependencies {
                let from = reference
                    .from
                    .strip_prefix(&abs_dir)
                    .ok()
                    .and_then(Path::to_str)
                    .map(|relative| relative.replace('\\', "/"));
                if id == heft::ID || from.as_deref().is_some_and(in_rig_profile) {
                    credits.insert(format!("{listing}|{}", reference.name));
                }
            }
        }
    }
    with_peer_credits(credits, manifest)
}

/// Whether `relative`, a path below a project directory, lies in a profile of a rig: `profiles/<profile>/…` of the
/// project itself, a rig, or of an installed package, `node_modules/<rig>/profiles/<profile>/…`.
fn in_rig_profile(relative: &str) -> bool {
    let parts: Vec<&str> = relative.split('/').collect();
    let start = match parts.as_slice() {
        ["node_modules", scope, _, ..] if scope.starts_with('@') => 3,
        ["node_modules", _, ..] => 2,
        _ => 0,
    };
    matches!(parts.get(start..), Some(["profiles", profile, _, ..]) if !profile.is_empty())
}

/// The specifiers of the `require` calls in `text` whose result a TypeScript `as` asserts a type of
/// (`require('./x') as typeof import('./x')`), read from the text: `require(` not preceded by a name character, a
/// quoted specifier (also a template literal without a substitution, ``require(`./x`)``), `)` and `as` followed by
/// whitespace.
fn type_asserted_requires(text: &str) -> Vec<&str> {
    const CALL: &str = "require(";
    let mut found = Vec::new();
    for (at, _) in text.match_indices(CALL) {
        let named = text[..at]
            .chars()
            .next_back()
            .is_some_and(|before| before.is_alphanumeric() || matches!(before, '_' | '$' | '.'));
        if named {
            continue;
        }
        let rest = text[at + CALL.len()..].trim_start();
        let Some(quote) = rest.chars().next().filter(|c| matches!(*c, '\'' | '"' | '`')) else {
            continue;
        };
        let body = &rest[1..];
        let Some(end) = body.find(quote) else { continue };
        if body[..end].contains("${") {
            continue;
        }
        let asserted = body[end + 1..]
            .trim_start()
            .strip_prefix(')')
            .and_then(|after| after.trim_start().strip_prefix("as"))
            .is_some_and(|after| after.starts_with(char::is_whitespace));
        if asserted {
            found.push(&body[..end]);
        }
    }
    found
}

/// `<file>|*` for every file that a code file of `files` loads with a `require` whose result an `as` asserts a type of
/// ([`type_asserted_requires`]), resolved as `resolver` resolves a specifier, and for the exports such a file
/// re-exports ([`follow_reexports`]). Only the files holding `require(` are scanned; `read` gives a file's source.
fn type_asserted_require_credits(
    files: &[String],
    resolver: &Resolver<'_>,
    read: &SourceReader<'_>,
) -> BTreeSet<String> {
    let mut seeds = Vec::new();
    for file in files {
        let Some(source) = read(file) else { continue };
        if !source.contains("require(") {
            continue;
        }
        seeds.extend(
            type_asserted_requires(&source)
                .into_iter()
                .filter_map(|specifier| resolver.resolve(file, specifier))
                .map(|target| (target, WHOLE_MODULE.to_owned())),
        );
    }
    follow_reexports(seeds, resolver, read)
}

/// The files of `files` that a `require` or `import()` of a template literal with a relative start matches
/// ([`is_template_call`]): skif's parser reads each substitution as a `*` within one path segment (`./commands/${c}`
/// reads `./commands/*`). Only the files holding a call of a template literal (``(` ``) are parsed.
fn template_import_targets(files: &[String], read: &SourceReader<'_>) -> BTreeSet<String> {
    glob_targets(files, read, "(`", &|source, import| {
        import.kind == ImportKind::ImportMetaGlob && is_template_call(source, import)
    })
}

/// The files of `files` that an `import.meta.glob` reading the matched files as text (`query: '?raw'`,
/// [`ImportKind::SourceGlob`]) matches. Only the files holding `import.meta.glob` are parsed.
fn raw_glob_targets(files: &[String], read: &SourceReader<'_>) -> BTreeSet<String> {
    glob_targets(files, read, "import.meta.glob", &|_, import| {
        import.kind == ImportKind::SourceGlob
    })
}

/// The files of `files` that the glob imports `keep` takes (given the importer's text) of a file holding `marker`
/// match, as skif's graph expands a glob (`expand_glob` in `crates/skif-js/src/graph.rs`): the pattern joined to the
/// importer's directory, a last segment without an extension or wildcard also matching with any code extension, and
/// the importer itself left out. A rooted (`/x`) or negated pattern matches nothing here. `read` gives a file's source;
/// a file it cannot read or parse imports nothing.
fn glob_targets(
    files: &[String],
    read: &SourceReader<'_>,
    marker: &str,
    keep: &dyn Fn(&str, &Import) -> bool,
) -> BTreeSet<String> {
    let options = ParseOptions {
        in_file_references: false,
        member_accesses: false,
    };
    let mut found = BTreeSet::new();
    for file in files {
        let Some(source) = read(file) else { continue };
        if !source.contains(marker) {
            continue;
        }
        let Ok(module) = parse_module(Path::new(file), &source, &options) else {
            continue;
        };
        let patterns: Vec<String> = module
            .imports
            .iter()
            .filter(|import| keep(&source, import) && !import.specifier.starts_with(['/', '!']))
            .filter_map(|import| join_relative(parent_dir(file), &import.specifier))
            .flat_map(|joined| {
                let last = joined.rsplit('/').next().unwrap_or_default();
                let extensionless = (!last.is_empty() && !last.contains(['.', '*', '?', '[', '{']))
                    .then(|| format!("{joined}.{{{}}}", CODE_EXTENSIONS.join(",")));
                std::iter::once(joined).chain(extensionless)
            })
            .collect();
        if patterns.is_empty() {
            continue;
        }
        let globs = IncompleteScopeGlobs::new(patterns);
        found.extend(
            files
                .iter()
                .filter(|candidate| *candidate != file && globs.is_match(candidate))
                .cloned(),
        );
    }
    found
}

/// Whether the glob `import` of the file whose text is `source` is a `require` or `import()` call of a template
/// literal: its text starts with `require` or `import(` and holds a backtick, unlike an `import.meta.glob` pattern.
fn is_template_call(source: &str, import: &Import) -> bool {
    let (Ok(start), Ok(end)) = (usize::try_from(import.span.start), usize::try_from(import.span.end)) else {
        return false;
    };
    source.get(start..end).is_some_and(|text| {
        let call = text.trim_start();
        (call.starts_with("require") || call.starts_with("import(")) && call.contains('`')
    })
}

/// Whether `path` holds `node_modules` inside a name (`node_modules_in_sandbox.test.mjs`) and has no `node_modules`
/// directory: knip skips every path that contains the text (`isInNodeModules` in its `util/path.js`), skif only the
/// files below a `node_modules` directory.
fn named_like_node_modules(path: &str) -> bool {
    path.contains("node_modules") && !path.split('/').any(|segment| segment == "node_modules")
}

/// The files of `unused` (files only skif reports unused) that are JavaScript (`.js`, `.jsx`, `.mjs`, `.cjs`) beside a
/// TypeScript file of the same stem and that a code file of `files` names by a relative specifier with that extension
/// (`require('../sum.js')`, read as [`stub_specifiers`] reads them): skif resolves such a specifier in TypeScript's
/// order, to the TypeScript twin, while knip takes the file it names. Only the files whose text holds the name of such
/// a file are scanned; `read` gives a file's source.
fn typescript_twin_files(
    unused: &BTreeSet<&str>,
    files: &[String],
    is_file: &FileProbe<'_>,
    read: &SourceReader<'_>,
) -> BTreeSet<String> {
    let twinned: BTreeSet<&str> = unused
        .iter()
        .copied()
        .filter(|path| {
            BUILT_EXTENSIONS
                .iter()
                .find_map(|extension| path.strip_suffix(extension))
                .is_some_and(|stem| TYPESCRIPT_EXTENSIONS.iter().any(|ts| is_file(&format!("{stem}.{ts}"))))
        })
        .collect();
    if twinned.is_empty() {
        return BTreeSet::new();
    }
    let names: BTreeSet<&str> = twinned
        .iter()
        .map(|path| path.rsplit('/').next().unwrap_or(*path))
        .collect();
    let mut found = BTreeSet::new();
    for file in files {
        let Some(source) = read(file) else { continue };
        if !names.iter().any(|name| source.contains(name)) {
            continue;
        }
        for specifier in stub_specifiers(&source) {
            if let Some(target) = join_relative(parent_dir(file), &specifier)
                && twinned.contains(target.as_str())
            {
                found.insert(target);
            }
        }
    }
    found
}

/// Whether a default import of the parsed module binds its `module.exports` object: a `CommonJS` module without a
/// `default` export (`binds_module_exports` in `crates/skif-js/src/usage.rs`).
fn binds_module_exports(module: &ModuleInfo) -> bool {
    module.has_cjs_exports && !module.exports.iter().any(|export| export.name == "default")
}

/// The keys of `exports` (`<file>|<export>`, exports only knip reports) of a `CommonJS` file ([`binds_module_exports`])
/// that a code file of `files` reads as a property of a default import of the file that does not escape
/// (`import routing from './ci-routing.cjs'; const { a } = routing;`), as skif's parser records the member reads.
/// Only the files whose text names the stem of such a file are parsed; `read` gives a file's source.
fn commonjs_default_members(
    exports: &BTreeSet<String>,
    files: &[String],
    resolver: &Resolver<'_>,
    read: &SourceReader<'_>,
) -> BTreeSet<String> {
    let files_of: BTreeSet<&str> = exports.iter().map(|key| key_path(key)).collect();
    let targets: BTreeSet<&str> = files_of
        .into_iter()
        .filter(|file| {
            read(file)
                .and_then(|source| parse_module(Path::new(file), &source, &ParseOptions::default()).ok())
                .is_some_and(|module| binds_module_exports(&module))
        })
        .collect();
    if targets.is_empty() {
        return BTreeSet::new();
    }
    let stems: BTreeSet<&str> = targets
        .iter()
        .map(|target| {
            let name = target.rsplit('/').next().unwrap_or(*target);
            name.split('.').next().unwrap_or(name)
        })
        .collect();
    let options = ParseOptions {
        in_file_references: false,
        member_accesses: true,
    };
    let mut credits = BTreeSet::new();
    for file in files {
        let Some(source) = read(file) else { continue };
        if !stems.iter().any(|stem| source.contains(stem)) {
            continue;
        }
        let Ok(module) = parse_module(Path::new(file), &source, &options) else {
            continue;
        };
        for (import, target) in module_targets(file, &module, resolver) {
            if !targets.contains(target.as_str()) {
                continue;
            }
            for name in &import.names {
                if matches!(name.kind, ImportedKind::Default) && name.local.is_some() && !name.escapes {
                    credits.extend(name.accessed_members.iter().map(|member| format!("{target}|{member}")));
                }
            }
        }
    }
    exports.intersection(&credits).cloned().collect()
}

/// The files that the installed file each project of `dirs` names as its `main` below `node_modules`
/// (`node_modules/expo/AppEntry.js`) imports by relative path, joined to the path the manifest names
/// ([`relative_imports`]): skif makes the `main` an entry and follows its imports back into the project. A project
/// whose `main` lies elsewhere, or whose installed file `read` cannot read, gives none.
fn installed_main_imports<'d>(
    dirs: impl IntoIterator<Item = &'d str>,
    layout: &Layout<'_>,
    read: &SourceReader<'_>,
) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for dir in dirs {
        let Some(manifest) = (layout.manifest)(dir) else {
            continue;
        };
        let Some(main) = manifest.get("main").and_then(Value::as_str) else {
            continue;
        };
        let main = main.strip_prefix("./").unwrap_or(main);
        if !main.starts_with("node_modules/") {
            continue;
        }
        let path = joined(dir, main);
        if let Some(source) = read(&path) {
            found.extend(relative_imports(&path, &source, layout.is_file));
        }
    }
    found
}

/// The names a TypeScript file writes as a type reference (`x: E`, `Array<E>`) or a type query (`typeof E`) by a plain
/// identifier, as knip's `TSTypeReference` and `TSTypeQuery` visitors record a reference of an imported binding.
#[derive(Default)]
struct TypeReferenceNames {
    names: BTreeSet<String>,
}

impl<'a> Visit<'a> for TypeReferenceNames {
    fn visit_ts_type_reference(&mut self, it: &TSTypeReference<'a>) {
        if let TSTypeName::IdentifierReference(id) = &it.type_name {
            self.names.insert(id.name.as_str().to_owned());
        }
        walk::walk_ts_type_reference(self, it);
    }

    fn visit_ts_type_query(&mut self, it: &TSTypeQuery<'a>) {
        if let Some(TSTypeName::IdentifierReference(id)) = it.expr_name.as_ts_type_name() {
            self.names.insert(id.name.as_str().to_owned());
        }
        walk::walk_ts_type_query(self, it);
    }
}

/// The names the file `path` refers to as a type ([`TypeReferenceNames`]); empty for a file oxc has no source type
/// for.
fn type_reference_names(path: &str, source: &str) -> BTreeSet<String> {
    let Ok(source_type) = SourceType::from_path(path) else {
        return BTreeSet::new();
    };
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, source_type).parse();
    let mut visitor = TypeReferenceNames::default();
    visitor.visit_program(&parsed.program);
    visitor.names
}

/// The keys of `members` (`<file>|<member>`, enum and namespace members only skif reports) whose enum, or namespace
/// with exported members, a code file of `files` imports by name from the member's file itself, without an alias, and
/// refers to as a type ([`type_reference_names`]): knip skips the member findings of an export whose importers hold
/// such a reference while `nsTypes` is not reported (`getMemberIssues` in knip's `graph/analyze.js`). Only the files
/// whose text names one of the exports are parsed; `read` gives a file's source.
fn type_referenced_members(
    members: &BTreeSet<String>,
    files: &[String],
    resolver: &Resolver<'_>,
    read: &SourceReader<'_>,
) -> BTreeSet<String> {
    let mut holders: BTreeMap<&str, Vec<(String, Vec<String>)>> = BTreeMap::new();
    for key in members {
        let path = key_path(key);
        if holders.contains_key(path) {
            continue;
        }
        let found = read(path)
            .and_then(|source| parse_module(Path::new(path), &source, &ParseOptions::default()).ok())
            .map_or_default(|module| {
                module
                    .exports
                    .iter()
                    .filter_map(|export| {
                        let names: Vec<String> = export
                            .enum_members
                            .iter()
                            .chain(&export.namespace_members)
                            .map(|member| member.name.clone())
                            .collect();
                        (!names.is_empty()).then(|| (export.name.clone(), names))
                    })
                    .collect()
            });
        holders.insert(path, found);
    }
    let wanted: BTreeSet<&str> = holders.values().flatten().map(|(name, _)| name.as_str()).collect();
    if wanted.is_empty() {
        return BTreeSet::new();
    }
    let options = ParseOptions {
        in_file_references: false,
        member_accesses: false,
    };
    let mut referenced: BTreeSet<String> = BTreeSet::new();
    for file in files {
        let Some(source) = read(file) else { continue };
        if !wanted.iter().any(|name| source.contains(name)) {
            continue;
        }
        let Ok(module) = parse_module(Path::new(file), &source, &options) else {
            continue;
        };
        let imported: Vec<String> = module_targets(file, &module, resolver)
            .into_iter()
            .filter(|(_, target)| holders.contains_key(target.as_str()))
            .flat_map(|(import, target)| {
                import.names.iter().filter_map(move |name| match &name.kind {
                    ImportedKind::Named(export) if name.local.as_deref() == Some(export.as_str()) => {
                        Some(format!("{target}|{export}"))
                    }
                    _ => None,
                })
            })
            .collect();
        if imported.is_empty() {
            continue;
        }
        let types = type_reference_names(file, &source);
        referenced.extend(
            imported
                .into_iter()
                .filter(|key| key.rsplit_once('|').is_some_and(|(_, name)| types.contains(name))),
        );
    }
    members
        .iter()
        .filter(|key| {
            let Some((path, member)) = key.split_once('|') else {
                return false;
            };
            holders.get(path).is_some_and(|found| {
                found.iter().any(|(name, names)| {
                    names.iter().any(|known| known == member) && referenced.contains(&format!("{path}|{name}"))
                })
            })
        })
        .cloned()
        .collect()
}

/// The facts of the known deviations that follow from how knip reads a module where skif reads it otherwise
/// (ADR 0021). Each credit is `<file>|<export>` (`<file>|*` for every export of the file), followed through the
/// `export … from` and `export *` re-exports of the files in between ([`follow_reexports`]).
#[derive(Default)]
struct KnipReadSets {
    /// `<file>|*` for every file an `import()` that knip reads as opaque loads ([`knip_read_credits`]).
    opaque_imports: BTreeSet<String>,
    /// `<file>|default` for every file an `import()` that knip binds to one name loads.
    import_defaults: BTreeSet<String>,
    /// `<file>|*` for every file a `jest.requireActual` loads.
    required_actuals: BTreeSet<String>,
    /// `<file>|*` for every file whose namespace imports only spy calls read.
    spied_namespaces: BTreeSet<String>,
    /// `<file>|*` for every file a namespace import of which an array literal or `Object.keys` holds by its name.
    enumerated_namespaces: BTreeSet<String>,
    /// `<file>|<export>` for every export a qualified import type names (`import('./x').Name`).
    import_types: BTreeSet<String>,
    /// The `exports` and `nsExports` keys only skif reports that name an `export * as X from` of their file
    /// ([`namespace_reexport_keys`]).
    namespace_reexports: BTreeSet<String>,
    /// The `exports` and `nsExports` keys only skif reports that a `CommonJS` assignment of a file that is no
    /// JavaScript file exports ([`typescript_commonjs_keys`]).
    typescript_commonjs: BTreeSet<String>,
    /// `<file>|<export>` for every export a declaration file of `tsconfig_declarations` re-exports
    /// ([`entry_reexports`]).
    declaration_reexports: BTreeSet<String>,
    /// The files of exports and types only skif reports that knip's `node` plugin makes entries
    /// ([`node_test_files`]).
    node_test_files: BTreeSet<String>,
    /// `<file>|<export>` for every export an entry point an `ng_package` names re-exports, with `<entry>|*` for each
    /// ([`ng_package_entries`], [`entry_reexports`]).
    ng_package_reexports: BTreeSet<String>,
}

/// The methods of `Object` that knip's call visitor takes for a whole read of a namespace they are given.
const ENUMERATING_METHODS: [&str; 4] = ["keys", "values", "entries", "getOwnPropertyNames"];

/// The extensions of the files knip reads `CommonJS` export assignments in (`isJS` in its
/// `typescript/visitors/walk.js`).
const JAVASCRIPT_EXTENSIONS: [&str; 4] = ["js", "mjs", "cjs", "jsx"];

/// The texts a file must hold for [`knip_read_credits`] to parse it.
const KNIP_READ_MARKERS: [&str; 3] = ["import(", "requireActual", "* as "];

/// What knip's visitors take from one code file where skif's parser reads it otherwise, read with oxc as knip reads
/// its own oxc tree (parentheses kept): the `import()` expressions it binds to names and those it reads as opaque, the
/// `jest.requireActual` calls of its Jest plugin, the names spy calls, array literals and `Object.keys` take, the
/// qualified import types and the `CommonJS` export assignments.
#[derive(Default)]
struct KnipReads {
    /// The local names of `vi` imported from `vitest`, which knip's Vitest mock visitor reads.
    vi_names: BTreeSet<String>,
    /// The local names of `jest`, the global and the import from `@jest/globals`, which knip's Jest mock visitor reads.
    jest_names: BTreeSet<String>,
    /// The start offsets of the `import()` expressions knip binds: in a declarator, through `.then`, in an awaited
    /// `Promise.all` given to an array pattern, returned by a callback argument, or named by a `vi.mock` that it reads
    /// for side effects (`handledImportExpressions` in knip).
    bound: BTreeSet<u32>,
    /// The specifiers of the other `import()` expressions with a literal specifier, which knip reads as opaque.
    opaque: Vec<String>,
    /// The specifiers of the `import()` expressions knip binds to one name, which it records as an import of `default`.
    defaults: Vec<String>,
    /// The specifiers of the `jest.requireActual` calls, which knip's Jest plugin reads as opaque.
    required_actuals: Vec<String>,
    /// The names a spy call takes as its object (`vi.spyOn(ns, 'a')`, `spyOn(ns, 'a')`), with the members it spies on.
    spied: BTreeMap<String, BTreeSet<String>>,
    /// The names read as the object of a member expression (`ns.a`, `ns['a']`) or destructured (`const { a } = ns`).
    member_objects: BTreeSet<String>,
    /// The names an array literal holds or an `Object.keys` (`values`, `entries`, `getOwnPropertyNames`) call takes.
    enumerated: BTreeSet<String>,
    /// `(specifier, name)` for each qualified import type, `import('./x').Name` (also `.Name.Member`).
    import_types: Vec<(String, String)>,
    /// The names that `exports.x = …`, `module.exports.x = …` and `module.exports = { a, b }` (all properties
    /// shorthand) export, as skif's parser reads them.
    commonjs_exports: BTreeSet<String>,
}

impl KnipReads {
    /// Records the `import()` at `start` of `specifier` as bound, and as an import of `default` when `one_name` binds
    /// it to one name.
    fn bind(&mut self, start: u32, specifier: String, one_name: bool) {
        self.bound.insert(start);
        if one_name {
            self.defaults.push(specifier);
        }
    }

    /// What knip's `handleVariableDeclarator` binds: an `import()` or awaited `import()` given to an object pattern,
    /// or, awaited, to one name; each `import()` of an awaited `Promise.all([...])` given to an array pattern, before a
    /// spread; and a name destructured, whose members it reads.
    fn read_declarator(&mut self, id: &BindingPattern<'_>, init: &Expression<'_>) {
        let (inner, awaited) = match init {
            Expression::AwaitExpression(awaited) => (&awaited.argument, true),
            other => (other, false),
        };
        if let Some((start, specifier)) = literal_import(inner) {
            match id {
                BindingPattern::ObjectPattern(_) => self.bind(start, specifier, false),
                BindingPattern::BindingIdentifier(_) if awaited => self.bind(start, specifier, true),
                _ => {}
            }
            return;
        }
        if let (BindingPattern::ArrayPattern(pattern), true) = (id, awaited)
            && let Some(elements) = promise_all_elements(inner)
        {
            for (at, element) in elements.iter().enumerate() {
                if matches!(element, ArrayExpressionElement::SpreadElement(_)) {
                    break;
                }
                let Some((start, specifier)) = element.as_expression().and_then(literal_import) else {
                    continue;
                };
                let one_name = matches!(
                    pattern.elements.get(at),
                    Some(Some(BindingPattern::BindingIdentifier(_)))
                );
                self.bind(start, specifier, one_name);
            }
        }
        if let (BindingPattern::ObjectPattern(_), Some(name)) = (id, identifier_name(init)) {
            self.member_objects.insert(name.to_owned());
        }
    }

    /// What knip reads from a call: `import(x).then(callback)` with a callback whose first parameter is one name or an
    /// object pattern (`handleImportThen`), a `vi.mock(import(x), () => ({ … }))` it reads for side effects (its
    /// Vitest plugin), `jest.requireActual(x)` (its Jest plugin), the names `Object.keys` and its kin take, a spy call
    /// on a name, and a callback argument that returns an `import()` (a loader).
    fn read_call(&mut self, call: &CallExpression<'_>) {
        let argument = |at: usize| call.arguments.get(at).and_then(Argument::as_expression);
        if let Expression::StaticMemberExpression(callee) = &call.callee {
            let method = callee.property.name.as_str();
            let object = identifier_name(&callee.object);
            if method == "then"
                && let Some((start, specifier)) = literal_import(&callee.object)
                && let Some(one_name) = argument(0).and_then(callback_binding)
            {
                self.bind(start, specifier, one_name);
            }
            if method == "mock"
                && object.is_some_and(|name| self.vi_names.contains(name))
                && let Some((start, specifier)) = argument(0).and_then(literal_import)
                && argument(1).is_some_and(is_object_factory)
            {
                self.bind(start, specifier, false);
            }
            if method == "requireActual"
                && object.is_some_and(|name| self.jest_names.contains(name))
                && let Some(specifier) = argument(0).and_then(literal_text)
            {
                self.required_actuals.push(specifier);
            }
            if object == Some("Object") && ENUMERATING_METHODS.contains(&method) {
                self.enumerated.extend(
                    call.arguments
                        .iter()
                        .filter_map(Argument::as_expression)
                        .filter_map(identifier_name)
                        .map(str::to_owned),
                );
                return;
            }
        }
        if is_spy_callee(&call.callee)
            && let Some(object) = argument(0).and_then(identifier_name)
            && let Some(member) = argument(1).and_then(literal_text)
        {
            self.spied.entry(object.to_owned()).or_default().insert(member);
        }
        for loaded in call.arguments.iter().filter_map(|passed| match passed {
            Argument::ArrowFunctionExpression(arrow) => arrow.body.as_expression(),
            _ => None,
        }) {
            let returned = match loaded {
                Expression::AwaitExpression(awaited) => &awaited.argument,
                other => other,
            };
            if let Some((start, specifier)) = literal_import(returned) {
                self.bind(start, specifier, false);
            }
        }
    }
}

impl<'a> Visit<'a> for KnipReads {
    fn visit_variable_declarator(&mut self, it: &VariableDeclarator<'a>) {
        if let Some(init) = &it.init {
            self.read_declarator(&it.id, init);
        }
        walk::walk_variable_declarator(self, it);
    }

    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        self.read_call(it);
        walk::walk_call_expression(self, it);
    }

    fn visit_import_expression(&mut self, it: &ImportExpression<'a>) {
        if !self.bound.contains(&it.span.start)
            && let Some(specifier) = literal_text(&it.source)
        {
            self.opaque.push(specifier);
        }
        walk::walk_import_expression(self, it);
    }

    fn visit_array_expression(&mut self, it: &ArrayExpression<'a>) {
        self.enumerated.extend(
            it.elements
                .iter()
                .filter_map(ArrayExpressionElement::as_expression)
                .filter_map(identifier_name)
                .map(str::to_owned),
        );
        walk::walk_array_expression(self, it);
    }

    fn visit_static_member_expression(&mut self, it: &StaticMemberExpression<'a>) {
        if let Some(name) = identifier_name(&it.object) {
            self.member_objects.insert(name.to_owned());
        }
        walk::walk_static_member_expression(self, it);
    }

    fn visit_computed_member_expression(&mut self, it: &ComputedMemberExpression<'a>) {
        if let Some(name) = identifier_name(&it.object) {
            self.member_objects.insert(name.to_owned());
        }
        walk::walk_computed_member_expression(self, it);
    }

    fn visit_ts_import_type(&mut self, it: &TSImportType<'a>) {
        if let (Some(specifier), Some(qualifier)) = (it.source.value.as_str(), &it.qualifier) {
            self.import_types
                .push((specifier.to_owned(), qualifier_head(qualifier).to_owned()));
        }
        walk::walk_ts_import_type(self, it);
    }

    fn visit_assignment_expression(&mut self, it: &AssignmentExpression<'a>) {
        if it.operator == AssignmentOperator::Assign
            && let AssignmentTarget::StaticMemberExpression(target) = &it.left
        {
            self.commonjs_exports.extend(commonjs_names(target, &it.right));
        }
        walk::walk_assignment_expression(self, it);
    }
}

/// The name of `expr` when it is a plain identifier.
fn identifier_name<'e>(expr: &'e Expression<'_>) -> Option<&'e str> {
    match expr {
        Expression::Identifier(id) => Some(id.name.as_str()),
        _ => None,
    }
}

/// The text of a string literal, or of a template literal without a substitution, as knip's `isStringLiteral` and
/// `getStringValue` read one.
fn literal_text(expr: &Expression<'_>) -> Option<String> {
    match expr {
        Expression::StringLiteral(literal) => literal.value.as_str().map(str::to_owned),
        Expression::TemplateLiteral(template) if template.expressions.is_empty() => {
            template.quasis.first().map(|quasi| quasi.value.raw.as_str().to_owned())
        }
        _ => None,
    }
}

/// The start offset and the literal specifier of `expr` when it is an `import()` of one.
fn literal_import(expr: &Expression<'_>) -> Option<(u32, String)> {
    let Expression::ImportExpression(import) = expr else {
        return None;
    };
    literal_text(&import.source).map(|specifier| (import.span.start, specifier))
}

/// The elements of the array literal that `Promise.all([...])` takes when `expr` is such a call.
fn promise_all_elements<'e, 'a>(expr: &'e Expression<'a>) -> Option<&'e [ArrayExpressionElement<'a>]> {
    let Expression::CallExpression(call) = expr else {
        return None;
    };
    let Expression::StaticMemberExpression(callee) = &call.callee else {
        return None;
    };
    if identifier_name(&callee.object) != Some("Promise") || callee.property.name.as_str() != "all" {
        return None;
    }
    match call.arguments.first() {
        Some(Argument::ArrayExpression(array)) => Some(&array.elements[..]),
        _ => None,
    }
}

/// Whether the first parameter of a callback (an arrow or a function expression) binds one name (`Some(true)`) or an
/// object pattern (`Some(false)`), the parameters knip's `handleImportThen` binds an `import()` to; `None` for any
/// other.
fn callback_binding(expr: &Expression<'_>) -> Option<bool> {
    let params = match expr {
        Expression::ArrowFunctionExpression(arrow) => &arrow.params,
        Expression::FunctionExpression(function) => &function.params,
        _ => return None,
    };
    match params
        .items
        .first()
        .filter(|param| param.initializer.is_none())
        .map(|param| &param.pattern)
    {
        Some(BindingPattern::BindingIdentifier(_)) => Some(true),
        Some(BindingPattern::ObjectPattern(_)) => Some(false),
        _ => None,
    }
}

/// Whether `expr` is the factory knip's Vitest mock visitor reads a `vi.mock(import(x), factory)` for side effects
/// with: a synchronous arrow without parameters that returns a parenthesised object literal without a spread.
fn is_object_factory(expr: &Expression<'_>) -> bool {
    let Expression::ArrowFunctionExpression(arrow) = expr else {
        return false;
    };
    if arrow.r#async || !arrow.params.items.is_empty() || arrow.params.rest.is_some() {
        return false;
    }
    let Some(Expression::ParenthesizedExpression(body)) = arrow.body.as_expression() else {
        return false;
    };
    let Expression::ObjectExpression(object) = &body.expression else {
        return false;
    };
    !object
        .properties
        .iter()
        .any(|property| matches!(property, ObjectPropertyKind::SpreadProperty(_)))
}

/// Whether `callee` is a test runner's spy on a named member, as skif's parser reads one (`spy_target` in
/// `crates/skif-js-syntax/src/members.rs`): `vi.spyOn`, `jest.spyOn` (any `.spyOn`) and Jasmine's `spyOn`.
fn is_spy_callee(callee: &Expression<'_>) -> bool {
    match callee {
        Expression::StaticMemberExpression(member) => member.property.name.as_str() == "spyOn",
        Expression::Identifier(id) => id.name.as_str() == "spyOn",
        _ => false,
    }
}

/// The export a qualified import type names: `A` for `import('x').A` and for `import('x').A.B`.
fn qualifier_head<'q>(qualifier: &'q TSImportTypeQualifier<'_>) -> &'q str {
    match qualifier {
        TSImportTypeQualifier::Identifier(name) => name.name.as_str(),
        TSImportTypeQualifier::QualifiedName(path) => qualifier_head(&path.left),
    }
}

/// The names an assignment to `target` of `right` exports as skif's parser reads `CommonJS` exports: the property of
/// `exports.x` and `module.exports.x`, and the keys of `module.exports = { a, b }` when every property is shorthand.
fn commonjs_names(target: &StaticMemberExpression<'_>, right: &Expression<'_>) -> Vec<String> {
    let property = target.property.name.as_str();
    let module_exports = |expr: &Expression<'_>| {
        matches!(expr, Expression::StaticMemberExpression(member)
            if identifier_name(&member.object) == Some("module") && member.property.name.as_str() == "exports")
    };
    if identifier_name(&target.object) == Some("exports") || module_exports(&target.object) {
        return vec![property.to_owned()];
    }
    if identifier_name(&target.object) != Some("module") || property != "exports" {
        return Vec::new();
    }
    let Expression::ObjectExpression(object) = right.get_inner_expression() else {
        return Vec::new();
    };
    object
        .properties
        .iter()
        .map(|property| match property {
            ObjectPropertyKind::ObjectProperty(property) if property.shorthand => match &property.key {
                PropertyKey::StaticIdentifier(key) => Some(key.name.as_str().to_owned()),
                _ => None,
            },
            _ => None,
        })
        .collect::<Option<Vec<String>>>()
        .unwrap_or_default()
}

/// What knip reads in the code file `path` ([`KnipReads`]), parsed with oxc, with the local names of `vi` and `jest`
/// that `module`, skif's parse of the file, imports ([`test_runner_names`]); `None` for a file oxc has no source type
/// for.
fn knip_reads(path: &str, source: &str, module: Option<&ModuleInfo>) -> Option<KnipReads> {
    let source_type = SourceType::from_path(path).ok()?;
    let (vi_names, jest_names) = test_runner_names(module);
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, source_type).parse();
    let mut reads = KnipReads {
        vi_names,
        jest_names,
        ..KnipReads::default()
    };
    reads.visit_program(&parsed.program);
    Some(reads)
}

/// The local names of `vi` imported from `vitest`, and of `jest`: the global and the import from `@jest/globals`.
fn test_runner_names(module: Option<&ModuleInfo>) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut vi = BTreeSet::new();
    let mut jest = BTreeSet::from(["jest".to_owned()]);
    for import in module.into_iter().flat_map(|module| &module.imports) {
        let (names, wanted) = match import.specifier.as_str() {
            "vitest" => (&mut vi, "vi"),
            "@jest/globals" => (&mut jest, "jest"),
            _ => continue,
        };
        names.extend(import.names.iter().filter_map(|name| match &name.kind {
            ImportedKind::Named(export) if export == wanted => name.local.clone(),
            _ => None,
        }));
    }
    (vi, jest)
}

/// The credits of the uses knip reads in the code files of `files` ([`knip_reads`]) where skif reads otherwise, each
/// followed through re-exports ([`follow_reexports`]), filling the `opaque_imports`, `import_defaults`,
/// `required_actuals`, `spied_namespaces`, `enumerated_namespaces` and `import_types` of [`KnipReadSets`]:
///
/// * an `import()` knip reads as opaque, and a `jest.requireActual`, credit every export of the module they load,
///   types included (knip's `OPAQUE` import); an `import()` knip binds to one name credits its `default`;
/// * a namespace import (`import * as ns`) whose name an array literal holds or `Object.keys` takes credits every
///   export of its module, whatever the name refers to there: knip's `ArrayExpression` visitor and its `Object.keys`
///   branch mark the namespace opaque without checking whether a parameter or variable shadows it;
/// * the namespace imports of a module that only spy calls read (`vi.spyOn(ns, 'a')`, read for skif as a use of `a`;
///   for knip a reference of `ns` as a whole), when no namespace import of the module is read through a member
///   otherwise, credit every export of the module: knip skips the exports and types of a module whose namespace
///   imports are all referenced as a whole while `nsExports` and `nsTypes` are not reported (`hasStrictlyNsReferences`
///   in its `graph/analyze.js`);
/// * a qualified import type credits the export it names (`import('./x').A`), which skif's parser reads as a use of
///   `A` and knip as an import of the module for its side effects (its `TSImportType` visitor).
///
/// Only the files holding one of [`KNIP_READ_MARKERS`] are parsed; `read` gives a file's source.
fn knip_read_credits(files: &[String], resolver: &Resolver<'_>, read: &SourceReader<'_>) -> KnipReadSets {
    let options = ParseOptions {
        in_file_references: false,
        member_accesses: true,
    };
    let (mut opaque, mut defaults, mut actuals) = (Vec::new(), Vec::new(), Vec::new());
    let (mut enumerated, mut import_types) = (Vec::new(), Vec::new());
    let mut spied: BTreeSet<String> = BTreeSet::new();
    let mut read_otherwise: BTreeSet<String> = BTreeSet::new();
    for file in files {
        let Some(source) = read(file) else { continue };
        if !KNIP_READ_MARKERS.iter().any(|marker| source.contains(marker)) {
            continue;
        }
        let module = parse_module(Path::new(file), &source, &options).ok();
        let Some(reads) = knip_reads(file, &source, module.as_ref()) else {
            continue;
        };
        let credited_in = |specifiers: &[String], name: &str| -> Vec<(String, String)> {
            specifiers
                .iter()
                .filter_map(|specifier| resolver.resolve(file, specifier))
                .map(|target| (target, name.to_owned()))
                .collect()
        };
        opaque.extend(credited_in(&reads.opaque[..], WHOLE_MODULE));
        defaults.extend(credited_in(&reads.defaults[..], "default"));
        actuals.extend(credited_in(&reads.required_actuals[..], WHOLE_MODULE));
        import_types.extend(
            reads
                .import_types
                .iter()
                .filter_map(|(specifier, name)| resolver.resolve(file, specifier).map(|target| (target, name.clone()))),
        );
        let Some(module) = module else { continue };
        for (import, target) in module_targets(file, &module, resolver) {
            if !matches!(import.kind, ImportKind::Static | ImportKind::Dynamic) {
                continue;
            }
            for name in &import.names {
                let (ImportedKind::Namespace, Some(local)) = (&name.kind, &name.local) else {
                    continue;
                };
                if reads.enumerated.contains(local) {
                    enumerated.push((target.clone(), WHOLE_MODULE.to_owned()));
                }
                let spy_only = reads.spied.get(local).is_some_and(|members| {
                    !name.escapes
                        && !reads.member_objects.contains(local)
                        && name.accessed_members.iter().all(|member| members.contains(member))
                });
                if spy_only {
                    spied.insert(target.clone());
                } else if reads.member_objects.contains(local) || !name.accessed_members.is_empty() {
                    read_otherwise.insert(target.clone());
                }
            }
        }
    }
    let spied_seeds: Vec<(String, String)> = spied
        .difference(&read_otherwise)
        .map(|target| (target.clone(), WHOLE_MODULE.to_owned()))
        .collect();
    KnipReadSets {
        opaque_imports: follow_reexports(opaque, resolver, read),
        import_defaults: follow_reexports(defaults, resolver, read),
        required_actuals: follow_reexports(actuals, resolver, read),
        spied_namespaces: follow_reexports(spied_seeds, resolver, read),
        enumerated_namespaces: follow_reexports(enumerated, resolver, read),
        import_types: follow_reexports(import_types, resolver, read),
        ..KnipReadSets::default()
    }
}

/// The keys of `keys` (`<file>|<export>`, exports and namespace exports only skif reports) whose file exports the name
/// as a namespace re-export (`export * as X from './x'`, also of the file itself): knip records an `export … from` as
/// a re-export of the source module and never lists it among the file's own exports, so it never reports one unused.
/// `read` gives a file's source; a file it cannot read or parse has none.
fn namespace_reexport_keys(keys: &BTreeSet<String>, read: &SourceReader<'_>) -> BTreeSet<String> {
    let options = ParseOptions {
        in_file_references: false,
        member_accesses: false,
    };
    let mut modules: BTreeMap<&str, Option<ModuleInfo>> = BTreeMap::new();
    keys.iter()
        .filter(|key| {
            let Some((path, name)) = key.split_once('|') else {
                return false;
            };
            modules
                .entry(path)
                .or_insert_with(|| {
                    read(path)
                        .filter(|source| source.contains("export * as "))
                        .and_then(|source| parse_module(Path::new(path), &source, &options).ok())
                })
                .as_ref()
                .is_some_and(|module| {
                    name != WHOLE_MODULE
                        && module
                            .exports
                            .iter()
                            .any(|export| export.kind == ExportKind::StarReExport && export.name == name)
                })
        })
        .cloned()
        .collect()
}

/// The keys of `keys` (`<file>|<export>`, exports and namespace exports only skif reports) in a code file that is no
/// JavaScript file ([`JAVASCRIPT_EXTENSIONS`]) whose `CommonJS` assignments export the name, as oxc reads them
/// ([`KnipReads`]): knip records `exports.x = …` and the other `CommonJS` exports in JavaScript files only. `read`
/// gives a file's source; only the files holding `exports` are parsed.
fn typescript_commonjs_keys(keys: &BTreeSet<String>, read: &SourceReader<'_>) -> BTreeSet<String> {
    let mut exported: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    keys.iter()
        .filter(|key| {
            let Some((path, name)) = key.split_once('|') else {
                return false;
            };
            let extension = path.rsplit_once('.').map_or("", |(_, extension)| extension);
            if JAVASCRIPT_EXTENSIONS.contains(&extension) {
                return false;
            }
            exported
                .entry(path)
                .or_insert_with(|| {
                    read(path)
                        .filter(|source| source.contains("exports"))
                        .and_then(|source| knip_reads(path, &source, None))
                        .map_or_else(BTreeSet::new, |reads| reads.commonjs_exports)
                })
                .contains(name)
        })
        .cloned()
        .collect()
}

/// The file name patterns knip's `node` plugin makes entries of a workspace with when a script of the workspace's or
/// the root's manifest runs `node --test` (`patterns` in its `plugins/node/index.js`), as tests on a path relative to
/// the workspace: `**/*{.,-,_}test.<ext>`, `**/test-*.<ext>`, `**/test.<ext>` and `**/test/**/*.<ext>`, each
/// extension one of these.
const NODE_TEST_EXTENSIONS: [&str; 6] = ["cjs", "mjs", "js", "cts", "mts", "ts"];

/// Whether `relative`, a path below a workspace with `/` separators, matches one of the patterns of knip's `node`
/// plugin ([`NODE_TEST_EXTENSIONS`]). A hidden file matches none, as the glob's `*` matches no leading dot.
fn is_node_test_file(relative: &str) -> bool {
    let Some((stem_path, extension)) = relative.rsplit_once('.') else {
        return false;
    };
    let (dirs, stem) = stem_path.rsplit_once('/').unwrap_or(("", stem_path));
    if !NODE_TEST_EXTENSIONS.contains(&extension) || stem.starts_with('.') {
        return false;
    }
    dirs.split('/').any(|segment| segment == "test")
        || stem == "test"
        || stem.starts_with("test-")
        || [".test", "-test", "_test"].iter().any(|suffix| stem.ends_with(suffix))
}

/// Whether a script of `manifest` runs `node --test` (also `nub --test`), as knip's `node` plugin (`hasNodeTest`)
/// reads its commands: each command of a `&&`, `||`, `;`, `&` or `|` chain, after its leading environment assignments,
/// whose program is `node` or `nub` and one of whose arguments is `--test`.
fn runs_node_test(manifest: &Value) -> bool {
    let Some(scripts) = manifest.get("scripts").and_then(Value::as_object) else {
        return false;
    };
    scripts.values().filter_map(Value::as_str).any(|script| {
        script.split(['&', '|', ';']).any(|command| {
            let mut words = command.split_whitespace().skip_while(|word| is_env_assignment(word));
            words.next().is_some_and(|program| matches!(program, "node" | "nub")) && words.any(|word| word == "--test")
        })
    })
}

/// Whether `word` is an environment assignment that leads a shell command (`NODE_ENV=test`).
fn is_env_assignment(word: &str) -> bool {
    word.split_once('=')
        .is_some_and(|(name, _)| !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
}

/// The files of `keys` (`<file>|<export>`) that knip's `node` plugin makes entries of their workspace: the plugin is
/// on in every workspace when the root's manifest has a script that runs `node --test`, else in a workspace whose own
/// manifest has one ([`runs_node_test`]), and makes the files that match its patterns entries ([`is_node_test_file`]),
/// whose exports knip does not judge.
fn node_test_files(keys: &BTreeSet<String>, layout: &Layout<'_>) -> BTreeSet<String> {
    let runs = |dir: &str| (layout.manifest)(dir).is_some_and(|manifest| runs_node_test(&manifest));
    let root = runs("");
    let mut workspaces: BTreeMap<String, bool> = BTreeMap::new();
    keys.iter()
        .map(|key| key_path(key))
        .filter(|path| {
            let workspace = owner_dir(path, &|dir: &str| layout.is_workspace(dir));
            let on = root || *workspaces.entry(workspace.clone()).or_insert_with(|| runs(&workspace));
            on && is_node_test_file(below(&workspace, path))
        })
        .map(str::to_owned)
        .collect()
}

/// The rule of `rules_angular` that packages Angular libraries for npm, and the attribute that names its entry points
/// (`NG_PACKAGE` in `crates/skif-js-plugins/src/plugins/bazel.rs`).
const NG_PACKAGE: (&str, &str) = ("ng_package", "deps");

/// The file of an entry point's Bazel package that skif's Bazel plugin makes a production entry (`NG_ENTRY_FILE`
/// there).
const NG_ENTRY_FILE: &str = "index.ts";

/// The entry points the `ng_package` calls of `build_files` name, as skif's Bazel plugin reads them
/// (`ng_package_entries` there): the [`NG_ENTRY_FILE`] of each Bazel package that a label of its `deps` names
/// (`//packages/core/primitives/signals`, `:core` for the `BUILD` file's own package) inside the project that owns the
/// `BUILD` file (`is_project`), when the file exists. Only the `BUILD` files of a Bazel repository that uses `rules_js`
/// are read ([`rules_js_build_files`]); `read` gives the source of a root file.
fn ng_package_entries(
    build_files: &[(String, String)],
    is_project: &dyn Fn(&str) -> bool,
    is_file: &FileProbe<'_>,
    read: &dyn Fn(&str) -> Option<String>,
) -> Vec<String> {
    let mut found = Vec::new();
    for (path, source, root) in rules_js_build_files(build_files, is_file, read) {
        let project = owner_dir(path, is_project);
        let package = below(root, &project);
        let bazel_package = below(root, parent_dir(path));
        for call in rule_calls(source).into_iter().filter(|call| call.rule == NG_PACKAGE.0) {
            for (_, label) in call.literals.iter().filter(|(attribute, _)| attribute == NG_PACKAGE.1) {
                let Some(inside) = bazel_label_package(label, bazel_package)
                    .and_then(|target| within_package(&target, package).map(str::to_owned))
                else {
                    continue;
                };
                let entry = joined(&joined(&project, &inside), NG_ENTRY_FILE);
                if is_file(&entry) {
                    found.push(entry);
                }
            }
        }
    }
    found.sort();
    found.dedup();
    found
}

/// The Bazel package, relative to the repository root, of a target `label` of the main repository written in a
/// `BUILD` file of `bazel_package` (`label_package` in `crates/skif-js-plugins/src/plugins/bazel.rs`): `//pkg`,
/// `//pkg:target` and `@//pkg` name `pkg`, `:target` and `target` the file's own package. A label of another
/// repository (`@npm//pkg`) names none.
fn bazel_label_package(label: &str, bazel_package: &str) -> Option<String> {
    let main = match label.strip_prefix('@') {
        Some(after) => after.trim_start_matches('@').strip_prefix("//")?,
        None => match label.strip_prefix("//") {
            Some(absolute) => absolute,
            None => return (!label.is_empty()).then(|| bazel_package.to_owned()),
        },
    };
    Some(main.split_once(':').map_or(main, |(package, _)| package).to_owned())
}

/// `target`, a directory relative to the repository root, relative to the npm package at `package` (`""` for the
/// repository root); `None` when it lies outside the package.
fn within_package<'t>(target: &'t str, package: &str) -> Option<&'t str> {
    if target == package {
        return Some("");
    }
    if package.is_empty() {
        return Some(target);
    }
    target.strip_prefix(package).and_then(|rest| rest.strip_prefix('/'))
}

/// Whether `path` is a `BUILD.bazel` or `BUILD` file.
fn is_build_file(path: &str) -> bool {
    BUILD_FILES.contains(&path.rsplit('/').next().unwrap_or(path))
}

/// A token of Starlark source: a name, the contents of a string literal, or a punctuation character.
#[derive(Debug, Clone, PartialEq, Eq)]
enum StarlarkToken {
    Name(String),
    Text(String),
    Punct(char),
}

/// The tokens of Starlark `source`, in order, without `#` comments and whitespace: names, string literals (single,
/// double and triple quoted, an escaped character taken as itself, a literal that is not closed ending with its line)
/// and every other character as punctuation.
fn starlark_tokens(source: &str) -> Vec<StarlarkToken> {
    let chars: Vec<char> = source.chars().collect();
    let mut found = Vec::new();
    let mut at = 0;
    while let Some(&character) = chars.get(at) {
        match character {
            '#' => {
                while chars.get(at).is_some_and(|&next| next != '\n') {
                    at += 1;
                }
            }
            '"' | '\'' => {
                let quote = character;
                let triple = chars.get(at + 1) == Some(&quote) && chars.get(at + 2) == Some(&quote);
                let width = if triple { 3 } else { 1 };
                at += width;
                let mut text = String::new();
                while let Some(&next) = chars.get(at) {
                    if next == quote
                        && (!triple || (chars.get(at + 1) == Some(&quote) && chars.get(at + 2) == Some(&quote)))
                    {
                        at += width;
                        break;
                    }
                    if next == '\n' && !triple {
                        break;
                    }
                    if next == '\\'
                        && let Some(&escaped) = chars.get(at + 1)
                    {
                        text.push(escaped);
                        at += 2;
                        continue;
                    }
                    text.push(next);
                    at += 1;
                }
                found.push(StarlarkToken::Text(text));
            }
            name if name.is_alphabetic() || name == '_' => {
                let start = at;
                while chars.get(at).is_some_and(|next| next.is_alphanumeric() || *next == '_') {
                    at += 1;
                }
                found.push(StarlarkToken::Name(chars[start..at].iter().collect()));
            }
            space if space.is_whitespace() => at += 1,
            other => {
                found.push(StarlarkToken::Punct(other));
                at += 1;
            }
        }
    }
    found
}

/// The contents of the string literals of Starlark `source`, in order, without those of `#` comments
/// ([`starlark_tokens`]).
fn starlark_strings(source: &str) -> Vec<String> {
    starlark_tokens(source)
        .into_iter()
        .filter_map(|token| match token {
            StarlarkToken::Text(text) => Some(text),
            StarlarkToken::Name(_) | StarlarkToken::Punct(_) => None,
        })
        .collect()
}

/// A top-level call of Starlark source as skif's Bazel plugin reads it (`Call` in
/// `crates/skif-js-plugins/src/bazel.rs`).
#[derive(Debug, Clone, PartialEq, Eq)]
struct RuleCall {
    /// The name called (`ts_project`).
    rule: String,
    /// `(attribute, literal)` for each string literal of a keyword argument's value, in order. A literal inside a
    /// nested call (`glob([...])`, `select({...})`) is left out.
    literals: Vec<(String, String)>,
    /// `(attribute, pattern)` for each include pattern of a `glob(...)` (also `native.glob`) that a keyword argument's
    /// value is or adds (`[":a.ts"] + glob([...])`): the string literals of the list given first or as `include`. Its
    /// `exclude` patterns are not read, and a `glob` inside another call (`select({...})`) is left out.
    globs: Vec<(String, String)>,
    /// Whether the call has the literal `testonly = True`.
    testonly: bool,
}

/// The name of the Starlark call whose arguments are file patterns.
const STARLARK_GLOB: &str = "glob";

/// The calls at the top level of Starlark `source`, in order; a method call (`"x".format(...)`) is none, as in skif's
/// Bazel plugin (`calls` in `crates/skif-js-plugins/src/bazel.rs`).
fn rule_calls(source: &str) -> Vec<RuleCall> {
    let tokens = starlark_tokens(source);
    let mut found = Vec::new();
    let mut open: Vec<char> = Vec::new();
    let mut current: Option<RuleCall> = None;
    let mut attribute: Option<String> = None;
    let mut at = 0;
    while let Some(token) = tokens.get(at) {
        match token {
            StarlarkToken::Name(name)
                if name == STARLARK_GLOB
                    && open.len() == 1
                    && tokens.get(at + 1) == Some(&StarlarkToken::Punct('(')) =>
            {
                let (patterns, next) = glob_includes(&tokens, at + 2);
                if let (Some(call), Some(named)) = (current.as_mut(), &attribute) {
                    call.globs
                        .extend(patterns.into_iter().map(|pattern| (named.clone(), pattern)));
                }
                at = next;
                continue;
            }
            StarlarkToken::Punct(bracket @ ('(' | '[' | '{')) => {
                if open.is_empty() && *bracket == '(' {
                    let called = at.checked_sub(1).and_then(|before| tokens.get(before));
                    let method =
                        at.checked_sub(2).and_then(|before| tokens.get(before)) == Some(&StarlarkToken::Punct('.'));
                    current = match called {
                        Some(StarlarkToken::Name(name)) if !method => Some(RuleCall {
                            rule: name.clone(),
                            literals: Vec::new(),
                            globs: Vec::new(),
                            testonly: false,
                        }),
                        _ => None,
                    };
                    attribute = None;
                }
                open.push(*bracket);
            }
            StarlarkToken::Punct(')' | ']' | '}') => {
                open.pop();
                if open.is_empty() {
                    found.extend(current.take());
                    attribute = None;
                }
            }
            StarlarkToken::Punct(',') if open.len() == 1 => attribute = None,
            StarlarkToken::Name(name)
                if open.len() == 1
                    && tokens.get(at + 1) == Some(&StarlarkToken::Punct('='))
                    && tokens.get(at + 2) != Some(&StarlarkToken::Punct('=')) =>
            {
                let is_true = matches!(tokens.get(at + 2), Some(StarlarkToken::Name(value)) if value == "True")
                    && matches!(tokens.get(at + 3), None | Some(StarlarkToken::Punct(',' | ')')));
                if name == "testonly"
                    && is_true
                    && let Some(call) = current.as_mut()
                {
                    call.testonly = true;
                }
                attribute = Some(name.clone());
            }
            StarlarkToken::Text(text) => {
                if let (Some(call), Some(named)) = (current.as_mut(), &attribute)
                    && !open.iter().skip(1).any(|bracket| *bracket == '(')
                {
                    call.literals.push((named.clone(), text.clone()));
                }
            }
            StarlarkToken::Name(_) | StarlarkToken::Punct(_) => {}
        }
        at += 1;
    }
    found.extend(current);
    found
}

/// The include patterns of the Starlark `glob(...)` whose arguments start at token `start`, just after its `(`, and
/// the position after its `)`: the string literals directly in the list literal given first or as `include`
/// (`glob_includes` in `crates/skif-js-plugins/src/bazel.rs`). The `exclude` patterns and every computed value are
/// left out.
fn glob_includes(tokens: &[StarlarkToken], start: usize) -> (Vec<String>, usize) {
    let mut patterns = Vec::new();
    let mut brackets: Vec<char> = Vec::new();
    let mut argument = 0_usize;
    let mut keyword: Option<&str> = None;
    let mut at = start;
    while let Some(token) = tokens.get(at) {
        match token {
            StarlarkToken::Punct(bracket @ ('(' | '[' | '{')) => brackets.push(*bracket),
            StarlarkToken::Punct(')' | ']' | '}') if brackets.is_empty() => return (patterns, at + 1),
            StarlarkToken::Punct(')' | ']' | '}') => {
                brackets.pop();
            }
            StarlarkToken::Punct(',') if brackets.is_empty() => {
                argument += 1;
                keyword = None;
            }
            StarlarkToken::Name(name)
                if brackets.is_empty() && tokens.get(at + 1) == Some(&StarlarkToken::Punct('=')) =>
            {
                keyword = Some(name.as_str());
                at += 2;
                continue;
            }
            StarlarkToken::Text(text) if brackets == ['['] => {
                if keyword.map_or(argument == 0, |name| name == "include") {
                    patterns.push(text.clone());
                }
            }
            StarlarkToken::Name(_) | StarlarkToken::Text(_) | StarlarkToken::Punct(_) => {}
        }
        at += 1;
    }
    (patterns, at)
}

/// The rules of skif's Bazel plugin, the attributes whose labels name the files a rule builds from or runs, and
/// whether the rule is a test (`RULES` in `crates/skif-js-plugins/src/plugins/bazel.rs`).
const BAZEL_RULES: [(&str, &[&str], bool); 6] = [
    ("js_binary", &["entry_point", "data"], false),
    ("js_run_devserver", &["entry_point", "data"], false),
    ("js_test", &["entry_point", "data"], true),
    ("js_library", &["srcs", "data"], false),
    ("ts_project", &["srcs", "data"], false),
    ("ng_project", &["srcs", "data"], false),
];

/// The attribute of [`BAZEL_RULES`] that names the file a rule runs; a `glob` there names nothing.
const BAZEL_ENTRY_POINT: &str = "entry_point";

/// The path a literal names in its own Bazel package (`package_path` in `crates/skif-js-plugins/src/bazel.rs`): a
/// relative file (`src/a.ts`) or a target of the package written as a label (`:src/a.ts`), without a leading `./`. A
/// label of another package or repository, an absolute path, one that climbs out with `..` and a template
/// (`{name}.js`, `$(location)`) name none.
fn bazel_package_path(literal: &str) -> Option<&str> {
    let path = literal.strip_prefix(':').unwrap_or(literal);
    let path = path.strip_prefix("./").unwrap_or(path);
    let plain = !path.is_empty()
        && !path.starts_with(['/', '@'])
        && !path.contains([':', '$', '{', '}'])
        && !path.split('/').any(|segment| segment == "..");
    plain.then_some(path)
}

/// The npm package a path below `node_modules` starts with (`@scope/name` or `name`); `None` for an empty name or a
/// scope without one.
fn npm_package_name(path: &str) -> Option<&str> {
    let name = package_name(path);
    let valid = match name.strip_prefix('@') {
        Some(scoped) => scoped
            .split_once('/')
            .is_some_and(|(scope, rest)| !scope.is_empty() && !rest.is_empty()),
        None => !name.is_empty(),
    };
    valid.then_some(name)
}

/// The Bazel package, relative to its repository root, and the npm package that `label` names as a linked dependency
/// when a `BUILD` file of the Bazel package `bazel_package` holds it (`linked_dependency` of skif's Bazel plugin): a
/// `node_modules/<name>` target of the main repository (`":node_modules/x"`, `"//pkg:node_modules/@scope/x"`,
/// `"@//pkg:node_modules/x"`) or the `<name>/package_json.bzl` of another repository (`"@npm//pkg:x/package_json.bzl"`).
fn linked_dependency(label: &str, bazel_package: &str) -> Option<(String, String)> {
    let (repository, rest) = match label.strip_prefix('@') {
        Some(after) => {
            let at = after.find("//")?;
            (after[..at].trim_start_matches('@'), &after[at..])
        }
        None => ("", label),
    };
    let (package, target) = match rest.strip_prefix("//") {
        Some(absolute) => absolute.split_once(':')?,
        None => (bazel_package, rest.strip_prefix(':').unwrap_or(rest)),
    };
    if let Some(name) = target.strip_suffix("/package_json.bzl") {
        let named = !repository.is_empty() && npm_package_name(name) == Some(name);
        return named.then(|| (package.to_owned(), name.to_owned()));
    }
    let linked = target.strip_prefix("node_modules/")?;
    if !repository.is_empty() {
        return None;
    }
    npm_package_name(linked).map(|name| (package.to_owned(), name.to_owned()))
}

/// The root of the Bazel repository that holds the directory `dir`: the nearest directory at or above it with one of
/// [`BAZEL_ROOT_FILES`], up to the checkout root.
fn bazel_root<'p>(dir: &'p str, is_file: &FileProbe<'_>) -> Option<&'p str> {
    let mut candidate = dir;
    loop {
        if BAZEL_ROOT_FILES.iter().any(|name| is_file(&joined(candidate, name))) {
            return Some(candidate);
        }
        if candidate.is_empty() {
            return None;
        }
        candidate = parent_dir(candidate);
    }
}

/// `<manifest>|<package>` for every dependency a `BUILD.bazel` or `BUILD` file credits, as skif's Bazel plugin reads
/// it: a string literal that is a linked-dependency label ([`linked_dependency`]) names a dependency of the package
/// whose directory is the label's Bazel package, below the file's Bazel repository root ([`bazel_root`], one whose
/// root files name [`RULES_JS`]), when that directory holds a `package.json` and the `BUILD` file lies in it.
/// `build_files` holds each file's path and source; `read` gives the source of a root file.
fn bazel_credits(
    build_files: &[(String, String)],
    is_file: &FileProbe<'_>,
    read: &dyn Fn(&str) -> Option<String>,
) -> BTreeSet<String> {
    let mut credits = BTreeSet::new();
    for (path, source, root) in rules_js_build_files(build_files, is_file, read) {
        let dir = parent_dir(path);
        let bazel_package = below(root, dir);
        for literal in starlark_strings(source) {
            let Some((package, name)) = linked_dependency(&literal, bazel_package) else {
                continue;
            };
            let package_dir = joined(root, &package);
            let manifest = joined(&package_dir, "package.json");
            let inside = package_dir.is_empty() || dir == package_dir || dir.starts_with(&format!("{package_dir}/"));
            if inside && is_file(&manifest) {
                credits.insert(format!("{manifest}|{name}"));
            }
        }
    }
    credits
}

/// The `(path, source, root)` of each of `build_files` below the root of a Bazel repository ([`bazel_root`]) whose root
/// files name [`RULES_JS`]; skif's Bazel plugin reads no other. `read` gives the source of a root file.
fn rules_js_build_files<'b>(
    build_files: &'b [(String, String)],
    is_file: &FileProbe<'_>,
    read: &dyn Fn(&str) -> Option<String>,
) -> Vec<(&'b str, &'b str, &'b str)> {
    let mut uses_rules_js: BTreeMap<&str, bool> = BTreeMap::new();
    let mut found = Vec::new();
    for (path, source) in build_files {
        let Some(root) = bazel_root(parent_dir(path), is_file) else {
            continue;
        };
        let named = *uses_rules_js.entry(root).or_insert_with(|| {
            BAZEL_ROOT_FILES
                .iter()
                .any(|name| read(&joined(root, name)).is_some_and(|text| text.contains(RULES_JS)))
        });
        if named {
            found.push((path.as_str(), source.as_str(), root));
        }
    }
    found
}

/// The files that the `BUILD` files of a `rules_js` repository ([`rules_js_build_files`]) name in an attribute of one
/// of skif's Bazel rules ([`BAZEL_RULES`]): the sources and data of a build and the entry point of a binary or test,
/// each a literal that names a file of the `BUILD` file's package ([`bazel_package_path`]) that exists. A test target
/// (`js_test`, or a rule with `testonly = True`) also names the files of `files` below the `BUILD` file's directory
/// that the include patterns of a `glob` of its `srcs` or `data` match, relative to that directory, without applying
/// its `exclude` patterns. A `glob` of any other target, one inside `select` and a label of another package name none,
/// as in skif's plugin (ADR 0031).
fn bazel_sources(
    build_files: &[(String, String)],
    files: &[String],
    is_file: &FileProbe<'_>,
    read: &dyn Fn(&str) -> Option<String>,
) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for (path, source, _) in rules_js_build_files(build_files, is_file, read) {
        let dir = parent_dir(path);
        for call in rule_calls(source) {
            let Some((_, attributes, test_rule)) = BAZEL_RULES.iter().find(|(rule, _, _)| *rule == call.rule) else {
                continue;
            };
            for (attribute, literal) in &call.literals {
                if let Some(file) = bazel_package_path(literal).map(|file| joined(dir, file))
                    && attributes.contains(&attribute.as_str())
                    && is_file(&file)
                {
                    found.insert(file);
                }
            }
            if !(*test_rule || call.testonly) {
                continue;
            }
            let patterns: Vec<String> = call
                .globs
                .iter()
                .filter(|(attribute, _)| attribute != BAZEL_ENTRY_POINT && attributes.contains(&attribute.as_str()))
                .filter_map(|(_, pattern)| bazel_package_path(pattern))
                .map(str::to_owned)
                .collect();
            if patterns.is_empty() {
                continue;
            }
            let globs = IncompleteScopeGlobs::new(patterns);
            let prefix = format!("{dir}/");
            found.extend(
                files
                    .iter()
                    .filter(|file| (dir.is_empty() || file.starts_with(&prefix)) && globs.is_match(below(dir, file)))
                    .cloned(),
            );
        }
    }
    found
}

/// The files only one tool reports as unused (`unused` but not `other_unused`) that are among `sources` or that such a
/// file reaches by relative imports through other such files ([`reach`]): the other tool uses what it reaches from
/// `sources`, the Bazel build sources ([`bazel_sources`]) or the files that skif loads where knip reads no import.
fn reach_from_sources(
    sources: &BTreeSet<String>,
    (unused, other_unused): (&BTreeSet<String>, &BTreeSet<String>),
    is_file: &FileProbe<'_>,
    read: &dyn Fn(&str) -> Option<String>,
) -> BTreeSet<String> {
    let only: BTreeSet<&str> = unused.difference(other_unused).map(String::as_str).collect();
    let seeds: Vec<&str> = only.iter().copied().filter(|file| sources.contains(*file)).collect();
    let mut reached = reach(seeds.clone(), &only, is_file, read);
    reached.extend(seeds.iter().map(|seed| (*seed).to_owned()));
    reached
}

/// `credits` ([`bazel_credits`]) with the dependencies they make used as peers, as skif credits a peer dependency of a
/// used package (`credit` in `crates/skif-js/src/deps.rs`): a dependency a manifest lists is credited when the
/// installed manifest of a credited dependency of the same manifest names it, or the package an `@types/` dependency
/// types, in `peerDependencies`, optional or not, and a peer so credited credits its own peers in turn. `manifest`
/// reads the `package.json` of a directory; a dependency's installed manifest is the nearest `node_modules/<name>` at
/// or above the listing package's directory ([`installed_manifest`]).
fn with_peer_credits(credits: BTreeSet<String>, manifest: &ManifestReader<'_>) -> BTreeSet<String> {
    let mut pending: Vec<String> = credits.iter().cloned().collect();
    let mut credited = credits;
    while let Some(key) = pending.pop() {
        let Some((listing, host)) = key.split_once('|') else {
            continue;
        };
        let dir = parent_dir(listing);
        let (Some(installed), Some(listed)) = (installed_manifest(dir, host, manifest), manifest(dir)) else {
            continue;
        };
        let Some(peers) = installed.get("peerDependencies").and_then(Value::as_object) else {
            continue;
        };
        let names = DEPENDENCY_FIELDS
            .iter()
            .filter_map(|field| listed.get(*field).and_then(Value::as_object))
            .flat_map(|dependencies| dependencies.keys());
        for name in names {
            let peer = peers.contains_key(name)
                || name
                    .strip_prefix("@types/")
                    .is_some_and(|typed| peers.contains_key(&typed_package(typed)));
            let candidate = format!("{listing}|{name}");
            if peer && credited.insert(candidate.clone()) {
                pending.push(candidate);
            }
        }
    }
    credited
}

/// The manifest of the package `name` as installed for the package in `dir`: the `package.json` of the nearest
/// `node_modules/<name>` at or above `dir`, up to the checkout root.
fn installed_manifest(dir: &str, name: &str, manifest: &ManifestReader<'_>) -> Option<Rc<Value>> {
    let installed = format!("node_modules/{name}");
    let mut candidate = dir;
    loop {
        if let Some(found) = manifest(&joined(candidate, &installed)) {
            return Some(found);
        }
        if candidate.is_empty() {
            return None;
        }
        candidate = parent_dir(candidate);
    }
}

/// The `BUILD.bazel` and `BUILD` files of a checkout whose root holds one of [`BAZEL_ROOT_FILES`], with their sources,
/// by path relative to the checkout with `/` separators, sorted: in the directories skif's file discovery enters (none
/// of [`SKIPPED_DIRECTORIES`], those of [`PACKAGE_ROOT_ONLY_DIRECTORIES`] only directly in the root or a directory
/// `is_package` accepts, no hidden directory but [`TOOL_DIRECTORIES`]), never through a symbolic link or into Bazel's
/// `bazel-*` output trees. Empty for a checkout that is no Bazel repository.
fn read_build_files(root: &Path, is_package: &dyn Fn(&str) -> bool) -> Vec<(String, String)> {
    if !BAZEL_ROOT_FILES.iter().any(|name| root.join(name).is_file()) {
        return Vec::new();
    }
    let mut found = Vec::new();
    let mut pending = vec![String::new()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = fs::read_dir(root.join(&dir)) else {
            continue;
        };
        for entry in entries.flatten() {
            let (Ok(name), Ok(kind)) = (entry.file_name().into_string(), entry.file_type()) else {
                continue;
            };
            let path = joined(&dir, &name);
            if kind.is_dir() {
                let root_only = PACKAGE_ROOT_ONLY_DIRECTORIES.contains(&name.as_str());
                let here = !root_only || dir.is_empty() || is_package(dir.as_str());
                let skipped = (SKIPPED_DIRECTORIES.contains(&name.as_str()) && here)
                    || (name.starts_with('.') && !TOOL_DIRECTORIES.contains(&name.as_str()))
                    || (dir.is_empty() && name.starts_with("bazel-"));
                if !skipped {
                    pending.push(path);
                }
            } else if kind.is_file()
                && BUILD_FILES.contains(&name.as_str())
                && let Ok(source) = fs::read_to_string(root.join(&path))
            {
                found.push((path, source));
            }
        }
    }
    found.sort();
    found
}

/// The `package.json` files knip reports a finding on.
fn knip_manifests(knip: &Keys) -> BTreeSet<String> {
    knip.values()
        .flatten()
        .map(|key| key_path(key.as_str()))
        .filter(|path| *path == "package.json" || path.ends_with("/package.json"))
        .map(str::to_owned)
        .collect()
}

/// Reads the manifests of a checkout, each once.
fn manifest_reader(root: &Path) -> impl Fn(&str) -> Option<Rc<Value>> + '_ {
    let cache: RefCell<BTreeMap<String, Option<Rc<Value>>>> = RefCell::default();
    move |dir: &str| {
        if let Some(known) = cache.borrow().get(dir) {
            return known.clone();
        }
        let path = root.join(dir).join("package.json");
        let manifest = path.is_file().then(|| {
            Rc::new(
                fs::read_to_string(&path)
                    .ok()
                    .and_then(|text| serde_json::from_str(&text).ok())
                    .unwrap_or(Value::Null),
            )
        });
        cache.borrow_mut().insert(dir.to_owned(), manifest.clone());
        manifest
    }
}

/// The workspace member directories of a checkout as skif's workspace discovery reads the root's patterns, relative
/// with `/` separators, the root left out.
fn workspace_members(root: &Path) -> BTreeSet<String> {
    declared_member_dirs(&OsFs, root)
        .iter()
        .filter_map(|dir| dir.strip_prefix(root).ok())
        .map(|dir| dir.to_string_lossy().replace('\\', "/").trim_matches('/').to_owned())
        .filter(|dir| !dir.is_empty() && dir != ".")
        .collect()
}

/// The knip report in its stdout, which may carry banners or log lines around the JSON (dotenv, console output of
/// config files). Tries every `{` that starts a line or a string key, and accepts the first object that is a report.
pub fn parse_knip_report(stdout: &[u8]) -> Result<Value, String> {
    let text = String::from_utf8_lossy(stdout);
    if text.trim().is_empty() {
        return Err("stdout is empty".to_owned());
    }
    let bytes = text.as_bytes();
    let (line_starts, inline): (Vec<usize>, Vec<usize>) = text
        .match_indices('{')
        .map(|(at, _)| at)
        .filter(|&at| at == 0 || bytes[at - 1] == b'\n' || bytes.get(at + 1) == Some(&b'"'))
        .partition(|&at| at == 0 || bytes[at - 1] == b'\n');
    let mut first_error = None;
    for start in line_starts.into_iter().chain(inline.into_iter().take(256)) {
        match serde_json::Deserializer::from_str(&text[start..])
            .into_iter::<Value>()
            .next()
        {
            Some(Ok(document))
                if document.get("issues").is_some_and(Value::is_array)
                    || document.get("files").is_some_and(Value::is_array) =>
            {
                return Ok(document);
            }
            Some(Err(error)) => {
                first_error.get_or_insert_with(|| error.to_string());
            }
            _ => {}
        }
    }
    Err(first_error.map_or_else(
        || format!("no knip report in the {} bytes of stdout", stdout.len()),
        |error| format!("no knip report in stdout; first parse error: {error}"),
    ))
}

/// The configuration files knip could not load, from its `ERROR: Error loading <file> (<cause>)` lines on stderr,
/// relative to the checkout. knip goes on without them and still prints a report, then exits with 2.
pub fn knip_config_errors(stderr: &str, root: &Path) -> BTreeSet<String> {
    stderr
        .lines()
        .filter_map(|line| line.trim().strip_prefix("ERROR: Error loading "))
        .filter_map(|rest| rest.split(" (").next())
        .map(|file| normalize(file.trim(), root))
        .filter(|file| !file.is_empty())
        .collect()
}

fn total(keys: &Keys) -> usize {
    keys.values().map(std::collections::BTreeSet::len).sum()
}

/// The install command of a repository (program first), or why there is none.
fn install_command(repo: &Repo, dir: &Path) -> Result<(String, Vec<String>), String> {
    if !repo.install {
        return Err(format!("install is disabled for this repository: {}", repo.notes));
    }
    if let Some((program, args)) = repo.install_command.split_first() {
        return Ok((program.clone(), args.to_vec()));
    }
    let (program, args): (&str, &[&str]) = match repo.package_manager.as_str() {
        "pnpm" => ("pnpm", &["install", "--ignore-scripts", "--frozen-lockfile"]),
        "npm" => ("npm", &["ci", "--ignore-scripts"]),
        "bun" => ("bun", &["install", "--ignore-scripts", "--frozen-lockfile"]),
        "yarn" if dir.join(".yarnrc.yml").is_file() => ("yarn", &["install", "--immutable", "--mode=skip-build"]),
        "yarn" => ("yarn", &["install", "--frozen-lockfile", "--ignore-scripts"]),
        other => return Err(format!("no install step for package manager {other}")),
    };
    Ok((program.to_owned(), args.iter().map(|a| (*a).to_owned()).collect()))
}

/// Installs the dependencies of a checkout without running lifecycle scripts, then runs the repository's
/// `build-command` if it has one, within `build-timeout-secs` (the install's time limit when unset); `Some(reason)` when
/// either fails.
///
/// # Errors
/// Fails when a command cannot be started or the marker cannot be written.
pub fn install(repo: &Repo, dir: &Path, scratch: &Path, default_timeout: Duration) -> Result<Option<String>> {
    let timeout = repo.install_timeout_secs.map_or(default_timeout, Duration::from_secs);
    let build_timeout = repo.build_timeout_secs.map_or(timeout, Duration::from_secs);
    let marker = dir.join(INSTALLED_MARKER);
    if is_installed(repo, dir) {
        return Ok(None);
    }
    let (program, args) = match install_command(repo, dir) {
        Ok(command) => command,
        Err(reason) => return Ok(Some(reason)),
    };
    if let Some(reason) = run_step(repo, dir, scratch, ("install", &program, &args), timeout)? {
        return Ok(Some(reason));
    }
    if let Some((program, args)) = repo.build_command.split_first()
        && let Some(reason) = run_step(repo, dir, scratch, ("build", program, args), build_timeout)?
    {
        return Ok(Some(reason));
    }
    fs::write(&marker, install_marker(repo)).with_context(|| format!("writing {}", marker.display()))?;
    Ok(None)
}

/// Runs one step (`install` or `build`: its name, program and arguments) of the install in the checkout, with its
/// output captured in the scratch directory under the step's name; `Some(reason)` when it cannot run or fails.
fn run_step(
    repo: &Repo,
    dir: &Path,
    scratch: &Path,
    (step, program, args): (&str, &str, &[String]),
    timeout: Duration,
) -> Result<Option<String>> {
    if find_on_path(program).is_none() {
        return Ok(Some(format!("{program} is not installed")));
    }
    eprintln!("parity: {} {program} {}", repo.name, args.join(" "));
    let run = capture(
        Command::new(program)
            .args(args)
            .current_dir(dir)
            .env("CI", "1")
            .env("COREPACK_ENABLE_DOWNLOAD_PROMPT", "0"),
        scratch,
        step,
        timeout,
    )?;
    if run.timed_out {
        return Ok(Some(format!(
            "{program} {step} timed out after {} s",
            timeout.as_secs()
        )));
    }
    if run.code != Some(0) {
        return Ok(Some(format!("{program} {step} failed: {}", run.failure_summary())));
    }
    Ok(None)
}

/// What the install marker of a checkout holds once the install (and the build, if any) succeeded: the pinned commit,
/// then a line for the corpus `install-command` and one for the `build-command` when the repository sets them, so a
/// marker written before a command was added or changed is stale.
fn install_marker(repo: &Repo) -> String {
    let mut marker = repo.rev.clone();
    for (label, command) in [("install", &repo.install_command), ("build", &repo.build_command)] {
        if !command.is_empty() {
            let _ = write!(marker, "\n{label}: {}", command.join(" "));
        }
    }
    marker
}

/// Whether the dependencies of a checkout are installed at its pinned commit: its install marker holds the commit
/// (and the build command, see [`install_marker`]).
pub fn is_installed(repo: &Repo, dir: &Path) -> bool {
    fs::read_to_string(dir.join(INSTALLED_MARKER)).is_ok_and(|marker| marker.trim() == install_marker(repo))
}

/// How many of the compared repositories were installed: `yes`, `no` or `partial (n of m)`.
fn installed_summary(entries: &BTreeMap<String, Entry>) -> String {
    let compared: Vec<&Entry> = entries.values().filter(|entry| entry.status == COMPARED).collect();
    let installed = compared.iter().filter(|entry| entry.installed == Some(true)).count();
    match installed {
        0 => "no".to_owned(),
        n if n == compared.len() => "yes".to_owned(),
        n => format!("partial ({n} of {})", compared.len()),
    }
}

struct Outcome {
    entry: Entry,
    detail: Value,
    /// The typed-versus-untyped entry, when the untyped pass ran.
    type_aware: Option<Entry>,
}

fn skipped(reason: impl Into<String>, type_aware: Option<Entry>) -> Outcome {
    let reason = reason.into();
    Outcome {
        detail: json!({ "status": diff::SKIPPED, "reason": reason }),
        entry: Entry::skipped(reason),
        type_aware,
    }
}

/// The categories the comparison counts and the known deviations it sets apart, as the baseline records them.
fn category_set() -> CategorySet {
    let compared: BTreeSet<String> = KNIP_CATEGORIES
        .iter()
        .map(|(_, category)| (*category).to_owned())
        .collect();
    CategorySet {
        compared: compared.into_iter().collect(),
        known_deviations: KNIP_DEVIATIONS
            .iter()
            .map(|deviation| deviation.id.to_owned())
            .collect(),
    }
}

/// The findings of `own` in `category` that `other` does not report.
fn one_sided(own: &Keys, other: &Keys, category: &str) -> BTreeSet<String> {
    let none = BTreeSet::new();
    let theirs = other.get(category).unwrap_or(&none);
    own.get(category)
        .map_or_default(|keys| keys.difference(theirs).cloned().collect())
}

/// The unused files of one side of the comparison (its `files` findings).
fn unused_files(keys: &Keys) -> &BTreeSet<String> {
    static NONE: BTreeSet<String> = BTreeSet::new();
    keys.get("files").unwrap_or(&NONE)
}

/// skif's report on a checkout, and why its install or build failed, if it did.
struct SkifRun {
    doc: Value,
    run: Captured,
    recursive: bool,
    install_failure: Option<String>,
}

/// Installs the checkout when the run and the repository ask for it, then runs skif on it; `Ok(Err(reason))` when the
/// repository is skipped. A failed install or build does not skip it: both tools run on what the checkout holds,
/// which counts as not installed, and the failure is recorded with the run.
fn install_and_run_skif(
    options: &Options,
    skif: &Path,
    repo: &Repo,
    root: &Path,
    scratch: &Path,
) -> Result<Result<SkifRun, String>> {
    // A repository with `install = false` is compared uninstalled, so a tool that cannot run there says why itself.
    let install_failure = if options.install && repo.install {
        install(repo, root, scratch, options.timeout)?
    } else {
        None
    };
    if let Some(reason) = &install_failure {
        eprintln!("parity: {} is compared without a complete install: {reason}", repo.name);
    }
    let (doc, run) = match run_skif(options, skif, repo, (root, scratch), true)? {
        Ok(report) => report,
        Err(reason) => return Ok(Err(reason)),
    };
    Ok(Ok(SkifRun {
        doc,
        run,
        recursive: skif_recursive(options, repo),
        install_failure,
    }))
}

/// Whether skif runs with `--recursive` on the repository.
const fn skif_recursive(options: &Options, repo: &Repo) -> bool {
    options.recursive || repo.recursive
}

/// Runs skif on a checkout (`root`, with its `scratch` directory), with `--type-aware` or with `--no-type-aware`, and
/// reads its JSON report; `Ok(Err(reason))` when it produced none.
fn run_skif(
    options: &Options,
    skif: &Path,
    repo: &Repo,
    (root, scratch): (&Path, &Path),
    typed: bool,
) -> Result<Result<(Value, Captured), String>> {
    let mut command = Command::new(skif);
    command
        .args([".", "--format", "json", "--no-install"])
        .current_dir(root);
    if skif_recursive(options, repo) {
        command.arg("--recursive");
    }
    command.arg(if typed {
        type_aware::TYPE_AWARE_FLAG
    } else {
        type_aware::NO_TYPE_AWARE_FLAG
    });
    if let Some(config) = write_skif_entry_config(&repo.entry, scratch)? {
        command.arg("--config").arg(config);
    }
    let (name, label) = if typed {
        ("skif", "skif-parity")
    } else {
        (type_aware::TOOL, "skif-parity-untyped")
    };
    let run = capture(&mut command, scratch, label, options.timeout)?;
    if run.timed_out {
        return Ok(Err(format!("{name} timed out")));
    }
    let doc: Value = match serde_json::from_slice(&run.stdout) {
        Ok(doc) => doc,
        Err(error) => return Ok(Err(format!("{name} output is not JSON: {error}"))),
    };
    if doc.get("diagnostics").is_none() {
        let message = doc["error"]["message"]
            .as_str()
            .unwrap_or("no diagnostics in the output");
        return Ok(Err(format!("{name} failed: {message}")));
    }
    Ok(Ok((doc, run)))
}

/// knip's report on a checkout, with knip's workspaces as they were while it ran.
struct KnipRun {
    doc: Value,
    run: Captured,
    /// The workspace member directories (`workspace_members`), read while the scratch root manifest that declares
    /// them for a Rush repository is there.
    members: BTreeSet<String>,
    /// knip ran with the scratch root manifest of a Rush repository (`knip-root = "rush"`).
    rush_root: bool,
}

/// Runs the pinned knip on a checkout, with the scratch root manifest (`knip_root`) there only while it runs;
/// `Ok(Err(reason))` when the repository is skipped.
fn run_knip(
    options: &Options,
    tools: &Tools,
    repo: &Repo,
    root: &Path,
    scratch: &Path,
) -> Result<Result<KnipRun, String>> {
    let scratch_root = match knip_root(repo, root) {
        Ok(manifest) => manifest,
        Err(reason) => return Ok(Err(format!("knip root: {reason}"))),
    };
    let spec = format!("knip@{}", tools.knip);
    let mut command = Command::new("npx");
    command
        .args(["--yes", spec.as_str(), "--reporter", "json"])
        .current_dir(root)
        .env("CI", "1")
        .env("DOTENV_CONFIG_QUIET", "true");
    if !options.repo_knip_config {
        command.arg("--config").arg(write_knip_config(&repo.entry, scratch)?);
    }
    let run = capture(&mut command, scratch, "knip", options.timeout)?;
    let members = workspace_members(root);
    let rush_root = scratch_root.is_some();
    drop(scratch_root);
    if run.timed_out {
        return Ok(Err("knip timed out".to_owned()));
    }
    match parse_knip_report(&run.stdout) {
        Ok(doc) => Ok(Ok(KnipRun {
            doc,
            run,
            members,
            rush_root,
        })),
        Err(reason) => Ok(Err(format!(
            "knip failed (exit {}): {reason}; stderr: {}",
            run.code.map_or_else(|| "none".to_owned(), |c| c.to_string()),
            run.stderr_summary()
        ))),
    }
}

/// What the classification reads of the two reports and the checkout, owned so a [`Classifier`] can borrow it.
struct Facts {
    not_judged: BTreeMap<String, NotJudgedScope>,
    skif_root_packages: BTreeSet<String>,
    foreign_paths: BTreeSet<String>,
    /// The configuration files knip could not load (`knip_config_errors`).
    knip_errors: BTreeSet<String>,
    knip_failed: BTreeSet<String>,
    knip_manifests: BTreeSet<String>,
    knip_unused_imports: BTreeSet<String>,
    skif_unused_imports: BTreeSet<String>,
    published_reach: BTreeSet<String>,
    knip_unused_credits: BTreeSet<String>,
    skif_unused_credits: BTreeSet<String>,
    bazel_credits: BTreeSet<String>,
    signature_types: BTreeSet<String>,
    ambient_modules: BTreeSet<String>,
    own_paths: BTreeSet<String>,
    paths_before_packages: BTreeSet<String>,
    whole_enum_members: BTreeSet<String>,
    published_reexports: BTreeSet<String>,
    root_entry_reexports: BTreeSet<String>,
    bazel_sources: BTreeSet<String>,
    non_member_credits: BTreeSet<String>,
    skipped_credits: BTreeSet<String>,
    non_member_members: BTreeSet<String>,
    skipped_members: BTreeSet<String>,
    knip_dot_ignores: Vec<(String, Vec<String>)>,
    reexport_aliases: BTreeSet<String>,
    tsconfig_declarations: BTreeSet<String>,
    unimported_declarations: BTreeSet<String>,
    declaration_reach: BTreeSet<String>,
    module_object_types: BTreeSet<String>,
    uninstalled_projects: BTreeSet<String>,
    knip_unused_members: BTreeSet<String>,
    knip_dot_ignore_imports: BTreeSet<String>,
    knip_dot_ignore_credits: BTreeSet<String>,
    heft_credits: BTreeSet<String>,
    type_asserted_requires: BTreeSet<String>,
    module_object_values: BTreeSet<String>,
    commonjs_default_members: BTreeSet<String>,
    template_import_reach: BTreeSet<String>,
    installed_main_reach: BTreeSet<String>,
    type_referenced_members: BTreeSet<String>,
    raw_glob_reach: BTreeSet<String>,
    typescript_twins: BTreeSet<String>,
    knip_reads: KnipReadSets,
    dependency_reads: DependencyReads,
}

impl Facts {
    /// Reads the facts of one repository: `skif` and `knip` are the findings of each side, `installed` whether the
    /// harness installed the checkout.
    fn read(
        (skif_doc, knip_stderr): (&Value, &str),
        (skif, knip): (&Keys, &Keys),
        root: &Path,
        layout: &Layout<'_>,
        installed: bool,
    ) -> Self {
        let knip_errors = knip_config_errors(knip_stderr, root);
        let is_project = |dir: &str| layout.is_project(dir);
        let knip_failed = knip_errors.iter().map(|file| owner_dir(file, &is_project)).collect();
        let read_source = |file: &str| fs::read_to_string(root.join(file)).ok();
        let workspace_of = |file: &str| owner_dir(file, &|dir: &str| layout.is_workspace(dir));
        let (knip_unused, skif_unused) = (unused_files(knip), unused_files(skif));
        let resolver = Resolver::new(layout, &read_source);
        let (gitignores, files): (Vec<String>, Vec<String>) =
            checkout_files(root, &|name: &str| name == GITIGNORE || has_code_extension(name))
                .into_iter()
                .partition(|path| path.rsplit('/').next() == Some(GITIGNORE));
        let build_files = read_build_files(root, &is_project);
        let knip_unlisted = one_sided(knip, skif, "unlisted");
        let root_packages = skif_root_packages(skif_doc);
        let root_dirs = skif_root_dirs(skif_doc);
        let knip_exports: BTreeSet<String> = EXPORT_LABELS
            .iter()
            .flat_map(|category| one_sided(knip, skif, category))
            .collect();
        let entries_of: &dyn Fn(&str) -> PluginEntries = &|dir: &str| {
            (layout.manifest)(dir).map_or_else(PluginEntries::none, |manifest| {
                let members = packages_below(&root_packages, dir);
                plugin_entries(&OsFs, root, (dir, root_dirs.contains(dir)), &manifest, &members)
            })
        };
        let (mut published, defaults) = entry_modules(&knip_exports, &files, layout, (&root_packages, entries_of));
        let (loaded_sources, published_files) = published_file_entries(&knip_exports, &files, layout, &read_source);
        published.extend(loaded_sources);
        let skif_exports_one_sided = IMPORTED_EXPORT_CATEGORIES
            .iter()
            .any(|category| !one_sided(skif, knip, category).is_empty());
        let (non_member_credits, skipped_credits) = if skif_exports_one_sided {
            outside_credits(&files, layout, &resolver, &read_source)
        } else {
            (BTreeSet::new(), BTreeSet::new())
        };
        let skif_members: BTreeSet<String> = MEMBER_CATEGORIES
            .iter()
            .flat_map(|category| one_sided(skif, knip, category))
            .collect();
        let (non_member_members, skipped_members) = if skif_members.is_empty() {
            (BTreeSet::new(), BTreeSet::new())
        } else {
            outside_members(&skif_members, &files, layout, &resolver, &read_source)
        };
        let has_packages =
            |dir: &str| root.join(dir).join("node_modules").is_dir() || root.join(dir).join(".pnp.cjs").is_file();
        let tsconfig_declarations = workspace_declarations(&files, layout, &read_source);
        let knip_imports: BTreeSet<String> = IMPORT_CATEGORIES
            .iter()
            .flat_map(|category| one_sided(knip, skif, category))
            .collect();
        let skif_only_unused: BTreeSet<&str> = skif_unused.difference(knip_unused).map(String::as_str).collect();
        let skif_types: BTreeSet<String> = TYPE_CATEGORIES
            .iter()
            .flat_map(|category| one_sided(skif, knip, category))
            .collect();
        let knip_values: BTreeSet<String> = VALUE_EXPORT_CATEGORIES
            .iter()
            .flat_map(|category| one_sided(knip, skif, category))
            .collect();
        let object_credits = if skif_types.is_empty() && knip_values.is_empty() {
            BTreeSet::new()
        } else {
            module_object_credits(&files, &resolver, &read_source)
        };
        let knip_members: BTreeSet<String> = MEMBER_CATEGORIES
            .iter()
            .flat_map(|category| one_sided(knip, skif, category))
            .collect();
        let knip_only_unused: BTreeSet<&str> = knip_unused.difference(skif_unused).map(String::as_str).collect();
        let knip_dot_ignores = dot_slash_ignores(
            &gitignores
                .iter()
                .filter_map(|path| Some((path.clone(), read_source(path.as_str())?)))
                .collect::<Vec<_>>(),
        );
        let dot_ignored: BTreeSet<String> = files
            .iter()
            .filter(|file| in_dot_slash_ignore(&knip_dot_ignores, file))
            .cloned()
            .collect();
        let dependency_owners: BTreeSet<String> = IMPORTED_CATEGORIES
            .iter()
            .flat_map(|category| one_sided(knip, skif, category))
            .map(|key| parent_dir(key_path(&key)).to_owned())
            .collect();
        let file_owners: BTreeSet<String> = knip_only_unused
            .iter()
            .map(|file| owner_dir(file, &is_project))
            .collect();
        let knip_reads = Self::knip_reads(
            (skif, knip),
            (&files, &build_files),
            (layout, &resolver),
            &tsconfig_declarations,
            &read_source,
        );
        let ignored: RefCell<BTreeMap<String, bool>> = RefCell::default();
        let is_ignored = |path: &str| {
            *ignored
                .borrow_mut()
                .entry(path.to_owned())
                .or_insert_with(|| git_ignores(root, path))
        };
        let bazel_outputs = bazel_outputs(&build_files, layout.is_file, &read_source);
        let dependency_reads = dependency_reads(
            (skif, knip),
            &DependencyCheckout {
                layout,
                resolver: &resolver,
                files: &files,
                read: &read_source,
                is_ignored: &is_ignored,
                bazel_outputs: &bazel_outputs,
            },
        );
        Self {
            not_judged: not_judged_dirs(skif_doc),
            foreign_paths: foreign_tsconfig_paths_keys(skif_doc, root),
            knip_errors,
            knip_failed,
            knip_manifests: knip_manifests(knip),
            knip_unused_imports: with_peer_credits(
                unused_file_imports(knip_unused, skif_unused, &read_source, &workspace_of),
                layout.manifest,
            ),
            skif_unused_imports: unused_file_imports(skif_unused, knip_unused, &read_source, &workspace_of),
            published_reach: published_reach(knip_unused, skif_unused, layout, &read_source),
            knip_unused_credits: unused_file_credits(knip_unused, skif_unused, &resolver, &read_source),
            skif_unused_credits: unused_file_credits(skif_unused, knip_unused, &resolver, &read_source),
            bazel_credits: with_peer_credits(
                bazel_credits(&build_files, layout.is_file, &read_source),
                layout.manifest,
            ),
            bazel_sources: reach_from_sources(
                &bazel_sources(&build_files, &files, layout.is_file, &read_source),
                (knip_unused, skif_unused),
                layout.is_file,
                &read_source,
            ),
            signature_types: signature_types(skif, knip, &read_source),
            ambient_modules: ambient_module_keys(&knip_unlisted, &files, &read_source),
            own_paths: own_paths_keys(&knip_unlisted, &resolver, &read_source, &is_ignored),
            paths_before_packages: paths_before_packages(&files, &resolver, layout, &read_source),
            whole_enum_members: whole_enum_members(
                &one_sided(knip, skif, "enumMembers"),
                &files,
                &resolver,
                &read_source,
            ),
            published_reexports: {
                let mut credits = entry_reexports(published, &resolver, &read_source);
                credits.extend(published_file_reexports(published_files, &resolver, &read_source));
                credits
            },
            root_entry_reexports: entry_reexports(defaults, &resolver, &read_source),
            non_member_credits,
            skipped_credits,
            non_member_members,
            skipped_members,
            knip_dot_ignore_imports: with_peer_credits(
                unused_file_imports(&dot_ignored, skif_unused, &read_source, &workspace_of),
                layout.manifest,
            ),
            knip_dot_ignore_credits: unused_file_credits(&dot_ignored, skif_unused, &resolver, &read_source),
            knip_dot_ignores,
            reexport_aliases: reexport_alias_duplicates(&one_sided(skif, knip, "duplicates"), &read_source),
            unimported_declarations: unimported_declarations(
                &knip_imports
                    .iter()
                    .map(|key| key_path(key).to_owned())
                    .filter(|path| tsconfig_declarations.contains(path))
                    .collect(),
                &files,
                &resolver,
                &read_source,
            ),
            declaration_reach: if skif_only_unused.is_empty() {
                BTreeSet::new()
            } else {
                reach(
                    tsconfig_declarations.iter().map(String::as_str).collect(),
                    &skif_only_unused,
                    layout.is_file,
                    &read_source,
                )
            },
            module_object_types: module_objects(&skif_types, &object_credits),
            module_object_values: if knip_values.is_empty() {
                BTreeSet::new()
            } else {
                let mut credits = namespace_reexport_object_credits(&files, &resolver, &read_source);
                credits.extend(object_credits.iter().cloned());
                module_objects(&knip_values, &credits)
            },
            knip_unused_members: if knip_members.is_empty() {
                BTreeSet::new()
            } else {
                members_used_by(&knip_members, knip_only_unused.iter().copied(), &resolver, &read_source)
            },
            heft_credits: heft_credits(
                dependency_owners.iter().map(String::as_str),
                &OsFs,
                root,
                layout.manifest,
            ),
            type_asserted_requires: if knip_values.is_empty() {
                BTreeSet::new()
            } else {
                type_asserted_require_credits(&files, &resolver, &read_source)
            },
            commonjs_default_members: commonjs_default_members(&knip_values, &files, &resolver, &read_source),
            template_import_reach: if knip_only_unused.is_empty() {
                BTreeSet::new()
            } else {
                reach_from_sources(
                    &template_import_targets(&files, &read_source),
                    (knip_unused, skif_unused),
                    layout.is_file,
                    &read_source,
                )
            },
            installed_main_reach: reach_from_sources(
                &installed_main_imports(file_owners.iter().map(String::as_str), layout, &read_source),
                (knip_unused, skif_unused),
                layout.is_file,
                &read_source,
            ),
            type_referenced_members: if skif_members.is_empty() {
                BTreeSet::new()
            } else {
                type_referenced_members(&skif_members, &files, &resolver, &read_source)
            },
            raw_glob_reach: if skif_only_unused.is_empty() {
                BTreeSet::new()
            } else {
                reach_from_sources(
                    &raw_glob_targets(&files, &read_source),
                    (skif_unused, knip_unused),
                    layout.is_file,
                    &read_source,
                )
            },
            typescript_twins: typescript_twin_files(&skif_only_unused, &files, layout.is_file, &read_source),
            knip_reads,
            dependency_reads,
            tsconfig_declarations,
            uninstalled_projects: if installed {
                uninstalled_projects((skif, knip), &is_project, &has_packages)
            } else {
                BTreeSet::new()
            },
            skif_root_packages: root_packages,
        }
    }

    /// The facts of the deviations that follow from how knip reads a module ([`KnipReadSets`]): `skif` and `knip`
    /// are the findings of each side, `files` the checkout's code files and `build_files` its `BUILD` files with their
    /// sources. Each fact is only gathered when a one-sided finding it can classify exists.
    fn knip_reads(
        (skif, knip): (&Keys, &Keys),
        (files, build_files): (&[String], &[(String, String)]),
        (layout, resolver): (&Layout<'_>, &Resolver<'_>),
        tsconfig_declarations: &BTreeSet<String>,
        read: &SourceReader<'_>,
    ) -> KnipReadSets {
        let collect = |own: &Keys, other: &Keys, categories: &[&str]| -> BTreeSet<String> {
            categories
                .iter()
                .flat_map(|category| one_sided(own, other, category))
                .collect()
        };
        let skif_exports = collect(skif, knip, &EXPORT_LABELS);
        let skif_values = collect(skif, knip, &VALUE_EXPORT_CATEGORIES);
        let knip_exports = collect(knip, skif, &EXPORT_LABELS);
        let reads = if skif_exports.is_empty() && knip_exports.is_empty() {
            KnipReadSets::default()
        } else {
            knip_read_credits(files, resolver, read)
        };
        let is_project = |dir: &str| layout.is_project(dir);
        KnipReadSets {
            namespace_reexports: namespace_reexport_keys(&skif_values, read),
            typescript_commonjs: typescript_commonjs_keys(&skif_values, read),
            declaration_reexports: if skif_exports.is_empty() {
                BTreeSet::new()
            } else {
                entry_reexports(tsconfig_declarations.iter().cloned().collect(), resolver, read)
            },
            node_test_files: node_test_files(&skif_exports, layout),
            ng_package_reexports: if knip_exports.is_empty() {
                BTreeSet::new()
            } else {
                entry_reexports(
                    ng_package_entries(build_files, &is_project, layout.is_file, read),
                    resolver,
                    read,
                )
            },
            ..reads
        }
    }

    /// The classifier of one repository's one-sided findings (`skif` and `knip` are the findings of each side).
    fn classifier<'a>(
        &'a self,
        repo: &'a Repo,
        options: &Options,
        (skif, knip): (&'a Keys, &'a Keys),
        rush_root: bool,
        layout: &'a Layout<'a>,
    ) -> Classifier<'a> {
        Classifier {
            knip_unread: &repo.knip_unread,
            include_test_files: options.include_test_files,
            not_judged: &self.not_judged,
            knip_failed: &self.knip_failed,
            knip_manifests: &self.knip_manifests,
            foreign_paths: &self.foreign_paths,
            knip_unused_files: unused_files(knip),
            skif_unused_files: unused_files(skif),
            knip_unused_imports: &self.knip_unused_imports,
            skif_unused_imports: &self.skif_unused_imports,
            skif_root_packages: &self.skif_root_packages,
            published_reach: &self.published_reach,
            knip_unused_credits: &self.knip_unused_credits,
            skif_unused_credits: &self.skif_unused_credits,
            bazel_credits: &self.bazel_credits,
            signature_types: &self.signature_types,
            ambient_modules: &self.ambient_modules,
            own_paths: &self.own_paths,
            paths_before_packages: &self.paths_before_packages,
            whole_enum_members: &self.whole_enum_members,
            published_reexports: &self.published_reexports,
            root_entry_reexports: &self.root_entry_reexports,
            bazel_sources: &self.bazel_sources,
            non_member_credits: &self.non_member_credits,
            skipped_credits: &self.skipped_credits,
            non_member_members: &self.non_member_members,
            skipped_members: &self.skipped_members,
            knip_dot_ignores: &self.knip_dot_ignores,
            reexport_aliases: &self.reexport_aliases,
            tsconfig_declarations: &self.tsconfig_declarations,
            unimported_declarations: &self.unimported_declarations,
            declaration_reach: &self.declaration_reach,
            module_object_types: &self.module_object_types,
            uninstalled_projects: &self.uninstalled_projects,
            knip_unused_members: &self.knip_unused_members,
            knip_dot_ignore_imports: &self.knip_dot_ignore_imports,
            knip_dot_ignore_credits: &self.knip_dot_ignore_credits,
            heft_credits: &self.heft_credits,
            type_asserted_requires: &self.type_asserted_requires,
            module_object_values: &self.module_object_values,
            commonjs_default_members: &self.commonjs_default_members,
            template_import_reach: &self.template_import_reach,
            installed_main_reach: &self.installed_main_reach,
            type_referenced_members: &self.type_referenced_members,
            raw_glob_reach: &self.raw_glob_reach,
            typescript_twins: &self.typescript_twins,
            knip_reads: &self.knip_reads,
            dependency_reads: &self.dependency_reads,
            rush_root,
            replaced_knip_config: !options.repo_knip_config,
            layout,
        }
    }
}

/// The narrowed packages of `not-judged-by-skif` with the globs of their scope, for `parity.json`.
fn narrowed_packages(not_judged: &BTreeMap<String, NotJudgedScope>) -> serde_json::Map<String, Value> {
    not_judged
        .iter()
        .filter_map(|(dir, scope)| match scope {
            NotJudgedScope::Within(globs) => Some((dir.clone(), json!(globs.patterns()))),
            NotJudgedScope::Package => None,
        })
        .collect()
}

fn parity_repo(
    options: &Options,
    skif: &Path,
    tools: &Tools,
    repo: &Repo,
    dir: &Path,
    skif_rev: &str,
) -> Result<Outcome> {
    let root = fs::canonicalize(dir).with_context(|| format!("resolving {}", dir.display()))?;
    let scratch = options.out.join("scratch").join(&repo.name);
    let skif_run = match install_and_run_skif(options, skif, repo, &root, &scratch)? {
        Ok(run) => run,
        Err(reason) => {
            let type_aware = options.untyped_pass.then(|| Entry::skipped(reason.clone()));
            return Ok(skipped(reason, type_aware));
        }
    };
    let installed = is_installed(repo, &root);
    let pass = if options.untyped_pass {
        Some(untyped_pass(
            options,
            skif,
            repo,
            (&root, &scratch),
            &skif_run,
            (installed, skif_rev),
        )?)
    } else {
        None
    };
    let knip_run = match run_knip(options, tools, repo, &root, &scratch)? {
        Ok(run) => run,
        Err(reason) => return Ok(skipped(reason, pass.map(|pass| pass.entry))),
    };
    let knip_found = knip_keys(&knip_run.doc, &root);
    let skif_found = compared_skif_keys(&skif_run.doc, &root, &knip_found);
    let untyped_found = pass
        .as_ref()
        .and_then(|pass| pass.doc.as_ref())
        .map(|doc| compared_skif_keys(doc, &root, &knip_found));
    let manifest = manifest_reader(&root);
    let is_file = |file: &str| root.join(file).is_file();
    let build_dirs = build_dir_reader(&root);
    let layout = Layout::new(knip_run.members, &manifest, &is_file).with_build_dirs(&build_dirs);
    let facts = Facts::read(
        (&skif_run.doc, &knip_run.run.stderr),
        (&skif_found, &knip_found),
        &root,
        &layout,
        installed,
    );
    let classifier = facts.classifier(repo, options, (&skif_found, &knip_found), knip_run.rush_root, &layout);
    let split = diff::split_known_sampled(
        &skif_found,
        &knip_found,
        &|side, category, key| {
            with_type_resolved(
                classifier.classify(side, category, key),
                (side, category, key),
                untyped_found.as_ref(),
            )
        },
        options.samples,
    );
    let (counts, samples) = diff::compare_sampled(&split.skif, &split.tool, options.samples);
    let detail = json!({
        "status": COMPARED,
        "installed": installed,
        "install_failure": skif_run.install_failure,
        "recursive": skif_run.recursive,
        "skif_rev": skif_rev,
        "knip_exit": knip_run.run.code,
        "knip_errors": facts.knip_errors,
        "knip_invalid": repo.knip_invalid,
        "known_deviations": split.known,
        "known_samples": split.samples,
        "not_judged_packages": facts.not_judged.keys().collect::<Vec<_>>(),
        "narrowed_packages": narrowed_packages(&facts.not_judged),
        "workspace_members": layout.members.len(),
        "skif_ms": skif_run.run.elapsed.as_millis(),
        "knip_ms": knip_run.run.elapsed.as_millis(),
        "skif_total": total(&skif_found),
        "knip_total": total(&knip_found),
        "categories": category_detail(&counts, &samples),
        "type_aware": pass.as_ref().map(|pass| &pass.detail),
    });
    Ok(Outcome {
        entry: Entry::compared(counts)
            .with_known(split.known)
            .with_tool_errors(facts.knip_errors.len())
            .with_invalid(repo.knip_invalid.clone())
            .with_installed(installed)
            .with_run(skif_run.recursive, skif_rev),
        detail,
        type_aware: pass.map(|pass| pass.entry),
    })
}

/// The deviation of a one-sided finding (`side`, `category`, `key`): the one the classifier names (`known`), else
/// `type-resolved` where the untyped run's findings (`untyped`, by knip category) settle it the other way.
fn with_type_resolved(
    known: Option<&'static str>,
    (side, category, key): (Side, &str, &str),
    untyped: Option<&Keys>,
) -> Option<&'static str> {
    known.or_else(|| {
        untyped
            .is_some_and(|untyped| type_aware::type_resolved(side, category, key, untyped))
            .then_some(TYPE_RESOLVED)
    })
}

/// skif's findings in the knip comparison, aligned with knip's (`knip`) where the two name one finding apart.
fn compared_skif_keys(doc: &Value, root: &Path, knip: &Keys) -> Keys {
    let mut keys = skif_keys(doc, root);
    align_export_labels(&mut keys, knip);
    align_unlisted_unresolved(&mut keys, knip);
    align_stale_mocks(&mut keys, knip, &stale_mock_keys(doc, root));
    keys
}

/// The untyped pass of one repository: its entry of the type-aware baseline, its detail in the full report and the
/// untyped report, when skif produced one.
struct UntypedPass {
    entry: Entry,
    detail: Value,
    doc: Option<Value>,
}

/// Runs skif with `--no-type-aware` on a checkout (`root`, with its `scratch` directory) and compares its findings
/// per rule with those of the typed run.
fn untyped_pass(
    options: &Options,
    skif: &Path,
    repo: &Repo,
    dirs: (&Path, &Path),
    typed: &SkifRun,
    (installed, skif_rev): (bool, &str),
) -> Result<UntypedPass> {
    let (doc, run) = match run_skif(options, skif, repo, dirs, false)? {
        Ok(report) => report,
        Err(reason) => {
            eprintln!("parity: {} has no untyped pass: {reason}", repo.name);
            return Ok(UntypedPass {
                detail: json!({ "status": diff::SKIPPED, "reason": reason }),
                entry: Entry::skipped(reason),
                doc: None,
            });
        }
    };
    let (root, _) = dirs;
    let (counts, samples) = diff::compare_sampled(
        &type_aware::rule_keys(&typed.doc, root),
        &type_aware::rule_keys(&doc, root),
        options.samples,
    );
    Ok(UntypedPass {
        detail: json!({
            "status": COMPARED,
            "untyped_ms": run.elapsed.as_millis(),
            "rules": type_aware::rule_detail(&counts, &samples),
        }),
        entry: Entry::compared(counts)
            .with_installed(installed)
            .with_run(typed.recursive, skif_rev),
        doc: Some(doc),
    })
}

/// The root `package.json` written into a checkout for the knip run; dropping it removes the file, so skif and the
/// syncpack diff never see it. An interrupted run leaves it behind, and `corpus::remove_scratch_root` removes it when
/// the checkout is next used.
struct ScratchRoot {
    path: PathBuf,
}

impl Drop for ScratchRoot {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_file(&self.path) {
            eprintln!(
                "parity: could not remove the scratch root manifest {}: {error}",
                self.path.display()
            );
        }
    }
}

/// The root manifest knip needs where the checkout has none: for `knip-root = "rush"`, a `package.json` in the
/// checkout whose `workspaces` are the `projectFolder`s of `rush.json`, there until the returned guard is dropped.
/// knip has no Rush support and needs a root manifest; running in the checkout itself gives it the `node_modules` of
/// the install. `None` when knip runs in the checkout as it is; `Err` says why the manifest could not be written.
fn knip_root(repo: &Repo, root: &Path) -> Result<Option<ScratchRoot>, String> {
    match repo.knip_root.as_deref() {
        None => Ok(None),
        Some(RUSH_KNIP_ROOT) => {
            let rush =
                fs::read_to_string(root.join("rush.json")).map_err(|error| format!("reading rush.json: {error}"))?;
            let manifest = rush_root_manifest(&rush)?;
            let path = root.join("package.json");
            if path.exists() && !is_scratch_root(&path) {
                return Err(format!(
                    "{} exists, so the repository needs no knip-root",
                    path.display()
                ));
            }
            fs::write(&path, manifest).map_err(|error| format!("writing the scratch package.json: {error}"))?;
            Ok(Some(ScratchRoot { path }))
        }
        Some(other) => Err(format!("unknown knip-root {other:?}")),
    }
}

/// The root `package.json` knip gets for a Rush repository: private, marked as the harness's own
/// ([`SCRATCH_ROOT_MARK`]), with the `projectFolder` of every project of `rush.json` (JSON with comments) as its
/// `workspaces`.
fn rush_root_manifest(rush_json: &str) -> Result<String, String> {
    let rush: Value =
        serde_json::from_str(&strip_jsonc(rush_json)).map_err(|error| format!("parsing rush.json: {error}"))?;
    let folders: Vec<&str> = rush
        .get("projects")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|project| project.get("projectFolder").and_then(Value::as_str))
        .collect();
    if folders.is_empty() {
        return Err("rush.json lists no projectFolder".to_owned());
    }
    Ok(json!({ "//": SCRATCH_ROOT_MARK, "private": true, "workspaces": folders }).to_string() + "\n")
}

/// JSON with comments made plain JSON: `//` and `/* */` comments outside strings are dropped, and so is a comma
/// before a closing `]` or `}`.
fn strip_jsonc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    while let Some(character) = chars.next() {
        if in_string {
            out.push(character);
            match character {
                '\\' => out.extend(chars.next()),
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match (character, chars.peek().copied()) {
            ('"', _) => {
                in_string = true;
                out.push(character);
            }
            ('/', Some('/')) => while chars.next_if(|next| *next != '\n').is_some() {},
            ('/', Some('*')) => {
                chars.next();
                let mut previous = ' ';
                for next in chars.by_ref() {
                    if previous == '*' && next == '/' {
                        break;
                    }
                    previous = next;
                }
                out.push(' ');
            }
            (']' | '}', _) => {
                let kept = out.trim_end().len();
                if out[..kept].ends_with(',') {
                    out.remove(kept - 1);
                }
                out.push(character);
            }
            _ => out.push(character),
        }
    }
    out
}

/// An empty knip configuration, so the repository's own settings (ignores, `files: off`, ignored workspaces) do not
/// hide findings. Returns its absolute path.
pub fn empty_knip_config(scratch: &Path) -> Result<PathBuf> {
    write_config(scratch, "knip-empty.json", &knip_config(&[]))
}

/// The root workspace's entry files: the defaults, then the corpus-level ones.
fn root_entry(entry: &[String]) -> Vec<String> {
    DEFAULT_ENTRY
        .iter()
        .map(|glob| (*glob).to_owned())
        .chain(entry.iter().cloned())
        .collect()
}

/// The knip configuration: empty, so the repository's own settings (ignores, `files: off`, ignored workspaces) do not
/// hide findings, apart from the corpus-level entry files of the root workspace.
fn knip_config(entry: &[String]) -> String {
    let config = if entry.is_empty() {
        json!({})
    } else {
        json!({ "workspaces": { ".": { "entry": root_entry(entry) } } })
    };
    config.to_string() + "\n"
}

/// The skif configuration that adds the corpus-level entry files to the root workspace. A JSON array of strings is a
/// TOML array as well.
fn skif_entry_config(entry: &[String]) -> String {
    format!("[js.workspaces.\".\"]\nentry = {}\n", json!(root_entry(entry)))
}

/// Writes the knip configuration of the comparison ([`knip_config`]) with the corpus-level `entry` files into the
/// scratch directory and returns its absolute path.
///
/// # Errors
/// Fails when the file cannot be written.
pub fn write_knip_config(entry: &[String], scratch: &Path) -> Result<PathBuf> {
    write_config(scratch, "knip-config.json", &knip_config(entry))
}

/// Writes the skif configuration that adds the corpus-level `entry` files ([`skif_entry_config`]) into the scratch
/// directory and returns its absolute path; `None`, writing nothing, when there are none.
///
/// # Errors
/// Fails when the file cannot be written.
pub fn write_skif_entry_config(entry: &[String], scratch: &Path) -> Result<Option<PathBuf>> {
    if entry.is_empty() {
        return Ok(None);
    }
    write_config(scratch, "skif-entry.toml", &skif_entry_config(entry)).map(Some)
}

/// Writes a configuration file into the scratch directory and returns its absolute path.
fn write_config(scratch: &Path, name: &str, contents: &str) -> Result<PathBuf> {
    fs::create_dir_all(scratch).with_context(|| format!("creating {}", scratch.display()))?;
    let path = fs::canonicalize(scratch)?.join(name);
    fs::write(&path, contents).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

fn category_detail(counts: &BTreeMap<String, Counts>, samples: &BTreeMap<String, Samples>) -> Value {
    Value::Object(
        counts
            .iter()
            .map(|(category, found)| {
                (
                    category.clone(),
                    json!({
                        "skif_only": found.skif_only,
                        "knip_only": found.tool_only,
                        "both": found.both,
                        "samples": samples.get(category).map(|s| json!({ "skif_only": s.skif_only, "knip_only": s.tool_only })),
                    }),
                )
            })
            .collect(),
    )
}

fn markdown(tool: &str, entries: &BTreeMap<String, Entry>) -> String {
    let mut out = format!(
        "# skif findings parity with {tool}\n\nDependencies installed: {}.\n\n| Repository | Status | skif only | {tool} only | Both | Not judged by skif | Known deviations |\n| --- | --- | ---: | ---: | ---: | ---: | ---: |\n",
        installed_summary(entries)
    );
    for (name, entry) in entries {
        if entry.status == COMPARED {
            let sum = |f: fn(&Counts) -> usize| entry.categories.values().map(f).sum::<usize>();
            let unjudged = entry
                .known_deviations
                .get(NOT_JUDGED)
                .map_or(0, |c| c.skif_only + c.tool_only);
            let mut status = if entry.tool_errors == 0 {
                "compared".to_owned()
            } else {
                format!("compared; {tool} could not load {} config files", entry.tool_errors)
            };
            if let Some(reason) = &entry.invalid {
                let _ = write!(status, "; not a valid comparison: {reason}");
            }
            let _ = writeln!(
                out,
                "| {} | {} | {} | {} | {} | {unjudged} | {} |",
                table_cell(name),
                table_cell(&status),
                sum(|c| c.skif_only),
                sum(|c| c.tool_only),
                sum(|c| c.both),
                diff::known_total(&entry.known_deviations) - unjudged
            );
        } else {
            let _ = writeln!(
                out,
                "| {} | skipped: {} | - | - | - | - | - |",
                table_cell(name),
                table_cell(entry.reason.as_deref().unwrap_or(""))
            );
        }
    }
    out + &diff::known_section(tool, &KNIP_DEVIATIONS, entries)
}

/// Runs the comparison on the parity suite and applies the baseline gate to the knip comparison and, with the untyped
/// pass, to the typed and untyped counts.
///
/// # Errors
/// Fails when npx or skif is missing, the corpus cannot be fetched, a command cannot start, the report cannot be
/// written, or a gate finds drift.
pub fn run(options: &Options) -> Result<()> {
    if (options.repo_knip_config || options.include_test_files) && (options.gate.write || options.gate.check) {
        bail!(
            "--repo-knip-config and --include-test-files change what is compared: the baselines are recorded without them"
        );
    }
    if !options.untyped_pass && options.gate.write {
        bail!(
            "--no-untyped-pass leaves the type-resolved findings in the compared counts: the baselines are recorded with the untyped pass"
        );
    }
    if find_on_path("npx").is_none() {
        bail!("npx is required to run the pinned knip: install Node.js");
    }
    let skif = fs::canonicalize(&options.skif).with_context(|| {
        format!(
            "skif binary not found at {} (build it or pass --skif)",
            options.skif.display()
        )
    })?;
    let suite = corpus::fetch_suite(&options.corpus_file, "parity", &options.only, &options.corpus_dir)?;
    let tools = &suite.tools;
    let tool = format!("knip@{}", tools.knip);
    let skif_rev = provenance::source_rev();
    let mut entries = BTreeMap::new();
    let mut type_entries = BTreeMap::new();
    let mut details = serde_json::Map::new();
    for (repo, dir) in &suite.repos {
        eprintln!("parity: {}", repo.name);
        let outcome = parity_repo(options, &skif, tools, repo, dir, &skif_rev)?;
        if let Some(reason) = &outcome.entry.reason {
            eprintln!("parity: {} skipped: {reason}", repo.name);
        }
        details.insert(repo.name.clone(), outcome.detail);
        entries.insert(repo.name.clone(), outcome.entry);
        if let Some(entry) = outcome.type_aware {
            type_entries.insert(repo.name.clone(), entry);
        }
    }
    let table = markdown(&tool, &entries) + &type_aware::markdown(&type_entries);
    fs::create_dir_all(&options.out).with_context(|| format!("creating {}", options.out.display()))?;
    let full = json!({
        "tool": tool,
        "installed": installed_summary(&entries),
        "install_requested": options.install,
        "skif_rev": skif_rev,
        "mode": diff::TYPE_AWARE,
        "untyped_pass": options.untyped_pass,
        "recursive": options.recursive,
        "repo_knip_config": options.repo_knip_config,
        "include_test_files": options.include_test_files,
        "repos": details,
    });
    fs::write(
        options.out.join("parity.json"),
        serde_json::to_string_pretty(&full)? + "\n",
    )?;
    fs::write(options.out.join("parity.md"), &table)?;
    println!("{table}");
    let current = Baseline {
        schema: BASELINE_SCHEMA,
        tool,
        mode: Some(diff::TYPE_AWARE.to_owned()),
        installed: entries.values().all(|entry| entry.installed != Some(false)),
        category_set: Some(category_set()),
        entries,
    };
    let compared = diff::conclude(&options.gate, &current);
    if !options.untyped_pass {
        if options.gate.check {
            eprintln!(
                "baseline: no untyped pass, so {} is not checked and type-resolved findings count as one-sided",
                options.type_aware_baseline.display()
            );
        }
        return compared;
    }
    let type_gate = Gate {
        baseline: options.type_aware_baseline.clone(),
        ..options.gate.clone()
    };
    let typed = diff::conclude(&type_gate, &type_aware::baseline(type_entries));
    compared.and(typed)
}

/// The deviations that follow from how knip reads dependencies, imports and commands where skif reads them otherwise
/// (ADR 0021), by `<category>|<key>` of each one-sided `unlisted`, `unresolved` and `binaries` finding they explain,
/// for each side ([`dependency_reads`]).
#[derive(Default)]
struct DependencyReads {
    tool: BTreeMap<String, &'static str>,
    skif: BTreeMap<String, &'static str>,
}

impl DependencyReads {
    /// The deviation a one-sided finding of `side` is, when one of these reads explains it.
    fn deviation(&self, side: Side, category: &str, key: &str) -> Option<&'static str> {
        let reads = match side {
            Side::Skif => &self.skif,
            Side::Tool => &self.tool,
        };
        reads.get(&format!("{category}|{key}")).copied()
    }
}

/// What the rules of [`dependency_reads`] read of a checkout.
struct DependencyCheckout<'c> {
    layout: &'c Layout<'c>,
    resolver: &'c Resolver<'c>,
    /// The code files of the checkout ([`checkout_files`]).
    files: &'c [String],
    read: &'c SourceReader<'c>,
    /// Whether git ignores a path of the checkout, existing or not ([`git_ignores`]).
    is_ignored: &'c FileProbe<'c>,
    /// The paths, without their extension, that the `BUILD` files of a `rules_js` repository declare as outputs
    /// ([`bazel_outputs`]).
    bazel_outputs: &'c BTreeSet<String>,
}

/// Whether a one-sided finding key (`<file>|<name>`) follows from a read of the checkout.
type DependencyRule = fn(&DependencyCheckout<'_>, &str) -> bool;

/// The rules for the one-sided findings knip reports: the category, the deviation and the test of a key.
const KNIP_DEPENDENCY_RULES: [(&str, &str, DependencyRule); 15] = [
    ("unlisted", DATA_URI, cut_data_uri),
    ("unlisted", MODULE_AUGMENTATION, augments_module),
    ("unlisted", JEST_MOCK_SPECIFIER, jest_mocks_package),
    ("unlisted", NEXTRA_ALIAS, nextra_alias),
    ("unlisted", EXPO_UPDATES, expo_updates),
    ("unresolved", NEXTRA_ALIAS, nextra_alias),
    ("unresolved", BAZEL_BUILD_FILE, bazel_output_import),
    ("unresolved", GITIGNORED_IMPORT, gitignored_import),
    ("unresolved", HEFT_CONFIG, rig_profile_import),
    ("unresolved", SHARED_CONFIG_REFERENCE, shared_config_reference),
    ("unresolved", SHELL_TEMPLATE_DIRECTORY, shell_template_directory),
    ("unresolved", NESTED_VITEST_SETUP, nested_vitest_setup),
    ("binaries", PASSTHROUGH_COMMAND, passthrough_command),
    ("binaries", BUN_FILE_ARGUMENT, bun_file_argument),
    ("binaries", COMMAND_DIRECTORY, command_directory),
];

/// The rules for the one-sided findings skif reports.
const SKIF_DEPENDENCY_RULES: [(&str, &str, DependencyRule); 2] = [
    ("unlisted", OPTIONAL_PEER_HOST, optional_peer_host),
    ("binaries", REQUIRED_CHILD_PROCESS, required_child_process),
];

/// The deviations of the one-sided findings of each side that a rule of [`KNIP_DEPENDENCY_RULES`] or
/// [`SKIF_DEPENDENCY_RULES`] explains, the first that applies to a finding.
fn dependency_reads((skif, knip): (&Keys, &Keys), checkout: &DependencyCheckout<'_>) -> DependencyReads {
    DependencyReads {
        tool: rule_matches((knip, skif), &KNIP_DEPENDENCY_RULES, checkout),
        skif: rule_matches((skif, knip), &SKIF_DEPENDENCY_RULES, checkout),
    }
}

/// `<category>|<key>` of each finding of `own` that `other` does not report, with the deviation of the first rule
/// that applies to it.
fn rule_matches(
    (own, other): (&Keys, &Keys),
    rules: &[(&str, &'static str, DependencyRule)],
    checkout: &DependencyCheckout<'_>,
) -> BTreeMap<String, &'static str> {
    let mut found = BTreeMap::new();
    for (category, id, applies) in rules {
        for key in one_sided(own, other, category) {
            let entry = format!("{category}|{key}");
            if !found.contains_key(&entry) && applies(checkout, &key) {
                found.insert(entry, *id);
            }
        }
    }
    found
}

/// The module `path` holds, as skif's parser reads it; `None` for a file it cannot read or parse.
fn parsed_module(read: &SourceReader<'_>, path: &str) -> Option<ModuleInfo> {
    let options = ParseOptions {
        in_file_references: false,
        member_accesses: false,
    };
    read(path).and_then(|source| parse_module(Path::new(path), &source, &options).ok())
}

/// What knip names a `data:` specifier by when it cuts it at its first colon (`sanitizeSpecifier` in its
/// `util/modules.js`).
const DATA_SCHEME: &str = "data";

/// Whether an unlisted-dependency key names [`DATA_SCHEME`] in a file that holds a quoted `data:` URI.
fn cut_data_uri(checkout: &DependencyCheckout<'_>, key: &str) -> bool {
    key.split_once('|').is_some_and(|(path, name)| {
        name == DATA_SCHEME
            && (checkout.read)(path).is_some_and(|source| {
                ['\'', '"', '`']
                    .iter()
                    .any(|quote| source.contains(&format!("{quote}{DATA_SCHEME}:")))
            })
    })
}

/// Whether the file of an unlisted-dependency key augments the package it names (`declare module 'name' { … }` in a
/// module file, `augmented_modules` of skif's parser).
fn augments_module(checkout: &DependencyCheckout<'_>, key: &str) -> bool {
    let Some((path, name)) = key.split_once('|') else {
        return false;
    };
    let package = bare_package(name);
    parsed_module(checkout.read, path).is_some_and(|module| {
        module
            .augmented_modules
            .iter()
            .any(|augmented| augmented == name || (package.is_some() && bare_package(augmented) == package))
    })
}

/// The mock calls of Jest that knip's Jest plugin records as an import of the module they name
/// (`plugins/jest/visitors/mock.js`), as skif's parser names them.
const KNIP_JEST_MOCKS: [MockCall; 7] = [
    MockCall::JestMock,
    MockCall::JestDoMock,
    MockCall::JestUnmock,
    MockCall::JestDontMock,
    MockCall::JestRequireActual,
    MockCall::JestRequireMock,
    MockCall::JestCreateMockFromModule,
];

/// Whether the file of an unlisted-dependency key names the package in one of [`KNIP_JEST_MOCKS`].
fn jest_mocks_package(checkout: &DependencyCheckout<'_>, key: &str) -> bool {
    let Some((path, name)) = key.split_once('|') else {
        return false;
    };
    let Some(package) = bare_package(name) else {
        return false;
    };
    parsed_module(checkout.read, path).is_some_and(|module| {
        module.imports.iter().any(|import| {
            matches!(import.kind, ImportKind::Mock)
                && import.call.is_some_and(|call| KNIP_JEST_MOCKS.contains(&call))
                && bare_package(&import.specifier) == Some(package)
        })
    })
}

/// The name of a Next.js configuration without its extension.
const NEXT_CONFIG_STEM: &str = "next.config";

/// The package whose wrapper of a Next.js configuration adds `turbopack.resolveAlias` entries.
const NEXTRA: &str = "nextra";

/// Whether the file of an unlisted-dependency or unresolved-import key is a Next.js configuration that imports
/// [`NEXTRA`] and does not hold the specifier.
fn nextra_alias(checkout: &DependencyCheckout<'_>, key: &str) -> bool {
    let Some((path, specifier)) = key.split_once('|') else {
        return false;
    };
    let name = path.rsplit('/').next().unwrap_or(path);
    let is_config = name
        .rsplit_once('.')
        .is_some_and(|(stem, extension)| stem == NEXT_CONFIG_STEM && CODE_EXTENSIONS.contains(&extension));
    is_config
        && (checkout.read)(path)
            .is_some_and(|source| !source.contains(specifier) && imported_packages(path, &source).contains(NEXTRA))
}

/// The package knip's Expo plugin names for every app that does not turn updates off.
const EXPO_UPDATES_PACKAGE: &str = "expo-updates";

/// Whether an unlisted-dependency key names [`EXPO_UPDATES_PACKAGE`] at an Expo app configuration (`app.json`, or
/// `app.config.` with `json` or a code extension).
fn expo_updates(_: &DependencyCheckout<'_>, key: &str) -> bool {
    key.split_once('|').is_some_and(|(path, name)| {
        let file = path.rsplit('/').next().unwrap_or(path);
        name == EXPO_UPDATES_PACKAGE
            && (file == "app.json"
                || file
                    .strip_prefix("app.config.")
                    .is_some_and(|extension| extension == "json" || CODE_EXTENSIONS.contains(&extension)))
    })
}

/// The attributes of a Bazel rule that declare the files it writes (ADR 0031).
const BAZEL_OUTPUT_ATTRIBUTES: [&str; 3] = ["outs", "out", "stdout"];

/// The files the `BUILD` files of a `rules_js` repository ([`rules_js_build_files`]) declare in one of
/// [`BAZEL_OUTPUT_ATTRIBUTES`] of a top-level call, each a literal that names a file of the `BUILD` file's package
/// ([`bazel_package_path`]), without its extension ([`output_stem`]), as skif's Bazel plugin takes a missing relative
/// import of one, with any extension or none, for generated code.
fn bazel_outputs(
    build_files: &[(String, String)],
    is_file: &FileProbe<'_>,
    read: &SourceReader<'_>,
) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for (path, source, _) in rules_js_build_files(build_files, is_file, read) {
        let dir = parent_dir(path);
        for call in rule_calls(source) {
            for (attribute, literal) in &call.literals {
                if BAZEL_OUTPUT_ATTRIBUTES.contains(&attribute.as_str())
                    && let Some(file) = bazel_package_path(literal)
                {
                    found.insert(output_stem(&joined(dir, file)).to_owned());
                }
            }
        }
    }
    found
}

/// `path` without a declaration extension ([`DECLARATION_EXTENSIONS`]) or the last extension of its file name.
fn output_stem(path: &str) -> &str {
    if let Some(stem) = DECLARATION_EXTENSIONS
        .iter()
        .find_map(|extension| path.strip_suffix(extension))
    {
        return stem;
    }
    let name = path.rfind('/').map_or(0, |at| at + 1);
    path[name..]
        .rfind('.')
        .filter(|dot| *dot > 0)
        .map_or(path, |dot| &path[..name + dot])
}

/// The path a relative specifier of an import key (`<file>|<specifier>`) names when no file is there.
fn missing_relative(checkout: &DependencyCheckout<'_>, key: &str) -> Option<String> {
    let (path, specifier) = key.split_once('|')?;
    let dir = parent_dir(path);
    if !is_relative_specifier(specifier) || resolve_relative(dir, specifier, checkout.layout.is_file).is_some() {
        return None;
    }
    join_relative(dir, specifier)
}

/// Whether an unresolved-import key names a missing file a `BUILD` file declares as an output ([`bazel_outputs`]).
fn bazel_output_import(checkout: &DependencyCheckout<'_>, key: &str) -> bool {
    missing_relative(checkout, key).is_some_and(|target| checkout.bazel_outputs.contains(output_stem(&target)))
}

/// Whether `is_ignored` accepts `path`, `path` with one of [`RESOLVED_EXTENSIONS`] or the `index` file of the
/// directory it names, as skif tries the files an extensionless import may name for ignored ones.
fn ignored_candidate(is_ignored: &FileProbe<'_>, path: &str) -> bool {
    std::iter::once(path.to_owned())
        .chain(RESOLVED_EXTENSIONS.iter().map(|extension| format!("{path}{extension}")))
        .chain(
            RESOLVED_EXTENSIONS
                .iter()
                .map(|extension| format!("{path}/index{extension}")),
        )
        .any(|candidate| is_ignored(&candidate))
}

/// Whether an unresolved-import key names a missing file that git ignores ([`ignored_candidate`]).
fn gitignored_import(checkout: &DependencyCheckout<'_>, key: &str) -> bool {
    missing_relative(checkout, key).is_some_and(|target| ignored_candidate(checkout.is_ignored, &target))
}

/// Whether git ignores `path`, relative to the checkout `root`, whether it exists or not (`git check-ignore`, which
/// reads every `.gitignore` from the root down to the path). A checkout git cannot read ignores nothing.
fn git_ignores(root: &Path, path: &str) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["check-ignore", "--quiet", "--no-index", "--", path])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Whether an unresolved-import key imports a package from a file of a rig profile of its project
/// ([`in_rig_profile`]).
fn rig_profile_import(checkout: &DependencyCheckout<'_>, key: &str) -> bool {
    let Some((path, specifier)) = key.split_once('|') else {
        return false;
    };
    let project = owner_dir(path, &|dir: &str| checkout.layout.is_project(dir));
    bare_package(specifier).is_some() && in_rig_profile(below(&project, path))
}

/// Whether `manifest` lists `name` in one of its [`DEPENDENCY_FIELDS`].
fn lists_package(manifest: &Value, name: &str) -> bool {
    DEPENDENCY_FIELDS.iter().any(|field| {
        manifest
            .get(*field)
            .and_then(Value::as_object)
            .is_some_and(|dependencies| dependencies.contains_key(name))
    })
}

/// The manifest of the package `name` a file of `dir` imports: the workspace package of that name, else the nearest
/// installed one ([`installed_manifest`]).
fn package_manifest(checkout: &DependencyCheckout<'_>, dir: &str, name: &str) -> Option<Rc<Value>> {
    checkout
        .resolver
        .packages
        .get(name)
        .and_then(|package| (checkout.layout.manifest)(package))
        .or_else(|| installed_manifest(dir, name, checkout.layout.manifest))
}

/// Whether the file of an unresolved-import key does not hold the specifier, a package, and a package the file
/// imports lists it ([`package_manifest`]).
fn shared_config_reference(checkout: &DependencyCheckout<'_>, key: &str) -> bool {
    let Some((path, specifier)) = key.split_once('|') else {
        return false;
    };
    let Some(package) = bare_package(specifier) else {
        return false;
    };
    let Some(source) = (checkout.read)(path) else {
        return false;
    };
    !source.contains(specifier)
        && imported_packages(path, &source).iter().any(|imported| {
            package_manifest(checkout, parent_dir(path), imported)
                .is_some_and(|manifest| lists_package(&manifest, package))
        })
}

/// What starts a Bun shell template, a command line of its own.
const SHELL_TEMPLATE: &str = "$`";

/// Whether the specifier of an unresolved-import key is relative, resolves from the directory of the file's project
/// and not from the file's own, and a line of the file runs it in a Bun shell template ([`SHELL_TEMPLATE`]).
fn shell_template_directory(checkout: &DependencyCheckout<'_>, key: &str) -> bool {
    let Some((path, specifier)) = key.split_once('|') else {
        return false;
    };
    if missing_relative(checkout, key).is_none() {
        return false;
    }
    let project = owner_dir(path, &|dir: &str| checkout.layout.is_project(dir));
    let argument = format!(" {specifier}");
    resolve_relative(&project, specifier, checkout.layout.is_file).is_some()
        && (checkout.read)(path).is_some_and(|source| {
            source.lines().any(|line| {
                line.split_once(SHELL_TEMPLATE)
                    .is_some_and(|(_, command)| command.contains(&argument))
            })
        })
}

/// The names of a Vitest or Vite configuration without its extension.
const VITEST_CONFIG_STEMS: [&str; 2] = ["vitest.config", "vite.config"];

/// Whether the file name of `path` is one of [`VITEST_CONFIG_STEMS`] with a code extension.
fn is_vitest_config(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.rsplit_once('.')
        .is_some_and(|(stem, extension)| VITEST_CONFIG_STEMS.contains(&stem) && CODE_EXTENSIONS.contains(&extension))
}

/// Whether the file of an unresolved-import key is a Vitest configuration ([`is_vitest_config`]) the specifier does
/// not resolve from, and one below its directory holds the specifier quoted and resolves it from its own directory.
fn nested_vitest_setup(checkout: &DependencyCheckout<'_>, key: &str) -> bool {
    let Some((path, specifier)) = key.split_once('|') else {
        return false;
    };
    let dir = parent_dir(path);
    let is_file = checkout.layout.is_file;
    if !is_vitest_config(path) || resolve_relative(dir, specifier, is_file).is_some() {
        return false;
    }
    let prefix = format!("{dir}/");
    let quoted = [format!("'{specifier}'"), format!("\"{specifier}\"")];
    checkout
        .files
        .iter()
        .filter(|file| file.as_str() != path && is_vitest_config(file) && (dir.is_empty() || file.starts_with(&prefix)))
        .any(|config| {
            resolve_relative(parent_dir(config), specifier, is_file).is_some()
                && (checkout.read)(config)
                    .is_some_and(|source| quoted.iter().any(|text| source.contains(text.as_str())))
        })
}

/// The commands skif reads as running the command in their arguments (`PASSTHROUGH` in
/// `crates/skif-js-plugins/src/scripts/tables.rs`).
const PASSTHROUGH_COMMANDS: [&str; 11] = [
    "command", "env", "exec", "nohup", "time", "xargs", "nice", "sudo", "timeout", "watch", "unbuffer",
];

/// Whether an unlisted-binary key names one of [`PASSTHROUGH_COMMANDS`].
fn passthrough_command(_: &DependencyCheckout<'_>, key: &str) -> bool {
    key.split_once('|')
        .is_some_and(|(_, binary)| PASSTHROUGH_COMMANDS.contains(&binary))
}

/// The words of a shell command line that separate one command from the next.
const COMMAND_SEPARATORS: [&str; 4] = ["&&", "||", ";", "|"];

/// The words of a command line as a shell splits it, roughly: split at whitespace, each without the quotes and
/// backticks around it, a trailing `;` and the start of a Bun shell template ([`SHELL_TEMPLATE`]) as separators of
/// their own.
fn command_words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    for raw in text.split_whitespace() {
        let word = if let Some(rest) = raw.strip_prefix(SHELL_TEMPLATE) {
            words.push(";".to_owned());
            rest
        } else {
            raw
        };
        let word = word.trim_matches(['"', '\'', '`']);
        if let Some(rest) = word.strip_suffix(';') {
            words.push(rest.to_owned());
            words.push(";".to_owned());
        } else {
            words.push(word.to_owned());
        }
    }
    words.retain(|word| !word.is_empty());
    words
}

/// The start of each whole-word occurrence of `word` in `line`: one that no name, path or key character joins.
fn word_starts(line: &str, word: &str) -> Vec<usize> {
    let joins = |c: char| c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | ':' | '@');
    line.match_indices(word)
        .map(|(at, _)| at)
        .filter(|&at| {
            !line[..at].chars().next_back().is_some_and(joins)
                && !line[at + word.len()..].chars().next().is_some_and(joins)
        })
        .collect()
}

/// The words of the last command of `words`, after the last of [`COMMAND_SEPARATORS`].
fn last_command(words: &[String]) -> &[String] {
    let start = words
        .iter()
        .rposition(|word| COMMAND_SEPARATORS.contains(&word.as_str()))
        .map_or(0, |at| at + 1);
    &words[start..]
}

/// The runner whose file argument knip may take for a binary.
const BUN: &str = "bun";

/// Whether an unlisted-binary key names a file with a code extension that a command of its file runs with [`BUN`].
fn bun_file_argument(checkout: &DependencyCheckout<'_>, key: &str) -> bool {
    let Some((path, binary)) = key.split_once('|') else {
        return false;
    };
    has_code_extension(binary)
        && (checkout.read)(path).is_some_and(|source| {
            source.lines().any(|line| {
                word_starts(line, binary).into_iter().any(|at| {
                    last_command(&command_words(&line[..at]))
                        .first()
                        .is_some_and(|word| word == BUN)
                })
            })
        })
}

/// Where a command runs, relative to the checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RunDirectory {
    /// Where the file starts it.
    Same,
    /// A directory the command computes (`${dir}`, `${{ … }}`), or one the harness cannot place.
    Computed,
    /// This directory.
    At(String),
}

/// Where a command whose words before a binary are `words` runs when it starts in `start`: the directory of the last
/// `cd` before it, and that of a `-C <dir>`, `--dir <dir>` or `--dir=<dir>` of its own `pnpm` command, each relative
/// to the one before. A directory holding `$`, or one that is absolute or leaves the checkout, is
/// [`RunDirectory::Computed`].
fn run_directory(words: &[String], start: &str) -> RunDirectory {
    let mut cwd: Option<String> = None;
    let mut command: Option<String> = None;
    let mut in_pnpm = false;
    let mut at = 0;
    while let Some(word) = words.get(at) {
        let word = word.as_str();
        let (target, step) = match (word, words.get(at + 1)) {
            ("cd", Some(dir)) => (Some((dir.as_str(), true)), 2),
            ("-C" | "--dir", Some(dir)) if in_pnpm => (Some((dir.as_str(), false)), 2),
            _ => (
                word.strip_prefix("--dir=").filter(|_| in_pnpm).map(|dir| (dir, false)),
                1,
            ),
        };
        if COMMAND_SEPARATORS.contains(&word) {
            command = None;
            in_pnpm = false;
        } else if word == "pnpm" {
            in_pnpm = true;
        }
        if let Some((dir, changes_cwd)) = target {
            let from = cwd.clone().unwrap_or_else(|| start.to_owned());
            let joined = (!dir.contains('$') && !dir.starts_with(['/', '~']))
                .then(|| join_relative(&from, dir))
                .flatten();
            let Some(joined) = joined else {
                return RunDirectory::Computed;
            };
            if changes_cwd {
                cwd = Some(joined);
            } else {
                command = Some(joined);
            }
        }
        at += step;
    }
    command.or(cwd).map_or(RunDirectory::Same, RunDirectory::At)
}

/// The `working-directory` that applies to the line `at` of a GitHub workflow, and whether it is that of its step
/// (which knip reads), else the `defaults.run.working-directory` of its job or of the workflow (which knip does not),
/// read from the indentation.
fn workflow_directory(lines: &[&str], at: usize) -> Option<(String, bool)> {
    let blank = |line: &str| line.trim().is_empty();
    let line_at = *lines.get(at)?;
    let step = (0..=at).rev().find(|&index| {
        let line = lines[index];
        line.trim_start().starts_with("- ") && (index == at || indentation(line) < indentation(line_at))
    })?;
    let dash = indentation(lines[step]);
    let end = (step + 1..lines.len())
        .find(|&index| !blank(lines[index]) && indentation(lines[index]) <= dash)
        .unwrap_or(lines.len());
    let own = (step..end).find_map(|index| {
        let line = lines[index];
        (index == step || indentation(line) == dash + 2)
            .then(|| yaml_value(line, "working-directory"))
            .flatten()
    });
    if let Some(dir) = own {
        return Some((dir, true));
    }
    let steps = (0..step)
        .rev()
        .find(|&index| !blank(lines[index]) && indentation(lines[index]) <= dash && lines[index].trim() == "steps:")?;
    let job = (0..steps)
        .rev()
        .find(|&index| !blank(lines[index]) && indentation(lines[index]) < indentation(lines[steps]))?;
    let job_end = (job + 1..lines.len())
        .find(|&index| !blank(lines[index]) && indentation(lines[index]) <= indentation(lines[job]))
        .unwrap_or(lines.len());
    defaults_directory(lines, (job + 1, job_end), indentation(lines[steps]))
        .or_else(|| defaults_directory(lines, (0, lines.len()), 0))
        .map(|dir| (dir, false))
}

/// The `working-directory` of a `defaults:` key at the indentation `depth` among the lines `start..end`.
fn defaults_directory(lines: &[&str], (start, end): (usize, usize), depth: usize) -> Option<String> {
    let defaults =
        (start..end).find(|&index| indentation(lines[index]) == depth && lines[index].trim() == "defaults:")?;
    lines[defaults + 1..end]
        .iter()
        .take_while(|line| line.trim().is_empty() || indentation(line) > depth)
        .find_map(|line| yaml_value(line, "working-directory"))
}

/// The number of bytes of leading whitespace of `line`.
fn indentation(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// The value of the YAML key `key` on `line` (also as the first key of a list item), without its quotes.
fn yaml_value(line: &str, key: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let trimmed = trimmed.strip_prefix("- ").unwrap_or(trimmed);
    let value = trimmed.strip_prefix(key)?.strip_prefix(':')?.trim();
    (!value.is_empty()).then(|| value.trim_matches(['"', '\'']).to_owned())
}

/// Whether `path` is a GitHub workflow.
fn is_workflow(path: &str) -> bool {
    path.starts_with(".github/workflows/")
        && path
            .rsplit_once('.')
            .is_some_and(|(_, extension)| matches!(extension, "yml" | "yaml"))
}

/// The scripts of the manifest `source`, each a command line; none for a manifest that cannot be parsed.
fn manifest_scripts(source: &str) -> Vec<String> {
    serde_json::from_str::<Value>(source)
        .ok()
        .and_then(|manifest| {
            manifest
                .get("scripts")
                .and_then(Value::as_object)
                .map(|scripts| scripts.values().filter_map(Value::as_str).map(str::to_owned).collect())
        })
        .unwrap_or_default()
}

/// Whether a command line of the file of an unlisted-binary key (a script of a manifest, else a line) runs the binary
/// in another project than the workspace knip judges it in, or in a computed directory ([`run_directory`],
/// [`workflow_directory`]): knip judges a command at the checkout root for a workflow, at its manifest's directory for
/// a script and at its project for any other file, and in the workspace of a step's `working-directory`.
fn command_directory(checkout: &DependencyCheckout<'_>, key: &str) -> bool {
    let Some((path, binary)) = key.split_once('|') else {
        return false;
    };
    let Some(source) = (checkout.read)(path) else {
        return false;
    };
    let layout = checkout.layout;
    let is_project = |dir: &str| layout.is_project(dir);
    let is_workspace = |dir: &str| layout.is_workspace(dir);
    let project_of = |dir: &str| owner_dir(&joined(dir, "package.json"), &is_project);
    let workspace_of = |dir: &str| owner_dir(&joined(dir, "package.json"), &is_workspace);
    let workflow = is_workflow(path);
    let manifest = path.rsplit('/').next() == Some("package.json");
    let base = if workflow {
        String::new()
    } else if manifest {
        parent_dir(path).to_owned()
    } else {
        owner_dir(path, &is_project)
    };
    let scripts = if manifest {
        manifest_scripts(&source)
    } else {
        Vec::new()
    };
    let lines: Vec<&str> = if manifest {
        scripts.iter().map(String::as_str).collect()
    } else {
        source.lines().collect()
    };
    lines.iter().enumerate().any(|(index, line)| {
        word_starts(line, binary).into_iter().any(|at| {
            let set = workflow.then(|| workflow_directory(&lines, index)).flatten();
            let (start, judged) = match &set {
                Some((dir, _)) if dir.contains('$') => return true,
                Some((dir, step)) => {
                    let Some(dir) = join_relative(&base, dir) else {
                        return false;
                    };
                    let judged = if *step { dir.clone() } else { base.clone() };
                    (dir, judged)
                }
                None => (base.clone(), base.clone()),
            };
            let run = match run_directory(&command_words(&line[..at]), &start) {
                RunDirectory::Computed => return true,
                RunDirectory::Same if set.is_none() => return false,
                RunDirectory::Same => start,
                RunDirectory::At(dir) => dir,
            };
            project_of(&run) != workspace_of(&judged)
        })
    })
}

/// Whether the file of an unlisted-dependency key lies in a workspace whose manifest, or that of a workspace enclosing
/// it, lists a package whose installed manifest names the dependency an optional peer ([`hosts_optional_peer`]).
fn optional_peer_host(checkout: &DependencyCheckout<'_>, key: &str) -> bool {
    let Some((path, name)) = key.split_once('|') else {
        return false;
    };
    let Some(package) = bare_package(name) else {
        return false;
    };
    let layout = checkout.layout;
    let is_workspace = |dir: &str| layout.is_workspace(dir);
    let mut workspace = owner_dir(path, &is_workspace);
    loop {
        if hosts_optional_peer(layout.manifest, &workspace, package) {
            return true;
        }
        if workspace.is_empty() {
            return false;
        }
        workspace = owner_dir(&workspace, &is_workspace);
    }
}

/// Whether the manifest of `dir` lists a package whose installed manifest ([`installed_manifest`]) names `package` in
/// `peerDependencies` and marks it `optional` in `peerDependenciesMeta`.
fn hosts_optional_peer(manifest: &ManifestReader<'_>, dir: &str, package: &str) -> bool {
    let Some(listed) = manifest(dir) else {
        return false;
    };
    DEPENDENCY_FIELDS
        .iter()
        .filter_map(|field| listed.get(*field).and_then(Value::as_object))
        .flat_map(|dependencies| dependencies.keys())
        .any(|host| {
            installed_manifest(dir, host, manifest).is_some_and(|installed| {
                let peer = installed
                    .get("peerDependencies")
                    .and_then(Value::as_object)
                    .is_some_and(|peers| peers.contains_key(package));
                let optional = installed
                    .get("peerDependenciesMeta")
                    .and_then(|meta| meta.get(package))
                    .and_then(|entry| entry.get("optional"))
                    .and_then(Value::as_bool)
                    == Some(true);
                peer && optional
            })
        })
}

/// The specifiers of the Node.js module whose calls run commands.
const CHILD_PROCESS: [&str; 2] = ["child_process", "node:child_process"];

/// Whether the code file of an unlisted-binary key loads [`CHILD_PROCESS`] with `require` and imports it with no
/// `import` declaration, as skif's parser reads its imports.
fn required_child_process(checkout: &DependencyCheckout<'_>, key: &str) -> bool {
    let path = key_path(key);
    has_code_extension(path)
        && parsed_module(checkout.read, path).is_some_and(|module| {
            let loads = |import: &&Import| CHILD_PROCESS.contains(&import.specifier.as_str());
            let required = module
                .imports
                .iter()
                .filter(loads)
                .any(|import| matches!(import.kind, ImportKind::Require));
            let declared = module.imports.iter().filter(loads).any(|import| {
                matches!(
                    import.kind,
                    ImportKind::Static | ImportKind::TypeOnly | ImportKind::SideEffect
                )
            });
            required && !declared
        })
}

fn is_json_file(name: &str) -> bool {
    Path::new(name)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use skif_workspace::MemoryFs;

    use super::*;

    fn set(keys: &Keys, category: &str) -> Vec<String> {
        keys[category].iter().cloned().collect()
    }

    #[test]
    fn normalizes_absolute_and_relative_paths() {
        let root = Path::new("/work/repo");
        assert_eq!(normalize("/work/repo/a/b.ts", root), "a/b.ts");
        assert_eq!(normalize("./a/b.ts", root), "a/b.ts");
        assert_eq!(normalize("a\\b.ts", root), "a/b.ts");
        assert_eq!(normalize("/work/repository/a.ts", root), "/work/repository/a.ts");
    }

    #[test]
    fn maps_skif_diagnostics_to_knip_categories() {
        let doc = json!({ "diagnostics": [
            { "rule": "unused-dependencies", "message": "`left-pad` is never used", "location": { "path": "packages/a/package.json" } },
            { "rule": "unlisted-dependencies", "message": "`react` is imported", "dependency": "react", "location": { "path": "/r/src/a.ts" } },
            { "rule": "unused-files", "message": "`src/dead.ts` is not reachable", "location": { "path": "src/dead.ts" } },
            { "rule": "duplicate-exports", "message": "`local` is exported as `b` and `a`", "location": { "path": "src/d.ts" } },
            { "rule": "version-mismatch", "message": "x", "location": { "path": "package.json" } },
        ]});
        let keys = skif_keys(&doc, Path::new("/r"));
        assert_eq!(set(&keys, "dependencies"), ["packages/a/package.json|left-pad"]);
        assert_eq!(set(&keys, "unlisted"), ["src/a.ts|react"]);
        assert_eq!(set(&keys, "files"), ["src/dead.ts"]);
        assert_eq!(set(&keys, "duplicates"), ["src/d.ts|a|b"]);
        assert_eq!(keys.len(), 4);
    }

    #[test]
    fn maps_knip_issues_of_every_shape() {
        let doc = json!({ "files": ["old.ts"], "issues": [
            { "file": "package.json", "dependencies": [{ "name": "zzz" }], "devDependencies": [] },
            { "file": "src/dead.ts", "files": [{ "name": "src/dead.ts" }] },
            { "file": "src/e.ts", "enumMembers": { "Color": [{ "name": "Red" }] }, "exports": [{ "name": "x", "line": 1 }],
              "duplicates": [[{ "name": "b" }, { "name": "a" }]], "nsTypes": [{ "name": "T" }], "classMembers": [{ "name": "m" }] },
        ]});
        let keys = knip_keys(&doc, Path::new("/r"));
        assert_eq!(set(&keys, "dependencies"), ["package.json|zzz"]);
        assert_eq!(set(&keys, "files"), ["old.ts", "src/dead.ts"]);
        assert_eq!(set(&keys, "enumMembers"), ["src/e.ts|Red"]);
        assert_eq!(set(&keys, "exports"), ["src/e.ts|x"]);
        assert_eq!(set(&keys, "duplicates"), ["src/e.ts|a|b"]);
        assert_eq!(set(&keys, "nsTypes"), ["src/e.ts|T"]);
        assert!(!keys.contains_key("namespaceMembers"));
        assert!(!keys.contains_key("devDependencies"));
        assert!(!keys.contains_key("classMembers"));
    }

    #[test]
    fn equal_findings_agree_completely() {
        let skif = json!({ "diagnostics": [
            { "rule": "unused-exports", "message": "export `x` is never imported", "location": { "path": "src/e.ts" } },
        ]});
        let knip = json!({ "issues": [{ "file": "src/e.ts", "exports": [{ "name": "x" }] }] });
        let (counts, _) = diff::compare(&skif_keys(&skif, Path::new("/r")), &knip_keys(&knip, Path::new("/r")));
        assert_eq!(
            counts["exports"],
            Counts {
                skif_only: 0,
                tool_only: 0,
                both: 1
            }
        );
    }

    #[test]
    fn install_commands_follow_the_package_manager() {
        let repo = |pm: &str, install: bool| Repo {
            name: "r".into(),
            url: String::new(),
            rev: "0".repeat(40),
            package_manager: pm.into(),
            suites: vec!["parity".into()],
            sparse: Vec::new(),
            install,
            notes: "why".into(),
            recursive: false,
            knip_unread: Vec::new(),
            install_timeout_secs: None,
            install_command: Vec::new(),
            entry: Vec::new(),
            bench_knip_args: Vec::new(),
            bench_knip_failure: None,
            bench_knip_config: None,
            bench_max_runs: None,
            bench_max_warmup: None,
            knip_invalid: None,
            knip_root: None,
            build_command: Vec::new(),
            build_timeout_secs: None,
        };
        let dir = Path::new("/nonexistent");
        assert_eq!(install_command(&repo("npm", true), dir).unwrap().1[0], "ci");
        assert!(
            install_command(&repo("pnpm", true), dir)
                .unwrap()
                .1
                .iter()
                .any(|a| a == "--ignore-scripts")
        );
        let mut overridden = repo("pnpm", true);
        overridden.install_command = vec!["npx".into(), "pnpm@9".into(), "install".into()];
        assert_eq!(
            install_command(&overridden, dir).unwrap(),
            ("npx".to_owned(), vec!["pnpm@9".to_owned(), "install".to_owned()])
        );
        assert!(install_command(&repo("rush", true), dir).is_err());
        assert!(install_command(&repo("pnpm", false), dir).unwrap_err().contains("why"));
    }

    #[test]
    fn member_keys_drop_the_qualifier_on_both_sides() {
        let skif = json!({ "diagnostics": [
            { "rule": "unused-enum-members", "message": "enum member `Color.Red` is never used", "location": { "path": "src/e.ts" } },
            { "rule": "unused-namespace-members", "message": "namespace member `ns.Helper` is never used", "location": { "path": "src/n.ts" } },
        ]});
        let knip = json!({ "issues": [
            { "file": "src/e.ts", "enumMembers": { "Color": [{ "name": "Red" }] } },
            { "file": "src/n.ts", "namespaceMembers": [{ "name": "ns.Helper" }] },
        ]});
        let root = Path::new("/r");
        let (counts, _) = diff::compare(&skif_keys(&skif, root), &knip_keys(&knip, root));
        assert_eq!(counts["enumMembers"].both, 1);
        assert_eq!(counts["namespaceMembers"].both, 1);
        assert_eq!(counts["enumMembers"].skif_only + counts["enumMembers"].tool_only, 0);
    }

    /// The JSON that `knip --reporter json` prints: every reported issue type is present on every row, empty or not,
    /// members are a flat list of `{ namespace, name }` (the parent first, the bare member name second), and a module
    /// that is only read through a namespace import lists its unused exports and types with that namespace.
    fn knip_report() -> Value {
        json!({ "issues": [
            { "file": "src/e.ts", "binaries": [], "dependencies": [], "duplicates": [],
              "enumMembers": [{ "namespace": "Color", "name": "Green", "line": 1, "col": 26, "pos": 25 }],
              "exports": [], "files": [], "namespaceMembers": [],
              "types": [{ "name": "Unused", "line": 2, "col": 13, "pos": 45 }], "unlisted": [], "unresolved": [] },
            { "file": "src/n.ts", "enumMembers": [],
              "exports": [{ "namespace": "ns", "name": "other", "line": 2, "col": 14, "pos": 36 }],
              "namespaceMembers": [{ "namespace": "Helpers", "name": "run", "line": 4, "col": 3, "pos": 80 }],
              "types": [{ "namespace": "ns", "name": "B", "line": 3, "col": 13, "pos": 60 }] },
        ]})
    }

    #[test]
    fn maps_the_json_knip_prints() {
        let keys = knip_keys(&knip_report(), Path::new("/r"));
        assert_eq!(set(&keys, "enumMembers"), ["src/e.ts|Green"]);
        assert_eq!(set(&keys, "types"), ["src/e.ts|Unused"]);
        assert_eq!(set(&keys, "nsExports"), ["src/n.ts|other"]);
        assert_eq!(set(&keys, "nsTypes"), ["src/n.ts|B"]);
        assert_eq!(set(&keys, "namespaceMembers"), ["src/n.ts|run"]);
        assert!(!keys.contains_key("exports"));
        assert!(!keys.contains_key("duplicates"));
    }

    #[test]
    fn every_member_and_namespace_finding_agrees_with_the_json_knip_prints() {
        let skif = json!({ "diagnostics": [
            { "rule": "unused-enum-members", "message": "enum member `Color.Green` is never used", "location": { "path": "src/e.ts" } },
            { "rule": "unused-types", "message": "type `Unused` is never imported by another module", "location": { "path": "src/e.ts" } },
            { "rule": "unused-namespace-exports", "message": "export `other` is never read through a namespace import", "location": { "path": "src/n.ts" } },
            { "rule": "unused-namespace-types", "message": "type `B` is never read through a namespace import", "location": { "path": "src/n.ts" } },
            { "rule": "unused-namespace-members", "message": "namespace member `Helpers.run` is never used", "location": { "path": "src/n.ts" } },
        ]});
        let root = Path::new("/r");
        let (counts, _) = diff::compare(&skif_keys(&skif, root), &knip_keys(&knip_report(), root));
        let both = Counts {
            skif_only: 0,
            tool_only: 0,
            both: 1,
        };
        for category in ["enumMembers", "types", "nsExports", "nsTypes", "namespaceMembers"] {
            assert_eq!(counts[category], both, "{category}");
        }
    }

    #[test]
    fn corpus_entry_files_reach_both_tools_after_the_defaults() {
        assert_eq!(knip_config(&[]), "{}\n");
        let entry = ["src/vs/workbench/workbench.desktop.main.ts".to_owned()];
        let knip: Value = serde_json::from_str(&knip_config(&entry)).unwrap();
        let expected = json!([DEFAULT_ENTRY[0], DEFAULT_ENTRY[1], entry[0]]);
        assert_eq!(knip["workspaces"]["."]["entry"], expected);
        let skif: toml::Value = toml::from_str(&skif_entry_config(&entry)).unwrap();
        let listed: Vec<&str> = skif["js"]["workspaces"]["."]["entry"]
            .as_array()
            .unwrap()
            .iter()
            .map(|glob| glob.as_str().unwrap())
            .collect();
        assert_eq!(listed, [DEFAULT_ENTRY[0], DEFAULT_ENTRY[1], entry[0].as_str()]);
    }

    #[test]
    fn finds_the_report_among_banners() {
        let noisy = b"Platform libraries usePool true\n[dotenv@17] injected env (0) from .env\n{\"issues\":[{\"file\":\"a.ts\",\"exports\":[{\"name\":\"x\"}]}]}\n";
        let doc = parse_knip_report(noisy).unwrap();
        assert_eq!(doc["issues"][0]["file"], "a.ts");
        let skipped_object = b"{\"log\":1}\n{\"files\":[\"b.ts\"],\"issues\":[]}\n";
        assert_eq!(parse_knip_report(skipped_object).unwrap()["files"][0], "b.ts");
        let pretty = b"banner\n{\n  \"issues\": []\n}\n";
        assert!(parse_knip_report(pretty).is_ok());
    }

    #[test]
    fn reports_why_no_report_was_found() {
        assert_eq!(parse_knip_report(b"  \n").unwrap_err(), "stdout is empty");
        assert!(
            parse_knip_report(b"just text\n")
                .unwrap_err()
                .contains("no knip report")
        );
        assert!(
            parse_knip_report(b"{\"issues\": [oops\n")
                .unwrap_err()
                .contains("first parse error")
        );
        assert!(parse_knip_report(b"{\"other\":1}\n").is_err());
    }

    #[test]
    fn classifies_unused_files_knip_alone_reports() {
        assert_eq!(classify_file("src/app.css", false), Some(NON_CODE_FILE));
        assert_eq!(classify_file("docs/index.mdx", true), Some(NON_CODE_FILE));
        assert_eq!(classify_file("pkg/test/a.ts", false), Some(TEST_FILE));
        assert_eq!(classify_file("pkg/src/a.spec.ts", false), Some(TEST_FILE));
        assert_eq!(classify_file("pkg/src/a_spec.ts", false), Some(TEST_FILE));
        assert_eq!(classify_file("pkg/__fixtures__/a.ts", false), Some(FIXTURE_FILE));
        assert_eq!(classify_file("pkg/fixtures/test/a.ts", false), Some(FIXTURE_FILE));
        assert_eq!(
            classify_file("pkg/x/colorize-fixtures/a.ts", false),
            None,
            "skif judges other test-data names"
        );
        assert_eq!(classify_file("pkg/testdata/a.ts", false), None);
        assert_eq!(classify_file("pkg/fixture/test/a.ts", false), Some(TEST_FILE));
        assert_eq!(classify_file("pkg/test/a.ts", true), None);
        assert_eq!(classify_file("pkg/fixtures/a.ts", true), None);
        assert_eq!(classify_file("pkg/src/a.ts", false), None);
        assert_eq!(classify_file("pkg/src/latest.tsx", false), None);
        assert_eq!(classify_file("typings/legacy.d.ts", false), Some(DECLARATION_FILE));
        assert_eq!(
            classify_file("pkg/test/globals.d.ts", true),
            Some(DECLARATION_FILE),
            "whatever `--include-test-files` says"
        );
        assert_eq!(classify_file("lib/watch.d.mts", false), None, "skif judges `.d.mts`");
        assert_eq!(classify_file("lib/types.d.cts", false), None);
        assert_eq!(classify_file("src/d.ts", false), None);
    }

    #[test]
    fn unlisted_imports_only_foreign_tsconfig_paths_map_split_off() {
        let skif_doc = json!({ "diagnostics": [
            { "rule": "unlisted-dependencies", "message": "`shared/value` resolves only through the `paths` of `packages/app/tsconfig.json`; add the mapping to this package's tsconfig or import it by package name",
              "location": { "path": "/r/packages/lib/src/index.ts" }, "dependency": "shared", "detail": "ForeignTsconfigPaths" },
            { "rule": "unlisted-dependencies", "message": "`left-pad` is imported but not listed in `package.json`",
              "location": { "path": "/r/packages/lib/src/index.ts" }, "dependency": "left-pad" },
            { "rule": "unused-files", "message": "`x.ts` is unused", "location": { "path": "/r/packages/lib/src/x.ts" },
              "detail": "ForeignTsconfigPaths" },
        ]});
        let root = Path::new("/r");
        let foreign = foreign_tsconfig_paths_keys(&skif_doc, root);
        assert_eq!(
            foreign.iter().map(String::as_str).collect::<Vec<_>>(),
            ["packages/lib/src/index.ts|shared"]
        );
        let skif = skif_keys(&skif_doc, root);
        let knip = Keys::from([(
            "unlisted".to_owned(),
            ["packages/lib/src/index.ts|shared"].map(str::to_owned).into(),
        )]);
        let split = diff::split_known(&skif, &Keys::new(), &|side, category, key| {
            (side == Side::Skif)
                .then(|| skif_only_deviation(category, key, &foreign))
                .flatten()
        });
        let (counts, _) = diff::compare(&split.skif, &split.tool);
        assert_eq!(
            counts["unlisted"],
            Counts {
                skif_only: 1,
                tool_only: 0,
                both: 0
            },
            "the plain unlisted import stays a difference"
        );
        assert_eq!(split.known[FOREIGN_TSCONFIG_PATHS].skif_only, 1);
        assert_eq!(split.known.len(), 1, "{:?}", split.known);
        // A finding both tools report is agreement, never a deviation.
        let both = diff::split_known(&skif, &knip, &|side, category, key| {
            (side == Side::Skif)
                .then(|| skif_only_deviation(category, key, &foreign))
                .flatten()
        });
        assert!(both.known.is_empty(), "{:?}", both.known);
        assert_eq!(
            skif_only_deviation("files", "packages/lib/src/index.ts|shared", &foreign),
            None
        );
    }

    #[test]
    fn unused_files_only_known_deviations_split_off() {
        let skif = Keys::from([("files".to_owned(), ["src/dead.ts".to_owned()].into())]);
        let knip = Keys::from([(
            "files".to_owned(),
            ["src/dead.ts", "src/a.css", "test/a.ts", "src/real.ts"]
                .map(str::to_owned)
                .into(),
        )]);
        let split = diff::split_known(&skif, &knip, &|side, category, key| {
            (side == Side::Tool && category == "files")
                .then(|| classify_file(key, false))
                .flatten()
        });
        let (counts, _) = diff::compare(&split.skif, &split.tool);
        assert_eq!(
            counts["files"],
            Counts {
                skif_only: 0,
                tool_only: 1,
                both: 1
            }
        );
        assert_eq!(split.known[NON_CODE_FILE].tool_only, 1);
        assert_eq!(split.known[TEST_FILE].tool_only, 1);
    }

    #[test]
    fn the_same_export_under_another_label_is_agreement() {
        let mut skif = Keys::from([
            (
                "exports".to_owned(),
                ["a.ts|ns", "a.ts|plain", "a.ts|mine"].map(str::to_owned).into(),
            ),
            ("types".to_owned(), ["b.ts|Shared"].map(str::to_owned).into()),
        ]);
        let knip = Keys::from([
            ("exports".to_owned(), ["a.ts|plain"].map(str::to_owned).into()),
            ("nsExports".to_owned(), ["a.ts|ns"].map(str::to_owned).into()),
            ("types".to_owned(), ["b.ts|Shared"].map(str::to_owned).into()),
        ]);
        align_export_labels(&mut skif, &knip);
        let (counts, _) = diff::compare(&skif, &knip);
        assert_eq!(
            counts["exports"],
            Counts {
                skif_only: 1,
                tool_only: 0,
                both: 1
            }
        );
        assert_eq!(
            counts["nsExports"],
            Counts {
                skif_only: 0,
                tool_only: 0,
                both: 1
            }
        );
        assert_eq!(counts["types"].both, 1);
    }

    #[test]
    fn an_import_one_tool_reports_unlisted_and_the_other_unresolved_is_agreement() {
        let mut skif = Keys::from([
            (
                "unlisted".to_owned(),
                strings(&[
                    "packages/a/tsconfig.json|tslib",
                    "packages/b/tsconfig.json|vite",
                    "packages/c/src/a.ts|left-pad",
                    "packages/d/tsconfig.json|vitest",
                    "packages/f/src/a.ts|shared",
                ]),
            ),
            (
                "unresolved".to_owned(),
                strings(&["packages/e/src/a.ts|@scope/x/sub", "packages/e/src/b.ts|./gone"]),
            ),
        ]);
        let knip = Keys::from([
            (
                "unresolved".to_owned(),
                strings(&[
                    "packages/a/tsconfig.json|tslib",
                    "packages/b/tsconfig.json|vite/client",
                    "packages/c/src/b.ts|left-pad",
                    "packages/d/tsconfig.json|vitest/globals",
                ]),
            ),
            (
                "unlisted".to_owned(),
                strings(&[
                    "packages/d/tsconfig.json|vitest",
                    "packages/e/src/a.ts|@scope/x",
                    "packages/e/src/b.ts|gone",
                ]),
            ),
        ]);
        align_unlisted_unresolved(&mut skif, &knip);
        assert_eq!(
            set(&skif, "unresolved"),
            [
                "packages/a/tsconfig.json|tslib",
                "packages/b/tsconfig.json|vite/client",
                "packages/e/src/b.ts|./gone",
            ]
        );
        assert_eq!(
            set(&skif, "unlisted"),
            [
                "packages/c/src/a.ts|left-pad",
                "packages/d/tsconfig.json|vitest",
                "packages/e/src/a.ts|@scope/x",
                "packages/f/src/a.ts|shared",
            ],
            "another file, a finding knip shares already and a relative specifier stay where they are"
        );
        let (counts, _) = diff::compare(&skif, &knip);
        assert_eq!(counts["unresolved"].both, 2);
        assert_eq!(counts["unlisted"].both, 2);
    }

    #[test]
    fn an_unresolved_import_skif_reports_as_a_stale_mock_on_the_same_file_and_specifier_is_agreement() {
        let doc = json!({ "diagnostics": [
            { "rule": "stale-mocks", "message": "`jest.requireActual('./pathToModule')` names a module that does not exist", "location": { "path": "/r/packages/t/a.test.ts" } },
            { "rule": "stale-mocks", "message": "`vi.mock('./gone')` names a module that does not exist", "location": { "path": "/r/packages/t/b.test.ts" } },
            { "rule": "unresolved-imports", "message": "`./other` cannot be resolved", "location": { "path": "/r/packages/t/c.ts" } },
        ]});
        let stale = stale_mock_keys(&doc, Path::new("/r"));
        assert_eq!(
            stale,
            strings(&["packages/t/a.test.ts|./pathToModule", "packages/t/b.test.ts|./gone"])
        );
        let mut skif = Keys::new();
        let knip = Keys::from([(
            "unresolved".to_owned(),
            strings(&[
                "packages/t/a.test.ts|./pathToModule",
                "packages/t/b.test.ts|./elsewhere",
                "packages/t/c.ts|./pathToModule",
            ]),
        )]);
        align_stale_mocks(&mut skif, &knip, &stale);
        assert_eq!(
            set(&skif, "unresolved"),
            ["packages/t/a.test.ts|./pathToModule"],
            "another specifier in the mocking file, or the same specifier in another file, stays one-sided"
        );
        let (counts, _) = diff::compare(&skif, &knip);
        assert_eq!(counts["unresolved"].both, 1);
        assert_eq!(counts["unresolved"].tool_only, 2);
    }

    #[test]
    fn knip_only_findings_in_packages_skif_did_not_judge_split_off() {
        let skif_doc = json!({ "diagnostics": [], "incomplete-packages": [
            { "name": "web", "dir": "packages/web", "reasons": [{ "cause": "unknown-spread" }] },
        ]});
        let dirs = not_judged_dirs(&skif_doc);
        assert_eq!(
            dirs,
            BTreeMap::from([("packages/web".to_owned(), NotJudgedScope::Package)])
        );
        assert_eq!(
            not_judged_dirs(&json!({ "incomplete-packages": [{ "dir": ".", "scope": "package" }] })),
            BTreeMap::from([(String::new(), NotJudgedScope::Package)])
        );
        assert!(not_judged_dirs(&json!({ "diagnostics": [] })).is_empty());

        let packages = ["packages/web", "packages/web/nested", "packages/api"];
        let is_package = |dir: &str| packages.contains(&dir);
        assert_eq!(owner_dir("packages/web/src/a.ts", &is_package), "packages/web");
        assert_eq!(owner_dir("packages/web/package.json|vite", &is_package), "packages/web");
        assert_eq!(
            owner_dir("packages/web/nested/a.ts", &is_package),
            "packages/web/nested"
        );
        assert_eq!(owner_dir("scripts/a.ts", &is_package), "");

        let knip = Keys::from([
            (
                "files".to_owned(),
                [
                    "packages/web/src/orphan.ts",
                    "packages/web/nested/orphan.ts",
                    "packages/api/orphan.ts",
                ]
                .map(str::to_owned)
                .into(),
            ),
            (
                "devDependencies".to_owned(),
                ["packages/web/package.json|vite"].map(str::to_owned).into(),
            ),
            (
                "exports".to_owned(),
                ["packages/web/src/a.ts|unused"].map(str::to_owned).into(),
            ),
        ]);
        let split = diff::split_known(&Keys::new(), &knip, &|side, category, key| {
            (side == Side::Tool && not_judged(category, key, &dirs, &is_package)).then_some(NOT_JUDGED)
        });
        assert_eq!(
            split.known[NOT_JUDGED],
            diff::KnownCounts {
                skif_only: 0,
                tool_only: 3
            }
        );
        let (counts, _) = diff::compare(&split.skif, &split.tool);
        assert_eq!(
            counts["files"].tool_only, 2,
            "a nested complete package and another package stay one-sided"
        );
        assert!(
            !counts.contains_key("exports"),
            "skif does not judge the exports of an incomplete package (ADR 0021)"
        );
        assert!(!counts.contains_key("devDependencies"));
    }

    #[test]
    fn not_judged_covers_exports_types_and_members_but_not_unlisted_or_duplicates() {
        let dirs = BTreeMap::from([("packages/web".to_owned(), NotJudgedScope::Package)]);
        let is_package = |dir: &str| dir == "packages/web";
        for category in [
            "exports",
            "types",
            "nsExports",
            "nsTypes",
            "enumMembers",
            "namespaceMembers",
        ] {
            assert!(
                not_judged(category, "packages/web/src/a.ts|x", &dirs, &is_package),
                "{category}"
            );
        }
        for category in [
            "unlisted",
            "unresolved",
            "binaries",
            "duplicates",
            "optionalPeerDependencies",
        ] {
            assert!(
                !not_judged(category, "packages/web/src/a.ts|x", &dirs, &is_package),
                "{category}"
            );
        }
        assert!(!not_judged("exports", "packages/api/src/a.ts|x", &dirs, &is_package));
    }

    #[test]
    fn a_narrowed_scope_sets_apart_only_the_files_its_globs_match_and_every_dependency() {
        let skif_doc = json!({ "diagnostics": [], "incomplete-packages": [
            { "name": "web", "dir": "packages/web", "reasons": [{ "cause": "unknown-config", "plugin": "storybook" }],
              "scope": { "within": ["src/**/*.stories.tsx"] } },
            { "name": "web", "dir": "packages/web", "reasons": [{ "cause": "unknown-config", "plugin": "vitest" }],
              "scope": { "within": ["test/*.ts", "src/**/*.stories.tsx"] } },
            { "name": "root", "dir": ".", "reasons": [{ "cause": "unknown-config" }],
              "scope": { "within": ["scripts/*.ts"] } },
            { "name": "api", "dir": "packages/api", "reasons": [{ "cause": "unknown-spread" }], "scope": "package" },
        ]});
        let dirs = not_judged_dirs(&skif_doc);
        assert_eq!(
            dirs,
            BTreeMap::from([
                (String::new(), NotJudgedScope::within(vec!["scripts/*.ts".to_owned()])),
                ("packages/api".to_owned(), NotJudgedScope::Package),
                (
                    "packages/web".to_owned(),
                    NotJudgedScope::within(vec!["src/**/*.stories.tsx".to_owned(), "test/*.ts".to_owned()])
                ),
            ]),
            "the scopes of one directory are joined"
        );
        let packages = ["packages/web", "packages/api"];
        let is_package = |dir: &str| packages.contains(&dir);
        for (category, key) in [
            ("files", "packages/web/src/ui/Button.stories.tsx"),
            ("exports", "packages/web/src/ui/Button.stories.tsx|Primary"),
            ("types", "packages/web/test/setup.ts|Fixture"),
            ("enumMembers", "packages/web/test/setup.ts|Red"),
            ("dependencies", "packages/web/package.json|react"),
            ("devDependencies", "packages/web/package.json|vitest"),
            ("files", "scripts/release.ts"),
            ("devDependencies", "package.json|tsx"),
            ("files", "packages/api/src/orphan.ts"),
        ] {
            assert!(not_judged(category, key, &dirs, &is_package), "{category} {key}");
        }
        for (category, key) in [
            ("files", "packages/web/src/ui/Button.tsx"),
            ("exports", "packages/web/src/ui/Button.tsx|Button"),
            ("files", "packages/web/test/deep/setup.ts"),
            ("files", "scripts/deep/release.ts"),
            ("files", "tools/a.ts"),
            ("unlisted", "packages/web/src/ui/Button.stories.tsx|react"),
        ] {
            assert!(!not_judged(category, key, &dirs, &is_package), "{category} {key}");
        }
        assert_eq!(
            not_judged_dirs(&json!({ "incomplete-packages": [
                { "dir": "packages/web", "scope": { "within": ["src/*.ts"] } },
                { "dir": "packages/web", "scope": "package" },
            ]})),
            BTreeMap::from([("packages/web".to_owned(), NotJudgedScope::Package)]),
            "a whole-package entry wins over a narrowed one"
        );
    }

    #[test]
    fn a_narrowed_scope_reaches_the_files_of_the_packages_below_its_package() {
        let dirs = BTreeMap::from([
            (
                String::new(),
                NotJudgedScope::within(vec!["packages/*/e2e/**".to_owned()]),
            ),
            (
                "packages/core".to_owned(),
                NotJudgedScope::within(vec!["**/__tests__/**".to_owned()]),
            ),
            ("apps".to_owned(), NotJudgedScope::Package),
        ]);
        let packages = [
            "packages/core",
            "packages/core/nested",
            "packages/web",
            "apps",
            "apps/site",
        ];
        let is_package = |dir: &str| packages.contains(&dir);
        for (category, key) in [
            ("files", "packages/core/nested/src/__tests__/helper.ts"),
            ("exports", "packages/core/nested/src/__tests__/helper.ts|setup"),
            ("types", "packages/web/e2e/run.ts|Options"),
            ("files", "packages/web/e2e/fixtures/app.ts"),
        ] {
            assert!(not_judged(category, key, &dirs, &is_package), "{category} {key}");
        }
        for (category, key) in [
            ("files", "packages/core/nested/src/helper.ts"),
            ("files", "packages/web/src/e2e/run.ts"),
            ("devDependencies", "packages/web/package.json|playwright"),
            ("devDependencies", "packages/core/nested/package.json|vitest"),
            ("unlisted", "packages/web/e2e/run.ts|react"),
            ("files", "apps/site/src/orphan.ts"),
        ] {
            assert!(!not_judged(category, key, &dirs, &is_package), "{category} {key}");
        }
    }

    #[test]
    fn reads_the_config_files_knip_could_not_load() {
        let stderr = "❌ Invalid environment variables: [\n\
            ERROR: Error loading /r/apps/expo/metro.config.js (Cannot find module 'metro-cache')\n\
            ERROR: Please fix or visit https://knip.dev/reference/known-issues\n\
            ERROR: Error loading packages/db/drizzle.config.ts (Missing POSTGRES_URL)\n";
        let errors = knip_config_errors(stderr, Path::new("/r"));
        assert_eq!(
            errors,
            BTreeSet::from([
                "apps/expo/metro.config.js".to_owned(),
                "packages/db/drizzle.config.ts".to_owned()
            ])
        );
        assert!(knip_config_errors("ERROR: something else\n", Path::new("/r")).is_empty());
    }

    #[test]
    fn the_summary_counts_not_judged_findings_apart_from_the_known_deviations() {
        let counts = BTreeMap::from([(
            "files".to_owned(),
            Counts {
                skif_only: 1,
                tool_only: 2,
                both: 3,
            },
        )]);
        let known = diff::Known::from([
            (
                NOT_JUDGED.to_owned(),
                diff::KnownCounts {
                    skif_only: 0,
                    tool_only: 4,
                },
            ),
            (
                NON_CODE_FILE.to_owned(),
                diff::KnownCounts {
                    skif_only: 0,
                    tool_only: 5,
                },
            ),
        ]);
        let entries = BTreeMap::from([
            ("r".to_owned(), Entry::compared(counts.clone()).with_known(known)),
            ("s".to_owned(), Entry::compared(counts.clone()).with_tool_errors(2)),
            (
                "t".to_owned(),
                Entry::compared(counts)
                    .with_tool_errors(1)
                    .with_invalid(Some("configs import unbuilt packages".to_owned())),
            ),
            (
                "u".to_owned(),
                Entry::skipped("knip failed (exit 2): no report; stderr: a | b\nc"),
            ),
        ]);
        let table = markdown("knip", &entries);
        assert!(table.contains("Dependencies installed: no."), "{table}");
        assert!(
            table.contains(
                "| t | compared; knip could not load 1 config files; not a valid comparison: configs import unbuilt packages | 1 | 2 | 3 | 0 | 0 |"
            ),
            "{table}"
        );
        assert!(table.contains("| r | compared | 1 | 2 | 3 | 4 | 5 |"), "{table}");
        assert!(
            table.contains("| u | skipped: knip failed (exit 2): no report; stderr: a \\| b c | - | - | - | - | - |\n"),
            "{table}"
        );
        assert!(
            table.contains("| s | compared; knip could not load 2 config files | 1 | 2 | 3 | 0 | 0 |"),
            "{table}"
        );
        assert!(table.contains("`not-judged-by-skif`"), "{table}");
    }

    #[cfg(unix)]
    #[test]
    fn a_build_runs_within_its_own_time_limit_and_a_failed_one_writes_no_marker() {
        let dir = std::env::temp_dir().join(format!("skif-bench-parity-build-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let scratch = dir.join("scratch");
        let mut repo: Repo = toml::from_str(&format!(
            "name = \"r\"\nurl = \"u\"\nrev = \"{}\"\npackage-manager = \"pnpm\"\n\
             install-command = [\"sh\", \"-c\", \"exit 0\"]\n\
             build-command = [\"sleep\", \"30\"]\nbuild-timeout-secs = 1\n",
            "a".repeat(40)
        ))
        .unwrap();
        assert_eq!(repo.build_timeout_secs, Some(1));
        let reason = install(&repo, &dir, &scratch, Duration::from_secs(600)).unwrap();
        assert_eq!(reason.as_deref(), Some("sleep build timed out after 1 s"));
        assert!(
            !is_installed(&repo, &dir),
            "a timed-out build leaves the checkout not installed"
        );

        // The builds below finish at once; a generous limit keeps a loaded machine from timing them out.
        repo.build_timeout_secs = Some(120);
        repo.build_command = vec!["sh".to_owned(), "-c".to_owned(), "exit 4".to_owned()];
        let reason = install(&repo, &dir, &scratch, Duration::from_secs(600))
            .unwrap()
            .unwrap();
        assert!(reason.starts_with("sh build failed"), "{reason}");
        assert!(!is_installed(&repo, &dir));

        repo.build_command = vec!["sh".to_owned(), "-c".to_owned(), "exit 0".to_owned()];
        assert_eq!(install(&repo, &dir, &scratch, Duration::from_secs(600)).unwrap(), None);
        assert!(is_installed(&repo, &dir));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_install_state_comes_from_the_checkout_markers() {
        let dir = std::env::temp_dir().join(format!("skif-bench-parity-installed-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut repo: Repo = toml::from_str(&format!(
            "name = \"r\"\nurl = \"u\"\nrev = \"{}\"\npackage-manager = \"pnpm\"\n",
            "a".repeat(40)
        ))
        .unwrap();
        assert!(!is_installed(&repo, &dir));
        fs::write(dir.join(INSTALLED_MARKER), format!("{}\n", "a".repeat(40))).unwrap();
        assert!(is_installed(&repo, &dir));
        repo.build_command = vec!["pnpm".to_owned(), "run".to_owned(), "build".to_owned()];
        assert!(
            !is_installed(&repo, &dir),
            "a marker written without the build is stale"
        );
        fs::write(dir.join(INSTALLED_MARKER), install_marker(&repo)).unwrap();
        assert!(is_installed(&repo, &dir));
        assert_eq!(
            install_marker(&repo),
            format!("{}\nbuild: pnpm run build", "a".repeat(40))
        );
        repo.install_command = vec!["pnpm".to_owned(), "install".to_owned()];
        assert!(
            !is_installed(&repo, &dir),
            "a marker written with another install command is stale"
        );
        assert_eq!(
            install_marker(&repo),
            format!("{}\ninstall: pnpm install\nbuild: pnpm run build", "a".repeat(40))
        );
        fs::write(dir.join(INSTALLED_MARKER), install_marker(&repo)).unwrap();
        assert!(is_installed(&repo, &dir));
        repo.rev = "b".repeat(40);
        assert!(!is_installed(&repo, &dir), "a marker of another commit is stale");
        fs::remove_dir_all(&dir).unwrap();

        let entry = |installed: bool| Entry::compared(BTreeMap::new()).with_installed(installed);
        let mut entries = BTreeMap::from([
            ("a".to_owned(), entry(true)),
            ("b".to_owned(), entry(true)),
            ("skipped".to_owned(), Entry::skipped("knip failed")),
        ]);
        assert_eq!(installed_summary(&entries), "yes");
        entries.insert("c".to_owned(), entry(false));
        assert_eq!(installed_summary(&entries), "partial (2 of 3)");
        assert_eq!(
            installed_summary(&BTreeMap::from([("c".to_owned(), entry(false))])),
            "no"
        );
    }

    #[test]
    fn the_recorded_category_set_holds_every_compared_category_and_deviation() {
        let set = category_set();
        assert_eq!(set.compared.len(), KNIP_CATEGORIES.len());
        assert!(
            set.compared.windows(2).all(|pair| pair[0] < pair[1]),
            "sorted: {:?}",
            set.compared
        );
        for id in [
            IN_KNIP_UNUSED_FILE,
            IN_SKIF_UNUSED_FILE,
            USED_IN_KNIP_UNUSED_FILE,
            USED_IN_SKIF_UNUSED_FILE,
            IMPORTED_BY_KNIP_UNUSED_FILE,
            IMPORTED_BY_SKIF_UNUSED_FILE,
            ENTRY_MODULE_DUPLICATE,
            BAZEL_BUILD_FILE,
            DECLARATION_FILE,
            SIGNATURE_TYPE,
            KNIP_CONFIG_ERROR,
            NOT_JUDGED,
            TYPE_RESOLVED,
        ] {
            assert!(set.known_deviations.iter().any(|known| known == id), "{id}");
        }
        assert_eq!(set.known_deviations.len(), KNIP_DEVIATIONS.len());
    }

    #[test]
    fn type_resolved_takes_the_one_sided_findings_only_one_mode_has_after_the_other_deviations() {
        let keys = |items: &[(&str, &[&str])]| -> Keys {
            items
                .iter()
                .map(|(category, names)| ((*category).to_owned(), names.iter().map(|n| (*n).to_owned()).collect()))
                .collect()
        };
        let typed = keys(&[(
            "enumMembers",
            &["a.ts|E.both", "a.ts|E.typed", "test/t.ts|E.typed", "a.ts|E.always"],
        )]);
        let untyped = keys(&[("enumMembers", &["a.ts|E.both", "a.ts|E.untyped", "a.ts|E.always"])]);
        let knip = keys(&[("enumMembers", &["a.ts|E.both", "a.ts|E.untyped", "a.ts|E.knip"])]);
        let classify = |side, category: &str, key: &str| {
            let known = key.starts_with("test/").then_some(TEST_FILE);
            with_type_resolved(known, (side, category, key), Some(&untyped))
        };
        let split = diff::split_known(&typed, &knip, &classify);
        assert_eq!(
            split.known[TYPE_RESOLVED],
            diff::KnownCounts {
                skif_only: 1,
                tool_only: 1
            }
        );
        assert_eq!(
            split.samples[TYPE_RESOLVED],
            ["enumMembers|a.ts|E.typed", "enumMembers|a.ts|E.untyped"]
        );
        assert_eq!(
            split.known[TEST_FILE],
            diff::KnownCounts {
                skif_only: 1,
                tool_only: 0
            },
            "another deviation explains a finding first"
        );
        assert_eq!(split.skif, keys(&[("enumMembers", &["a.ts|E.always", "a.ts|E.both"])]));
        assert_eq!(split.tool, keys(&[("enumMembers", &["a.ts|E.both", "a.ts|E.knip"])]));

        let without_pass = diff::split_known(&typed, &knip, &|side, category, key| {
            with_type_resolved(None, (side, category, key), None)
        });
        assert!(without_pass.known.is_empty(), "{:?}", without_pass.known);
    }

    #[test]
    fn a_rush_repository_gets_a_root_manifest_from_rush_json() {
        let rush = r#"/**
 * Rush's main config, see https://rushjs.io // not a comment end
 */
{
  "$schema": "https://developer.microsoft.com/json-schemas/rush/v5/rush.schema.json",
  // "projects": [] in a comment is ignored
  "rushVersion": "5.0.0",
  "note": "a \"quoted\" /* string */ stays",
  "projects": [
    { "packageName": "@rushstack/heft", "projectFolder": "apps/heft" },
    /* a block comment */
    { "packageName": "@rushstack/node-core-library", "projectFolder": "libraries/node-core-library", },
  ],
}
"#;
        let plain: Value = serde_json::from_str(&strip_jsonc(rush)).unwrap();
        assert_eq!(plain["note"], r#"a "quoted" /* string */ stays"#);
        assert_eq!(plain["projects"].as_array().unwrap().len(), 2);
        let manifest: Value = serde_json::from_str(&rush_root_manifest(rush).unwrap()).unwrap();
        assert_eq!(
            manifest,
            json!({ "//": SCRATCH_ROOT_MARK, "private": true, "workspaces": ["apps/heft", "libraries/node-core-library"] })
        );
        assert!(
            rush_root_manifest(r#"{ "projects": [] }"#)
                .unwrap_err()
                .contains("no projectFolder")
        );
        assert!(
            rush_root_manifest("{ not json")
                .unwrap_err()
                .contains("parsing rush.json")
        );
    }

    #[test]
    fn knip_gets_a_root_manifest_in_a_rush_checkout_only_while_it_runs() {
        let mut repo: Repo = toml::from_str(&format!(
            "name = \"r\"\nurl = \"u\"\nrev = \"{}\"\npackage-manager = \"rush\"\n",
            "a".repeat(40)
        ))
        .unwrap();
        let missing = Path::new("/nonexistent/checkout");
        assert!(knip_root(&repo, missing).unwrap().is_none());
        repo.knip_root = Some("lerna".to_owned());
        assert!(knip_root(&repo, missing).err().unwrap().contains("unknown knip-root"));
        repo.knip_root = Some(RUSH_KNIP_ROOT.to_owned());
        assert!(knip_root(&repo, missing).err().unwrap().contains("reading rush.json"));

        let root = std::env::temp_dir().join(format!("skif-bench-parity-rush-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("rush.json"),
            r#"{ "projects": [{ "packageName": "a", "projectFolder": "apps/a" }] }"#,
        )
        .unwrap();
        let manifest = root.join("package.json");
        let guard = knip_root(&repo, &root).unwrap().unwrap();
        let written: Value = serde_json::from_str(&fs::read_to_string(&manifest).unwrap()).unwrap();
        assert_eq!(written["workspaces"], json!(["apps/a"]));
        assert!(is_scratch_root(&manifest));
        drop(guard);
        assert!(
            !manifest.exists(),
            "the checkout has no root manifest after the knip run"
        );

        fs::write(&manifest, written.to_string()).unwrap();
        drop(knip_root(&repo, &root).unwrap().unwrap());
        assert!(
            !manifest.exists(),
            "a manifest an interrupted run left is replaced and removed"
        );

        fs::write(&manifest, r#"{ "name": "real" }"#).unwrap();
        assert!(knip_root(&repo, &root).err().unwrap().contains("needs no knip-root"));
        assert_eq!(fs::read_to_string(&manifest).unwrap(), r#"{ "name": "real" }"#);
        fs::remove_dir_all(&root).unwrap();
    }

    fn strings(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    static NO_PATHS: [String; 0] = [];
    static NO_KEYS: BTreeSet<String> = BTreeSet::new();
    static NO_READS: KnipReadSets = KnipReadSets {
        opaque_imports: BTreeSet::new(),
        import_defaults: BTreeSet::new(),
        required_actuals: BTreeSet::new(),
        spied_namespaces: BTreeSet::new(),
        enumerated_namespaces: BTreeSet::new(),
        import_types: BTreeSet::new(),
        namespace_reexports: BTreeSet::new(),
        typescript_commonjs: BTreeSet::new(),
        declaration_reexports: BTreeSet::new(),
        node_test_files: BTreeSet::new(),
        ng_package_reexports: BTreeSet::new(),
    };
    static ALL_JUDGED: BTreeMap<String, NotJudgedScope> = BTreeMap::new();
    static NO_DEPENDENCY_READS: DependencyReads = DependencyReads {
        tool: BTreeMap::new(),
        skif: BTreeMap::new(),
    };

    /// A classifier over `layout` that knows nothing of the two reports: no path knip does not read, no package skif
    /// left unjudged or knip could not load a config of, no unused file and nothing one imports, no root of a
    /// recursive run and no Rush root. A test sets the facts it needs with `..bare(&layout)`.
    fn bare<'a>(layout: &'a Layout<'a>) -> Classifier<'a> {
        Classifier {
            knip_unread: &NO_PATHS,
            include_test_files: false,
            not_judged: &ALL_JUDGED,
            knip_failed: &NO_KEYS,
            knip_manifests: &NO_KEYS,
            foreign_paths: &NO_KEYS,
            knip_unused_files: &NO_KEYS,
            skif_unused_files: &NO_KEYS,
            knip_unused_imports: &NO_KEYS,
            skif_unused_imports: &NO_KEYS,
            skif_root_packages: &NO_KEYS,
            published_reach: &NO_KEYS,
            knip_unused_credits: &NO_KEYS,
            skif_unused_credits: &NO_KEYS,
            bazel_credits: &NO_KEYS,
            signature_types: &NO_KEYS,
            ambient_modules: &NO_KEYS,
            own_paths: &NO_KEYS,
            paths_before_packages: &NO_KEYS,
            whole_enum_members: &NO_KEYS,
            published_reexports: &NO_KEYS,
            root_entry_reexports: &NO_KEYS,
            bazel_sources: &NO_KEYS,
            non_member_credits: &NO_KEYS,
            skipped_credits: &NO_KEYS,
            non_member_members: &NO_KEYS,
            skipped_members: &NO_KEYS,
            knip_dot_ignores: &[],
            reexport_aliases: &NO_KEYS,
            tsconfig_declarations: &NO_KEYS,
            unimported_declarations: &NO_KEYS,
            declaration_reach: &NO_KEYS,
            module_object_types: &NO_KEYS,
            uninstalled_projects: &NO_KEYS,
            knip_unused_members: &NO_KEYS,
            knip_dot_ignore_imports: &NO_KEYS,
            knip_dot_ignore_credits: &NO_KEYS,
            heft_credits: &NO_KEYS,
            type_asserted_requires: &NO_KEYS,
            module_object_values: &NO_KEYS,
            commonjs_default_members: &NO_KEYS,
            template_import_reach: &NO_KEYS,
            installed_main_reach: &NO_KEYS,
            type_referenced_members: &NO_KEYS,
            raw_glob_reach: &NO_KEYS,
            typescript_twins: &NO_KEYS,
            knip_reads: &NO_READS,
            dependency_reads: &NO_DEPENDENCY_READS,
            rush_root: false,
            replaced_knip_config: false,
            layout,
        }
    }

    /// Runs `check` with a classifier for a checkout with these workspace members, manifests (by directory) and
    /// `package.json` files knip reports on, and no other facts.
    fn with_classifier(
        members: &[&str],
        manifests: &[(&str, Value)],
        judged_by_knip: &[&str],
        check: impl FnOnce(&Classifier<'_>),
    ) {
        let read: BTreeMap<String, Rc<Value>> = manifests
            .iter()
            .map(|(dir, manifest)| ((*dir).to_owned(), Rc::new(manifest.clone())))
            .collect();
        let manifest = move |dir: &str| read.get(dir).cloned();
        let every_file = |_: &str| true;
        let layout = Layout::new(strings(members), &manifest, &every_file);
        let judged = strings(judged_by_knip);
        check(&Classifier {
            knip_manifests: &judged,
            ..bare(&layout)
        });
    }

    #[test]
    fn fixture_findings_cover_every_category_skif_is_silent_in_there() {
        with_classifier(&[], &[], &[], |classifier| {
            for category in [
                "files",
                "exports",
                "types",
                "nsExports",
                "nsTypes",
                "enumMembers",
                "namespaceMembers",
                "unresolved",
                "unlisted",
            ] {
                let key = if category == "files" {
                    "pkg/__fixtures__/a.ts"
                } else {
                    "pkg/__fixtures__/a.ts|x"
                };
                assert_eq!(
                    classifier.classify(Side::Tool, category, key),
                    Some(FIXTURE_FILE),
                    "{category}"
                );
            }
            for category in ["duplicates", "binaries", "dependencies"] {
                assert_eq!(
                    classifier.classify(Side::Tool, category, "pkg/fixtures/a.ts|x"),
                    None,
                    "{category}"
                );
            }
            assert_eq!(
                classifier.classify(Side::Tool, "unresolved", "pkg/fixture/a.ts|./b"),
                None
            );
            assert_eq!(classifier.classify(Side::Tool, "unresolved", "fixtures.ts|./b"), None);
            assert_eq!(
                classifier.classify(Side::Skif, "unresolved", "pkg/fixtures/a.ts|./b"),
                None
            );
        });
    }

    #[test]
    fn binaries_rush_provides_shell_builtins_and_skif_s_global_binaries_split_off() {
        let manifest = |dir: &str| dir.is_empty().then(|| Rc::new(json!({ "name": "root" })));
        let rush = |file: &str| file == "rush.json";
        let plain = |_: &str| false;
        let checkouts: [(&FileProbe<'_>, Option<&str>); 2] = [(&rush, Some(RUSH_PROVIDED_COMMAND)), (&plain, None)];
        for (is_file, expected) in checkouts {
            let layout = Layout::new(BTreeSet::new(), &manifest, is_file);
            let classifier = bare(&layout);
            for binary in ["rush", "rushx", "rush-pnpm"] {
                assert_eq!(
                    classifier.classify(Side::Tool, "binaries", &format!("apps/a/package.json|{binary}")),
                    expected,
                    "{binary} with a rush.json: {}",
                    expected.is_some()
                );
            }
        }
        let layout = Layout::new(BTreeSet::new(), &manifest, &plain);
        let classifier = bare(&layout);
        for (category, key, expected) in [
            ("binaries", "package.json|read", Some(SHELL_BUILTIN)),
            ("binaries", "package.json|ulimit", Some(SHELL_BUILTIN)),
            ("binaries", "package.json|docker", Some(GLOBAL_BINARY)),
            ("binaries", "package.json|curl", Some(GLOBAL_BINARY)),
            ("binaries", "package.json|bazel", Some(GLOBAL_BINARY)),
            ("binaries", "package.json|eslint", None),
            ("unlisted", "src/a.ts|docker", None),
        ] {
            assert_eq!(
                classifier.classify(Side::Tool, category, key),
                expected,
                "{category} {key}"
            );
        }
        assert_eq!(
            classifier.classify(Side::Skif, "binaries", "package.json|docker"),
            None,
            "a global binary skif reports stays compared"
        );
    }

    #[test]
    fn unlisted_data_uris_split_off() {
        with_classifier(&[], &[("", json!({ "name": "root" }))], &[], |classifier| {
            assert_eq!(
                classifier.classify(Side::Tool, "unlisted", "src/a.test.ts|data:text"),
                Some(DATA_URI)
            );
            assert_eq!(
                classifier.classify(Side::Tool, "unlisted", "src/a.test.ts|database"),
                None
            );
            assert_eq!(
                classifier.classify(Side::Tool, "unresolved", "src/a.test.ts|data:text"),
                None
            );
        });
    }

    #[test]
    fn the_knip_config_the_harness_replaces_splits_off_at_a_workspace_root() {
        let manifests = [
            ("", json!({ "name": "root", "workspaces": ["packages/*"] })),
            ("packages/a", json!({ "name": "a" })),
        ];
        let read: BTreeMap<String, Rc<Value>> = manifests
            .iter()
            .map(|(dir, manifest)| ((*dir).to_owned(), Rc::new(manifest.clone())))
            .collect();
        let manifest = move |dir: &str| read.get(dir).cloned();
        let every_file = |_: &str| true;
        let layout = Layout::new(strings(&["packages/a"]), &manifest, &every_file);
        let replaced = Classifier {
            replaced_knip_config: true,
            ..bare(&layout)
        };
        for key in ["knip.ts", "packages/a/knip.config.js"] {
            assert_eq!(
                replaced.classify(Side::Tool, "files", key),
                Some(REPLACED_KNIP_CONFIG),
                "{key}"
            );
        }
        for key in ["packages/a/src/knip.ts", "knip-helpers.ts"] {
            assert_eq!(replaced.classify(Side::Tool, "files", key), None, "{key}");
        }
        assert_eq!(
            bare(&layout).classify(Side::Tool, "files", "knip.ts"),
            None,
            "knip read the repository's own configuration"
        );
    }

    #[test]
    fn findings_only_skif_reports_on_manifests_knip_does_not_judge_split_off() {
        let manifests = [
            ("", json!({ "name": "root", "workspaces": ["packages/*"] })),
            ("packages/a", json!({ "name": "a" })),
            (
                "examples/app",
                json!({ "name": "app", "dependencies": { "left-pad": "1" } }),
            ),
            ("judged", json!({ "name": "judged" })),
        ];
        with_classifier(&["packages/a"], &manifests, &["judged/package.json"], |classifier| {
            for (category, key) in [
                ("dependencies", "examples/app/package.json|left-pad"),
                ("devDependencies", "examples/app/package.json|vitest"),
                ("optionalPeerDependencies", "examples/app/package.json|react"),
                ("binaries", "examples/app/package.json|turbo"),
            ] {
                assert_eq!(
                    classifier.classify(Side::Skif, category, key),
                    Some(KNIP_UNJUDGED_MANIFEST),
                    "{key}"
                );
            }
            for (category, key) in [
                ("unlisted", "examples/app/src/a.ts|react"),
                ("unlisted", "examples/app/.eslintrc.json|@nrwl/eslint-plugin-nx"),
                ("binaries", "examples/app/.github/workflows/ci.yml|turbo"),
                ("files", "examples/app/src/orphan.ts"),
            ] {
                assert_eq!(
                    classifier.classify(Side::Skif, category, key),
                    Some(NON_MEMBER_PROJECT),
                    "{category} {key}"
                );
            }
            for (category, key) in [
                ("dependencies", "packages/a/package.json|left-pad"),
                ("dependencies", "package.json|left-pad"),
                ("dependencies", "judged/package.json|left-pad"),
                ("binaries", "judged/package.json|turbo"),
                ("exports", "examples/app/src/a.ts|unused"),
                ("unresolved", "examples/app/src/a.ts|./gone"),
                ("unlisted", "packages/a/src/a.ts|react"),
                ("files", "packages/a/src/orphan.ts"),
            ] {
                assert_eq!(classifier.classify(Side::Skif, category, key), None, "{category} {key}");
            }
            assert_eq!(
                classifier.classify(Side::Tool, "dependencies", "examples/app/package.json|left-pad"),
                None,
                "knip does not report on a manifest it does not judge"
            );
        });
    }

    #[test]
    fn knip_only_findings_in_files_below_non_member_projects_split_off() {
        let manifests = [
            ("", json!({ "name": "root", "workspaces": ["packages/*"] })),
            ("packages/a", json!({ "name": "a" })),
            ("packages/a/vendor/lib", json!({ "name": "vendored" })),
            (
                "examples/app",
                json!({ "name": "app", "devDependencies": { "@mastra/core": "1" } }),
            ),
            ("examples/app/esm", json!({ "type": "module" })),
        ];
        with_classifier(&["packages/a"], &manifests, &[], |classifier| {
            for key in [
                "examples/app/src/index.ts",
                "examples/app/esm/index.ts",
                "packages/a/vendor/lib/index.js",
            ] {
                assert_eq!(
                    classifier.classify(Side::Tool, "files", key),
                    Some(NON_MEMBER_PROJECT),
                    "{key}"
                );
            }
            assert_eq!(
                classifier.classify(Side::Tool, "unlisted", "examples/app/esm/a.ts|@mastra/core/agent"),
                Some(NON_MEMBER_PROJECT),
                "the project's own manifest lists the package"
            );
            for (category, key) in [
                ("exports", "examples/app/src/a.ts|unused"),
                ("types", "examples/app/src/a.ts|Unused"),
                ("nsExports", "examples/app/src/a.ts|helper"),
                ("nsTypes", "examples/app/src/a.ts|Helper"),
                ("enumMembers", "examples/app/src/a.ts|Red"),
                ("namespaceMembers", "examples/app/src/a.ts|run"),
                ("duplicates", "examples/app/src/a.ts|a|b"),
                ("unresolved", "examples/app/src/a.ts|../../types/cache"),
                ("unresolved", "packages/a/vendor/lib/index.js|./gone"),
            ] {
                assert_eq!(
                    classifier.classify(Side::Tool, category, key),
                    Some(NON_MEMBER_PROJECT),
                    "{category} {key}"
                );
            }
            for (category, key) in [
                ("unlisted", "examples/app/src/a.ts|zod"),
                ("dependencies", "examples/app/package.json|left-pad"),
                ("binaries", "examples/app/package.json|tsc"),
                ("exports", "packages/a/src/a.ts|unused"),
                ("unresolved", "scripts/a.ts|./gone"),
                ("files", "packages/a/src/orphan.ts"),
                ("files", "scripts/orphan.ts"),
            ] {
                assert_eq!(classifier.classify(Side::Tool, category, key), None, "{key}");
            }
            assert_eq!(
                classifier.classify(Side::Skif, "files", "examples/app/src/index.ts"),
                Some(NON_MEMBER_PROJECT),
                "a recursive run lints the project with its own entry files"
            );
            assert_eq!(
                classifier.classify(Side::Skif, "exports", "examples/app/src/a.ts|unused"),
                None,
                "knip judges the exports of the project from the enclosing workspace"
            );
        });
    }

    #[test]
    fn in_file_findings_in_a_project_a_recursive_run_lints_as_a_root_stay_compared() {
        let skif_doc = json!({ "diagnostics": [], "roots": [
            { "path": ".", "packages": [".", "packages/a"] },
            { "path": "examples/app", "packages": ["examples/app", "examples/app/plugins/x/"] },
        ]});
        let roots = skif_root_packages(&skif_doc);
        assert_eq!(
            roots,
            strings(&["", "examples/app", "examples/app/plugins/x", "packages/a"])
        );
        assert!(skif_root_packages(&json!({ "diagnostics": [] })).is_empty());

        let read: BTreeMap<String, Rc<Value>> = [
            ("", json!({ "name": "root", "workspaces": ["packages/*"] })),
            ("packages/a", json!({ "name": "a" })),
            (
                "examples/app",
                json!({ "name": "app", "dependencies": { "react": "1" } }),
            ),
            ("examples/other", json!({ "name": "other" })),
        ]
        .into_iter()
        .map(|(dir, manifest)| (dir.to_owned(), Rc::new(manifest)))
        .collect();
        let manifest = move |dir: &str| read.get(dir).cloned();
        let every_file = |_: &str| true;
        let layout = Layout::new(strings(&["packages/a"]), &manifest, &every_file);
        let classifier = Classifier {
            skif_root_packages: &roots,
            ..bare(&layout)
        };
        for (category, key) in [
            ("exports", "examples/app/src/a.ts|unused"),
            ("types", "examples/app/src/a.ts|Unused"),
            ("nsExports", "examples/app/src/a.ts|helper"),
            ("nsTypes", "examples/app/src/a.ts|Helper"),
            ("enumMembers", "examples/app/src/a.ts|Red"),
            ("namespaceMembers", "examples/app/src/a.ts|run"),
            ("duplicates", "examples/app/src/a.ts|a|b"),
        ] {
            assert_eq!(classifier.classify(Side::Tool, category, key), None, "{category} {key}");
            assert_eq!(
                classifier.classify(Side::Tool, category, &key.replace("examples/app", "examples/other")),
                Some(NON_MEMBER_PROJECT),
                "{category}: a project that is no root of the run"
            );
        }
        for category in ["exports", "types", "nsExports", "nsTypes"] {
            assert_eq!(
                classifier.classify(Side::Tool, category, "examples/app/src/index.ts|getStaticProps"),
                Some(NON_MEMBER_PROJECT),
                "{category}: the exports of the project's entry are public in skif's run of it"
            );
        }
        assert_eq!(
            classifier.classify(Side::Tool, "enumMembers", "examples/app/index.ts|Red"),
            None
        );
        assert_eq!(
            classifier.classify(Side::Tool, "duplicates", "examples/app/index.ts|a|b"),
            Some(ENTRY_MODULE_DUPLICATE)
        );
        for (category, key) in [
            ("files", "examples/app/src/orphan.ts"),
            ("unresolved", "examples/app/src/a.ts|./gone"),
            ("unlisted", "examples/app/src/a.ts|react"),
        ] {
            assert_eq!(
                classifier.classify(Side::Tool, category, key),
                Some(NON_MEMBER_PROJECT),
                "{category} {key}"
            );
        }
    }

    #[test]
    fn knip_only_findings_below_directories_skif_discovery_skips_split_off() {
        let manifests = [
            ("", json!({ "name": "root" })),
            ("packages/a", json!({ "name": "a" })),
            (".tools", json!({ "name": "tools" })),
            (".vscode/extensions/helper", json!({ "name": "helper" })),
        ];
        let members = ["packages/a", ".tools", ".vscode/extensions/helper"];
        with_classifier(&members, &manifests, &[], |classifier| {
            for (category, key) in [
                ("files", ".claude/skills/template/index.ts"),
                ("files", ".vscode/extensions/stray.ts"),
                ("files", ".vscode/extensions/helper/.cache/x.ts"),
                ("files", "app/.vercel/output/functions/index.func/index.js"),
                ("files", "out/main.js"),
                ("files", "packages/a/coverage/report.js"),
                ("files", "packages/a/src/dist/bundle.js"),
                ("unresolved", ".eslint-plugin-local/rule.ts|./missing"),
            ] {
                assert_eq!(
                    classifier.classify(Side::Tool, category, key),
                    Some(SKIPPED_DIRECTORY),
                    "{key}"
                );
            }
            for key in [
                ".github/scripts/release.ts",
                ".storybook/main.ts",
                ".tools/index.ts",
                ".vscode/extensions/helper/src/extension.ts",
                "packages/a/src/out/emit.ts",
                "packages/a/src/target.ts",
            ] {
                assert_eq!(classifier.classify(Side::Tool, "files", key), None, "{key}");
            }
        });
    }

    #[test]
    fn typescript_sources_of_published_paths_split_off() {
        let manifests = [
            ("", json!({ "name": "root", "bin": { "tool": "./cli.js" } })),
            (
                "packages/core",
                json!({ "name": "core", "exports": {
                    ".": { "types": "./index.d.ts", "default": "./bundles/index.js" },
                    "./schematics/*": { "default": "./schematics/*.js" },
                    "./*": "./*",
                } }),
            ),
            (
                "packages/common",
                json!({ "name": "common", "exports": { "./locales/*": { "types": "./locales/*.d.ts" } } }),
            ),
        ];
        with_classifier(&["packages/core", "packages/common"], &manifests, &[], |classifier| {
            for key in [
                "cli.ts",
                "packages/core/index.ts",
                "packages/core/bundles/index.ts",
                "packages/core/schematics/migrations/model-output/index.ts",
                "packages/common/locales/ff-CM.ts",
            ] {
                assert_eq!(
                    classifier.classify(Side::Tool, "files", key),
                    Some(PUBLISHED_SOURCE),
                    "{key}"
                );
            }
            for key in [
                "packages/core/src/other.ts",
                "packages/core/schematics.ts",
                "packages/common/locales/ff-CM.mts",
                "packages/common/locale/ff-CM.ts",
            ] {
                assert_eq!(classifier.classify(Side::Tool, "files", key), None, "{key}");
            }
            // A JavaScript file `exports` publishes as it is is a published file, not a source of one.
            assert_eq!(
                classifier.classify(Side::Tool, "files", "packages/core/schematics/a.js"),
                Some(PUBLISHED_FILE)
            );
            assert!(
                !classifier
                    .layout
                    .publishes_source_of("packages/core/schematics/types.d.ts")
            );
            assert_eq!(
                classifier.classify(Side::Tool, "files", "packages/core/schematics/types.d.ts"),
                Some(DECLARATION_FILE)
            );
            for category in ["exports", "types", "nsExports", "nsTypes"] {
                assert_eq!(
                    classifier.classify(Side::Tool, category, "packages/core/index.ts|unused"),
                    Some(PUBLISHED_SOURCE),
                    "the exports of an entry are public: {category}"
                );
                assert_eq!(
                    classifier.classify(Side::Tool, category, "packages/core/src/other.ts|unused"),
                    None,
                    "{category}"
                );
                assert_eq!(
                    classifier.classify(Side::Skif, category, "packages/core/index.ts|unused"),
                    None,
                    "{category}"
                );
            }
            for category in ["enumMembers", "namespaceMembers"] {
                assert_eq!(
                    classifier.classify(Side::Tool, category, "packages/core/index.ts|Red"),
                    None,
                    "skif judges the members of a used export: {category}"
                );
            }
        });
    }

    #[test]
    fn duplicates_only_knip_reports_in_default_entries_and_published_sources_split_off() {
        let manifests = [
            ("", json!({ "name": "root", "workspaces": ["packages/*"] })),
            ("packages/a", json!({ "name": "a", "main": "./dist/server.js" })),
        ];
        with_classifier(&["packages/a"], &manifests, &[], |classifier| {
            for key in [
                "packages/a/src/server.ts|default|format",
                "packages/a/index.mjs|a|b",
                "packages/a/src/index.ts|a|b",
                "packages/a/cli.cts|a|b",
                "src/main.tsx|a|b",
            ] {
                assert_eq!(
                    classifier.classify(Side::Tool, "duplicates", key),
                    Some(ENTRY_MODULE_DUPLICATE),
                    "{key}"
                );
            }
            for (side, category, key) in [
                (Side::Tool, "duplicates", "packages/a/src/util.ts|a|b"),
                (Side::Tool, "duplicates", "packages/a/lib/index.ts|a|b"),
                (Side::Tool, "duplicates", "packages/a/src/main/index.ts|a|b"),
                (Side::Tool, "duplicates", "packages/a/index.json|a|b"),
                (Side::Tool, "exports", "packages/a/src/index.ts|unused"),
                (Side::Skif, "duplicates", "packages/a/src/index.ts|a|b"),
            ] {
                assert_eq!(
                    classifier.classify(side, category, key),
                    None,
                    "{side:?} {category} {key}"
                );
            }
            assert_eq!(
                classifier.classify(Side::Tool, "exports", "packages/a/src/server.ts|unused"),
                Some(PUBLISHED_SOURCE)
            );
        });
    }

    #[test]
    fn published_paths_map_to_their_sources_through_the_build_directories_of_the_package() {
        assert_eq!(
            build_source("dist", "src", "dist/a/b.js").as_deref(),
            Some("src/a/b.js")
        );
        assert_eq!(build_source("dist/esm", "", "dist/esm/a.js").as_deref(), Some("a.js"));
        assert_eq!(build_source("dist", "src", "dist").as_deref(), Some("src"));
        assert_eq!(build_source("dist", "src", "distant/a.js"), None);
        assert_eq!(build_source("", "src", "a.js"), None);

        for (config, found) in [
            (
                "{ // the ESM build\n \"compilerOptions\": { \"outDir\": \"./dist/esm\", \"rootDir\": \"./src\", }, }",
                Some(("dist/esm", "src")),
            ),
            (
                r#"{ "compilerOptions": { "outDir": "dist" }, "include": ["src/**/*.ts", "src/types.d.ts"] }"#,
                Some(("dist", "src")),
            ),
            (
                r#"{ "compilerOptions": { "outDir": "lib" }, "include": ["src/**/*", "scripts/build.ts"] }"#,
                Some(("lib", "")),
            ),
            (
                r#"{ "compilerOptions": { "outDir": "dist" }, "files": ["src/a/index.ts", "src/b.ts"] }"#,
                Some(("dist", "src")),
            ),
            (r#"{ "compilerOptions": { "outDir": "dist" } }"#, None),
            (r#"{ "compilerOptions": { "rootDir": "src" } }"#, None),
            (
                r#"{ "compilerOptions": { "outDir": "../out", "rootDir": "src" } }"#,
                None,
            ),
            ("{ not json", None),
        ] {
            assert_eq!(
                tsconfig_build_dir(config),
                found.map(|(out_dir, root_dir)| (out_dir.to_owned(), root_dir.to_owned())),
                "{config}"
            );
        }

        let read: BTreeMap<String, Rc<Value>> = [
            ("", json!({ "name": "root" })),
            (
                "packages/ai",
                json!({ "name": "ai", "exports": {
                    ".": { "import": "./dist/esm/index.js", "require": "./dist/cjs/index.js" },
                    "./tools": "./dist/esm/tools/index.js",
                } }),
            ),
        ]
        .into_iter()
        .map(|(dir, manifest)| (dir.to_owned(), Rc::new(manifest)))
        .collect();
        let manifest = move |dir: &str| read.get(dir).cloned();
        let files = strings(&["packages/ai/src/index.ts", "packages/ai/src/tools/index.ts"]);
        let is_file = |file: &str| files.contains(file);
        let build_dirs = |dir: &str| {
            if dir == "packages/ai" {
                vec![
                    ("dist/esm".to_owned(), "src".to_owned()),
                    ("dist/cjs".to_owned(), "src".to_owned()),
                ]
            } else {
                Vec::new()
            }
        };
        let without = Layout::new(strings(&["packages/ai"]), &manifest, &is_file);
        assert!(
            !without.publishes_source_of("packages/ai/src/index.ts"),
            "`dist/esm` maps to `src/esm` by convention alone"
        );
        let layout = Layout::new(strings(&["packages/ai"]), &manifest, &is_file).with_build_dirs(&build_dirs);
        assert!(layout.publishes_source_of("packages/ai/src/index.ts"));
        assert!(layout.publishes_source_of("packages/ai/src/tools/index.ts"));
        let classifier = bare(&layout);
        assert_eq!(
            classifier.classify(Side::Tool, "files", "packages/ai/src/index.ts"),
            Some(PUBLISHED_SOURCE)
        );
        assert_eq!(
            classifier.classify(Side::Tool, "types", "packages/ai/src/tools/index.ts|ToolOptions"),
            Some(PUBLISHED_SOURCE)
        );
    }

    #[test]
    fn the_harness_reads_the_build_directories_of_a_package_from_its_tsconfig_files() {
        let root = std::env::temp_dir().join(format!("skif-bench-parity-build-dirs-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let package = root.join("packages/ai");
        fs::create_dir_all(&package).unwrap();
        fs::write(
            package.join("tsconfig.json"),
            r#"{ "compilerOptions": { "strict": true } }"#,
        )
        .unwrap();
        fs::write(
            package.join("tsconfig.build.esm.json"),
            r#"{ "extends": "./tsconfig.json", "compilerOptions": { "outDir": "dist/esm", "rootDir": "src" } }"#,
        )
        .unwrap();
        fs::write(
            package.join("tsconfig.build.cjs.json"),
            r#"{ "compilerOptions": { "outDir": "dist/cjs" }, "include": ["src"] }"#,
        )
        .unwrap();
        fs::write(
            package.join("base.json"),
            r#"{ "compilerOptions": { "outDir": "out", "rootDir": "." } }"#,
        )
        .unwrap();
        let read = build_dir_reader(&root);
        assert_eq!(
            read("packages/ai"),
            vec![
                ("dist/cjs".to_owned(), "src".to_owned()),
                ("dist/esm".to_owned(), "src".to_owned()),
            ],
            "in file name order; a file that is no tsconfig is not read"
        );
        assert_eq!(read("packages/missing"), Vec::<(String, String)>::new());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn published_paths_below_build_directories_and_without_extensions_map_to_their_sources() {
        let manifests = [
            ("", json!({ "name": "root" })),
            (
                "libraries/heft",
                json!({ "name": "heft", "main": "./lib-commonjs/index.js", "exports": {
                    "./lib/*": { "types": "./lib/*.d.ts", "default": "./lib-commonjs/*.js" },
                    "./package.json": "./package.json",
                } }),
            ),
            (
                "extensions/git",
                json!({ "name": "git", "main": "./out/main", "browser": "./dist/browser/extension" }),
            ),
            (
                "packages/tool",
                json!({ "name": "tool", "bin": { "tool": "./bin/tool" } }),
            ),
            (
                "packages/styles",
                json!({ "name": "styles", "main": "./dist/index.css" }),
            ),
        ];
        let members = ["libraries/heft", "extensions/git", "packages/tool", "packages/styles"];
        with_classifier(&members, &manifests, &[], |classifier| {
            for key in [
                "libraries/heft/src/index.ts",
                "libraries/heft/src/utilities/fs.ts",
                "libraries/heft/src/plugins/copy/CopyFilesPlugin.tsx",
                "extensions/git/src/main.ts",
                "extensions/git/src/browser/extension.ts",
                "packages/tool/bin/tool.ts",
            ] {
                assert_eq!(
                    classifier.classify(Side::Tool, "files", key),
                    Some(PUBLISHED_SOURCE),
                    "{key}"
                );
            }
            for key in [
                "libraries/heft/scripts/build.ts",
                "extensions/git/src/other.ts",
                "extensions/git/main.ts",
                "packages/tool/src/tool.ts",
                "packages/styles/src/index.ts",
            ] {
                assert_eq!(classifier.classify(Side::Tool, "files", key), None, "{key}");
            }
            assert!(
                !classifier.layout.publishes_source_of("libraries/heft/src/index.d.ts"),
                "a declaration file is no source of a published path"
            );
        });
        assert!(is_published_twin("./lib-commonjs/*.js", "src/a/b", "ts"));
        assert!(!is_published_twin("./lib-commonjs/*.js", "lib-commonjs-src/a", "ts"));
        assert!(
            !is_published_twin("./bundles/*.js", "src/index", "ts"),
            "`bundles` is not a build directory"
        );
        assert!(
            !is_published_twin("./out/*", "src/main", "js"),
            "only TypeScript sources"
        );
    }

    #[test]
    fn a_published_path_without_a_star_names_only_the_first_source_that_exists() {
        let read: BTreeMap<String, Rc<Value>> = [
            ("", json!({ "name": "root" })),
            (
                "packages/x",
                json!({ "name": "x", "main": "./lib/x.js", "types": "./lib/y.d.ts" }),
            ),
        ]
        .into_iter()
        .map(|(dir, manifest)| (dir.to_owned(), Rc::new(manifest)))
        .collect();
        let manifest = move |dir: &str| read.get(dir).cloned();
        let both = strings(&["packages/x/src/x.ts", "packages/x/lib/x.ts", "packages/x/lib/y.ts"]);
        let is_file = |file: &str| both.contains(file);
        let layout = Layout::new(strings(&["packages/x"]), &manifest, &is_file);
        assert!(layout.publishes_source_of("packages/x/src/x.ts"));
        assert!(
            !layout.publishes_source_of("packages/x/lib/x.ts"),
            "skif maps `./lib/x.js` to `src/x.ts`, the first candidate that exists"
        );
        assert!(
            layout.publishes_source_of("packages/x/lib/y.ts"),
            "the path itself when no `src` twin exists"
        );
        assert!(
            !layout.publishes_source_of("packages/x/src/y.ts"),
            "a source that does not exist"
        );

        let only_lib = strings(&["packages/x/lib/x.ts", "packages/x/lib/x.tsx"]);
        let is_file = |file: &str| only_lib.contains(file);
        let layout = Layout::new(strings(&["packages/x"]), &manifest, &is_file);
        assert!(layout.publishes_source_of("packages/x/lib/x.ts"));
        assert!(
            !layout.publishes_source_of("packages/x/lib/x.tsx"),
            "`.ts` comes before `.tsx`"
        );
    }

    /// The manifests of a checkout with an n8n node package in `packages/nodes`, by directory.
    fn n8n_manifests() -> impl Fn(&str) -> Option<Rc<Value>> {
        let read: BTreeMap<String, Rc<Value>> = [
            ("", json!({ "name": "root" })),
            (
                "packages/nodes",
                json!({ "name": "nodes", "main": "index.js", "n8n": {
                    "nodes": ["dist/nodes/A/A.node.js"],
                    "credentials": ["dist/credentials/AApi.credentials.js"],
                } }),
            ),
        ]
        .into_iter()
        .map(|(dir, manifest)| (dir.to_owned(), Rc::new(manifest)))
        .collect();
        move |dir: &str| read.get(dir).cloned()
    }

    #[test]
    fn the_node_and_credential_files_of_an_n8n_node_package_are_published_sources() {
        let manifest = n8n_manifests();
        let files = strings(&[
            "packages/nodes/nodes/A/A.node.ts",
            "packages/nodes/credentials/AApi.credentials.ts",
            "packages/nodes/nodes/A/GenericFunctions.ts",
            "packages/nodes/src/nodes/B/B.node.ts",
        ]);
        let is_file = |file: &str| files.contains(file);
        let layout = Layout::new(strings(&["packages/nodes"]), &manifest, &is_file);
        assert!(
            layout.publishes_source_of("packages/nodes/nodes/A/A.node.ts"),
            "`dist/nodes/A/A.node.js` is built from `nodes/A/A.node.ts`"
        );
        assert!(layout.publishes_source_of("packages/nodes/credentials/AApi.credentials.ts"));
        assert!(
            !layout.publishes_source_of("packages/nodes/nodes/A/GenericFunctions.ts"),
            "a file the package does not list"
        );
        assert!(
            !layout.publishes_source_of("packages/nodes/src/nodes/B/B.node.ts"),
            "a node the package does not list"
        );
    }

    #[test]
    fn files_a_published_source_reaches_through_files_only_knip_reports_unused_are_published_sources() {
        let manifest = n8n_manifests();
        let sources: BTreeMap<&str, &str> = [
            (
                "packages/nodes/nodes/A/A.node.ts",
                "import { request } from './GenericFunctions';\n\
                 import * as fields from './descriptions';\n\
                 import { both } from '../Both';\n\
                 import text from './Raw.ts?raw';\n\
                 import { outside } from '../../../../../outside';\n",
            ),
            (
                "packages/nodes/nodes/A/GenericFunctions.ts",
                "export const request = 1;\n",
            ),
            (
                "packages/nodes/nodes/A/descriptions/index.ts",
                "export * from './Fields.js';\n",
            ),
            ("packages/nodes/nodes/A/descriptions/Fields.ts", "export const a = 1;\n"),
            ("packages/nodes/nodes/A/Raw.ts", "export const raw = 1;\n"),
            ("packages/nodes/nodes/Both.ts", "import { other } from './Other';\n"),
            ("packages/nodes/nodes/Other.ts", "export const other = 1;\n"),
        ]
        .into_iter()
        .collect();
        let is_file = |file: &str| sources.contains_key(file);
        let layout = Layout::new(strings(&["packages/nodes"]), &manifest, &is_file);
        let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
        let knip_unused: BTreeSet<String> = sources.keys().map(|file| (*file).to_owned()).collect();
        let skif_unused = strings(&["packages/nodes/nodes/Both.ts"]);
        assert_eq!(
            published_reach(&knip_unused, &skif_unused, &layout, &read),
            strings(&[
                "packages/nodes/nodes/A/GenericFunctions.ts",
                "packages/nodes/nodes/A/descriptions/Fields.ts",
                "packages/nodes/nodes/A/descriptions/index.ts",
            ]),
            "a file skif reports unused too ends the chain, and a `?raw` import reads no module"
        );
        assert_eq!(
            published_reach(&knip_unused, &knip_unused, &layout, &read),
            BTreeSet::new(),
            "only files knip alone reports unused"
        );
    }

    #[test]
    fn relative_specifiers_resolve_with_extensions_twins_and_index_files() {
        let files = strings(&["src/a.ts", "src/b/index.tsx", "src/c.mts", "index.ts"]);
        let is_file = |file: &str| files.contains(file);
        assert_eq!(resolve_relative("src", "./a", &is_file).as_deref(), Some("src/a.ts"));
        assert_eq!(
            resolve_relative("src/b", "../a.js", &is_file).as_deref(),
            Some("src/a.ts")
        );
        assert_eq!(
            resolve_relative("src", "./b", &is_file).as_deref(),
            Some("src/b/index.tsx")
        );
        assert_eq!(
            resolve_relative("src", "./c.mjs", &is_file).as_deref(),
            Some("src/c.mts")
        );
        assert_eq!(resolve_relative("src", "..", &is_file).as_deref(), Some("index.ts"));
        assert_eq!(resolve_relative("src", "./missing", &is_file), None);
        assert_eq!(
            resolve_relative("src", "../../a", &is_file),
            None,
            "outside the checkout"
        );
    }

    #[test]
    fn a_star_matches_one_or_more_characters_across_directories() {
        assert!(wildcard_match("schematics/*", "schematics/a/b/index"));
        assert!(wildcard_match("locales/*-x", "locales/ff-x"));
        assert!(!wildcard_match("schematics/*", "schematics/"));
        assert!(!wildcard_match("locales/*-x", "locales/ff-y"));
        assert!(wildcard_match("index", "index"));
        assert_eq!(package_name("@mastra/core/agent"), "@mastra/core");
        assert_eq!(package_name("lodash/fp"), "lodash");
        assert_eq!(package_name("react"), "react");
    }

    #[test]
    fn the_harness_reads_members_and_manifests_from_the_checkout() {
        let root = std::env::temp_dir().join(format!("skif-bench-parity-layout-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for dir in ["packages/a", "packages/b", "examples/app"] {
            fs::create_dir_all(root.join(dir)).unwrap();
            fs::write(root.join(dir).join("package.json"), r#"{"name":"x"}"#).unwrap();
        }
        fs::create_dir_all(root.join("packages/no-manifest")).unwrap();
        fs::create_dir_all(root.join("examples/broken")).unwrap();
        fs::write(root.join("examples/broken/package.json"), "{ not json").unwrap();
        fs::write(root.join("package.json"), r#"{"name":"root"}"#).unwrap();
        fs::write(
            root.join("pnpm-workspace.yaml"),
            "packages:\n  - 'packages/*'\n  - '!packages/b'\n",
        )
        .unwrap();
        let root = fs::canonicalize(&root).unwrap();
        assert_eq!(workspace_members(&root), strings(&["packages/a"]));
        let read = manifest_reader(&root);
        assert_eq!(read("examples/app").as_deref(), Some(&json!({ "name": "x" })));
        assert_eq!(
            read("examples/broken").as_deref(),
            Some(&Value::Null),
            "an unparseable manifest is still a manifest"
        );
        assert!(read("packages/no-manifest").is_none());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn findings_inside_a_file_the_other_tool_calls_unused_and_config_errors_split_off_on_both_sides() {
        let read: BTreeMap<String, Rc<Value>> = [
            ("", json!({ "name": "root" })),
            ("packages/cfg", json!({ "name": "cfg" })),
        ]
        .into_iter()
        .map(|(dir, manifest)| (dir.to_owned(), Rc::new(manifest)))
        .collect();
        let manifest = move |dir: &str| read.get(dir).cloned();
        let every_file = |_: &str| true;
        let layout = Layout::new(strings(&["packages/cfg"]), &manifest, &every_file);
        let knip_unused = strings(&["examples/a/src/dead.ts"]);
        let skif_unused = strings(&["packages/b/src/gone.ts"]);
        let failed = strings(&["packages/cfg"]);
        let classifier = Classifier {
            knip_failed: &failed,
            knip_unused_files: &knip_unused,
            skif_unused_files: &skif_unused,
            ..bare(&layout)
        };
        for (category, key) in [
            ("exports", "examples/a/src/dead.ts|x"),
            ("types", "examples/a/src/dead.ts|T"),
            ("unlisted", "examples/a/src/dead.ts|react"),
            ("duplicates", "examples/a/src/dead.ts|a|b"),
            ("unresolved", "examples/a/src/dead.ts|./gone"),
        ] {
            assert_eq!(
                classifier.classify(Side::Skif, category, key),
                Some(IN_KNIP_UNUSED_FILE),
                "{category} {key}"
            );
        }
        assert_eq!(
            classifier.classify(Side::Tool, "exports", "packages/b/src/gone.ts|x"),
            Some(IN_SKIF_UNUSED_FILE)
        );
        for (side, category, key) in [
            (Side::Skif, "files", "examples/a/src/dead.ts"),
            (Side::Skif, "binaries", "examples/a/src/dead.ts|tsc"),
            (Side::Skif, "exports", "examples/a/src/live.ts|x"),
            (Side::Skif, "exports", "packages/b/src/gone.ts|x"),
            (Side::Tool, "exports", "examples/a/src/dead.ts|x"),
        ] {
            assert_eq!(
                classifier.classify(side, category, key),
                None,
                "{side:?} {category} {key}"
            );
        }
        for side in [Side::Skif, Side::Tool] {
            assert_eq!(
                classifier.classify(side, "exports", "packages/cfg/src/a.ts|x"),
                Some(KNIP_CONFIG_ERROR),
                "{side:?}"
            );
        }
        assert_eq!(
            classifier.classify(Side::Skif, "exports", "packages/other/a.ts|x"),
            None
        );
    }

    #[test]
    fn the_import_scan_names_the_bare_packages_a_file_imports() {
        let source = r##"import React from "react";
import type { Config } from "@scope/config/types";
import "side-effect/register";
import { join } from "node:path";
import local from "./local";
import internal from "#internal";
export { helper } from "re-exported";
const lazy = await import("lazy-package");
const required = require("required-package/sub");
const files = import.meta.glob("./pages/*.ts");
vi.mock("mocked-package");
"##;
        let packages = imported_packages("src/a.ts", source);
        assert_eq!(
            packages.iter().map(String::as_str).collect::<Vec<_>>(),
            [
                "@scope/config",
                "lazy-package",
                "re-exported",
                "react",
                "required-package",
                "side-effect"
            ]
        );
        assert!(imported_packages("styles/a.css", ".a { color: red }").is_empty());
        assert_eq!(bare_package("https://esm.sh/react"), None);
        assert_eq!(bare_package("C:\\deps\\a.js"), None);
        assert_eq!(bare_package("@scope/name/deep"), Some("@scope/name"));
        assert_eq!(typed_package("node"), "node");
        assert_eq!(typed_package("babel__core"), "@babel/core");
    }

    #[test]
    fn dependencies_only_files_knip_lists_unused_import_split_off() {
        let sources = BTreeMap::from([
            (
                "libraries/a/src/dead.ts",
                "import { x } from 'left-pad';\nimport core from '@babel/core';\n",
            ),
            ("libraries/a/vendor/old.ts", "import 'from-nested';\n"),
            ("scripts/dead.ts", "import 'root-only';\n"),
        ]);
        let unused: BTreeSet<String> = sources
            .keys()
            .map(|file| (*file).to_owned())
            .chain(["libraries/a/src/gone.ts".to_owned()])
            .collect();
        let workspaces = ["libraries/a"];
        let read_source = |file: &str| sources.get(file).map(|source| (*source).to_owned());
        let workspace_of = |file: &str| owner_dir(file, &|dir: &str| workspaces.contains(&dir));
        let imports = unused_file_imports(&unused, &BTreeSet::new(), &read_source, &workspace_of);
        assert_eq!(
            imports,
            strings(&[
                "libraries/a/package.json|@babel/core",
                "libraries/a/package.json|from-nested",
                "libraries/a/package.json|left-pad",
                "package.json|root-only",
            ]),
            "a file counts to its workspace, an unreadable one imports nothing"
        );
        assert_eq!(
            unused_file_imports(
                &unused,
                &strings(&["libraries/a/src/dead.ts", "scripts/dead.ts"]),
                &read_source,
                &workspace_of
            ),
            strings(&["libraries/a/package.json|from-nested"]),
            "a file skif calls unused too credits nothing on either side"
        );

        let read: BTreeMap<String, Rc<Value>> =
            [("", json!({ "name": "root" })), ("libraries/a", json!({ "name": "a" }))]
                .into_iter()
                .map(|(dir, manifest)| (dir.to_owned(), Rc::new(manifest)))
                .collect();
        let manifest = move |dir: &str| read.get(dir).cloned();
        let every_file = |_: &str| true;
        let layout = Layout::new(strings(&workspaces), &manifest, &every_file);
        let classifier = Classifier {
            knip_unused_files: &unused,
            knip_unused_imports: &imports,
            ..bare(&layout)
        };
        for (category, key) in [
            ("dependencies", "libraries/a/package.json|left-pad"),
            ("devDependencies", "libraries/a/package.json|from-nested"),
            ("optionalPeerDependencies", "libraries/a/package.json|@babel/core"),
            ("devDependencies", "libraries/a/package.json|@types/babel__core"),
            ("devDependencies", "package.json|root-only"),
        ] {
            assert_eq!(
                classifier.classify(Side::Tool, category, key),
                Some(USED_IN_KNIP_UNUSED_FILE),
                "{category} {key}"
            );
        }
        for (side, category, key) in [
            (Side::Tool, "dependencies", "package.json|left-pad"),
            (Side::Tool, "dependencies", "libraries/a/package.json|root-only"),
            (Side::Tool, "devDependencies", "libraries/a/package.json|@types/node"),
            (Side::Tool, "binaries", "libraries/a/package.json|left-pad"),
            (Side::Tool, "unlisted", "libraries/a/src/live.ts|left-pad"),
            (Side::Skif, "dependencies", "libraries/a/package.json|left-pad"),
        ] {
            assert_eq!(
                classifier.classify(side, category, key),
                None,
                "{side:?} {category} {key}"
            );
        }

        let skif_unused = strings(&["libraries/a/src/dead.ts"]);
        let imports = unused_file_imports(&unused, &skif_unused, &read_source, &workspace_of);
        let both_unused = Classifier {
            skif_unused_files: &skif_unused,
            knip_unused_imports: &imports,
            ..classifier
        };
        assert_eq!(
            both_unused.classify(Side::Tool, "dependencies", "libraries/a/package.json|left-pad"),
            None,
            "a dependency only a file both tools call unused imports stays compared"
        );
        assert_eq!(
            both_unused.classify(Side::Tool, "devDependencies", "libraries/a/package.json|from-nested"),
            Some(USED_IN_KNIP_UNUSED_FILE)
        );
    }

    #[test]
    fn dependencies_only_files_skif_reports_unused_import_split_off_on_skif_side() {
        let sources = BTreeMap::from([
            (
                "libraries/a/src/dead.ts",
                "import { x } from 'left-pad';\nimport core from '@babel/core';\n",
            ),
            ("libraries/a/src/both.ts", "import 'both-unused';\n"),
        ]);
        let skif_unused = strings(&["libraries/a/src/dead.ts", "libraries/a/src/both.ts"]);
        let knip_unused = strings(&["libraries/a/src/both.ts"]);
        let read_source = |file: &str| sources.get(file).map(|source| (*source).to_owned());
        let workspace_of = |file: &str| owner_dir(file, &|dir: &str| dir == "libraries/a");
        let imports = unused_file_imports(&skif_unused, &knip_unused, &read_source, &workspace_of);
        assert_eq!(
            imports,
            strings(&[
                "libraries/a/package.json|@babel/core",
                "libraries/a/package.json|left-pad"
            ]),
            "only the files skif alone reports unused are scanned"
        );

        let read: BTreeMap<String, Rc<Value>> =
            [("", json!({ "name": "root" })), ("libraries/a", json!({ "name": "a" }))]
                .into_iter()
                .map(|(dir, manifest)| (dir.to_owned(), Rc::new(manifest)))
                .collect();
        let manifest = move |dir: &str| read.get(dir).cloned();
        let every_file = |_: &str| true;
        let layout = Layout::new(strings(&["libraries/a"]), &manifest, &every_file);
        let classifier = Classifier {
            knip_unused_files: &knip_unused,
            skif_unused_files: &skif_unused,
            skif_unused_imports: &imports,
            ..bare(&layout)
        };
        for (category, key) in [
            ("dependencies", "libraries/a/package.json|left-pad"),
            ("optionalPeerDependencies", "libraries/a/package.json|@babel/core"),
            ("devDependencies", "libraries/a/package.json|@types/babel__core"),
        ] {
            assert_eq!(
                classifier.classify(Side::Skif, category, key),
                Some(USED_IN_SKIF_UNUSED_FILE),
                "{category} {key}"
            );
        }
        for (side, category, key) in [
            (Side::Skif, "dependencies", "libraries/a/package.json|both-unused"),
            (Side::Skif, "dependencies", "package.json|left-pad"),
            (Side::Skif, "binaries", "libraries/a/package.json|left-pad"),
            (Side::Tool, "dependencies", "libraries/a/package.json|left-pad"),
        ] {
            assert_eq!(
                classifier.classify(side, category, key),
                None,
                "{side:?} {category} {key}"
            );
        }
    }

    #[test]
    fn exports_only_files_one_tool_lists_unused_import_split_off_through_re_exports() {
        let sources = BTreeMap::from([
            (
                "packages/a/src/dead.ts",
                "import { helper, type Shape } from './util';\n\
                 import * as ns from './ns';\n\
                 import value from './def';\n\
                 import './side';\n\
                 import { deep, alias, own } from './barrel';\n\
                 const lazy = import('./lazy');\n\
                 console.log(helper, ns.read, value, deep, alias, own, lazy);\n",
            ),
            (
                "packages/a/src/both.ts",
                "import { second } from './util';\nconsole.log(second);\n",
            ),
            (
                "packages/a/src/util.ts",
                "export const helper = 1;\nexport const second = 2;\nexport const unusedHelper = 3;\nexport type Shape = { a: number };\n",
            ),
            (
                "packages/a/src/ns.ts",
                "export const read = 1;\nexport const unread = 2;\n",
            ),
            ("packages/a/src/def.ts", "export default 1;\nexport const other = 2;\n"),
            ("packages/a/src/side.ts", "export const effect = 1;\n"),
            (
                "packages/a/src/barrel.ts",
                "export * from './impl';\nexport { original as alias } from './other';\nexport const own = 1;\n",
            ),
            (
                "packages/a/src/impl.ts",
                "export const deep = 1;\nexport const own = 2;\nexport const shallow = 3;\n",
            ),
            (
                "packages/a/src/other.ts",
                "export const original = 1;\nexport const rest = 2;\n",
            ),
            ("packages/a/src/lazy.ts", "export const anything = 1;\n"),
        ]);
        let is_file = |file: &str| sources.contains_key(file);
        let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
        let unused = strings(&["packages/a/src/dead.ts", "packages/a/src/both.ts"]);
        let both = strings(&["packages/a/src/both.ts"]);
        let credits = unused_file_credits(&unused, &both, &Resolver::relative(&is_file), &read);
        assert_eq!(
            credits,
            strings(&[
                "packages/a/src/barrel.ts|alias",
                "packages/a/src/barrel.ts|deep",
                "packages/a/src/barrel.ts|own",
                "packages/a/src/def.ts|default",
                "packages/a/src/impl.ts|deep",
                "packages/a/src/lazy.ts|*",
                "packages/a/src/ns.ts|read",
                "packages/a/src/other.ts|original",
                "packages/a/src/util.ts|Shape",
                "packages/a/src/util.ts|helper",
            ]),
            "a side-effect import uses nothing, an `export *` forwards only names the barrel does not export itself, \
             and a file both tools call unused credits nothing"
        );
        assert!(unused_file_credits(&unused, &unused, &Resolver::relative(&is_file), &read).is_empty());

        let manifest = |dir: &str| (dir.is_empty() || dir == "packages/a").then(|| Rc::new(json!({ "name": "a" })));
        let layout = Layout::new(strings(&["packages/a"]), &manifest, &is_file);
        let classifier = Classifier {
            knip_unused_files: &unused,
            skif_unused_files: &both,
            knip_unused_credits: &credits,
            skif_unused_credits: &credits,
            ..bare(&layout)
        };
        for (category, key) in [
            ("exports", "packages/a/src/util.ts|helper"),
            ("types", "packages/a/src/util.ts|Shape"),
            ("nsExports", "packages/a/src/ns.ts|read"),
            ("exports", "packages/a/src/impl.ts|deep"),
            ("exports", "packages/a/src/lazy.ts|anything"),
            ("duplicates", "packages/a/src/impl.ts|deep|shallow"),
        ] {
            assert_eq!(
                classifier.classify(Side::Tool, category, key),
                Some(IMPORTED_BY_KNIP_UNUSED_FILE),
                "{category} {key}"
            );
            assert_eq!(
                classifier.classify(Side::Skif, category, key),
                Some(IMPORTED_BY_SKIF_UNUSED_FILE),
                "{category} {key}"
            );
        }
        for (category, key) in [
            ("exports", "packages/a/src/util.ts|unusedHelper"),
            ("exports", "packages/a/src/util.ts|second"),
            ("nsExports", "packages/a/src/ns.ts|unread"),
            ("exports", "packages/a/src/impl.ts|own"),
            ("exports", "packages/a/src/impl.ts|shallow"),
            ("exports", "packages/a/src/other.ts|rest"),
            ("exports", "packages/a/src/side.ts|effect"),
            ("enumMembers", "packages/a/src/lazy.ts|Red"),
            ("unlisted", "packages/a/src/lazy.ts|react"),
        ] {
            assert_eq!(classifier.classify(Side::Tool, category, key), None, "{category} {key}");
        }
    }

    fn no_manifest(_: &str) -> Option<Rc<Value>> {
        None
    }

    #[test]
    fn tsconfig_paths_map_a_specifier_through_the_pattern_typescript_picks() {
        let sources = BTreeMap::from([
            (
                "tsconfig.base.json",
                r#"{ "compilerOptions": { "baseUrl": ".", "paths": {
                    "@app/*": ["packages/app/src/*"],
                    "@app/special": ["packages/app/special.ts"],
                    "vendor/*": ["node_modules/vendor/*"] } } }"#,
            ),
            (
                "packages/app/tsconfig.json",
                "{ \"extends\": \"../../tsconfig.base.json\", // the shared paths\n \"include\": [\"src\"] }",
            ),
            (
                "packages/web/tsconfig.json",
                r#"{ "compilerOptions": { "paths": { "@/*": ["${configDir}/src/*", "./fallback/*"] } } }"#,
            ),
            (
                "packages/web/lib/tsconfig.json",
                r#"{ "compilerOptions": { "strict": true } }"#,
            ),
            ("packages/app/src/util.ts", ""),
            ("packages/app/src/deep/index.ts", ""),
            ("packages/app/special.ts", ""),
            ("node_modules/vendor/x.js", ""),
            ("packages/web/fallback/only.ts", ""),
            ("packages/web/src/lib/api.ts", ""),
        ]);
        let is_file = |file: &str| sources.contains_key(file);
        let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
        let layout = Layout::new(BTreeSet::new(), &no_manifest, &is_file);
        let resolver = Resolver::new(&layout, &read);
        for (importer, specifier, expected) in [
            ("packages/app/src/a.ts", "@app/util", Some("packages/app/src/util.ts")),
            (
                "packages/app/src/a.ts",
                "@app/deep",
                Some("packages/app/src/deep/index.ts"),
            ),
            ("packages/app/src/a.ts", "@app/special", Some("packages/app/special.ts")),
            ("packages/app/src/a.ts", "@app/missing", None),
            ("packages/app/src/a.ts", "vendor/x", None),
            (
                "packages/web/src/a.ts",
                "@/lib/api",
                Some("packages/web/src/lib/api.ts"),
            ),
            ("packages/web/src/a.ts", "@/only", Some("packages/web/fallback/only.ts")),
            ("packages/web/lib/a.ts", "@/lib/api", None),
            ("other/a.ts", "@app/util", None),
        ] {
            assert_eq!(
                resolver.paths_target(importer, specifier).as_deref(),
                expected,
                "{importer} {specifier}: an exact pattern wins, a target in node_modules or one the nearest config does \
                 not map is none"
            );
        }
        assert_eq!(
            Resolver::relative(&is_file).resolve("packages/app/src/a.ts", "@app/util"),
            None,
            "a resolver of relative specifiers reads no paths"
        );
    }

    #[test]
    fn unlisted_imports_a_declare_module_or_the_importer_s_own_tsconfig_paths_explain_split_off() {
        let sources = BTreeMap::from([
            (
                "src/typings/vscode.d.ts",
                "declare module 'vscode' {\n  export const x: number;\n}\n",
            ),
            (
                "src/typings/icons.d.ts",
                "declare module 'icons/*' {\n  const value: string;\n  export default value;\n}\n",
            ),
            (
                "src/augment.ts",
                "import 'react';\ndeclare module 'react' {\n  interface Extra {}\n}\nexport {};\n",
            ),
            (
                "src/a.ts",
                "import * as vscode from 'vscode';\nimport home from 'icons/home';\nimport React from 'react';\n\
                 import { api } from '@/lib/api';\nimport z from 'zod';\nconsole.log(vscode, home, React, api, z);\n",
            ),
            (
                "tsconfig.json",
                r#"{ "compilerOptions": { "paths": { "@/*": ["./src/*"] } } }"#,
            ),
            ("src/lib/api.ts", "export const api = 1;\n"),
        ]);
        let is_file = |file: &str| sources.contains_key(file);
        let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
        let layout = Layout::new(BTreeSet::new(), &no_manifest, &is_file);
        let resolver = Resolver::new(&layout, &read);
        let files: Vec<String> = sources.keys().map(|file| (*file).to_owned()).collect();
        let unlisted = strings(&[
            "src/a.ts|vscode",
            "src/a.ts|icons",
            "src/a.ts|react",
            "src/a.ts|@/lib",
            "src/a.ts|zod",
        ]);
        let ambient = ambient_module_keys(&unlisted, &files, &read);
        assert_eq!(
            ambient,
            strings(&["src/a.ts|icons", "src/a.ts|vscode"]),
            "a pattern declares the subpaths it matches, and an augmentation in a module file declares nothing"
        );
        assert!(ambient_module_keys(&BTreeSet::new(), &files, &read).is_empty());
        let own = own_paths_keys(&unlisted, &resolver, &read, &|_: &str| false);
        assert_eq!(own, strings(&["src/a.ts|@/lib"]));

        let classifier = Classifier {
            ambient_modules: &ambient,
            own_paths: &own,
            ..bare(&layout)
        };
        for (side, key, expected) in [
            (Side::Tool, "src/a.ts|vscode", Some(AMBIENT_MODULE)),
            (Side::Tool, "src/a.ts|icons", Some(AMBIENT_MODULE)),
            (Side::Tool, "src/a.ts|@/lib", Some(OWN_TSCONFIG_PATHS)),
            (Side::Tool, "src/a.ts|react", None),
            (Side::Tool, "src/a.ts|zod", None),
            (Side::Skif, "src/a.ts|vscode", None),
        ] {
            assert_eq!(classifier.classify(side, "unlisted", key), expected, "{side:?} {key}");
        }
    }

    #[test]
    fn dependencies_and_files_of_imports_tsconfig_paths_map_before_the_package_split_off() {
        let sources = BTreeMap::from([
            (
                "packages/app/tsconfig.json",
                r#"{ "compilerOptions": { "paths": { "@acme/ui": ["./src/ui/index.ts"], "@acme/ui/*": ["./src/ui/*"],
                    "@acme/theme": ["../theme/src/index.ts"] } } }"#,
            ),
            (
                "packages/app/src/main.ts",
                "import { Button } from '@acme/ui';\nimport { cn } from '@acme/ui/cn';\nimport z from 'zod';\n\
                 import { dark } from '@acme/theme';\nconsole.log(Button, cn, z, dark);\n",
            ),
            ("packages/app/src/ui/index.ts", "export const Button = 1;\n"),
            ("packages/app/src/ui/cn.ts", "export const cn = 1;\n"),
            ("packages/theme/src/index.ts", "export const dark = 1;\n"),
            (
                "packages/other/src/main.ts",
                "import { Button } from '@acme/ui';\nconsole.log(Button);\n",
            ),
        ]);
        let manifests: BTreeMap<&str, Rc<Value>> = [
            ("", json!({ "name": "root", "workspaces": ["packages/*"] })),
            (
                "packages/app",
                json!({ "name": "app", "dependencies": { "@acme/ui": "1.0.0", "@acme/theme": "workspace:*", "zod": "3.0.0" } }),
            ),
            ("packages/other", json!({ "name": "other", "dependencies": { "@acme/ui": "1.0.0" } })),
            ("packages/theme", json!({ "name": "@acme/theme" })),
        ]
        .into_iter()
        .map(|(dir, manifest)| (dir, Rc::new(manifest)))
        .collect();
        let manifest = |dir: &str| manifests.get(dir).cloned();
        let is_file = |file: &str| sources.contains_key(file);
        let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
        let layout = Layout::new(
            strings(&["packages/app", "packages/other", "packages/theme"]),
            &manifest,
            &is_file,
        );
        let resolver = Resolver::new(&layout, &read);
        let files: Vec<String> = sources.keys().map(|file| (*file).to_owned()).collect();
        let found = paths_before_packages(&files, &resolver, &layout, &read);
        assert_eq!(
            found,
            strings(&[
                "packages/app/package.json|@acme/ui",
                "packages/app/src/ui/cn.ts",
                "packages/app/src/ui/index.ts",
            ]),
            "a mapping into another workspace member is an import of it, a workspace without a configuration that \
             maps the package keeps it, and a mapping to no file is none"
        );
        let classifier = Classifier {
            paths_before_packages: &found,
            ..bare(&layout)
        };
        for (side, category, key, expected) in [
            (
                Side::Skif,
                "dependencies",
                "packages/app/package.json|@acme/ui",
                Some(PATHS_BEFORE_PACKAGES),
            ),
            (
                Side::Tool,
                "files",
                "packages/app/src/ui/cn.ts",
                Some(PATHS_BEFORE_PACKAGES),
            ),
            (Side::Skif, "dependencies", "packages/app/package.json|zod", None),
            (
                Side::Skif,
                "dependencies",
                "packages/app/package.json|@acme/theme",
                None,
            ),
            (Side::Skif, "dependencies", "packages/other/package.json|@acme/ui", None),
            (Side::Skif, "files", "packages/app/src/ui/cn.ts", None),
            (Side::Tool, "dependencies", "packages/app/package.json|@acme/ui", None),
        ] {
            assert_eq!(
                classifier.classify(side, category, key),
                expected,
                "{side:?} {category} {key}"
            );
        }
    }

    #[test]
    fn exports_files_only_knip_lists_unused_import_through_tsconfig_paths_and_workspace_packages_split_off() {
        let sources = BTreeMap::from([
            (
                "apps/web/tsconfig.json",
                r#"{ "compilerOptions": { "paths": { "@/*": ["./src/*"] } } }"#,
            ),
            (
                "apps/web/src/components/login.tsx",
                "import { login } from '@/lib/api';\nimport { Button } from '@acme/ui';\nimport { cn } from '@acme/ui/cn';\n\
                 console.log(login, Button, cn);\n",
            ),
            (
                "apps/web/src/lib/api.ts",
                "export const login = 1;\nexport const logout = 2;\n",
            ),
            ("packages/ui/src/index.ts", "export * from './button';\n"),
            (
                "packages/ui/src/button.ts",
                "export const Button = 1;\nexport const Link = 2;\n",
            ),
            ("packages/ui/src/cn.ts", "export const cn = 1;\n"),
        ]);
        let manifests: BTreeMap<&str, Rc<Value>> = [
            ("", json!({ "name": "root", "workspaces": ["apps/*", "packages/*"] })),
            ("apps/web", json!({ "name": "web" })),
            (
                "packages/ui",
                json!({ "name": "@acme/ui", "exports": { ".": "./dist/index.js", "./cn": "./dist/cn.js" } }),
            ),
        ]
        .into_iter()
        .map(|(dir, manifest)| (dir, Rc::new(manifest)))
        .collect();
        let manifest = |dir: &str| manifests.get(dir).cloned();
        let is_file = |file: &str| sources.contains_key(file);
        let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
        let layout = Layout::new(strings(&["apps/web", "packages/ui"]), &manifest, &is_file);
        let resolver = Resolver::new(&layout, &read);
        let unused = strings(&["apps/web/src/components/login.tsx"]);
        let credits = unused_file_credits(&unused, &BTreeSet::new(), &resolver, &read);
        for credited in [
            "apps/web/src/lib/api.ts|login",
            "packages/ui/src/index.ts|Button",
            "packages/ui/src/button.ts|Button",
            "packages/ui/src/cn.ts|cn",
        ] {
            assert!(credits.contains(credited), "{credited} in {credits:?}");
        }
        for uncredited in ["apps/web/src/lib/api.ts|logout", "packages/ui/src/button.ts|Link"] {
            assert!(!credits.contains(uncredited), "{uncredited} in {credits:?}");
        }
        assert!(
            unused_file_credits(&unused, &BTreeSet::new(), &Resolver::relative(&is_file), &read).is_empty(),
            "without paths and workspace packages the file imports nothing by relative path"
        );
    }

    #[test]
    fn members_of_an_enum_used_as_a_whole_split_off() {
        let sources = BTreeMap::from([
            (
                "src/enums.ts",
                "export enum KeyCode { A, B }\nexport enum Language { En, De }\nexport enum Read { P, Q }\n\
                 export enum Typed { M, N }\nexport enum Local { L1, L2 }\nexport enum Lazy { Z1, Z2 }\n\
                 export const names = Object.keys(Local);\n",
            ),
            ("src/barrel.ts", "export * from './enums';\n"),
            (
                "src/api.ts",
                "import * as enums from './enums';\nexport const api = { KeyCode: enums.KeyCode, p: enums.Read.P };\n",
            ),
            (
                "src/dto.ts",
                "import { Language } from './barrel';\nexport class Dto { @IsEnum(Language) language!: string; }\n",
            ),
            (
                "src/input.ts",
                "import { Typed } from './barrel';\nexport function take(value: Typed): void { console.log(value); }\n",
            ),
            (
                "src/lazy.ts",
                "export async function load() { return import('./other'); }\n",
            ),
        ]);
        let is_file = |file: &str| sources.contains_key(file);
        let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
        let layout = Layout::new(BTreeSet::new(), &no_manifest, &is_file);
        let resolver = Resolver::new(&layout, &read);
        let files: Vec<String> = sources.keys().map(|file| (*file).to_owned()).collect();
        let members = strings(&[
            "src/enums.ts|A",
            "src/enums.ts|En",
            "src/enums.ts|L1",
            "src/enums.ts|Q",
            "src/enums.ts|M",
            "src/enums.ts|Z1",
            "src/gone.ts|A",
        ]);
        let found = whole_enum_members(&members, &files, &resolver, &read);
        assert_eq!(
            found,
            strings(&["src/enums.ts|A", "src/enums.ts|En", "src/enums.ts|L1"]),
            "a namespace member in an object literal, a decorator argument through a barrel and `Object.keys` in the \
             own file use the whole enum; a member read, a type annotation and an enum no file names do not"
        );
        assert!(whole_enum_members(&BTreeSet::new(), &files, &resolver, &read).is_empty());

        let classifier = Classifier {
            whole_enum_members: &found,
            ..bare(&layout)
        };
        assert_eq!(
            classifier.classify(Side::Tool, "enumMembers", "src/enums.ts|A"),
            Some(WHOLE_ENUM_USE)
        );
        for (side, category, key) in [
            (Side::Tool, "enumMembers", "src/enums.ts|Q"),
            (Side::Skif, "enumMembers", "src/enums.ts|A"),
            (Side::Tool, "exports", "src/enums.ts|A"),
        ] {
            assert_eq!(
                classifier.classify(side, category, key),
                None,
                "{side:?} {category} {key}"
            );
        }
    }

    /// A manifest reader over these manifests, by directory.
    fn manifests_of(manifests: &[(&str, Value)]) -> impl Fn(&str) -> Option<Rc<Value>> + use<> {
        let read: BTreeMap<String, Rc<Value>> = manifests
            .iter()
            .map(|(dir, manifest)| ((*dir).to_owned(), Rc::new(manifest.clone())))
            .collect();
        move |dir: &str| read.get(dir).cloned()
    }

    #[test]
    fn exports_a_published_source_or_a_nested_root_s_default_entry_re_exports_split_off() {
        let manifest = manifests_of(&[
            ("", json!({ "name": "root", "workspaces": ["packages/*"] })),
            (
                "packages/ai",
                json!({ "name": "@n8n/ai", "exports": { ".": "./dist/index.js" } }),
            ),
            ("e2e/registry", json!({ "name": "registry-setup", "private": true })),
        ]);
        let sources = BTreeMap::from([
            (
                "packages/ai/src/index.ts",
                "export * from './gen-ai/attributes';\nexport { get } from './node-catalog/get';\n",
            ),
            (
                "packages/ai/src/gen-ai/attributes.ts",
                "export const a = 1;\nexport const b = 2;\n",
            ),
            (
                "packages/ai/src/node-catalog/get.ts",
                "export function get() {}\nexport function other() {}\n",
            ),
            ("packages/ai/src/unused.ts", "export const lonely = 1;\n"),
            ("e2e/registry/index.js", "export * from './registry.js';\n"),
            ("e2e/registry/registry.js", "export const start = 1;\n"),
            ("e2e/registry/other.js", "export const stop = 1;\n"),
        ]);
        let is_file = |file: &str| sources.contains_key(file);
        let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
        let layout = Layout::new(strings(&["packages/ai"]), &manifest, &is_file);
        let resolver = Resolver::new(&layout, &read);
        let files: Vec<String> = sources.keys().map(|file| (*file).to_owned()).collect();
        let roots = strings(&["e2e/registry"]);
        let keys = strings(&[
            "packages/ai/src/gen-ai/attributes.ts|a",
            "packages/ai/src/node-catalog/get.ts|get",
            "packages/ai/src/unused.ts|lonely",
            "e2e/registry/registry.js|start",
        ]);
        let no_plugins: &dyn Fn(&str) -> PluginEntries = &|_: &str| PluginEntries::none();
        let (published, defaults) = entry_modules(&keys, &files, &layout, (&roots, no_plugins));
        assert_eq!(published, ["packages/ai/src/index.ts"]);
        assert_eq!(
            defaults,
            ["e2e/registry/index.js"],
            "a workspace's default entry is knip's entry as well"
        );
        assert_eq!(
            entry_modules(&BTreeSet::new(), &files, &layout, (&roots, no_plugins)),
            (Vec::new(), Vec::new())
        );
        let published_reexports = entry_reexports(published, &resolver, &read);
        let root_entry_reexports = entry_reexports(defaults, &resolver, &read);
        let classifier = Classifier {
            published_reexports: &published_reexports,
            root_entry_reexports: &root_entry_reexports,
            skif_root_packages: &roots,
            ..bare(&layout)
        };
        for (category, key, expected) in [
            (
                "exports",
                "packages/ai/src/gen-ai/attributes.ts|a",
                Some(PUBLISHED_SOURCE),
            ),
            (
                "types",
                "packages/ai/src/node-catalog/get.ts|get",
                Some(PUBLISHED_SOURCE),
            ),
            ("exports", "packages/ai/src/node-catalog/get.ts|other", None),
            ("exports", "packages/ai/src/unused.ts|lonely", None),
            ("enumMembers", "packages/ai/src/gen-ai/attributes.ts|a", None),
            ("exports", "e2e/registry/registry.js|start", Some(NON_MEMBER_PROJECT)),
            ("exports", "e2e/registry/other.js|stop", None),
        ] {
            assert_eq!(
                classifier.classify(Side::Tool, category, key),
                expected,
                "{category} {key}"
            );
        }
    }

    #[test]
    fn exports_of_the_files_skif_s_plugins_make_entries_of_a_nested_root_split_off() {
        let kyc = json!({ "name": "kyc", "dependencies": { "@mastra/core": "1.0.0" } });
        let gql = json!({ "name": "gql", "devDependencies": { "@graphql-codegen/cli": "2.13.7" } });
        let manifest = manifests_of(&[
            ("", json!({ "name": "root", "workspaces": ["packages/*"] })),
            ("templates/kyc", kyc.clone()),
            ("examples/gql", gql.clone()),
        ]);
        let sources = BTreeMap::from([
            ("templates/kyc/package.json", kyc.to_string()),
            (
                "templates/kyc/src/mastra/tools/start.ts",
                "export type StartInput = { id: string };\nexport const startTool = 1;\n".to_owned(),
            ),
            (
                "templates/kyc/src/mastra/workflows/flow.ts",
                "export { collect } from './steps/collect';\nexport const flow = 1;\n".to_owned(),
            ),
            (
                "templates/kyc/src/mastra/workflows/steps/collect.ts",
                "export const collect = 1;\nexport const resumeSchema = 1;\n".to_owned(),
            ),
            ("examples/gql/package.json", gql.to_string()),
            (
                "examples/gql/codegen.ts",
                "export default { schema: 'schema.graphql', generates: { './lib/gql/': { preset: 'client' } } };\n"
                    .to_owned(),
            ),
            (
                "examples/gql/lib/gql/graphql.ts",
                "export type Maybe<T> = T | null;\n".to_owned(),
            ),
            ("examples/gql/lib/other.ts", "export type Other = 1;\n".to_owned()),
        ]);
        let mut memory = MemoryFs::new();
        for (path, source) in &sources {
            memory = memory.with_file(format!("/repo/{path}"), source);
        }
        let root = Path::new("/repo");
        let kyc_entries = plugin_entries(&memory, root, ("templates/kyc", true), &kyc, &[]);
        assert!(kyc_entries.contains("src/mastra/tools/start.ts"));
        assert!(kyc_entries.contains("src/mastra/workflows/flow.ts"));
        assert!(
            !kyc_entries.contains("src/mastra/workflows/steps/collect.ts"),
            "Mastra's workflow glob takes the files directly in `workflows`"
        );
        let gql_entries = plugin_entries(&memory, root, ("examples/gql", true), &gql, &[]);
        assert!(gql_entries.contains("lib/gql/graphql.ts"), "a `generates` output");
        assert!(gql_entries.contains("codegen.ts"), "a code configuration file");
        assert!(!gql_entries.contains("lib/other.ts"));
        assert!(!PluginEntries::none().contains("lib/gql/graphql.ts"));
        assert_eq!(
            packages_below(
                &strings(&["", "templates/kyc", "templates/kyc/portal", "templates/kycx"]),
                "templates/kyc"
            ),
            ["portal"]
        );

        let is_file = |file: &str| sources.contains_key(file);
        let read = |file: &str| sources.get(file).cloned();
        let layout = Layout::new(strings(&["packages/app"]), &manifest, &is_file);
        let resolver = Resolver::new(&layout, &read);
        let files: Vec<String> = sources.keys().map(|file| (*file).to_owned()).collect();
        let roots = strings(&["templates/kyc", "examples/gql"]);
        let manifests = [("templates/kyc", &kyc), ("examples/gql", &gql)];
        let entries_of: &dyn Fn(&str) -> PluginEntries = &|dir: &str| {
            manifests
                .iter()
                .find(|(project, _)| *project == dir)
                .map_or_else(PluginEntries::none, |(_, manifest)| {
                    plugin_entries(&memory, root, (dir, true), manifest, &[])
                })
        };
        let keys = strings(&[
            "templates/kyc/src/mastra/tools/start.ts|StartInput",
            "examples/gql/lib/gql/graphql.ts|Maybe",
        ]);
        let (published, defaults) = entry_modules(&keys, &files, &layout, (&roots, entries_of));
        assert_eq!(published, Vec::<String>::new());
        assert_eq!(
            defaults,
            [
                "examples/gql/codegen.ts",
                "examples/gql/lib/gql/graphql.ts",
                "templates/kyc/src/mastra/tools/start.ts",
                "templates/kyc/src/mastra/workflows/flow.ts",
            ]
        );
        let (_, not_root) = entry_modules(&keys, &files, &layout, (&BTreeSet::new(), entries_of));
        assert!(
            not_root.is_empty(),
            "a project skif does not lint as a root package has no entries here"
        );
        let root_entry_reexports = entry_reexports(defaults, &resolver, &read);
        let classifier = Classifier {
            root_entry_reexports: &root_entry_reexports,
            skif_root_packages: &roots,
            ..bare(&layout)
        };
        for (category, key, expected) in [
            (
                "types",
                "templates/kyc/src/mastra/tools/start.ts|StartInput",
                Some(NON_MEMBER_PROJECT),
            ),
            (
                "exports",
                "templates/kyc/src/mastra/workflows/steps/collect.ts|collect",
                Some(NON_MEMBER_PROJECT),
            ),
            (
                "exports",
                "templates/kyc/src/mastra/workflows/steps/collect.ts|resumeSchema",
                None,
            ),
            (
                "nsTypes",
                "examples/gql/lib/gql/graphql.ts|Maybe",
                Some(NON_MEMBER_PROJECT),
            ),
            ("types", "examples/gql/lib/other.ts|Other", None),
            ("enumMembers", "templates/kyc/src/mastra/tools/start.ts|A", None),
        ] {
            assert_eq!(
                classifier.classify(Side::Tool, category, key),
                expected,
                "{category} {key}"
            );
        }
    }

    #[test]
    fn the_sources_a_published_file_loads_from_build_output_are_published_sources() {
        let manifest = manifests_of(&[
            ("", json!({ "name": "root", "workspaces": ["packages/*"] })),
            (
                "packages/next",
                json!({ "name": "next", "main": "./dist/server/next.js",
                        "files": ["dist", "client.js", "client.d.ts", "app.js", "*.d.mts"] }),
            ),
        ]);
        let sources = BTreeMap::from([
            (
                "packages/next/client.js",
                "module.exports = require('./dist/client/index')\n",
            ),
            ("packages/next/client.d.ts", "export * from './dist/client/index'\n"),
            ("packages/next/app.js", "module.exports = require('./compat/app')\n"),
            ("packages/next/dist/server/next.js", "require('../client/other');\n"),
            (
                "packages/next/src/client/index.tsx",
                "import { help } from './helper';\nhelp();\n",
            ),
            ("packages/next/src/client/helper.ts", "export function help() {}\n"),
            ("packages/next/src/client/other.ts", "export const other = 1;\n"),
            ("packages/next/compat/app.ts", "export const app = 1;\n"),
            ("packages/next/src/lonely.ts", "export const lonely = 1;\n"),
        ]);
        let is_file = |file: &str| sources.contains_key(file);
        let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
        let layout = Layout::new(strings(&["packages/next"]), &manifest, &is_file);
        assert_eq!(
            published_file_sources(&layout, "packages/next", &read),
            ["packages/next/src/client/index.tsx"],
            "a specifier outside build output and one from a file inside it map to nothing"
        );
        let knip_unused = strings(&[
            "packages/next/src/client/index.tsx",
            "packages/next/src/client/helper.ts",
            "packages/next/src/client/other.ts",
            "packages/next/compat/app.ts",
            "packages/next/src/lonely.ts",
        ]);
        let reach = published_reach(&knip_unused, &BTreeSet::new(), &layout, &read);
        assert_eq!(
            reach,
            strings(&[
                "packages/next/src/client/helper.ts",
                "packages/next/src/client/index.tsx"
            ])
        );
        let classifier = Classifier {
            published_reach: &reach,
            ..bare(&layout)
        };
        for (key, expected) in [
            ("packages/next/src/client/index.tsx", Some(PUBLISHED_SOURCE)),
            ("packages/next/src/client/helper.ts", Some(PUBLISHED_SOURCE)),
            ("packages/next/src/client/other.ts", None),
            ("packages/next/src/lonely.ts", None),
        ] {
            assert_eq!(classifier.classify(Side::Tool, "files", key), expected, "{key}");
        }
    }

    #[test]
    fn findings_in_files_a_dot_slash_gitignore_pattern_hides_from_knip_split_off_on_skif_side() {
        let gitignores = [
            (
                "packages/core/.gitignore".to_owned(),
                "dist\n./storage/\n./a/b\n!./keep\n./*.log\n# ./c\n./\n./x/../y\n".to_owned(),
            ),
            (".gitignore".to_owned(), "./root-only\n".to_owned()),
        ];
        let ignores = dot_slash_ignores(&gitignores);
        assert_eq!(
            ignores,
            [
                ("packages/core".to_owned(), vec!["storage".to_owned()]),
                ("packages/core".to_owned(), vec!["a".to_owned(), "b".to_owned()]),
            ]
        );
        for (path, expected) in [
            ("packages/core/src/storage/index.ts", true),
            ("packages/core/storage", true),
            ("packages/core/src/a/b/c.ts", true),
            ("packages/core/src/a/c.ts", false),
            ("packages/core/src/storage-utils.ts", false),
            ("packages/core-x/src/storage/x.ts", false),
            ("packages/other/src/storage/x.ts", false),
            ("root-only/x.ts", false),
        ] {
            assert_eq!(in_dot_slash_ignore(&ignores, path), expected, "{path}");
        }
        let every_file = |_: &str| true;
        let layout = Layout::new(BTreeSet::new(), &no_manifest, &every_file);
        let classifier = Classifier {
            knip_dot_ignores: &ignores,
            ..bare(&layout)
        };
        for (side, category, key, expected) in [
            (
                Side::Skif,
                "duplicates",
                "packages/core/src/storage/logs.ts|createLog|log",
                Some(KNIP_DOT_IGNORE),
            ),
            (
                Side::Skif,
                "files",
                "packages/core/src/storage/old.ts",
                Some(KNIP_DOT_IGNORE),
            ),
            (Side::Skif, "exports", "packages/core/src/other.ts|x", None),
            (Side::Tool, "exports", "packages/core/src/storage/logs.ts|x", None),
        ] {
            assert_eq!(
                classifier.classify(side, category, key),
                expected,
                "{side:?} {category} {key}"
            );
        }
    }

    #[test]
    fn duplicates_whose_aliases_name_an_export_of_an_import_split_off_on_skif_side() {
        let sources = BTreeMap::from([
            (
                "locations.ts",
                "import { Daily as importedDaily } from './constants';\n\
                 export const DailyLocationType = importedDaily;\n\
                 export const CalVideoLocationType = DailyLocationType;\n",
            ),
            (
                "menu.ts",
                "export const inset = 'pl-1';\nexport const labelInset = inset;\n",
            ),
            (
                "with-default.ts",
                "import { a } from './a';\nexport const b = a;\nexport const c = b;\nexport default c;\n",
            ),
            ("broken.ts", "export const = ;\n"),
        ]);
        let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
        let duplicates = strings(&[
            "locations.ts|CalVideoLocationType|DailyLocationType",
            "menu.ts|inset|labelInset",
            "with-default.ts|b|c|default",
            "broken.ts|a|b",
            "missing.ts|a|b",
        ]);
        let found = reexport_alias_duplicates(&duplicates, &read);
        assert_eq!(found, strings(&["locations.ts|CalVideoLocationType|DailyLocationType"]));
        let every_file = |_: &str| true;
        let layout = Layout::new(BTreeSet::new(), &no_manifest, &every_file);
        let classifier = Classifier {
            reexport_aliases: &found,
            ..bare(&layout)
        };
        for (side, key, expected) in [
            (
                Side::Skif,
                "locations.ts|CalVideoLocationType|DailyLocationType",
                Some(REEXPORT_ALIAS),
            ),
            (Side::Skif, "menu.ts|inset|labelInset", None),
            (Side::Tool, "locations.ts|CalVideoLocationType|DailyLocationType", None),
        ] {
            assert_eq!(classifier.classify(side, "duplicates", key), expected, "{side:?} {key}");
        }
    }

    #[test]
    fn declaration_and_javascript_files_a_manifest_publishes_as_they_are_split_off() {
        let manifest = manifests_of(&[
            ("", json!({ "name": "root", "workspaces": ["packages/*"] })),
            (
                "packages/next",
                json!({ "name": "next", "files": ["dist", "cache.js", "cache.d.ts", "*.d.mts"],
                        "exports": { "./navigation": "./navigation.js" } }),
            ),
        ]);
        let every_file = |_: &str| true;
        let layout = Layout::new(strings(&["packages/next"]), &manifest, &every_file);
        for (path, expected) in [
            ("packages/next/cache.d.ts", true),
            ("packages/next/cache.js", true),
            ("packages/next/navigation.js", true),
            ("packages/next/types.d.mts", true),
            ("packages/next/lib/server.js", false),
            ("packages/next/other.js", false),
            ("packages/next/cache.ts", false),
        ] {
            assert_eq!(layout.publishes_file(path), expected, "{path}");
        }
        let classifier = bare(&layout);
        for (side, category, key, expected) in [
            (
                Side::Tool,
                "types",
                "packages/next/cache.d.ts|Cache",
                Some(PUBLISHED_FILE),
            ),
            (
                Side::Tool,
                "exports",
                "packages/next/navigation.js|redirect",
                Some(PUBLISHED_FILE),
            ),
            (Side::Tool, "files", "packages/next/cache.js", Some(PUBLISHED_FILE)),
            (Side::Tool, "files", "packages/next/cache.d.ts", Some(DECLARATION_FILE)),
            (Side::Tool, "enumMembers", "packages/next/cache.js|X", None),
            (Side::Tool, "exports", "packages/next/other.js|x", None),
            (Side::Skif, "exports", "packages/next/cache.js|x", None),
        ] {
            assert_eq!(
                classifier.classify(side, category, key),
                expected,
                "{side:?} {category} {key}"
            );
        }
    }

    #[test]
    fn files_bazel_rules_build_from_and_what_they_import_split_off() {
        let build = "load(\"@aspect_rules_ts//ts:defs.bzl\", \"ts_project\")\n\
                     ts_project(\n    name = \"pipeline\",\n    srcs = [\"index.mts\", \":helpers/format.mts\", \
                     \"//other:x.mts\", \"missing.mts\"] + glob([\"globbed/*.mts\"]),\n    \
                     tsconfig = \"tsconfig.json\",\n    data = select({\"//:on\": [\"on.mts\"]}),\n)\n\
                     js_binary(name = \"run\", entry_point = \"run.mts\", data = [\"data.json\"])\n\
                     js_unknown(name = \"x\", srcs = [\"unknown.mts\"])\n\
                     \"tail.mts\".format(x)\n";
        let literals: Vec<(String, String, String)> = rule_calls(build)
            .into_iter()
            .flat_map(|call| {
                let rule = call.rule;
                call.literals
                    .into_iter()
                    .map(move |(attribute, literal)| (rule.clone(), attribute, literal))
            })
            .collect();
        let named = |rule: &str, attribute: &str| -> Vec<&str> {
            literals
                .iter()
                .filter(|(called, key, _)| called == rule && key == attribute)
                .map(|(_, _, literal)| literal.as_str())
                .collect()
        };
        assert_eq!(
            named("ts_project", "srcs"),
            ["index.mts", ":helpers/format.mts", "//other:x.mts", "missing.mts"],
            "a glob is a nested call"
        );
        assert!(named("ts_project", "data").is_empty(), "a select is a nested call");
        assert_eq!(named("js_binary", "entry_point"), ["run.mts"]);
        assert!(
            literals
                .iter()
                .all(|(called, _, _)| called != "load" && called != "format")
        );

        let rules_js = "bazel_dep(name = \"aspect_rules_js\", version = \"2.0.0\")\n";
        let sources = BTreeMap::from([
            ("MODULE.bazel", rules_js),
            ("adev/pipeline/BUILD.bazel", build),
            (
                "adev/pipeline/index.mts",
                "import { util } from './lib/util.mts';\nconsole.log(util);\n",
            ),
            ("adev/pipeline/helpers/format.mts", "export const format = 1;\n"),
            (
                "adev/pipeline/lib/util.mts",
                "export { deep } from './deep.mts';\nexport const util = 1;\n",
            ),
            ("adev/pipeline/lib/deep.mts", "export const deep = 1;\n"),
            ("adev/pipeline/run.mts", "console.log(1);\n"),
            ("adev/pipeline/data.json", "{}"),
            ("adev/pipeline/unknown.mts", "export const unknown = 1;\n"),
            ("adev/pipeline/globbed/a.mts", "export const a = 1;\n"),
            ("adev/pipeline/on.mts", "export const on = 1;\n"),
            ("plain/MODULE.bazel", "module(name = \"plain\")\n"),
            ("plain/BUILD.bazel", "ts_project(name = \"a\", srcs = [\"a.mts\"])\n"),
            ("plain/a.mts", "export const a = 1;\n"),
        ]);
        let is_file = |file: &str| sources.contains_key(file);
        let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
        let build_files: Vec<(String, String)> = sources
            .iter()
            .filter(|(path, _)| is_build_file(path))
            .map(|(path, source)| ((*path).to_owned(), (*source).to_owned()))
            .collect();
        let files: Vec<String> = sources.keys().map(|path| (*path).to_owned()).collect();
        let sources = bazel_sources(&build_files, &files, &is_file, &read);
        assert_eq!(
            sources,
            strings(&[
                "adev/pipeline/data.json",
                "adev/pipeline/helpers/format.mts",
                "adev/pipeline/index.mts",
                "adev/pipeline/run.mts",
            ]),
            "a label of another package, a missing file, a rule skif does not read, the glob of a target that is no \
             test and a repository without rules_js name none"
        );
        let knip_unused = strings(&[
            "adev/pipeline/index.mts",
            "adev/pipeline/lib/util.mts",
            "adev/pipeline/lib/deep.mts",
            "adev/pipeline/unknown.mts",
            "adev/pipeline/globbed/a.mts",
            "plain/a.mts",
        ]);
        let reached = reach_from_sources(&sources, (&knip_unused, &BTreeSet::new()), &is_file, &read);
        assert_eq!(
            reached,
            strings(&[
                "adev/pipeline/index.mts",
                "adev/pipeline/lib/deep.mts",
                "adev/pipeline/lib/util.mts",
            ])
        );
        assert!(reach_from_sources(&sources, (&knip_unused, &knip_unused), &is_file, &read).is_empty());

        let layout = Layout::new(BTreeSet::new(), &no_manifest, &is_file);
        let classifier = Classifier {
            bazel_sources: &reached,
            ..bare(&layout)
        };
        for (category, key, expected) in [
            ("files", "adev/pipeline/index.mts", Some(BAZEL_BUILD_FILE)),
            ("files", "adev/pipeline/lib/deep.mts", Some(BAZEL_BUILD_FILE)),
            ("files", "adev/pipeline/unknown.mts", None),
            ("files", "adev/pipeline/globbed/a.mts", None),
            ("exports", "adev/pipeline/index.mts|x", None),
        ] {
            assert_eq!(
                classifier.classify(Side::Tool, category, key),
                expected,
                "{category} {key}"
            );
        }
    }

    #[test]
    fn the_glob_patterns_of_a_bazel_test_target_name_build_sources() {
        let build = "ts_project(\n    name = \"test_lib\",\n    testonly = True,\n    srcs = glob(\n        \
                     [\"*_spec.ts\", \"testing/**/*.ts\"],\n        exclude = [\"skipped_spec.ts\"],\n    ) + \
                     [\"extra.ts\"],\n)\n\
                     ng_project(name = \"specs\", testonly = True, srcs = native.glob(include = [\"ng/*.ts\"]))\n\
                     js_test(name = \"run\", entry_point = \"run.mjs\", data = glob([\"fixtures/*.mjs\"]))\n\
                     ts_project(name = \"lib\", srcs = glob([\"src/*.ts\"]))\n\
                     ts_project(name = \"maybe\", testonly = False, srcs = glob([\"maybe/*.ts\"]))\n\
                     ts_project(name = \"nested\", testonly = True, srcs = select({\"//:on\": glob([\"on/*.ts\"])}))\n\
                     ts_project(name = \"climbs\", testonly = True, srcs = glob([\"../up/*.ts\"]))\n";
        let sources = BTreeMap::from([
            (
                "MODULE.bazel",
                "bazel_dep(name = \"aspect_rules_js\", version = \"2.0.0\")\n",
            ),
            ("pkg/BUILD.bazel", build),
            ("pkg/a_spec.ts", "export {};\n"),
            ("pkg/skipped_spec.ts", "export {};\n"),
            ("pkg/extra.ts", "export {};\n"),
            ("pkg/testing/deep/helper.ts", "export {};\n"),
            ("pkg/ng/b.spec.ts", "export {};\n"),
            ("pkg/fixtures/c.mjs", "export {};\n"),
            ("pkg/run.mjs", "export {};\n"),
            ("pkg/src/d.ts", "export {};\n"),
            ("pkg/maybe/e.ts", "export {};\n"),
            ("pkg/on/f.ts", "export {};\n"),
            ("up/g.ts", "export {};\n"),
            ("other/a_spec.ts", "export {};\n"),
        ]);
        let is_file = |file: &str| sources.contains_key(file);
        let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
        let build_files = vec![("pkg/BUILD.bazel".to_owned(), build.to_owned())];
        let files: Vec<String> = sources.keys().map(|path| (*path).to_owned()).collect();
        assert_eq!(
            bazel_sources(&build_files, &files, &is_file, &read),
            strings(&[
                "pkg/a_spec.ts",
                "pkg/extra.ts",
                "pkg/fixtures/c.mjs",
                "pkg/ng/b.spec.ts",
                "pkg/run.mjs",
                "pkg/skipped_spec.ts",
                "pkg/testing/deep/helper.ts",
            ]),
            "the include patterns of a test target's glob match below its BUILD file, its exclude patterns are not \
             applied, and the glob of a production target, one inside select and one that climbs out name none"
        );
    }

    #[test]
    fn the_sources_a_published_node_bin_stub_loads_are_published_sources() {
        assert_eq!(
            stub_specifiers(
                "#!/usr/bin/env node\nrequire('../lib-commonjs/start.js');\nimport(\"./x\");\nrequire('fs');\n"
            ),
            ["../lib-commonjs/start.js", "./x"]
        );
        let manifest = manifests_of(&[
            ("", json!({ "name": "root" })),
            (
                "apps/client",
                json!({ "name": "client", "bin": { "client": "./bin/client" } }),
            ),
            ("apps/plain", json!({ "name": "plain", "bin": "./bin/plain" })),
        ]);
        let sources = BTreeMap::from([
            (
                "apps/client/bin/client",
                "#!/usr/bin/env node\nrequire('../lib-commonjs/start.js');\n",
            ),
            ("apps/client/src/start.ts", "import { run } from './run';\nrun();\n"),
            ("apps/client/src/run.ts", "export function run() {}\n"),
            ("apps/client/src/lonely.ts", "export const lonely = 1;\n"),
            ("apps/plain/bin/plain", "#!/bin/sh\nexec ../src/x.js\n"),
            ("apps/plain/src/x.ts", "export const x = 1;\n"),
        ]);
        let is_file = |file: &str| sources.contains_key(file);
        let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
        let layout = Layout::new(strings(&["apps/client", "apps/plain"]), &manifest, &is_file);
        assert_eq!(
            bin_stub_sources(&layout, "apps/client", &read),
            ["apps/client/src/start.ts"]
        );
        assert!(
            bin_stub_sources(&layout, "apps/plain", &read).is_empty(),
            "a shell stub is no node stub"
        );
        let knip_unused = strings(&[
            "apps/client/src/start.ts",
            "apps/client/src/run.ts",
            "apps/client/src/lonely.ts",
            "apps/plain/src/x.ts",
        ]);
        assert_eq!(
            published_reach(&knip_unused, &BTreeSet::new(), &layout, &read),
            strings(&["apps/client/src/run.ts", "apps/client/src/start.ts"])
        );
    }

    #[test]
    fn exports_only_skif_reports_whose_users_skif_does_not_read_split_off_on_skif_side() {
        let manifest = manifests_of(&[
            ("", json!({ "name": "root" })),
            (
                ".github/scripts",
                json!({ "name": "scripts", "type": "module", "private": true }),
            ),
        ]);
        let sources = BTreeMap::from([
            (
                "scripts/preview/labels.mjs",
                "export const PREFIX = 'x';\nexport const other = 1;\n",
            ),
            (
                ".github/scripts/preview.mjs",
                "import { PREFIX } from '../../scripts/preview/labels.mjs';\nconsole.log(PREFIX);\n",
            ),
            (".github/scripts/own.mjs", "export const inner = 1;\n"),
            (
                ".github/scripts/user.mjs",
                "import { inner } from './own.mjs';\nconsole.log(inner);\n",
            ),
            (
                ".devcontainer/ssh.test.mjs",
                "import { alias } from '../scripts/ssh.mjs';\nconsole.log(alias);\n",
            ),
            ("scripts/ssh.mjs", "export const alias = 1;\nexport const kept = 2;\n"),
        ]);
        let is_file = |file: &str| sources.contains_key(file);
        let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
        let layout = Layout::new(BTreeSet::new(), &manifest, &is_file);
        let resolver = Resolver::new(&layout, &read);
        let files: Vec<String> = sources.keys().map(|file| (*file).to_owned()).collect();
        let (non_member, skipped) = outside_credits(&files, &layout, &resolver, &read);
        assert!(
            non_member.contains("scripts/preview/labels.mjs|PREFIX"),
            "{non_member:?}"
        );
        assert!(skipped.contains("scripts/ssh.mjs|alias"), "{skipped:?}");
        assert!(!skipped.contains("scripts/preview/labels.mjs|PREFIX"), "{skipped:?}");
        let classifier = Classifier {
            non_member_credits: &non_member,
            skipped_credits: &skipped,
            ..bare(&layout)
        };
        for (side, category, key, expected) in [
            (
                Side::Skif,
                "exports",
                "scripts/preview/labels.mjs|PREFIX",
                Some(NON_MEMBER_PROJECT),
            ),
            (Side::Skif, "exports", "scripts/ssh.mjs|alias", Some(SKIPPED_DIRECTORY)),
            (Side::Skif, "exports", "scripts/preview/labels.mjs|other", None),
            (Side::Skif, "exports", "scripts/ssh.mjs|kept", None),
            (Side::Skif, "exports", ".github/scripts/own.mjs|inner", None),
            (Side::Skif, "enumMembers", "scripts/ssh.mjs|alias", None),
            (Side::Tool, "exports", "scripts/ssh.mjs|alias", None),
        ] {
            assert_eq!(
                classifier.classify(side, category, key),
                expected,
                "{side:?} {category} {key}"
            );
        }
    }

    #[test]
    fn members_only_skif_reports_whose_users_skif_does_not_read_split_off_on_skif_side() {
        let manifest = manifests_of(&[
            ("", json!({ "name": "root" })),
            (".github/scripts", json!({ "name": "scripts", "private": true })),
        ]);
        let sources = BTreeMap::from([
            (
                "src/phases.ts",
                "export enum Phase { Start = 'start', Stop = 'stop', Idle = 'idle' }\nexport enum Kept { Only = 1 }\n",
            ),
            ("src/mode.ts", "export enum Mode { On, Off }\n"),
            (
                "src/tools.ts",
                "export namespace Tools {\nexport const run = 1;\nexport const idle = 2;\nexport const spare = 3;\n}\n",
            ),
            (
                "src/index.ts",
                "export * from './phases';\nexport { Tools as T } from './tools';\n",
            ),
            (
                ".github/scripts/preview.ts",
                "import { Phase } from '../../src/phases';\nconsole.log(Phase.Start);\n",
            ),
            (
                ".github/scripts/all.ts",
                "import * as api from '../../src/index';\nconsole.log(api.Phase.Stop);\n",
            ),
            (
                ".github/scripts/values.ts",
                "import { Mode } from '../../src/mode';\nconsole.log(Object.values(Mode));\n",
            ),
            (
                ".devcontainer/check.ts",
                "import { Tools } from '../src/tools';\nconsole.log(Tools.run);\n",
            ),
            (
                ".devcontainer/alias.ts",
                "import { T } from '../src/index';\nconsole.log(T.idle);\n",
            ),
        ]);
        let is_file = |file: &str| sources.contains_key(file);
        let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
        let layout = Layout::new(BTreeSet::new(), &manifest, &is_file);
        let resolver = Resolver::new(&layout, &read);
        let files: Vec<String> = sources.keys().map(|file| (*file).to_owned()).collect();
        let members = strings(&[
            "src/phases.ts|Start",
            "src/phases.ts|Stop",
            "src/phases.ts|Idle",
            "src/phases.ts|Only",
            "src/mode.ts|On",
            "src/mode.ts|Off",
            "src/tools.ts|run",
            "src/tools.ts|idle",
            "src/tools.ts|spare",
        ]);
        let (non_member, skipped) = outside_members(&members, &files, &layout, &resolver, &read);
        assert_eq!(
            non_member,
            strings(&[
                "src/mode.ts|Off",
                "src/mode.ts|On",
                "src/phases.ts|Start",
                "src/phases.ts|Stop",
            ])
        );
        assert_eq!(skipped, strings(&["src/tools.ts|idle", "src/tools.ts|run"]));
        let classifier = Classifier {
            non_member_members: &non_member,
            skipped_members: &skipped,
            ..bare(&layout)
        };
        for (side, category, key, expected) in [
            (
                Side::Skif,
                "enumMembers",
                "src/phases.ts|Start",
                Some(NON_MEMBER_PROJECT),
            ),
            (
                Side::Skif,
                "enumMembers",
                "src/phases.ts|Stop",
                Some(NON_MEMBER_PROJECT),
            ),
            (Side::Skif, "enumMembers", "src/mode.ts|On", Some(NON_MEMBER_PROJECT)),
            (Side::Skif, "enumMembers", "src/phases.ts|Idle", None),
            (Side::Skif, "enumMembers", "src/phases.ts|Only", None),
            (
                Side::Skif,
                "namespaceMembers",
                "src/tools.ts|run",
                Some(SKIPPED_DIRECTORY),
            ),
            (
                Side::Skif,
                "namespaceMembers",
                "src/tools.ts|idle",
                Some(SKIPPED_DIRECTORY),
            ),
            (Side::Skif, "namespaceMembers", "src/tools.ts|spare", None),
            (Side::Skif, "exports", "src/tools.ts|run", None),
            (Side::Tool, "enumMembers", "src/phases.ts|Start", None),
        ] {
            assert_eq!(
                classifier.classify(side, category, key),
                expected,
                "{side:?} {category} {key}"
            );
        }
    }

    #[test]
    fn install_dependent_findings_of_a_project_without_installed_packages_split_off_on_both_sides() {
        let skif = Keys::from([("unlisted".to_owned(), strings(&["package.json|ultracite"]))]);
        let knip = Keys::from([
            ("binaries".to_owned(), strings(&["package.json|husky"])),
            (
                "devDependencies".to_owned(),
                strings(&["package.json|oxfmt", "examples/basic/package.json|turbo"]),
            ),
            ("exports".to_owned(), strings(&["src/a.ts|x"])),
        ]);
        let manifest = manifests_of(&[
            ("", json!({ "name": "root" })),
            ("examples/basic", json!({ "name": "basic" })),
        ]);
        let every_file = |_: &str| true;
        let layout = Layout::new(BTreeSet::new(), &manifest, &every_file);
        let is_project = |dir: &str| layout.is_project(dir);
        let has_packages = |dir: &str| dir == "examples/basic";
        let uninstalled = uninstalled_projects((&skif, &knip), &is_project, &has_packages);
        assert_eq!(uninstalled, strings(&[""]));
        let installed_root = |dir: &str| dir.is_empty();
        assert!(uninstalled_projects((&skif, &knip), &is_project, &installed_root).is_empty());
        let classifier = Classifier {
            uninstalled_projects: &uninstalled,
            ..bare(&layout)
        };
        for (side, category, key, expected) in [
            (Side::Tool, "binaries", "package.json|husky", Some(UNINSTALLED_PROJECT)),
            (
                Side::Tool,
                "devDependencies",
                "package.json|oxfmt",
                Some(UNINSTALLED_PROJECT),
            ),
            (
                Side::Skif,
                "unlisted",
                "package.json|ultracite",
                Some(UNINSTALLED_PROJECT),
            ),
            (Side::Tool, "devDependencies", "examples/basic/package.json|turbo", None),
            (Side::Tool, "exports", "src/a.ts|x", None),
        ] {
            assert_eq!(
                classifier.classify(side, category, key),
                expected,
                "{side:?} {category} {key}"
            );
        }
    }

    #[test]
    fn exports_only_knip_reports_that_a_used_export_s_signature_names_split_off() {
        let source = "export interface Props { a: string }\n\
                      export interface Hidden { a: string }\n\
                      export interface Entry { a: string }\n\
                      export interface Lonely { a: string }\n\
                      export function Widget(p: Props): void {}\n\
                      export function unused(h: Hidden): void {}\n\
                      export class Store { get(): Entry { return { a: '' }; } }\n\
                      export function open(): Store { return new Store(); }\n";
        let read = |file: &str| (file == "src/ops.ts").then(|| source.to_owned());
        let skif = Keys::from([
            (
                "exports".to_owned(),
                strings(&["src/ops.ts|unused", "src/ops.ts|Store"]),
            ),
            ("types".to_owned(), strings(&["src/ops.ts|Lonely"])),
        ]);
        let knip = Keys::from([
            (
                "types".to_owned(),
                strings(&[
                    "src/ops.ts|Props",
                    "src/ops.ts|Hidden",
                    "src/ops.ts|Entry",
                    "src/ops.ts|Lonely",
                    "src/gone.ts|Props",
                ]),
            ),
            ("exports".to_owned(), strings(&["src/ops.ts|unused"])),
        ]);
        let found = signature_types(&skif, &knip, &read);
        assert_eq!(
            found,
            strings(&["src/ops.ts|Entry", "src/ops.ts|Props"]),
            "the signature of an export skif reports does not count, one of an export in a used signature does, and \
             a file that cannot be read yields nothing"
        );

        let manifest = |dir: &str| dir.is_empty().then(|| Rc::new(json!({ "name": "root" })));
        let every_file = |_: &str| true;
        let layout = Layout::new(BTreeSet::new(), &manifest, &every_file);
        let classifier = Classifier {
            signature_types: &found,
            ..bare(&layout)
        };
        for category in ["types", "nsTypes", "exports", "nsExports"] {
            assert_eq!(
                classifier.classify(Side::Tool, category, "src/ops.ts|Props"),
                Some(SIGNATURE_TYPE),
                "{category}"
            );
        }
        for (side, category, key) in [
            (Side::Tool, "types", "src/ops.ts|Hidden"),
            (Side::Tool, "enumMembers", "src/ops.ts|Props"),
            (Side::Skif, "types", "src/ops.ts|Props"),
        ] {
            assert_eq!(
                classifier.classify(side, category, key),
                None,
                "{side:?} {category} {key}"
            );
        }
    }

    #[test]
    fn starlark_strings_and_linked_dependency_labels_read_as_skif_reads_them() {
        assert_eq!(
            starlark_strings("x = ['a', \"b\"] # \"comment\"\ny = \"\"\"c \"quoted\" d\"\"\" + 'e\\'f' + \"\""),
            ["a", "b", "c \"quoted\" d", "e'f", ""]
        );
        for (label, found) in [
            (":node_modules/esbuild", Some(("packages/app", "esbuild"))),
            ("node_modules/esbuild", Some(("packages/app", "esbuild"))),
            (":node_modules/@types/node", Some(("packages/app", "@types/node"))),
            (":node_modules/typescript/bin/tsc", Some(("packages/app", "typescript"))),
            ("//:node_modules/typescript", Some(("", "typescript"))),
            ("//packages/lib:node_modules/zod", Some(("packages/lib", "zod"))),
            ("@//packages/lib:node_modules/zod", Some(("packages/lib", "zod"))),
            ("@@//packages/lib:node_modules/zod", Some(("packages/lib", "zod"))),
            (
                "@npm//packages/app:esbuild/package_json.bzl",
                Some(("packages/app", "esbuild")),
            ),
            ("@npm//:@babel/cli/package_json.bzl", Some(("", "@babel/cli"))),
            ("@npm//packages/app:node_modules/esbuild", None),
            ("//packages/app:esbuild/package_json.bzl", None),
            ("@npm//:defs.bzl", None),
            ("@npm//:esbuild/dir/package_json.bzl", None),
            (":node_modules", None),
            (":node_modules/@scope", None),
            ("//packages/app/node_modules/esbuild", None),
            ("src/index.ts", None),
            ("@types/node", None),
        ] {
            let expected = found.map(|(package, name)| (package.to_owned(), name.to_owned()));
            assert_eq!(linked_dependency(label, "packages/app"), expected, "{label}");
        }
    }

    #[test]
    fn dependencies_bazel_build_files_credit_and_unlisted_findings_at_them_split_off() {
        let rules_js = "bazel_dep(name = \"aspect_rules_js\", version = \"2.0.0\")\n";
        let root_files: BTreeMap<&str, &str> = [
            ("MODULE.bazel", rules_js),
            ("e2e/js/MODULE.bazel", rules_js),
            ("e2e/plain/MODULE.bazel", "module(name = \"plain\")\n"),
        ]
        .into_iter()
        .collect();
        let manifests = strings(&[
            "package.json",
            "packages/app/package.json",
            "e2e/js/package.json",
            "e2e/plain/package.json",
        ]);
        let is_file = |file: &str| root_files.contains_key(file) || manifests.contains(file);
        let read = |file: &str| root_files.get(file).map(|source| (*source).to_owned());
        let build_files: Vec<(String, String)> = [
            (
                "packages/app/BUILD.bazel",
                "load(\"@npm//packages/app:esbuild/package_json.bzl\", esbuild = \"bin\")\n\
                 # \":node_modules/commented\" is no label\n\
                 js_library(name = \"lib\", srcs = [\"src/index.ts\"], deps = [\":node_modules/zod\", \
                 \"//:node_modules/typescript\", '//packages/other:node_modules/left-pad'])\n",
            ),
            (
                "packages/app/src/BUILD",
                "deps = [\"//packages/app:node_modules/@types/node\"]\n",
            ),
            (
                "tools/BUILD.bazel",
                "deps = [\"//packages/app:node_modules/react\", \"//:node_modules/prettier\"]\n",
            ),
            ("e2e/js/BUILD.bazel", "deps = [\":node_modules/uvu\"]\n"),
            ("e2e/plain/BUILD.bazel", "deps = [\":node_modules/jasmine\"]\n"),
        ]
        .into_iter()
        .map(|(path, source)| (path.to_owned(), source.to_owned()))
        .collect();
        let credits = bazel_credits(&build_files, &is_file, &read);
        assert_eq!(
            credits,
            strings(&[
                "e2e/js/package.json|uvu",
                "package.json|prettier",
                "package.json|typescript",
                "packages/app/package.json|@types/node",
                "packages/app/package.json|esbuild",
                "packages/app/package.json|zod",
            ]),
            "a label of another package's directory, a package without a manifest, a comment and a Bazel root \
             without rules_js credit nothing"
        );

        let manifest = |dir: &str| {
            manifests
                .contains(&joined(dir, "package.json"))
                .then(|| Rc::new(json!({ "name": dir })))
        };
        let layout = Layout::new(strings(&["packages/app"]), &manifest, &is_file);
        let classifier = Classifier {
            bazel_credits: &credits,
            ..bare(&layout)
        };
        for (side, category, key) in [
            (Side::Tool, "devDependencies", "packages/app/package.json|zod"),
            (Side::Tool, "dependencies", "package.json|typescript"),
            (
                Side::Tool,
                "optionalPeerDependencies",
                "packages/app/package.json|esbuild",
            ),
            (Side::Skif, "unlisted", "packages/app/BUILD.bazel|lodash"),
            (Side::Skif, "unlisted", "packages/app/src/BUILD|lodash"),
        ] {
            assert_eq!(
                classifier.classify(side, category, key),
                Some(BAZEL_BUILD_FILE),
                "{side:?} {category} {key}"
            );
        }
        for (side, category, key) in [
            (Side::Tool, "dependencies", "packages/app/package.json|react"),
            (Side::Skif, "dependencies", "packages/app/package.json|zod"),
            (Side::Tool, "unlisted", "packages/app/BUILD.bazel|lodash"),
            (Side::Skif, "unlisted", "packages/app/src/index.ts|lodash"),
            (Side::Skif, "unlisted", "packages/app/BUILD.bazel.bak|lodash"),
        ] {
            assert_eq!(
                classifier.classify(side, category, key),
                None,
                "{side:?} {category} {key}"
            );
        }
    }

    #[test]
    fn the_peers_of_a_dependency_a_build_file_credits_are_credited_too() {
        let manifests: BTreeMap<&str, Value> = [
            (
                "",
                json!({ "devDependencies": {
                    "@angular/build": "21", "@angular/localize": "21", "@angular/compiler-cli": "21",
                    "@types/foo": "1", "left-pad": "1"
                } }),
            ),
            (
                "node_modules/@angular/build",
                json!({
                    "peerDependencies": { "@angular/localize": "21", "@angular/core": "21", "foo": "1" },
                    "peerDependenciesMeta": { "@angular/localize": { "optional": true } }
                }),
            ),
            (
                "node_modules/@angular/localize",
                json!({ "peerDependencies": { "@angular/compiler-cli": "21" } }),
            ),
            ("node_modules/host-x", json!({ "peerDependencies": { "peer-y": "1" } })),
            (
                "modules",
                json!({ "dependencies": { "@angular/localize": "21", "@angular/compiler-cli": "21" } }),
            ),
            (
                "modules/node_modules/@angular/localize",
                json!({ "peerDependencies": { "@angular/compiler-cli": "21" } }),
            ),
            (
                "packages/b",
                json!({
                    "dependencies": { "@angular/build": "21" },
                    "peerDependencies": { "@angular/localize": "21" }
                }),
            ),
            (
                "packages/a",
                json!({ "dependencies": { "host-x": "1", "peer-y": "1" } }),
            ),
        ]
        .into_iter()
        .collect();
        let manifest = |dir: &str| manifests.get(dir).cloned().map(Rc::new);
        let credits = strings(&[
            "package.json|@angular/build",
            "modules/package.json|@angular/localize",
            "packages/b/package.json|@angular/build",
        ]);
        assert_eq!(
            with_peer_credits(credits, &manifest),
            strings(&[
                "modules/package.json|@angular/compiler-cli",
                "modules/package.json|@angular/localize",
                "package.json|@angular/build",
                "package.json|@angular/compiler-cli",
                "package.json|@angular/localize",
                "package.json|@types/foo",
                "packages/b/package.json|@angular/build",
                "packages/b/package.json|@angular/localize",
            ]),
            "an optional peer, the peer of a credited peer, an `@types/` twin of a peer and a peer installed above the \
             package are credited; a peer the manifest does not list and the peers of an uncredited host are not"
        );
    }

    #[test]
    fn the_harness_reads_the_build_files_of_a_bazel_checkout_where_skif_discovery_looks() {
        let root = std::env::temp_dir().join(format!("skif-bench-parity-bazel-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for dir in [
            "packages/a/src/coverage",
            "packages/a/coverage",
            "node_modules/x",
            ".cache",
            "docs/build",
            "coverage",
            "tools/coverage/bundle",
            "tools/out",
        ] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        for file in [
            "BUILD.bazel",
            "packages/a/package.json",
            "packages/a/BUILD",
            "packages/a/src/BUILD.bazel",
            "packages/a/src/coverage/BUILD",
            "packages/a/coverage/BUILD.bazel",
            "node_modules/x/BUILD.bazel",
            ".cache/BUILD",
            "docs/build/notes.md",
            "coverage/BUILD.bazel",
            "tools/coverage/bundle/BUILD.bazel",
            "tools/out/BUILD",
        ] {
            fs::write(root.join(file), format!("# {file}\n")).unwrap();
        }
        let is_package = |dir: &str| root.join(dir).join("package.json").is_file();
        assert!(read_build_files(&root, &is_package).is_empty(), "no Bazel root file");
        fs::write(root.join("MODULE.bazel"), "bazel_dep(name = \"aspect_rules_js\")\n").unwrap();
        let found = read_build_files(&root, &is_package);
        assert_eq!(
            found.iter().map(|(path, _)| path.as_str()).collect::<Vec<_>>(),
            [
                "BUILD.bazel",
                "packages/a/BUILD",
                "packages/a/src/BUILD.bazel",
                "packages/a/src/coverage/BUILD",
                "tools/coverage/bundle/BUILD.bazel",
                "tools/out/BUILD",
            ],
            "`coverage`, `out` and `target` are skipped only directly in the root or a package directory"
        );
        assert_eq!(found[1].1, "# packages/a/BUILD\n");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn knip_only_findings_in_the_files_of_a_rush_root_split_off() {
        let read: BTreeMap<String, Rc<Value>> = [
            ("apps/heft", json!({ "name": "@rushstack/heft" })),
            (
                "common/autoinstallers/lint",
                json!({ "name": "lint", "dependencies": { "eslint": "9" } }),
            ),
        ]
        .into_iter()
        .map(|(dir, manifest)| (dir.to_owned(), Rc::new(manifest)))
        .collect();
        let manifest = move |dir: &str| read.get(dir).cloned();
        let every_file = |_: &str| true;
        let layout = Layout::new(strings(&["apps/heft"]), &manifest, &every_file);
        let classifier = |rush_root: bool| Classifier {
            rush_root,
            ..bare(&layout)
        };
        let rush = classifier(true);
        for (category, key) in [
            ("files", ".prettierrc.js"),
            ("files", "common/scripts/install-run-rush.js"),
            ("files", "common/config/subspaces/default/.pnpmfile.cjs"),
            ("unlisted", "common/scripts/install-run.js|semver"),
            ("exports", "common/scripts/install-run.js|run"),
        ] {
            assert_eq!(
                rush.classify(Side::Tool, category, key),
                Some(RUSH_ROOT_FILE),
                "{category} {key}"
            );
        }
        for (side, category, key) in [
            (Side::Tool, "files", "apps/heft/src/orphan.ts"),
            (Side::Tool, "unlisted", "apps/heft/src/a.ts|semver"),
            (Side::Skif, "files", ".prettierrc.js"),
        ] {
            assert_eq!(rush.classify(side, category, key), None, "{side:?} {category} {key}");
        }
        assert_eq!(
            rush.classify(Side::Tool, "files", "common/autoinstallers/lint/index.js"),
            Some(NON_MEMBER_PROJECT),
            "a project of its own below the root is not a root file"
        );
        assert_eq!(
            classifier(false).classify(Side::Tool, "files", ".prettierrc.js"),
            None,
            "only a Rush root"
        );
    }

    #[test]
    fn a_marker_only_manifest_does_not_own_the_findings_below_it() {
        let read: BTreeMap<String, Rc<Value>> = [
            ("", json!({ "name": "root" })),
            (
                "packages/a",
                json!({ "name": "a", "exports": { "./esm/*": { "default": "./dist/esm/*.js" } } }),
            ),
            ("packages/a/src/esm", json!({ "type": "module" })),
            ("packages/b", json!({ "name": "b" })),
            ("packages/b/src/esm", json!({ "type": "module" })),
            ("packages/c", json!({ "name": "c" })),
            ("packages/c/src/esm", json!({ "type": "module" })),
        ]
        .into_iter()
        .map(|(dir, manifest)| (dir.to_owned(), Rc::new(manifest)))
        .collect();
        let manifest = move |dir: &str| read.get(dir).cloned();
        let every_file = |_: &str| true;
        let layout = Layout::new(
            strings(&["packages/a", "packages/b", "packages/c"]),
            &manifest,
            &every_file,
        );
        let not_judged_dirs = BTreeMap::from([(
            "packages/a".to_owned(),
            NotJudgedScope::within(vec!["src/esm/*.stories.ts".to_owned()]),
        )]);
        let knip_failed = strings(&["packages/b"]);
        let classifier = Classifier {
            not_judged: &not_judged_dirs,
            knip_failed: &knip_failed,
            ..bare(&layout)
        };
        assert_eq!(
            classifier.classify(Side::Tool, "files", "packages/a/src/esm/button.stories.ts"),
            Some(NOT_JUDGED),
            "the narrowed scope is matched relative to the package, not to the marker's directory"
        );
        assert_eq!(
            classifier.classify(Side::Tool, "files", "packages/a/src/esm/index.ts"),
            Some(PUBLISHED_SOURCE),
            "the package publishes the source below the marker"
        );
        assert_eq!(
            classifier.classify(Side::Tool, "exports", "packages/b/src/esm/index.ts|unused"),
            Some(KNIP_CONFIG_ERROR),
            "the config error of the package covers the files below the marker"
        );
        assert_eq!(
            classifier.classify(Side::Tool, "files", "packages/a/src/other.ts"),
            None
        );
        assert_eq!(
            classifier.classify(Side::Tool, "files", "packages/c/src/esm/coverage/report.ts"),
            None,
            "a marker directory is no package root for `coverage`"
        );
    }

    #[test]
    fn earlier_deviations_take_precedence_over_the_layout_ones() {
        let manifests = [
            ("", json!({ "name": "root" })),
            ("examples/app", json!({ "name": "app" })),
        ];
        let read: BTreeMap<String, Rc<Value>> = manifests
            .iter()
            .map(|(dir, manifest)| ((*dir).to_owned(), Rc::new(manifest.clone())))
            .collect();
        let manifest = move |dir: &str| read.get(dir).cloned();
        let every_file = |_: &str| true;
        let layout = Layout::new(BTreeSet::new(), &manifest, &every_file);
        let unread = ["examples/app/docs/".to_owned()];
        let not_judged_dirs = BTreeMap::from([("examples/app".to_owned(), NotJudgedScope::Package)]);
        let classifier = Classifier {
            knip_unread: &unread,
            not_judged: &not_judged_dirs,
            ..bare(&layout)
        };
        assert_eq!(
            classifier.classify(Side::Tool, "files", "examples/app/docs/a.ts"),
            Some(KNIP_UNREAD)
        );
        assert_eq!(
            classifier.classify(Side::Tool, "files", "examples/app/test/a.ts"),
            Some(TEST_FILE)
        );
        assert_eq!(
            classifier.classify(Side::Tool, "files", "examples/app/src/a.ts"),
            Some(NOT_JUDGED),
            "skif names the package it did not judge"
        );
        assert_eq!(
            classifier.classify(Side::Tool, "files", "examples/app/.cache/a.ts"),
            Some(NOT_JUDGED)
        );
    }

    #[test]
    fn the_declaration_files_a_workspace_tsconfig_compiles_are_the_ones_knip_makes_entries() {
        let sources = BTreeMap::from([
            ("tsconfig.json", r#"{ "files": ["typings/listed.d.ts"] }"#),
            (
                "packages/types/tsconfig.json",
                r#"{ // inherits the selection
                    "extends": "./tsconfig.base.json", "exclude": ["legacy"] }"#,
            ),
            (
                "packages/types/tsconfig.base.json",
                r#"{ "include": ["."], "compilerOptions": { "outDir": "build" } }"#,
            ),
            (
                "packages/app/tsconfig.json",
                r#"{ "include": ["src", "../../shared/api.d.ts"] }"#,
            ),
            ("packages/plain/tsconfig.json", "{}"),
            ("tools/tsconfig.json", "{}"),
        ]);
        let manifest = manifests_of(&[
            ("", json!({ "name": "root", "workspaces": ["packages/*"] })),
            ("packages/types", json!({ "name": "@acme/types" })),
            ("packages/app", json!({ "name": "@acme/app" })),
            ("packages/bare", json!({ "name": "@acme/bare" })),
            ("packages/plain", json!({ "name": "@acme/plain" })),
        ]);
        let is_file = |file: &str| sources.contains_key(file);
        let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
        let layout = Layout::new(
            strings(&["packages/types", "packages/app", "packages/bare", "packages/plain"]),
            &manifest,
            &is_file,
        );
        let files: Vec<String> = [
            "typings/listed.d.ts",
            "typings/other.d.ts",
            "packages/types/Calendar.d.ts",
            "packages/types/nested/schedule.d.mts",
            "packages/types/utils.ts",
            "packages/types/legacy/old.d.ts",
            "packages/types/build/index.d.ts",
            "packages/types/.cache/cached.d.ts",
            "packages/app/src/env.d.ts",
            "packages/app/other.d.ts",
            "shared/api.d.ts",
            "packages/bare/bare.d.ts",
            "packages/plain/plain.d.ts",
            "tools/tool.d.ts",
        ]
        .iter()
        .map(|file| (*file).to_owned())
        .collect();
        assert_eq!(
            workspace_declarations(&files, &layout, &read),
            strings(&[
                "typings/listed.d.ts",
                "packages/types/Calendar.d.ts",
                "packages/types/nested/schedule.d.mts",
                "packages/app/src/env.d.ts",
                "shared/api.d.ts",
                "packages/plain/plain.d.ts",
            ]),
            "`files` alone selects no more; an inherited `include` stays relative to its configuration, less \
             `exclude`, the `outDir` and hidden directories; `include` may name files outside the workspace; no \
             `include` and no `files` select everything; a workspace without a `tsconfig.json` and a project that \
             is not a workspace select nothing"
        );
    }

    #[test]
    fn declaration_files_no_code_file_imports_are_told_apart_from_imported_ones() {
        let sources = BTreeMap::from([
            (
                "packages/types/Calendar.d.ts",
                "export interface Calendar { id: string }\n",
            ),
            (
                "packages/types/AppGetServerSideProps.d.ts",
                "import type { Session } from 'next-auth';\nexport type Props = Session;\n",
            ),
            ("src/api/git.d.ts", "export interface API { state: string }\n"),
            ("src/typings/index.d.ts", "export type Typing = 1;\n"),
            ("src/lone.d.ts", "import 'missing';\nexport {};\n"),
            ("src/button/index.ts", "export const Button = 1;\n"),
            (
                "src/button/index.d.ts",
                "import { Base } from '../component';\nexport declare class Button extends Base {}\n",
            ),
            (
                "apps/web/page.ts",
                "import type { Calendar } from '@acme/types/Calendar';\nexport const page: Calendar = { id: '' };\n",
            ),
            (
                "src/main.ts",
                "import type { API } from './api/git.js';\nimport type { Typing } from './typings';\n\
                 import { Button } from './button';\nexport const main = Button;\n",
            ),
            ("src/mentions.ts", "export const lone = 'lone';\n"),
        ]);
        let manifest = manifests_of(&[
            ("", json!({ "name": "root", "workspaces": ["packages/*", "apps/*"] })),
            ("packages/types", json!({ "name": "@acme/types" })),
            ("apps/web", json!({ "name": "web" })),
        ]);
        let is_file = |file: &str| sources.contains_key(file);
        let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
        let layout = Layout::new(strings(&["packages/types", "apps/web"]), &manifest, &is_file);
        let resolver = Resolver::new(&layout, &read);
        let files: Vec<String> = sources.keys().map(|file| (*file).to_owned()).collect();
        let declarations = strings(&[
            "packages/types/Calendar.d.ts",
            "packages/types/AppGetServerSideProps.d.ts",
            "src/api/git.d.ts",
            "src/typings/index.d.ts",
            "src/lone.d.ts",
            "src/button/index.d.ts",
        ]);
        assert_eq!(
            unimported_declarations(&declarations, &files, &resolver, &read),
            strings(&[
                "packages/types/AppGetServerSideProps.d.ts",
                "src/lone.d.ts",
                "src/button/index.d.ts"
            ]),
            "a workspace package subpath, a built extension and a directory's `index` import a declaration file; a \
             file that only names one does not, and a source beside it takes the import"
        );
        assert!(unimported_declarations(&BTreeSet::new(), &files, &resolver, &read).is_empty());
    }

    #[test]
    fn findings_that_knip_s_tsconfig_declaration_entries_explain_split_off() {
        let sources = BTreeMap::from([
            (
                "src/golden.d.ts",
                "import { SomeDirective } from './some.directive';\nexport declare class Golden {}\n",
            ),
            (
                "src/some.directive.ts",
                "import { base } from './base';\nexport class SomeDirective { b = base; }\n",
            ),
            ("src/base.ts", "export const base = 1;\n"),
            ("src/other.ts", "export const other = 1;\n"),
        ]);
        let is_file = |file: &str| sources.contains_key(file);
        let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
        let only: BTreeSet<&str> = ["src/some.directive.ts", "src/base.ts", "src/other.ts"]
            .into_iter()
            .collect();
        let reached = reach(vec!["src/golden.d.ts"], &only, &is_file, &read);
        assert_eq!(reached, strings(&["src/some.directive.ts", "src/base.ts"]));
        let manifest = manifests_of(&[
            ("", json!({ "name": "root", "workspaces": ["packages/*"] })),
            ("packages/types", json!({ "name": "@acme/types" })),
        ]);
        let layout = Layout::new(strings(&["packages/types"]), &manifest, &is_file);
        let declarations = strings(&[
            "packages/types/Calendar.d.ts",
            "packages/types/AppGetServerSideProps.d.ts",
            "src/golden.d.ts",
        ]);
        let unimported = strings(&["packages/types/AppGetServerSideProps.d.ts"]);
        let classifier = Classifier {
            tsconfig_declarations: &declarations,
            unimported_declarations: &unimported,
            declaration_reach: &reached,
            ..bare(&layout)
        };
        for (side, category, key, expected) in [
            (
                Side::Skif,
                "types",
                "packages/types/Calendar.d.ts|Calendar",
                Some(TSCONFIG_DECLARATION_FILE),
            ),
            (
                Side::Skif,
                "exports",
                "packages/types/Calendar.d.ts|ProviderResult",
                Some(TSCONFIG_DECLARATION_FILE),
            ),
            (
                Side::Skif,
                "enumMembers",
                "src/golden.d.ts|Auto",
                Some(TSCONFIG_DECLARATION_FILE),
            ),
            (
                Side::Skif,
                "files",
                "src/some.directive.ts",
                Some(TSCONFIG_DECLARATION_FILE),
            ),
            (Side::Skif, "files", "src/base.ts", Some(TSCONFIG_DECLARATION_FILE)),
            (
                Side::Tool,
                "unlisted",
                "packages/types/AppGetServerSideProps.d.ts|next-auth",
                Some(TSCONFIG_DECLARATION_FILE),
            ),
            (
                Side::Tool,
                "unresolved",
                "packages/types/AppGetServerSideProps.d.ts|./dist/index.js",
                Some(TSCONFIG_DECLARATION_FILE),
            ),
            (Side::Skif, "files", "src/other.ts", None),
            (Side::Skif, "types", "src/types.ts|Calendar", None),
            (Side::Skif, "duplicates", "packages/types/Calendar.d.ts|a|b", None),
            (
                Side::Skif,
                "unlisted",
                "packages/types/AppGetServerSideProps.d.ts|next-auth",
                None,
            ),
            (Side::Tool, "unlisted", "packages/types/Calendar.d.ts|next", None),
            (Side::Tool, "types", "packages/types/Calendar.d.ts|Calendar", None),
        ] {
            assert_eq!(
                classifier.classify(side, category, key),
                expected,
                "{side:?} {category} {key}"
            );
        }
    }

    #[test]
    fn types_of_a_module_an_import_uses_as_a_whole_object_split_off_on_skif_side() {
        let sources = BTreeMap::from([
            (
                "app-store/crm.apps.generated.ts",
                "export const CrmServiceMap = {\n  bigin: import('./zoho-bigin/lib/CrmService'),\n  \
                 video: import('./barrel'),\n};\n",
            ),
            (
                "app-store/zoho-bigin/lib/CrmService.ts",
                "export type BiginToken = { token: string };\nexport interface BiginContact { id: string }\n\
                 export default class BiginCrmService {}\n",
            ),
            ("app-store/barrel.ts", "export * from './video/VideoApiAdapter';\n"),
            (
                "app-store/video/VideoApiAdapter.ts",
                "export type DailyEventResult = { id: string };\nexport const adapter = 1;\n",
            ),
            (
                "app-store/static.ts",
                "import { helper } from './helpers';\nexport const used = helper;\n",
            ),
            (
                "app-store/helpers.ts",
                "export type HelperOptions = { on: boolean };\nexport const helper = 1;\n",
            ),
            ("app-store/lonely.ts", "export type LonelyType = string;\n"),
        ]);
        let is_file = |file: &str| sources.contains_key(file);
        let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
        let layout = Layout::new(BTreeSet::new(), &no_manifest, &is_file);
        let resolver = Resolver::new(&layout, &read);
        let files: Vec<String> = sources.keys().map(|file| (*file).to_owned()).collect();
        let types = strings(&[
            "app-store/zoho-bigin/lib/CrmService.ts|BiginContact",
            "app-store/zoho-bigin/lib/CrmService.ts|BiginToken",
            "app-store/video/VideoApiAdapter.ts|DailyEventResult",
            "app-store/helpers.ts|HelperOptions",
            "app-store/lonely.ts|LonelyType",
        ]);
        let credits = module_object_credits(&files, &resolver, &read);
        let found = module_objects(&types, &credits);
        assert_eq!(
            found,
            strings(&[
                "app-store/zoho-bigin/lib/CrmService.ts|BiginContact",
                "app-store/zoho-bigin/lib/CrmService.ts|BiginToken",
                "app-store/video/VideoApiAdapter.ts|DailyEventResult",
            ]),
            "an `import()` uses its module as a whole, also through `export *`; a named import and no import do not"
        );
        assert!(module_objects(&BTreeSet::new(), &credits).is_empty());
        let classifier = Classifier {
            module_object_types: &found,
            ..bare(&layout)
        };
        for (side, category, key, expected) in [
            (
                Side::Skif,
                "types",
                "app-store/zoho-bigin/lib/CrmService.ts|BiginContact",
                Some(MODULE_OBJECT_TYPES),
            ),
            (
                Side::Skif,
                "nsTypes",
                "app-store/zoho-bigin/lib/CrmService.ts|BiginToken",
                Some(MODULE_OBJECT_TYPES),
            ),
            (
                Side::Skif,
                "exports",
                "app-store/zoho-bigin/lib/CrmService.ts|BiginContact",
                None,
            ),
            (
                Side::Tool,
                "types",
                "app-store/zoho-bigin/lib/CrmService.ts|BiginContact",
                None,
            ),
            (Side::Skif, "types", "app-store/helpers.ts|HelperOptions", None),
        ] {
            assert_eq!(
                classifier.classify(side, category, key),
                expected,
                "{side:?} {category} {key}"
            );
        }
    }

    /// The manifests among `sources`, by directory: what `<dir>/package.json` holds, parsed.
    fn manifests_in(sources: &BTreeMap<&str, &str>) -> BTreeMap<String, Rc<Value>> {
        sources
            .iter()
            .filter_map(|(path, text)| {
                let dir = path.strip_suffix("package.json")?.trim_end_matches('/');
                Some((dir.to_owned(), Rc::new(serde_json::from_str(text).unwrap())))
            })
            .collect()
    }

    /// Asserts how `classifier` classifies each `(side, category, key)` of `cases`.
    fn assert_classified(classifier: &Classifier<'_>, cases: &[(Side, &str, &str, Option<&str>)]) {
        for (side, category, key, expected) in cases {
            assert_eq!(
                classifier.classify(*side, category, key),
                *expected,
                "{side:?} {category} {key}"
            );
        }
    }

    #[test]
    fn dependencies_heft_configs_and_rig_profiles_credit_split_off_on_knip_side() {
        let sources = BTreeMap::from([
            (
                "app/package.json",
                r#"{ "name": "app", "devDependencies": { "@rushstack/heft": "1", "@rushstack/heft-lint-plugin": "1", "local-rig": "1", "left-pad": "1" } }"#,
            ),
            (
                "app/config/heft.json",
                r#"{ "phasesByName": { "build": { "tasksByName": { "lint": { "taskPlugin": { "pluginPackage": "@rushstack/heft-lint-plugin" } } } } } }"#,
            ),
            ("app/config/rig.json", r#"{ "rigPackageName": "local-rig" }"#),
            (
                "rig/package.json",
                r#"{ "name": "local-rig", "dependencies": { "@rushstack/heft": "1", "@rushstack/heft-jest-plugin": "1", "jest-environment-node": "1", "unused-dep": "1" } }"#,
            ),
            (
                "rig/profiles/default/config/heft.json",
                r#"{ "phasesByName": { "test": { "tasksByName": { "jest": { "taskPlugin": { "pluginPackage": "@rushstack/heft-jest-plugin" } } } } } }"#,
            ),
            (
                "rig/node_modules/@rushstack/heft-jest-plugin/package.json",
                r#"{ "name": "@rushstack/heft-jest-plugin", "peerDependencies": { "jest-environment-node": "*" } }"#,
            ),
            (
                "plain/package.json",
                r#"{ "name": "plain", "dependencies": { "@rushstack/heft-lint-plugin": "1" } }"#,
            ),
            (
                "plain/config/heft.json",
                r#"{ "heftPlugins": [{ "pluginPackage": "@rushstack/heft-lint-plugin" }] }"#,
            ),
        ]);
        let mut memory = MemoryFs::new();
        for (path, source) in &sources {
            memory = memory.with_file(format!("/repo/{path}"), source);
        }
        let manifests = manifests_in(&sources);
        let manifest = |dir: &str| manifests.get(dir).cloned();
        let credits = heft_credits(
            ["app", "rig", "plain", "missing"],
            &memory,
            Path::new("/repo"),
            &manifest,
        );
        assert_eq!(
            credits,
            strings(&[
                "app/package.json|@rushstack/heft-lint-plugin",
                "app/package.json|local-rig",
                "rig/package.json|@rushstack/heft",
                "rig/package.json|@rushstack/heft-jest-plugin",
                "rig/package.json|jest-environment-node",
            ]),
            "a project's Heft configs, a rig's own profiles and the peers of what they load; a project that neither \
             depends on Heft nor has a `config/rig.json` enables no `heft` plugin"
        );
        for (path, expected) in [
            ("profiles/default/config/heft.json", true),
            ("node_modules/@acme/web-rig/profiles/app/tsconfig-base.json", true),
            ("node_modules/web-rig/profiles/app/tsconfig-base.json", true),
            ("config/heft.json", false),
            ("src/profiles/default/config.json", false),
            ("profiles/default", false),
            ("node_modules/@acme/web-rig/config/heft.json", false),
        ] {
            assert_eq!(in_rig_profile(path), expected, "{path}");
        }
        let is_file = |file: &str| sources.contains_key(file);
        let layout = Layout::new(strings(&["app", "rig", "plain"]), &manifest, &is_file);
        let classifier = Classifier {
            heft_credits: &credits,
            ..bare(&layout)
        };
        assert_classified(
            &classifier,
            &[
                (
                    Side::Tool,
                    "devDependencies",
                    "app/package.json|@rushstack/heft-lint-plugin",
                    Some(HEFT_CONFIG),
                ),
                (
                    Side::Tool,
                    "dependencies",
                    "rig/package.json|jest-environment-node",
                    Some(HEFT_CONFIG),
                ),
                (Side::Tool, "devDependencies", "app/package.json|left-pad", None),
                (Side::Tool, "dependencies", "rig/package.json|unused-dep", None),
                (
                    Side::Tool,
                    "dependencies",
                    "plain/package.json|@rushstack/heft-lint-plugin",
                    None,
                ),
                (
                    Side::Skif,
                    "devDependencies",
                    "app/package.json|@rushstack/heft-lint-plugin",
                    None,
                ),
            ],
        );
    }

    #[test]
    fn findings_knip_misses_for_files_a_nested_dot_slash_ignore_hides_split_off_on_knip_side() {
        let sources = BTreeMap::from([
            ("packages/core/.gitignore", "./memory\n"),
            (
                "packages/core/src/processors/memory/cache.ts",
                "import { LRUCache } from 'lru-cache';\nimport { topic } from '../../agent/topic';\n\
                 console.log(LRUCache, topic);\n",
            ),
            (
                "packages/core/src/agent/topic.ts",
                "export const topic = 1;\nexport const other = 2;\n",
            ),
            (
                "packages/core/src/index.ts",
                "import { z } from 'zod';\nconsole.log(z);\n",
            ),
        ]);
        let is_file = |file: &str| sources.contains_key(file);
        let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
        let layout = Layout::new(BTreeSet::new(), &no_manifest, &is_file);
        let resolver = Resolver::new(&layout, &read);
        let ignores = dot_slash_ignores(&[("packages/core/.gitignore".to_owned(), "./memory\n".to_owned())]);
        let hidden: BTreeSet<String> = sources
            .keys()
            .filter(|file| in_dot_slash_ignore(&ignores, file))
            .map(|file| (*file).to_owned())
            .collect();
        assert_eq!(hidden, strings(&["packages/core/src/processors/memory/cache.ts"]));
        let workspace_of = |_: &str| "packages/core".to_owned();
        let imports = unused_file_imports(&hidden, &BTreeSet::new(), &read, &workspace_of);
        let credits = unused_file_credits(&hidden, &BTreeSet::new(), &resolver, &read);
        let classifier = Classifier {
            knip_dot_ignore_imports: &imports,
            knip_dot_ignore_credits: &credits,
            ..bare(&layout)
        };
        assert_classified(
            &classifier,
            &[
                (
                    Side::Tool,
                    "dependencies",
                    "packages/core/package.json|lru-cache",
                    Some(KNIP_DOT_IGNORE),
                ),
                (
                    Side::Tool,
                    "exports",
                    "packages/core/src/agent/topic.ts|topic",
                    Some(KNIP_DOT_IGNORE),
                ),
                (Side::Tool, "dependencies", "packages/core/package.json|zod", None),
                (Side::Tool, "exports", "packages/core/src/agent/topic.ts|other", None),
            ],
        );
        assert_ne!(
            classifier.classify(Side::Skif, "dependencies", "packages/core/package.json|lru-cache"),
            Some(KNIP_DOT_IGNORE),
            "skif's own findings are not set apart"
        );
    }

    #[test]
    fn requires_a_typescript_as_asserts_a_type_of_are_read_from_the_text() {
        let text = "const a = require('./a') as typeof import('./a');\n\
                    const b = (require(\"./b\")\n  as typeof import('./b'));\n\
                    const c = require('./c');\n\
                    const d = myrequire('./d') as D;\n\
                    const e = require('./e') asserted;\n\
                    const f = require(name) as F;\n\
                    const g = require(\n  `./g`\n) as typeof import('./g');\n\
                    const h = require(`./${name}`) as H;\n";
        assert_eq!(type_asserted_requires(text), ["./a", "./b", "./g"]);
    }

    #[test]
    fn exports_of_a_module_a_type_asserted_require_loads_split_off_on_knip_side() {
        let sources = BTreeMap::from([
            (
                "src/stream-ops.ts",
                "let m = require('./stream-ops.node') as typeof import('./stream-ops.node');\nexport const go = m.go;\n",
            ),
            (
                "src/stream-ops.node.ts",
                "export const go = 1;\nexport * from './shared';\n",
            ),
            ("src/shared.ts", "export const helper = 1;\n"),
            (
                "src/plain.ts",
                "const p = require('./other');\nconsole.log(p.lonely);\n",
            ),
            ("src/other.ts", "export const lonely = 1;\n"),
        ]);
        let is_file = |file: &str| sources.contains_key(file);
        let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
        let layout = Layout::new(BTreeSet::new(), &no_manifest, &is_file);
        let resolver = Resolver::new(&layout, &read);
        let files: Vec<String> = sources.keys().map(|file| (*file).to_owned()).collect();
        let credits = type_asserted_require_credits(&files, &resolver, &read);
        assert_eq!(credits, strings(&["src/shared.ts|*", "src/stream-ops.node.ts|*"]));
        let classifier = Classifier {
            type_asserted_requires: &credits,
            ..bare(&layout)
        };
        assert_classified(
            &classifier,
            &[
                (
                    Side::Tool,
                    "exports",
                    "src/stream-ops.node.ts|go",
                    Some(TYPE_ASSERTED_REQUIRE),
                ),
                (
                    Side::Tool,
                    "nsExports",
                    "src/shared.ts|helper",
                    Some(TYPE_ASSERTED_REQUIRE),
                ),
                (Side::Tool, "exports", "src/other.ts|lonely", None),
                (Side::Tool, "types", "src/stream-ops.node.ts|go", None),
                (Side::Skif, "exports", "src/stream-ops.node.ts|go", None),
            ],
        );
    }

    #[test]
    fn files_a_template_literal_require_matches_and_what_they_reach_split_off_on_knip_side() {
        let sources = BTreeMap::from([
            (
                "lib/npm.js",
                "const load = (c) => require(`./commands/${c}`);\nmodule.exports = load;\n",
            ),
            (
                "lib/commands/access.js",
                "const base = require('../base-cmd.js');\nmodule.exports = base;\n",
            ),
            ("lib/commands/stage/index.js", "module.exports = 1;\n"),
            ("lib/commands/stage/list.js", "module.exports = 2;\n"),
            ("lib/base-cmd.js", "module.exports = 3;\n"),
            ("lib/meta.js", "const all = import.meta.glob(`./meta/*.js`);\n"),
            ("lib/meta/one.js", "module.exports = 4;\n"),
            ("lib/orphan.js", "module.exports = 5;\n"),
        ]);
        let is_file = |file: &str| sources.contains_key(file);
        let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
        let files: Vec<String> = sources.keys().map(|file| (*file).to_owned()).collect();
        let targets = template_import_targets(&files, &read);
        assert_eq!(
            targets,
            strings(&["lib/commands/access.js", "lib/commands/stage/index.js"]),
            "a substitution stands for one path segment, or a directory's `index`; an `import.meta.glob` is no \
             template literal call"
        );
        let knip_unused = strings(&[
            "lib/base-cmd.js",
            "lib/commands/access.js",
            "lib/commands/stage/index.js",
            "lib/commands/stage/list.js",
            "lib/meta/one.js",
            "lib/orphan.js",
        ]);
        let reached = reach_from_sources(&targets, (&knip_unused, &BTreeSet::new()), &is_file, &read);
        assert_eq!(
            reached,
            strings(&[
                "lib/base-cmd.js",
                "lib/commands/access.js",
                "lib/commands/stage/index.js"
            ])
        );
        let layout = Layout::new(BTreeSet::new(), &no_manifest, &is_file);
        let classifier = Classifier {
            template_import_reach: &reached,
            ..bare(&layout)
        };
        assert_classified(
            &classifier,
            &[
                (
                    Side::Tool,
                    "files",
                    "lib/commands/access.js",
                    Some(TEMPLATE_LITERAL_IMPORT),
                ),
                (Side::Tool, "files", "lib/base-cmd.js", Some(TEMPLATE_LITERAL_IMPORT)),
                (Side::Tool, "files", "lib/commands/stage/list.js", None),
                (Side::Tool, "files", "lib/orphan.js", None),
                (Side::Skif, "files", "lib/commands/access.js", None),
            ],
        );
    }

    #[test]
    fn exports_a_default_import_of_a_commonjs_module_reads_split_off_on_knip_side() {
        let sources = BTreeMap::from([
            (
                "scripts/routing.cjs",
                "function a() {}\nfunction b() {}\nfunction c() {}\nmodule.exports = { a, b, c };\n",
            ),
            (
                "scripts/routing.test.ts",
                "import routing from './routing.cjs';\nconst { a } = routing;\nrouting.b();\n",
            ),
            ("scripts/esm.ts", "export default {};\nexport const d = 1;\n"),
            (
                "scripts/esm.test.ts",
                "import esm from './esm';\nconst { d } = esm;\nconsole.log(d);\n",
            ),
        ]);
        let is_file = |file: &str| sources.contains_key(file);
        let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
        let layout = Layout::new(BTreeSet::new(), &no_manifest, &is_file);
        let resolver = Resolver::new(&layout, &read);
        let files: Vec<String> = sources.keys().map(|file| (*file).to_owned()).collect();
        let exports = strings(&[
            "scripts/esm.ts|d",
            "scripts/routing.cjs|a",
            "scripts/routing.cjs|b",
            "scripts/routing.cjs|c",
        ]);
        let found = commonjs_default_members(&exports, &files, &resolver, &read);
        assert_eq!(
            found,
            strings(&["scripts/routing.cjs|a", "scripts/routing.cjs|b"]),
            "the properties read through the default import of `module.exports`; an ES module's default is no \
             module object"
        );
        let classifier = Classifier {
            commonjs_default_members: &found,
            ..bare(&layout)
        };
        assert_classified(
            &classifier,
            &[
                (
                    Side::Tool,
                    "exports",
                    "scripts/routing.cjs|a",
                    Some(COMMONJS_DEFAULT_IMPORT),
                ),
                (
                    Side::Tool,
                    "nsExports",
                    "scripts/routing.cjs|b",
                    Some(COMMONJS_DEFAULT_IMPORT),
                ),
                (Side::Tool, "exports", "scripts/routing.cjs|c", None),
                (Side::Tool, "exports", "scripts/esm.ts|d", None),
                (Side::Skif, "exports", "scripts/routing.cjs|a", None),
            ],
        );
    }

    #[test]
    fn files_an_installed_main_imports_and_what_they_reach_split_off_on_knip_side() {
        let sources = BTreeMap::from([
            (
                "examples/rn/package.json",
                r#"{ "name": "rn", "main": "node_modules/expo/AppEntry.js" }"#,
            ),
            (
                "examples/rn/node_modules/expo/AppEntry.js",
                "import App from '../../App';\nregister(App);\n",
            ),
            (
                "examples/rn/App.tsx",
                "import { List } from './src/List';\nexport default function App() { return List; }\n",
            ),
            ("examples/rn/src/List.tsx", "export const List = 1;\n"),
            ("examples/rn/src/Orphan.tsx", "export const Orphan = 1;\n"),
            (
                "examples/web/package.json",
                r#"{ "name": "web", "main": "dist/index.js" }"#,
            ),
            ("examples/web/dist/index.js", "import '../src/page';\n"),
            ("examples/web/src/page.ts", "export const page = 1;\n"),
        ]);
        let manifests = manifests_in(&sources);
        let manifest = |dir: &str| manifests.get(dir).cloned();
        let is_file = |file: &str| sources.contains_key(file);
        let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
        let layout = Layout::new(strings(&["examples/rn", "examples/web"]), &manifest, &is_file);
        let imports = installed_main_imports(["examples/rn", "examples/web", "missing"], &layout, &read);
        assert_eq!(
            imports,
            strings(&["examples/rn/App.tsx"]),
            "a `main` outside `node_modules` is the project's own file"
        );
        let knip_unused = strings(&[
            "examples/rn/App.tsx",
            "examples/rn/src/List.tsx",
            "examples/rn/src/Orphan.tsx",
            "examples/web/src/page.ts",
        ]);
        let reached = reach_from_sources(&imports, (&knip_unused, &BTreeSet::new()), &is_file, &read);
        assert_eq!(reached, strings(&["examples/rn/App.tsx", "examples/rn/src/List.tsx"]));
        let classifier = Classifier {
            installed_main_reach: &reached,
            ..bare(&layout)
        };
        assert_classified(
            &classifier,
            &[
                (Side::Tool, "files", "examples/rn/src/List.tsx", Some(INSTALLED_MAIN)),
                (Side::Tool, "files", "examples/rn/src/Orphan.tsx", None),
                (Side::Tool, "files", "examples/web/src/page.ts", None),
                (Side::Skif, "files", "examples/rn/App.tsx", None),
            ],
        );
    }

    #[test]
    fn value_exports_of_a_module_used_as_a_whole_object_split_off_on_knip_side() {
        let sources = BTreeMap::from([
            (
                "nodes/methods/listSearch.ts",
                "export async function searchA() {}\nexport async function searchB() {}\n",
            ),
            (
                "nodes/Node.ts",
                "import * as listSearch from './methods/listSearch';\nexport const methods = { listSearch };\n",
            ),
            (
                "nodes/helpers.ts",
                "export const used = 1;\nexport const unusedHelper = 2;\n",
            ),
            (
                "nodes/user.ts",
                "import * as helpers from './helpers';\nconsole.log(helpers.used);\n",
            ),
        ]);
        let is_file = |file: &str| sources.contains_key(file);
        let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
        let layout = Layout::new(BTreeSet::new(), &no_manifest, &is_file);
        let resolver = Resolver::new(&layout, &read);
        let files: Vec<String> = sources.keys().map(|file| (*file).to_owned()).collect();
        let credits = module_object_credits(&files, &resolver, &read);
        let values = module_objects(
            &strings(&["nodes/helpers.ts|unusedHelper", "nodes/methods/listSearch.ts|searchA"]),
            &credits,
        );
        assert_eq!(values, strings(&["nodes/methods/listSearch.ts|searchA"]));
        let classifier = Classifier {
            module_object_values: &values,
            ..bare(&layout)
        };
        assert_classified(
            &classifier,
            &[
                (
                    Side::Tool,
                    "nsExports",
                    "nodes/methods/listSearch.ts|searchA",
                    Some(MODULE_OBJECT_VALUES),
                ),
                (Side::Tool, "exports", "nodes/helpers.ts|unusedHelper", None),
                (Side::Tool, "types", "nodes/methods/listSearch.ts|searchA", None),
                (Side::Skif, "nsExports", "nodes/methods/listSearch.ts|searchA", None),
            ],
        );
    }

    #[test]
    fn members_of_an_export_an_importer_refers_to_as_a_type_split_off_on_skif_side() {
        let sources = BTreeMap::from([
            (
                "common/charCode.ts",
                "export const enum CharCode { Hash = 35, Digit0 = 48 }\n",
            ),
            (
                "common/color.ts",
                "import { CharCode } from './charCode';\n\
                 export function digit(code: CharCode): number { return code === CharCode.Digit0 ? 0 : 1; }\n",
            ),
            (
                "common/result.ts",
                "export type CompareResult = number;\n\
                 export namespace CompareResult { export function isLess(r: CompareResult) { return r < 0; } }\n\
                 export enum Mode { A, B }\n",
            ),
            (
                "common/compare.ts",
                "import { CompareResult, Mode } from './result';\n\
                 export const g = (r: CompareResult) => Mode.A;\n",
            ),
            ("common/flags.ts", "export enum Flag { On, Off }\n"),
            (
                "common/use-flag.ts",
                "import { Flag as F } from './flags';\nexport const h = (f: F) => f;\n",
            ),
        ]);
        let is_file = |file: &str| sources.contains_key(file);
        let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
        let layout = Layout::new(BTreeSet::new(), &no_manifest, &is_file);
        let resolver = Resolver::new(&layout, &read);
        let files: Vec<String> = sources.keys().map(|file| (*file).to_owned()).collect();
        assert_eq!(
            type_reference_names(
                "common/use-flag.ts",
                "type T = typeof Q;\nlet x: Array<P> = [];\nlet y = z as R.S;\n"
            ),
            strings(&["Array", "P", "Q"])
        );
        let members = strings(&[
            "common/charCode.ts|Hash",
            "common/flags.ts|Off",
            "common/result.ts|B",
            "common/result.ts|isLess",
        ]);
        let found = type_referenced_members(&members, &files, &resolver, &read);
        assert_eq!(
            found,
            strings(&["common/charCode.ts|Hash", "common/result.ts|isLess"]),
            "an enum used only as a value and one imported under an alias stay compared"
        );
        let classifier = Classifier {
            type_referenced_members: &found,
            ..bare(&layout)
        };
        assert_classified(
            &classifier,
            &[
                (
                    Side::Skif,
                    "enumMembers",
                    "common/charCode.ts|Hash",
                    Some(TYPE_REFERENCED_MEMBERS),
                ),
                (
                    Side::Skif,
                    "namespaceMembers",
                    "common/result.ts|isLess",
                    Some(TYPE_REFERENCED_MEMBERS),
                ),
                (Side::Skif, "enumMembers", "common/result.ts|B", None),
                (Side::Skif, "enumMembers", "common/flags.ts|Off", None),
                (Side::Tool, "enumMembers", "common/charCode.ts|Hash", None),
            ],
        );
    }

    #[test]
    fn members_a_file_only_knip_lists_unused_reads_split_off_on_knip_side() {
        let sources = BTreeMap::from([
            (
                "nodes/Brevo/GenericFunctions.ts",
                "export namespace BrevoNode {\n  export const INTERCEPTORS = new Map();\n  export const OTHER = 1;\n}\n",
            ),
            (
                "nodes/Brevo/AttributeDescription.ts",
                "import { BrevoNode } from './GenericFunctions';\nconsole.log(BrevoNode.INTERCEPTORS);\n",
            ),
        ]);
        let is_file = |file: &str| sources.contains_key(file);
        let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
        let layout = Layout::new(BTreeSet::new(), &no_manifest, &is_file);
        let resolver = Resolver::new(&layout, &read);
        let members = strings(&[
            "nodes/Brevo/GenericFunctions.ts|INTERCEPTORS",
            "nodes/Brevo/GenericFunctions.ts|OTHER",
        ]);
        let used = members_used_by(&members, ["nodes/Brevo/AttributeDescription.ts"], &resolver, &read);
        assert_eq!(used, strings(&["nodes/Brevo/GenericFunctions.ts|INTERCEPTORS"]));
        let classifier = Classifier {
            knip_unused_members: &used,
            ..bare(&layout)
        };
        assert_classified(
            &classifier,
            &[
                (
                    Side::Tool,
                    "namespaceMembers",
                    "nodes/Brevo/GenericFunctions.ts|INTERCEPTORS",
                    Some(IMPORTED_BY_KNIP_UNUSED_FILE),
                ),
                (
                    Side::Tool,
                    "namespaceMembers",
                    "nodes/Brevo/GenericFunctions.ts|OTHER",
                    None,
                ),
                (
                    Side::Skif,
                    "namespaceMembers",
                    "nodes/Brevo/GenericFunctions.ts|INTERCEPTORS",
                    None,
                ),
            ],
        );
    }

    #[test]
    fn peers_of_a_package_a_file_only_knip_lists_unused_imports_are_used_in_that_file() {
        let sources = BTreeMap::from([
            (
                "app/package.json",
                r#"{ "name": "app", "dependencies": { "@react-navigation/stack": "1", "react-native-screens": "1", "left-pad": "1" } }"#,
            ),
            (
                "app/node_modules/@react-navigation/stack/package.json",
                r#"{ "name": "@react-navigation/stack", "peerDependencies": { "react-native-screens": "*" } }"#,
            ),
            (
                "app/App.tsx",
                "import { createStackNavigator } from '@react-navigation/stack';\nconsole.log(createStackNavigator);\n",
            ),
        ]);
        let manifests = manifests_in(&sources);
        let manifest = |dir: &str| manifests.get(dir).cloned();
        let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
        let workspace_of = |_: &str| "app".to_owned();
        let imports = with_peer_credits(
            unused_file_imports(&strings(&["app/App.tsx"]), &BTreeSet::new(), &read, &workspace_of),
            &manifest,
        );
        assert_eq!(
            imports,
            strings(&[
                "app/package.json|@react-navigation/stack",
                "app/package.json|react-native-screens",
            ])
        );
        let is_file = |file: &str| sources.contains_key(file);
        let layout = Layout::new(strings(&["app"]), &manifest, &is_file);
        let classifier = Classifier {
            knip_unused_imports: &imports,
            ..bare(&layout)
        };
        assert_classified(
            &classifier,
            &[
                (
                    Side::Tool,
                    "dependencies",
                    "app/package.json|react-native-screens",
                    Some(USED_IN_KNIP_UNUSED_FILE),
                ),
                (Side::Tool, "dependencies", "app/package.json|left-pad", None),
            ],
        );
    }

    #[test]
    fn findings_in_paths_knip_takes_for_node_modules_split_off_on_skif_side() {
        for (path, expected) in [
            ("js/private/test/node_modules_in_sandbox.test.mjs", true),
            ("src/my_node_modules/index.js", true),
            ("app/node_modules/pkg/node_modules_x.js", false),
            ("src/modules.js", false),
        ] {
            assert_eq!(named_like_node_modules(path), expected, "{path}");
        }
        with_classifier(&[], &[], &[], |classifier| {
            assert_classified(
                classifier,
                &[
                    (
                        Side::Skif,
                        "files",
                        "js/private/test/node_modules_in_sandbox.test.mjs",
                        Some(KNIP_NODE_MODULES_NAME),
                    ),
                    (
                        Side::Skif,
                        "exports",
                        "src/my_node_modules/index.js|x",
                        Some(KNIP_NODE_MODULES_NAME),
                    ),
                    (Side::Skif, "files", "src/modules.js", None),
                    (Side::Skif, "files", "app/node_modules/pkg/node_modules_x.js", None),
                ],
            );
            assert_ne!(
                classifier.classify(Side::Tool, "files", "js/private/test/node_modules_in_sandbox.test.mjs"),
                Some(KNIP_NODE_MODULES_NAME),
                "knip's own findings are not set apart"
            );
        });
    }

    #[test]
    fn files_an_import_meta_glob_reads_as_text_split_off_on_skif_side() {
        let sources = BTreeMap::from([
            (
                "src/app/views.ts",
                "const raw = import.meta.glob('../components/*.vue', { query: '?raw' });\n\
                 const modules = import.meta.glob('./*.vue');\nconsole.log(raw, modules);\n",
            ),
            ("src/components/Button.vue", "<template><div /></template>\n"),
            ("src/app/Local.vue", "<template><span /></template>\n"),
            ("src/other/Lonely.vue", "<template><p /></template>\n"),
        ]);
        let is_file = |file: &str| sources.contains_key(file);
        let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
        let files: Vec<String> = sources.keys().map(|file| (*file).to_owned()).collect();
        let targets = raw_glob_targets(&files, &read);
        assert_eq!(
            targets,
            strings(&["src/components/Button.vue"]),
            "a glob loading modules reads no text"
        );
        let skif_unused = strings(&["src/components/Button.vue", "src/other/Lonely.vue"]);
        let reached = reach_from_sources(&targets, (&skif_unused, &BTreeSet::new()), &is_file, &read);
        let layout = Layout::new(BTreeSet::new(), &no_manifest, &is_file);
        let classifier = Classifier {
            raw_glob_reach: &reached,
            ..bare(&layout)
        };
        assert_classified(
            &classifier,
            &[
                (Side::Skif, "files", "src/components/Button.vue", Some(RAW_GLOB_READ)),
                (Side::Skif, "files", "src/other/Lonely.vue", None),
                (Side::Tool, "files", "src/components/Button.vue", None),
            ],
        );
    }

    #[test]
    fn javascript_files_named_beside_a_typescript_twin_split_off_on_skif_side() {
        let sources = BTreeMap::from([
            ("ex/sum.ts", "export default 1;\n"),
            ("ex/sum.js", "module.exports = 1;\n"),
            (
                "ex/__tests__/sum.test.js",
                "const a = require('../sum.ts');\nconst b = require('../sum.js');\nconst c = require('../lone.js');\n",
            ),
            ("ex/other.ts", "export default 2;\n"),
            ("ex/other.js", "module.exports = 2;\n"),
            ("ex/user.js", "const o = require('./other');\n"),
            ("ex/lone.js", "module.exports = 3;\n"),
        ]);
        let is_file = |file: &str| sources.contains_key(file);
        let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
        let files: Vec<String> = sources.keys().map(|file| (*file).to_owned()).collect();
        let unused: BTreeSet<&str> = ["ex/sum.js", "ex/other.js", "ex/lone.js"].into_iter().collect();
        let twins = typescript_twin_files(&unused, &files, &is_file, &read);
        assert_eq!(
            twins,
            strings(&["ex/sum.js"]),
            "a twin no specifier names with its extension, and a file without a twin, stay compared"
        );
        let layout = Layout::new(BTreeSet::new(), &no_manifest, &is_file);
        let classifier = Classifier {
            typescript_twins: &twins,
            ..bare(&layout)
        };
        assert_classified(
            &classifier,
            &[
                (Side::Skif, "files", "ex/sum.js", Some(TYPESCRIPT_TWIN)),
                (Side::Skif, "files", "ex/other.js", None),
                (Side::Tool, "files", "ex/sum.js", None),
            ],
        );
    }

    /// The [`KnipReadSets`] that [`knip_read_credits`] reads from the code files of `sources`, resolved by relative
    /// path only.
    fn knip_read_sets(sources: &BTreeMap<&str, &str>) -> KnipReadSets {
        let is_file = |file: &str| sources.contains_key(file);
        let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
        let layout = Layout::new(BTreeSet::new(), &no_manifest, &is_file);
        let resolver = Resolver::new(&layout, &read);
        let files: Vec<String> = sources.keys().map(|file| (*file).to_owned()).collect();
        knip_read_credits(&files, &resolver, &read)
    }

    #[test]
    fn import_expressions_knip_binds_and_those_it_reads_as_opaque_are_told_apart() {
        let source = "import { vi } from 'vitest';\n\
                      import('./opaque');\n\
                      const { a } = await import('./destructured');\n\
                      const one = await import('./one');\n\
                      const notAwaited = import('./not-awaited');\n\
                      import('./then').then((m) => m.x);\n\
                      import('./then-object').then(({ x }) => x);\n\
                      const [p, { q }] = await Promise.all([import('./p'), import('./q')]);\n\
                      vi.mock(import('./mocked'), () => ({ x: 1 }));\n\
                      vi.mock(import('./spread'), () => ({ ...base }));\n\
                      lazy(() => import('./lazy'));\n\
                      import(name);\n";
        let module = parse_module(
            Path::new("src/load.ts"),
            source,
            &ParseOptions {
                in_file_references: false,
                member_accesses: true,
            },
        )
        .ok();
        let reads = knip_reads("src/load.ts", source, module.as_ref()).unwrap();
        assert_eq!(reads.opaque, ["./opaque", "./not-awaited", "./spread"]);
        assert_eq!(reads.defaults, ["./one", "./then", "./p"]);
    }

    #[test]
    fn exports_of_a_module_an_opaque_or_bound_import_expression_loads_split_off_on_skif_side() {
        let sources = BTreeMap::from([
            (
                "src/load.ts",
                "import('./opaque');\nconst one = await import('./one');\n",
            ),
            (
                "src/opaque.ts",
                "export const a = 1;\nexport type T = 1;\nexport * from './deep';\n",
            ),
            ("src/deep.ts", "export const deep = 1;\n"),
            ("src/one.ts", "export default 1;\nexport const other = 2;\n"),
            ("src/lonely.ts", "export const lonely = 1;\n"),
        ]);
        let reads = knip_read_sets(&sources);
        assert_eq!(reads.opaque_imports, strings(&["src/deep.ts|*", "src/opaque.ts|*"]));
        assert_eq!(reads.import_defaults, strings(&["src/one.ts|default"]));
        let is_file = |file: &str| sources.contains_key(file);
        let layout = Layout::new(BTreeSet::new(), &no_manifest, &is_file);
        let classifier = Classifier {
            knip_reads: &reads,
            ..bare(&layout)
        };
        assert_classified(
            &classifier,
            &[
                (Side::Skif, "exports", "src/opaque.ts|a", Some(OPAQUE_DYNAMIC_IMPORT)),
                (Side::Skif, "types", "src/opaque.ts|T", Some(OPAQUE_DYNAMIC_IMPORT)),
                (Side::Skif, "exports", "src/deep.ts|deep", Some(OPAQUE_DYNAMIC_IMPORT)),
                (
                    Side::Skif,
                    "exports",
                    "src/one.ts|default",
                    Some(DYNAMIC_IMPORT_DEFAULT),
                ),
                (Side::Skif, "exports", "src/one.ts|other", None),
                (Side::Skif, "exports", "src/lonely.ts|lonely", None),
                (Side::Skif, "files", "src/opaque.ts", None),
                (Side::Tool, "exports", "src/opaque.ts|a", None),
            ],
        );
    }

    #[test]
    fn exports_of_a_module_jest_require_actual_loads_split_off_on_skif_side() {
        let sources = BTreeMap::from([
            (
                "src/a.test.ts",
                "jest.mock('./utils', () => ({ ...jest.requireActual('./utils') }));\n",
            ),
            (
                "src/b.test.ts",
                "import { jest as j } from '@jest/globals';\nj.requireActual('./aliased');\n\
                 other.requireActual('./not-jest');\n",
            ),
            ("src/utils.ts", "export type Options = 1;\n"),
            ("src/aliased.ts", "export const aliased = 1;\n"),
            ("src/not-jest.ts", "export const notJest = 1;\n"),
        ]);
        let reads = knip_read_sets(&sources);
        assert_eq!(reads.required_actuals, strings(&["src/aliased.ts|*", "src/utils.ts|*"]));
        let is_file = |file: &str| sources.contains_key(file);
        let layout = Layout::new(BTreeSet::new(), &no_manifest, &is_file);
        let classifier = Classifier {
            knip_reads: &reads,
            ..bare(&layout)
        };
        assert_classified(
            &classifier,
            &[
                (Side::Skif, "types", "src/utils.ts|Options", Some(JEST_REQUIRE_ACTUAL)),
                (
                    Side::Skif,
                    "exports",
                    "src/aliased.ts|aliased",
                    Some(JEST_REQUIRE_ACTUAL),
                ),
                (Side::Skif, "exports", "src/not-jest.ts|notJest", None),
                (Side::Tool, "types", "src/utils.ts|Options", None),
            ],
        );
    }

    #[test]
    fn exports_of_a_module_whose_namespace_imports_only_spy_calls_read_split_off_on_skif_side() {
        let sources = BTreeMap::from([
            (
                "src/a.test.ts",
                "import * as mail from './mail';\nimport * as util from './util';\nimport * as both from './both';\n\
                 vi.spyOn(mail, 'send');\nvi.spyOn(util, 'go');\nutil.go();\nspyOn(both, 'x');\n",
            ),
            ("src/b.ts", "import * as again from './both';\nconsole.log(again.y);\n"),
            ("src/mail.ts", "export function send() {}\nexport type Mail = 1;\n"),
            ("src/util.ts", "export function go() {}\n"),
            ("src/both.ts", "export const x = 1;\nexport const y = 2;\n"),
        ]);
        let reads = knip_read_sets(&sources);
        assert_eq!(
            reads.spied_namespaces,
            strings(&["src/mail.ts|*"]),
            "a namespace also read through a member, here or in another file, is read as knip reads one"
        );
        let is_file = |file: &str| sources.contains_key(file);
        let layout = Layout::new(BTreeSet::new(), &no_manifest, &is_file);
        let classifier = Classifier {
            knip_reads: &reads,
            ..bare(&layout)
        };
        assert_classified(
            &classifier,
            &[
                (Side::Skif, "exports", "src/mail.ts|send", Some(SPIED_NAMESPACE)),
                (Side::Skif, "types", "src/mail.ts|Mail", Some(SPIED_NAMESPACE)),
                (Side::Skif, "exports", "src/util.ts|go", None),
                (Side::Skif, "exports", "src/both.ts|x", None),
                (Side::Tool, "exports", "src/mail.ts|send", None),
            ],
        );
    }

    #[test]
    fn exports_of_a_module_whose_namespace_name_an_array_or_object_keys_holds_split_off_on_skif_side() {
        let sources = BTreeMap::from([
            (
                "src/git.ts",
                "import * as path from './path';\nimport * as keys from './keys';\nimport * as plain from './plain';\n\
                 export function lsTree(path: string) { return ['ls-tree', path]; }\n\
                 export const names = Object.keys(keys);\nexport const a = plain.a;\n",
            ),
            (
                "src/path.ts",
                "export function format() {}\nexport function toNamespacedPath() {}\n",
            ),
            ("src/keys.ts", "export const k = 1;\n"),
            ("src/plain.ts", "export const a = 1;\nexport const b = 2;\n"),
        ]);
        let reads = knip_read_sets(&sources);
        assert_eq!(
            reads.enumerated_namespaces,
            strings(&["src/keys.ts|*", "src/path.ts|*"]),
            "a parameter that shadows the namespace still counts, as in knip"
        );
        let is_file = |file: &str| sources.contains_key(file);
        let layout = Layout::new(BTreeSet::new(), &no_manifest, &is_file);
        let classifier = Classifier {
            knip_reads: &reads,
            ..bare(&layout)
        };
        assert_classified(
            &classifier,
            &[
                (Side::Skif, "exports", "src/path.ts|format", Some(ENUMERATED_NAMESPACE)),
                (Side::Skif, "exports", "src/keys.ts|k", Some(ENUMERATED_NAMESPACE)),
                (Side::Skif, "exports", "src/plain.ts|b", None),
                (Side::Tool, "exports", "src/path.ts|format", None),
            ],
        );
    }

    #[test]
    fn types_a_qualified_import_type_names_split_off_on_knip_side() {
        let sources = BTreeMap::from([
            (
                "src/main.ts",
                "type W = import('./window').Window;\ntype N = import('./ns').A.B;\n\
                 type Q = typeof import('./whole');\n",
            ),
            (
                "src/window.ts",
                "export interface Window {}\nexport interface Other {}\n",
            ),
            ("src/ns.ts", "export namespace A { export type B = 1; }\n"),
            ("src/whole.ts", "export const whole = 1;\n"),
        ]);
        let reads = knip_read_sets(&sources);
        assert_eq!(reads.import_types, strings(&["src/ns.ts|A", "src/window.ts|Window"]));
        assert!(
            reads.opaque_imports.is_empty(),
            "an import type is no import expression"
        );
        let is_file = |file: &str| sources.contains_key(file);
        let layout = Layout::new(BTreeSet::new(), &no_manifest, &is_file);
        let classifier = Classifier {
            knip_reads: &reads,
            ..bare(&layout)
        };
        assert_classified(
            &classifier,
            &[
                (Side::Tool, "types", "src/window.ts|Window", Some(IMPORT_TYPE_QUALIFIER)),
                (Side::Tool, "nsTypes", "src/ns.ts|A", Some(IMPORT_TYPE_QUALIFIER)),
                (Side::Tool, "types", "src/window.ts|Other", None),
                (Side::Tool, "exports", "src/whole.ts|whole", None),
                (Side::Skif, "types", "src/window.ts|Window", None),
            ],
        );
    }

    #[test]
    fn namespace_re_exports_and_commonjs_exports_of_typescript_files_split_off_on_skif_side() {
        let sources = BTreeMap::from([
            (
                "src/index.ts",
                "export * as Config from './config';\nexport * as self from './index';\nexport const plain = 1;\n",
            ),
            ("src/join.ts", "exports.joinPath = join;\nmodule.exports.other = 1;\n"),
            ("src/obj.ts", "const a = 1;\nconst b = 2;\nmodule.exports = { a, b };\n"),
            ("src/mixed.ts", "const a = 1;\nmodule.exports = { a, b: 2 };\n"),
            ("lib/plain.js", "exports.js = 1;\n"),
        ]);
        let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
        let keys = strings(&[
            "src/index.ts|Config",
            "src/index.ts|self",
            "src/index.ts|plain",
            "src/join.ts|joinPath",
            "src/join.ts|other",
            "src/join.ts|missing",
            "src/obj.ts|a",
            "src/mixed.ts|a",
            "lib/plain.js|js",
        ]);
        let reads = KnipReadSets {
            namespace_reexports: namespace_reexport_keys(&keys, &read),
            typescript_commonjs: typescript_commonjs_keys(&keys, &read),
            ..KnipReadSets::default()
        };
        assert_eq!(
            reads.namespace_reexports,
            strings(&["src/index.ts|Config", "src/index.ts|self"])
        );
        assert_eq!(
            reads.typescript_commonjs,
            strings(&["src/join.ts|joinPath", "src/join.ts|other", "src/obj.ts|a"]),
            "a JavaScript file, and an object literal with a property that is not shorthand, are read as knip reads \
             them"
        );
        let is_file = |file: &str| sources.contains_key(file);
        let layout = Layout::new(BTreeSet::new(), &no_manifest, &is_file);
        let classifier = Classifier {
            knip_reads: &reads,
            ..bare(&layout)
        };
        assert_classified(
            &classifier,
            &[
                (Side::Skif, "nsExports", "src/index.ts|Config", Some(NAMESPACE_REEXPORT)),
                (Side::Skif, "exports", "src/index.ts|self", Some(NAMESPACE_REEXPORT)),
                (Side::Skif, "types", "src/index.ts|Config", None),
                (Side::Skif, "exports", "src/index.ts|plain", None),
                (
                    Side::Skif,
                    "exports",
                    "src/join.ts|joinPath",
                    Some(TYPESCRIPT_COMMONJS_EXPORT),
                ),
                (Side::Skif, "exports", "src/mixed.ts|a", None),
                (Side::Tool, "exports", "src/index.ts|Config", None),
            ],
        );
    }

    #[test]
    fn files_knip_s_node_plugin_matches_are_read_as_its_patterns_read_them() {
        for (path, expected) in [
            ("test/helpers.ts", true),
            ("src/test/setup.js", true),
            ("a.test.ts", true),
            ("a-test.mjs", true),
            ("a_test.cts", true),
            ("src/test-run.ts", true),
            ("test.js", true),
            ("src/a.spec.ts", false),
            ("testing/a.ts", false),
            ("src/contest.ts", false),
            ("src/.test.ts", false),
            ("a.test.tsx", false),
            ("test/data.json", false),
        ] {
            assert_eq!(is_node_test_file(path), expected, "{path}");
        }
        for (script, expected) in [
            ("node --test", true),
            ("NODE_ENV=test node --import tsx --test test/*.ts", true),
            ("pnpm build && nub --test", true),
            ("tsc; node --test", true),
            ("node test.js", false),
            ("vitest --test", false),
            ("echo node --test", false),
        ] {
            assert_eq!(
                runs_node_test(&json!({ "scripts": { "test": script } })),
                expected,
                "{script}"
            );
        }
        assert!(!runs_node_test(&json!({ "name": "no-scripts" })));
    }

    #[test]
    fn exports_of_files_knip_s_node_plugin_makes_entries_split_off_on_skif_side() {
        let keys = strings(&[
            "packages/a/test/helpers.ts|help",
            "packages/a/src/x.ts|x",
            "packages/b/test/helpers.ts|help",
        ]);
        let is_file = |_: &str| false;
        let own = manifests_of(&[
            ("", json!({ "name": "root", "workspaces": ["packages/*"] })),
            (
                "packages/a",
                json!({ "name": "a", "scripts": { "test": "node --test" } }),
            ),
            ("packages/b", json!({ "name": "b" })),
        ]);
        let layout = Layout::new(strings(&["packages/a", "packages/b"]), &own, &is_file);
        let files = node_test_files(&keys, &layout);
        assert_eq!(files, strings(&["packages/a/test/helpers.ts"]));
        let root = manifests_of(&[
            ("", json!({ "name": "root", "scripts": { "test": "node --test" } })),
            ("packages/a", json!({ "name": "a" })),
            ("packages/b", json!({ "name": "b" })),
        ]);
        let root_layout = Layout::new(strings(&["packages/a", "packages/b"]), &root, &is_file);
        assert_eq!(
            node_test_files(&keys, &root_layout),
            strings(&["packages/a/test/helpers.ts", "packages/b/test/helpers.ts"]),
            "a root script turns the plugin on in every workspace"
        );
        let reads = KnipReadSets {
            node_test_files: files,
            ..KnipReadSets::default()
        };
        let classifier = Classifier {
            knip_reads: &reads,
            ..bare(&layout)
        };
        assert_classified(
            &classifier,
            &[
                (
                    Side::Skif,
                    "exports",
                    "packages/a/test/helpers.ts|help",
                    Some(NODE_TEST_ENTRY),
                ),
                (
                    Side::Skif,
                    "types",
                    "packages/a/test/helpers.ts|Help",
                    Some(NODE_TEST_ENTRY),
                ),
                (Side::Skif, "exports", "packages/a/src/x.ts|x", None),
                (Side::Skif, "exports", "packages/b/test/helpers.ts|help", None),
                (Side::Tool, "exports", "packages/a/test/helpers.ts|help", None),
            ],
        );
    }

    #[test]
    fn exports_a_tsconfig_declaration_entry_re_exports_split_off_on_skif_side() {
        let sources = BTreeMap::from([
            ("src/module.compiled.d.ts", "export * from './module';\n"),
            ("src/module.ts", "export type RouteModule = 1;\n"),
            ("src/other.ts", "export type Other = 1;\n"),
        ]);
        let is_file = |file: &str| sources.contains_key(file);
        let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
        let layout = Layout::new(BTreeSet::new(), &no_manifest, &is_file);
        let resolver = Resolver::new(&layout, &read);
        let declarations = strings(&["src/module.compiled.d.ts"]);
        let reads = KnipReadSets {
            declaration_reexports: entry_reexports(declarations.iter().cloned().collect(), &resolver, &read),
            ..KnipReadSets::default()
        };
        let classifier = Classifier {
            tsconfig_declarations: &declarations,
            knip_reads: &reads,
            ..bare(&layout)
        };
        assert_classified(
            &classifier,
            &[
                (
                    Side::Skif,
                    "types",
                    "src/module.ts|RouteModule",
                    Some(TSCONFIG_DECLARATION_FILE),
                ),
                (Side::Skif, "types", "src/other.ts|Other", None),
                (Side::Skif, "files", "src/module.ts", None),
                (Side::Tool, "types", "src/module.ts|RouteModule", None),
            ],
        );
    }

    #[test]
    fn bazel_labels_name_the_package_skif_s_bazel_plugin_reads() {
        for (label, expected) in [
            ("//packages/core", Some("packages/core")),
            ("//packages/core:lib", Some("packages/core")),
            ("@//packages/core", Some("packages/core")),
            (":core", Some("here")),
            ("core", Some("here")),
            ("@npm//packages/core", None),
            ("", None),
        ] {
            assert_eq!(bazel_label_package(label, "here").as_deref(), expected, "{label}");
        }
        assert_eq!(within_package("packages/core", "packages/core"), Some(""));
        assert_eq!(within_package("packages/core/a", "packages/core"), Some("a"));
        assert_eq!(within_package("packages/core/a", ""), Some("packages/core/a"));
        assert_eq!(within_package("packages/corex", "packages/core"), None);
    }

    #[test]
    fn exports_an_ng_package_entry_point_re_exports_split_off_on_knip_side() {
        let sources = BTreeMap::from([
            (
                "MODULE.bazel",
                "bazel_dep(name = \"aspect_rules_js\", version = \"2.0.0\")\n",
            ),
            ("packages/core/package.json", r#"{ "name": "@angular/core" }"#),
            ("packages/core/index.ts", "export * from './src/api';\n"),
            ("packages/core/src/api.ts", "export const api = 1;\n"),
            (
                "packages/core/primitives/signals/index.ts",
                "export const signal = 1;\n",
            ),
            ("packages/core/src/lonely.ts", "export const lonely = 1;\n"),
            ("packages/other/index.ts", "export const other = 1;\n"),
        ]);
        let build_files = vec![(
            "packages/core/BUILD.bazel".to_owned(),
            "ng_package(name = \"npm_package\", deps = [\":core\", \"//packages/core/primitives/signals\", \
             \"//packages/core/missing\", \"//packages/other:lib\", \"@npm//packages/core/src\"])\n"
                .to_owned(),
        )];
        let manifests = manifests_in(&sources);
        let manifest = |dir: &str| manifests.get(dir).cloned();
        let is_file = |file: &str| sources.contains_key(file);
        let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
        let layout = Layout::new(strings(&["packages/core"]), &manifest, &is_file);
        let is_project = |dir: &str| layout.is_project(dir);
        let entries = ng_package_entries(&build_files, &is_project, &is_file, &read);
        assert_eq!(
            entries,
            ["packages/core/index.ts", "packages/core/primitives/signals/index.ts"],
            "a label outside the project, of another repository or of a package without an entry file names none"
        );
        let resolver = Resolver::new(&layout, &read);
        let reads = KnipReadSets {
            ng_package_reexports: entry_reexports(entries, &resolver, &read),
            ..KnipReadSets::default()
        };
        let classifier = Classifier {
            knip_reads: &reads,
            ..bare(&layout)
        };
        assert_classified(
            &classifier,
            &[
                (
                    Side::Tool,
                    "exports",
                    "packages/core/index.ts|api",
                    Some(NG_PACKAGE_ENTRY),
                ),
                (
                    Side::Tool,
                    "exports",
                    "packages/core/src/api.ts|api",
                    Some(NG_PACKAGE_ENTRY),
                ),
                (
                    Side::Tool,
                    "nsExports",
                    "packages/core/primitives/signals/index.ts|signal",
                    Some(NG_PACKAGE_ENTRY),
                ),
                (Side::Tool, "exports", "packages/core/src/lonely.ts|lonely", None),
                (Side::Tool, "exports", "packages/other/index.ts|other", None),
                (Side::Tool, "files", "packages/core/src/lonely.ts", None),
                (Side::Skif, "exports", "packages/core/src/api.ts|api", None),
            ],
        );
    }

    #[test]
    fn a_workspace_subpath_into_unbuilt_output_resolves_to_its_source() {
        let sources = BTreeMap::from([
            ("packages/next/src/server/web/types.ts", "export type Req = 1;\n"),
            ("packages/next/src/index.ts", "export const next = 1;\n"),
        ]);
        let manifest = manifests_of(&[(
            "packages/next",
            json!({ "name": "next", "exports": { ".": "./dist/index.js" } }),
        )]);
        let is_file = |file: &str| sources.contains_key(file);
        let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
        let layout = Layout::new(strings(&["packages/next"]), &manifest, &is_file);
        let resolver = Resolver::new(&layout, &read);
        for (specifier, expected) in [
            (
                "next/dist/server/web/types",
                Some("packages/next/src/server/web/types.ts"),
            ),
            ("next", Some("packages/next/src/index.ts")),
            ("next/dist/server/missing", None),
            ("other/dist/server/web/types", None),
        ] {
            assert_eq!(
                resolver.resolve("apps/web/page.ts", specifier).as_deref(),
                expected,
                "{specifier}"
            );
        }
    }

    #[test]
    fn exports_a_published_declaration_file_re_exports_split_off_on_knip_side() {
        let sources = BTreeMap::from([
            (
                "packages/next/server.d.ts",
                "export * from 'next/dist/server/web/types';\nexport { local } from './src/local';\n",
            ),
            ("packages/next/src/server/web/types.ts", "export type Req = 1;\n"),
            (
                "packages/next/src/local.ts",
                "export const local = 1;\nexport const unshared = 2;\n",
            ),
            ("packages/next/src/lonely.ts", "export const lonely = 1;\n"),
        ]);
        let manifest = manifests_of(&[("packages/next", json!({ "name": "next", "files": ["server.d.ts"] }))]);
        let is_file = |file: &str| sources.contains_key(file);
        let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
        let layout = Layout::new(strings(&["packages/next"]), &manifest, &is_file);
        let resolver = Resolver::new(&layout, &read);
        let files: Vec<String> = sources.keys().map(|file| (*file).to_owned()).collect();
        let keys = strings(&[
            "packages/next/src/server/web/types.ts|Req",
            "packages/next/src/local.ts|local",
            "packages/next/src/lonely.ts|lonely",
        ]);
        let (_, published) = published_file_entries(&keys, &files, &layout, &read);
        assert_eq!(published, ["packages/next/server.d.ts"]);
        let credits = published_file_reexports(published, &resolver, &read);
        assert_eq!(
            credits,
            strings(&[
                "packages/next/src/local.ts|local",
                "packages/next/src/server/web/types.ts|*"
            ]),
            "the published file's own findings are `published-file`"
        );
        let classifier = Classifier {
            published_reexports: &credits,
            ..bare(&layout)
        };
        assert_classified(
            &classifier,
            &[
                (
                    Side::Tool,
                    "types",
                    "packages/next/src/server/web/types.ts|Req",
                    Some(PUBLISHED_SOURCE),
                ),
                (
                    Side::Tool,
                    "exports",
                    "packages/next/src/local.ts|local",
                    Some(PUBLISHED_SOURCE),
                ),
                (Side::Tool, "exports", "packages/next/src/local.ts|unshared", None),
                (Side::Tool, "exports", "packages/next/src/lonely.ts|lonely", None),
                (Side::Skif, "exports", "packages/next/src/local.ts|local", None),
            ],
        );
    }

    #[test]
    fn value_exports_a_named_import_of_a_namespace_re_export_uses_as_a_whole_split_off_on_knip_side() {
        let sources = BTreeMap::from([
            (
                "nodes/methods/index.ts",
                "export * as listSearch from './listSearch';\nexport * as loadOptions from './loadOptions';\n",
            ),
            ("nodes/methods/listSearch.ts", "export async function searchA() {}\n"),
            ("nodes/methods/loadOptions.ts", "export async function loadA() {}\n"),
            (
                "nodes/Node.ts",
                "import { listSearch, loadOptions } from './methods';\nexport const methods = { listSearch };\n\
                 console.log(loadOptions.loadA);\n",
            ),
        ]);
        let is_file = |file: &str| sources.contains_key(file);
        let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
        let layout = Layout::new(BTreeSet::new(), &no_manifest, &is_file);
        let resolver = Resolver::new(&layout, &read);
        let files: Vec<String> = sources.keys().map(|file| (*file).to_owned()).collect();
        let credits = namespace_reexport_object_credits(&files, &resolver, &read);
        assert_eq!(
            credits,
            strings(&["nodes/methods/listSearch.ts|*"]),
            "a namespace re-export read through a member is no use as a whole"
        );
        let values = module_objects(
            &strings(&[
                "nodes/methods/listSearch.ts|searchA",
                "nodes/methods/loadOptions.ts|loadA",
            ]),
            &credits,
        );
        let classifier = Classifier {
            module_object_values: &values,
            ..bare(&layout)
        };
        assert_classified(
            &classifier,
            &[
                (
                    Side::Tool,
                    "exports",
                    "nodes/methods/listSearch.ts|searchA",
                    Some(MODULE_OBJECT_VALUES),
                ),
                (Side::Tool, "exports", "nodes/methods/loadOptions.ts|loadA", None),
            ],
        );
    }

    /// Asserts the deviation [`dependency_reads`] finds for each `(side, category, key)` of `cases`, each judged alone
    /// as the one finding of its side, in a checkout of `sources` (the manifests among them, [`manifests_in`]) with these workspace members,
    /// where git ignores the paths at or below those of `ignored`.
    fn assert_dependency_reads(
        sources: &BTreeMap<&str, &str>,
        (members, ignored): (&[&str], &[&str]),
        cases: &[(Side, &str, &str, Option<&str>)],
    ) {
        let manifests = manifests_in(sources);
        let manifest = |dir: &str| manifests.get(dir).cloned();
        let is_file = |file: &str| sources.contains_key(file);
        let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
        let is_ignored = |path: &str| {
            ignored
                .iter()
                .any(|dir| path == *dir || path.starts_with(&format!("{dir}/")))
        };
        let layout = Layout::new(strings(members), &manifest, &is_file);
        let resolver = Resolver::new(&layout, &read);
        let files: Vec<String> = sources.keys().map(|file| (*file).to_owned()).collect();
        let build_files: Vec<(String, String)> = sources
            .iter()
            .filter(|(path, _)| is_build_file(path))
            .map(|(path, source)| ((*path).to_owned(), (*source).to_owned()))
            .collect();
        let outputs = bazel_outputs(&build_files, &is_file, &read);
        let checkout = DependencyCheckout {
            layout: &layout,
            resolver: &resolver,
            files: &files,
            read: &read,
            is_ignored: &is_ignored,
            bazel_outputs: &outputs,
        };
        for (side, category, key, expected) in cases {
            let finding = Keys::from([((*category).to_owned(), BTreeSet::from([(*key).to_owned()]))]);
            let reads = match side {
                Side::Skif => dependency_reads((&finding, &Keys::new()), &checkout),
                Side::Tool => dependency_reads((&Keys::new(), &finding), &checkout),
            };
            assert_eq!(
                reads.deviation(*side, category, key),
                *expected,
                "{side:?} {category} {key}"
            );
        }
    }

    #[test]
    fn unlisted_dependencies_knip_reads_from_data_uris_augmentations_mocks_and_configs_split_off_on_knip_side() {
        let sources = BTreeMap::from([
            ("package.json", r#"{ "name": "root" }"#),
            (
                "src/data.mjs",
                "import a from 'data:text/javascript,export default 1';\nconsole.log(a);\n",
            ),
            ("src/plain.mjs", "import a from 'database';\nconsole.log(a);\n"),
            (
                "src/augment.ts",
                "import { x } from './x';\ndeclare module 'expect' {\n  interface Matchers<R> {\n    toBeFoo(): R;\n  }\n}\nexport const y = x;\n",
            ),
            ("src/x.ts", "export const x = 1;\n"),
            ("src/uses.ts", "import { expect } from 'expect';\nexpect(1);\n"),
            (
                "src/mock.test.ts",
                "import { vi } from 'vitest';\njest.mock('virtual-mod');\njest.requireActual('@scope/real/sub');\nvi.mock('vi-mocked');\n",
            ),
            (
                "docs/next.config.mjs",
                "import nextra from 'nextra';\nexport default nextra({})({ reactStrictMode: true });\n",
            ),
            (
                "web/next.config.mjs",
                "import mdx from '@next/mdx';\nexport default mdx()({});\n",
            ),
        ]);
        assert_dependency_reads(
            &sources,
            (&[], &[]),
            &[
                (Side::Tool, "unlisted", "src/data.mjs|data", Some(DATA_URI)),
                (Side::Tool, "unlisted", "src/plain.mjs|data", None),
                (Side::Skif, "unlisted", "src/data.mjs|data", None),
                (
                    Side::Tool,
                    "unlisted",
                    "src/augment.ts|expect",
                    Some(MODULE_AUGMENTATION),
                ),
                (Side::Tool, "unlisted", "src/uses.ts|expect", None),
                (
                    Side::Tool,
                    "unlisted",
                    "src/mock.test.ts|virtual-mod",
                    Some(JEST_MOCK_SPECIFIER),
                ),
                (
                    Side::Tool,
                    "unlisted",
                    "src/mock.test.ts|@scope/real",
                    Some(JEST_MOCK_SPECIFIER),
                ),
                (Side::Tool, "unlisted", "src/mock.test.ts|vi-mocked", None),
                (Side::Tool, "unlisted", "src/mock.test.ts|vitest", None),
                (
                    Side::Tool,
                    "unlisted",
                    "docs/next.config.mjs|@vercel/turbopack-next",
                    Some(NEXTRA_ALIAS),
                ),
                (
                    Side::Tool,
                    "unresolved",
                    "docs/next.config.mjs|./content/*",
                    Some(NEXTRA_ALIAS),
                ),
                (Side::Tool, "unlisted", "docs/next.config.mjs|nextra", None),
                (
                    Side::Tool,
                    "unlisted",
                    "web/next.config.mjs|@vercel/turbopack-next",
                    None,
                ),
                (Side::Tool, "unlisted", "app/app.json|expo-updates", Some(EXPO_UPDATES)),
                (
                    Side::Tool,
                    "unlisted",
                    "app/app.config.ts|expo-updates",
                    Some(EXPO_UPDATES),
                ),
                (Side::Tool, "unlisted", "app/app.json|expo-notifications", None),
                (Side::Tool, "unlisted", "app/other.json|expo-updates", None),
            ],
        );
    }

    #[test]
    fn unresolved_imports_knip_resolves_otherwise_split_off_on_knip_side() {
        let sources = BTreeMap::from([
            (
                "MODULE.bazel",
                "bazel_dep(name = \"aspect_rules_js\", version = \"2.0.0\")\n",
            ),
            (
                "pkg/BUILD.bazel",
                "genrule(\n    name = \"sources\",\n    outs = [\n        \"dist/index.d.ts\",\n        \"LICENSE\",\n    ],\n    cmd = \"cp $< $@\",\n)\n",
            ),
            ("pkg/index.d.ts", "import './dist/index.js';\nimport './gone.js';\n"),
            ("db/index.ts", "export * from './types';\nexport * from './schema';\n"),
            ("rigs/web-rig/package.json", r#"{ "name": "web-rig" }"#),
            (
                "packages/style-config/package.json",
                r#"{ "name": "@acme/style-config", "dependencies": { "postcss-html": "1" } }"#,
            ),
            (
                "app/package.json",
                r#"{ "name": "app", "devDependencies": { "@acme/style-config": "1" } }"#,
            ),
            (
                "app/stylelint.config.mjs",
                "import { base } from '@acme/style-config/base';\nexport default { ...base, plugins: ['stylelint-order'] };\n",
            ),
            ("desktop/package.json", r#"{ "name": "desktop" }"#),
            ("desktop/script/copy.ts", "export {};\n"),
            ("desktop/script/other.ts", "export {};\n"),
            (
                "desktop/script/build.ts",
                "import { $ } from 'bun';\nawait $`bun ./script/copy.ts ${channel}`;\nawait import('./script/other.ts');\n",
            ),
            (
                "vitest.config.ts",
                "export default { test: { projects: await discover() } };\n",
            ),
            (
                "stores/pg/vitest.config.ts",
                "export default { test: { globalSetup: ['src/__tests__/global-setup.ts'] } };\n",
            ),
            ("stores/pg/src/__tests__/global-setup.ts", "export default () => {};\n"),
            ("tooling.config.ts", "export default {};\n"),
        ]);
        assert_dependency_reads(
            &sources,
            (&["packages/style-config", "app"], &["db/types.ts"]),
            &[
                (
                    Side::Tool,
                    "unresolved",
                    "pkg/index.d.ts|./dist/index.js",
                    Some(BAZEL_BUILD_FILE),
                ),
                (Side::Tool, "unresolved", "pkg/index.d.ts|./gone.js", None),
                (Side::Tool, "unresolved", "db/index.ts|./types", Some(GITIGNORED_IMPORT)),
                (Side::Tool, "unresolved", "db/index.ts|./schema", None),
                (
                    Side::Tool,
                    "unresolved",
                    "rigs/web-rig/profiles/library/tsconfig-base.json|tslib",
                    Some(HEFT_CONFIG),
                ),
                (
                    Side::Tool,
                    "unresolved",
                    "rigs/web-rig/config/tsconfig.json|tslib",
                    None,
                ),
                (
                    Side::Tool,
                    "unresolved",
                    "rigs/web-rig/profiles/library/tsconfig-base.json|./local",
                    None,
                ),
                (
                    Side::Tool,
                    "unresolved",
                    "app/stylelint.config.mjs|postcss-html",
                    Some(SHARED_CONFIG_REFERENCE),
                ),
                (Side::Tool, "unresolved", "app/stylelint.config.mjs|postcss-scss", None),
                (
                    Side::Tool,
                    "unresolved",
                    "app/stylelint.config.mjs|stylelint-order",
                    None,
                ),
                (
                    Side::Tool,
                    "unresolved",
                    "desktop/script/build.ts|./script/copy.ts",
                    Some(SHELL_TEMPLATE_DIRECTORY),
                ),
                (
                    Side::Tool,
                    "unresolved",
                    "desktop/script/build.ts|./script/missing.ts",
                    None,
                ),
                (
                    Side::Tool,
                    "unresolved",
                    "desktop/script/build.ts|./script/other.ts",
                    None,
                ),
                (
                    Side::Tool,
                    "unresolved",
                    "vitest.config.ts|src/__tests__/global-setup.ts",
                    Some(NESTED_VITEST_SETUP),
                ),
                (Side::Tool, "unresolved", "vitest.config.ts|src/missing-setup.ts", None),
                (
                    Side::Tool,
                    "unresolved",
                    "tooling.config.ts|src/__tests__/global-setup.ts",
                    None,
                ),
                (Side::Skif, "unresolved", "db/index.ts|./types", None),
            ],
        );
    }

    #[test]
    fn unlisted_binaries_of_commands_knip_runs_elsewhere_split_off_on_knip_side() {
        let workflow = "name: ci\njobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      \
                        - run: pnpm -C packages/zone electrontest\n      \
                        - name: Copilot\n        working-directory: tools/copilot\n        run: pnpm exec tsx check.ts\n      \
                        - name: Members\n        working-directory: packages/app\n        run: pnpm exec vitest\n      \
                        - name: Plain\n        run: pnpm exec prettier --check .\n      \
                        - name: Computed\n        working-directory: ${{ steps.info.outputs.dir }}\n        run: pnpm napi create-npm-dirs\n  \
                        publish:\n    runs-on: ubuntu-latest\n    defaults:\n      run:\n        shell: bash\n        working-directory: tools/copilot\n    \
                        steps:\n      - run: pnpm update-templates\n";
        let sources = BTreeMap::from([
            (
                "package.json",
                r#"{ "name": "root", "scripts": { "serve": "cd build/rspack && pnpm exec rspack serve", "lint": "pnpm exec eslint ." } }"#,
            ),
            ("build/rspack/package.json", r#"{ "name": "rspack-build" }"#),
            ("tools/copilot/package.json", r#"{ "name": "copilot" }"#),
            ("docs/package.json", r#"{ "name": "docs" }"#),
            (
                "packages/zone/package.json",
                r#"{ "name": "zone", "scripts": { "electrontest": "node test.js" } }"#,
            ),
            (
                "packages/app/package.json",
                "{ \"name\": \"app\", \"scripts\": {\n  \"dev:api\": \"pnpm --dir ../zone electrontest\",\n  \
                 \"fmt\": \"cd ../../docs && pnpm oxfmt-mdx --write\",\n  \"watch-fix\": \"watch -n 1 'sed -i x'\"\n} }",
            ),
            ("packages/sdk/package.json", r#"{ "name": "sdk" }"#),
            (
                "packages/sdk/script/verify.ts",
                "import { $ } from 'bun';\nawait $`bun imports.mjs`.cwd(consumer);\nawait $`node other.mjs`;\n",
            ),
            (
                "scripts/build.mjs",
                "import { $ } from 'zx';\nawait $`cd ${config.rootDir} && pnpm generate-licenses`;\n",
            ),
            (".github/workflows/ci.yml", workflow),
        ]);
        let members = ["packages/zone", "packages/app", "packages/sdk", "docs"];
        let ci = ".github/workflows/ci.yml";
        let binary = |file: &str, name: &str| format!("{file}|{name}");
        let keys = [
            (binary(ci, "electrontest"), Some(COMMAND_DIRECTORY)),
            (binary(ci, "tsx"), Some(COMMAND_DIRECTORY)),
            (binary(ci, "vitest"), None),
            (binary(ci, "prettier"), None),
            (binary(ci, "napi"), Some(COMMAND_DIRECTORY)),
            (binary(ci, "update-templates"), Some(COMMAND_DIRECTORY)),
            (binary("package.json", "rspack"), Some(COMMAND_DIRECTORY)),
            (binary("package.json", "eslint"), None),
            (
                binary("packages/app/package.json", "electrontest"),
                Some(COMMAND_DIRECTORY),
            ),
            (
                binary("packages/app/package.json", "oxfmt-mdx"),
                Some(COMMAND_DIRECTORY),
            ),
            (binary("packages/app/package.json", "watch"), Some(PASSTHROUGH_COMMAND)),
            (binary("packages/app/package.json", "sed"), None),
            (
                binary("packages/sdk/script/verify.ts", "imports.mjs"),
                Some(BUN_FILE_ARGUMENT),
            ),
            (binary("packages/sdk/script/verify.ts", "other.mjs"), None),
            (
                binary("scripts/build.mjs", "generate-licenses"),
                Some(COMMAND_DIRECTORY),
            ),
        ];
        let cases: Vec<(Side, &str, &str, Option<&str>)> = keys
            .iter()
            .map(|(key, expected)| (Side::Tool, "binaries", key.as_str(), *expected))
            .chain([(Side::Skif, "binaries", "package.json|rspack", None)])
            .collect();
        assert_dependency_reads(&sources, (&members, &[]), &cases);
    }

    #[test]
    fn commands_run_where_their_directory_options_and_workflow_keys_say() {
        assert_eq!(
            run_directory(&command_words("cd ../../docs && pnpm "), "packages/app"),
            RunDirectory::At("docs".to_owned())
        );
        assert_eq!(
            run_directory(&command_words("pnpm -C a build && pnpm "), ""),
            RunDirectory::Same,
            "a directory option holds for its own command only"
        );
        assert_eq!(
            run_directory(&command_words("git -C a log && "), ""),
            RunDirectory::Same,
            "only pnpm's -C names the directory"
        );
        assert_eq!(
            run_directory(&command_words("pnpm --dir=tools/x exec "), ""),
            RunDirectory::At("tools/x".to_owned())
        );
        for computed in ["$`cd ${root} && pnpm ", "cd /tmp && ", "cd ../.. && "] {
            assert_eq!(
                run_directory(&command_words(computed), "a"),
                RunDirectory::Computed,
                "{computed}"
            );
        }
        assert_eq!(output_stem("pkg/dist/index.d.ts"), "pkg/dist/index");
        assert_eq!(output_stem("pkg/dist/index.js"), "pkg/dist/index");
        assert_eq!(output_stem("pkg/LICENSE"), "pkg/LICENSE");
        assert_eq!(output_stem("pkg/.env"), "pkg/.env");
        let lines = [
            "jobs:",
            "  a:",
            "    steps:",
            "      - name: x",
            "        with:",
            "          working-directory: nested",
            "        run: |",
            "          pnpm x",
        ];
        assert_eq!(workflow_directory(&lines, 7), None, "a deeper key is not the step's");
    }

    #[test]
    fn unlisted_dependencies_and_binaries_only_skif_reports_from_optional_peers_and_required_child_process_split_off() {
        let sources = BTreeMap::from([
            (
                "package.json",
                r#"{ "name": "root", "devDependencies": { "vitest": "4" } }"#,
            ),
            (
                "node_modules/vitest/package.json",
                r#"{ "name": "vitest", "peerDependencies": { "@vitest/coverage-v8": "*", "jsdom": "*", "happy-dom": "*" }, "peerDependenciesMeta": { "@vitest/coverage-v8": { "optional": true }, "jsdom": { "optional": true } } }"#,
            ),
            ("packages/utils/package.json", r#"{ "name": "utils" }"#),
            (
                "scripts/release.js",
                "const { execSync } = require('child_process');\nexecSync('sentry-cli releases new 1');\n",
            ),
            (
                "scripts/esm.mjs",
                "import { execSync } from 'node:child_process';\nconst cp = require('child_process');\nexecSync('sqlite3 db', cp);\n",
            ),
            ("scripts/plain.js", "run('x');\n"),
        ]);
        assert_dependency_reads(
            &sources,
            (&["packages/utils"], &[]),
            &[
                (
                    Side::Skif,
                    "unlisted",
                    "packages/utils/vite.config.ts|@vitest/coverage-v8",
                    Some(OPTIONAL_PEER_HOST),
                ),
                (
                    Side::Skif,
                    "unlisted",
                    "packages/utils/a.test.ts|jsdom",
                    Some(OPTIONAL_PEER_HOST),
                ),
                (Side::Skif, "unlisted", "packages/utils/a.test.ts|happy-dom", None),
                (Side::Skif, "unlisted", "packages/utils/a.test.ts|left-pad", None),
                (Side::Tool, "unlisted", "packages/utils/a.test.ts|jsdom", None),
                (
                    Side::Skif,
                    "binaries",
                    "scripts/release.js|sentry-cli",
                    Some(REQUIRED_CHILD_PROCESS),
                ),
                (Side::Skif, "binaries", "scripts/esm.mjs|sqlite3", None),
                (Side::Skif, "binaries", "scripts/plain.js|x", None),
                (Side::Skif, "binaries", "package.json|sentry-cli", None),
                (Side::Tool, "binaries", "scripts/release.js|sentry-cli", None),
            ],
        );
    }

    #[test]
    fn unlisted_dependencies_below_nested_projects_of_members_or_listed_above_split_off_on_knip_side() {
        let manifests = [
            (
                "",
                json!({ "name": "root", "workspaces": ["packages/*", "extensions"] }),
            ),
            ("packages/runtime", json!({ "name": "runtime" })),
            (
                "packages/runtime/src/__tests__/test_root",
                json!({ "name": "test_root", "dependencies": {} }),
            ),
            ("extensions", json!({ "name": "extensions" })),
            (
                "extensions/copilot",
                json!({ "name": "copilot", "devDependencies": { "vitest": "1" } }),
            ),
            ("extensions/copilot/src/plugin", json!({ "name": "plugin" })),
            (
                "templates/agent",
                json!({ "name": "agent", "devDependencies": { "@playwright/test": "1" } }),
            ),
            ("templates/agent/client", json!({ "name": "client" })),
            ("examples/app", json!({ "name": "app" })),
        ];
        let read: BTreeMap<String, Rc<Value>> = manifests
            .iter()
            .map(|(dir, manifest)| ((*dir).to_owned(), Rc::new(manifest.clone())))
            .collect();
        let manifest = move |dir: &str| read.get(dir).cloned();
        let every_file = |_: &str| true;
        let layout = Layout::new(strings(&["packages/runtime", "extensions"]), &manifest, &every_file);
        let reads = DependencyReads {
            tool: BTreeMap::from([(
                "unlisted|packages/runtime/src/__tests__/test_root/data.mjs|data".to_owned(),
                DATA_URI,
            )]),
            skif: BTreeMap::from([(
                "binaries|scripts/release.js|sentry-cli".to_owned(),
                REQUIRED_CHILD_PROCESS,
            )]),
        };
        let classifier = Classifier {
            dependency_reads: &reads,
            ..bare(&layout)
        };
        assert_classified(
            &classifier,
            &[
                (
                    Side::Tool,
                    "unlisted",
                    "packages/runtime/src/__tests__/test_root/root.js|RegularModule",
                    Some(NON_MEMBER_PROJECT),
                ),
                (
                    Side::Tool,
                    "unlisted",
                    "extensions/copilot/src/plugin/test/a.spec.ts|vitest",
                    Some(NON_MEMBER_PROJECT),
                ),
                (
                    Side::Tool,
                    "unlisted",
                    "templates/agent/client/test/e2e/a.spec.ts|@playwright/test",
                    Some(NON_MEMBER_PROJECT),
                ),
                (
                    Side::Tool,
                    "unlisted",
                    "templates/agent/client/test/e2e/a.spec.ts|zod",
                    None,
                ),
                (Side::Tool, "unlisted", "examples/app/src/a.ts|zod", None),
                (Side::Tool, "unlisted", "packages/runtime/src/a.ts|zod", None),
                (
                    Side::Tool,
                    "unlisted",
                    "packages/runtime/src/__tests__/test_root/data.mjs|data",
                    Some(DATA_URI),
                ),
                (
                    Side::Skif,
                    "binaries",
                    "scripts/release.js|sentry-cli",
                    Some(REQUIRED_CHILD_PROCESS),
                ),
                (Side::Tool, "binaries", "scripts/release.js|sentry-cli", None),
            ],
        );
    }

    #[test]
    fn tsconfig_paths_into_ignored_generated_code_are_the_importer_s_own_paths() {
        let sources = BTreeMap::from([
            (
                "tsconfig.json",
                r#"{ "compilerOptions": { "paths": { "electron": ["./.build/typings/electron.d.ts"], "@db/*": ["./db/*"] } } }"#,
            ),
            (
                "src/a.ts",
                "import { app } from 'electron';\nimport { Role } from '@db/enums';\nimport { real } from '@db/real';\n\
                 import { gone } from '@db/gone';\nimport z from 'zod';\nconsole.log(app, Role, real, gone, z);\n",
            ),
            ("db/real.ts", "export const real = 1;\n"),
        ]);
        let is_file = |file: &str| sources.contains_key(file);
        let read = |file: &str| sources.get(file).map(|source| (*source).to_owned());
        let layout = Layout::new(BTreeSet::new(), &no_manifest, &is_file);
        let resolver = Resolver::new(&layout, &read);
        let unlisted = strings(&[
            "src/a.ts|electron",
            "src/a.ts|@db/enums",
            "src/a.ts|@db/real",
            "src/a.ts|@db/gone",
            "src/a.ts|zod",
        ]);
        let ignored = |path: &str| path.starts_with(".build/") || path == "db/enums";
        assert_eq!(
            own_paths_keys(&unlisted, &resolver, &read, &ignored),
            strings(&["src/a.ts|@db/enums", "src/a.ts|@db/real", "src/a.ts|electron"])
        );
        assert_eq!(
            own_paths_keys(&unlisted, &resolver, &read, &|_: &str| false),
            strings(&["src/a.ts|@db/real"]),
            "a missing target git does not ignore is no generated code"
        );
    }
}
