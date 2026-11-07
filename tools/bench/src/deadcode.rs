//! A generated workspace with seeded dead code and an answer key, so dead-code precision and recall can be measured
//! against a right answer instead of against another tool.
//!
//! Every package holds known dead code (unused files, exports, types, dependencies and devDependencies) and live code
//! that is reached only through a pattern that has caused false findings in real repositories: namespace member reads,
//! a namespace read as a bare value, `declare module` for a virtual module, test mocks, `typeof import()`, dynamic
//! `import()`, re-export chains, `import.meta.glob`, files run by package scripts, tsconfig `paths` aliases, fixture
//! directories, an optional dependency loaded in a `try`, and `#` subpath imports through the `imports` field.
//!
//! [`build`] is pure and deterministic for a seed. Findings are keyed as in the knip parity harness: `<file>` for an
//! unused file and `<file>|<name>` otherwise, within a knip issue type (`files`, `exports`, `types`, `dependencies`,
//! `devDependencies`, ...). [`score`] compares a tool's keys with the key.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::PathBuf;

use anyhow::{Result, bail};
use serde::Serialize;
use serde_json::{Value, json};

use crate::rng::Rng;

/// Version of the answer-key format.
pub const ANSWER_KEY_SCHEMA: u32 = 1;
/// The categories the answer key seeds; findings in any other category are false by construction.
pub const SEEDED_CATEGORIES: [&str; 5] = ["files", "exports", "types", "dependencies", "devDependencies"];

/// Options of [`build`].
#[derive(Debug, Clone)]
pub struct Options {
    pub packages: usize,
    pub seed: u64,
}

/// One finding the answer key expects: a knip issue type and the finding key.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Expected {
    pub category: String,
    pub key: String,
}

/// One piece of live code and the pattern that reaches it; a finding with this key is a false finding caused by the
/// pattern.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Live {
    pub pattern: &'static str,
    pub key: String,
}

/// What a correct dead-code tool reports on the generated workspace.
#[derive(Debug, Clone, Serialize)]
pub struct AnswerKey {
    pub schema: u32,
    pub seed: u64,
    pub packages: usize,
    /// Every expected finding, sorted.
    pub expected: Vec<Expected>,
    /// Live code reached through a pattern, sorted; never a finding.
    pub live: Vec<Live>,
}

impl AnswerKey {
    /// The expected findings as keys by category.
    #[must_use]
    pub fn keys(&self) -> BTreeMap<String, BTreeSet<String>> {
        let mut keys: BTreeMap<String, BTreeSet<String>> = SEEDED_CATEGORIES
            .iter()
            .map(|c| ((*c).to_owned(), BTreeSet::new()))
            .collect();
        for expected in &self.expected {
            keys.entry(expected.category.clone())
                .or_default()
                .insert(expected.key.clone());
        }
        keys
    }

    /// The pattern of the live code a (false) finding key names: the key itself, or the file of a `<file>|<name>` key
    /// when the whole file is live.
    #[must_use]
    pub fn pattern_of(&self, key: &str) -> Option<&'static str> {
        let file = key.split('|').next().unwrap_or(key);
        self.live
            .iter()
            .find(|live| live.key == key)
            .or_else(|| self.live.iter().find(|live| live.key == file))
            .map(|live| live.pattern)
    }
}

/// The generated workspace and its answer key.
#[derive(Debug)]
pub struct DeadCode {
    pub files: Vec<(PathBuf, String)>,
    pub answer_key: AnswerKey,
}

/// The live patterns, by id, with what they exercise.
pub const PATTERNS: [(&str, &str); 14] = [
    (
        "namespace-member",
        "exports read as members of a namespace import (`ns.member`)",
    ),
    (
        "namespace-bare",
        "a namespace import read as a bare value, which uses every export",
    ),
    (
        "ts-namespace",
        "members of an exported TypeScript `namespace` read as `Ns.member`",
    ),
    (
        "declare-module",
        "an import of a virtual module declared with `declare module` in a `.d.ts`",
    ),
    (
        "test-mock",
        "a manual mock in `__mocks__`, loaded by the test runner for `vi.mock()`",
    ),
    (
        "typeof-import",
        "an export reached only through `typeof import(\"./file\")`",
    ),
    ("dynamic-import", "a module loaded with `await import(\"./file\")`"),
    (
        "re-export-chain",
        "an export reached through `export *` and `export { } from` chains",
    ),
    ("import-meta-glob", "modules loaded with `import.meta.glob`"),
    (
        "script-entry",
        "a file run by a `package.json` script (`node scripts/x.mjs`)",
    ),
    ("tsconfig-paths", "an import through a tsconfig `paths` alias"),
    (
        "fixture-dir",
        "a file in a `fixtures` directory (test data, not dead code)",
    ),
    (
        "optional-dependency",
        "an optional dependency loaded in a `try` with `import()`",
    ),
    ("subpath-import", "a `#` subpath import mapped by the `imports` field"),
];

/// Plain runtime libraries for the dependencies: none has a knip plugin or a skif tool mapping that could make it used
/// by configuration, so whether it is used depends on the imports alone.
const LIBRARIES: [&str; 48] = [
    "axios",
    "bignumber.js",
    "cac",
    "chalk",
    "classnames",
    "clsx",
    "colorette",
    "commander",
    "consola",
    "cross-spawn",
    "date-fns",
    "dayjs",
    "debug",
    "decimal.js",
    "defu",
    "destr",
    "eventemitter3",
    "execa",
    "fast-glob",
    "fflate",
    "globby",
    "immer",
    "ini",
    "js-yaml",
    "kleur",
    "lodash",
    "lru-cache",
    "luxon",
    "marked",
    "mime",
    "minimist",
    "mitt",
    "mri",
    "ms",
    "nanoid",
    "ohash",
    "p-limit",
    "p-map",
    "pako",
    "papaparse",
    "pathe",
    "picocolors",
    "qs",
    "ramda",
    "semver",
    "ufo",
    "uuid",
    "zod",
];

fn to_json(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default() + "\n"
}

struct Package {
    index: usize,
    dir: String,
    rng: Rng,
    files: Vec<(PathBuf, String)>,
    expected: Vec<Expected>,
    live: Vec<Live>,
}

impl Package {
    fn path(&self, relative: &str) -> String {
        format!("{}/{relative}", self.dir)
    }

    fn file(&mut self, relative: &str, text: String) {
        self.files.push((PathBuf::from(self.path(relative)), text));
    }

    fn expect(&mut self, category: &str, key: String) {
        self.expected.push(Expected {
            category: category.to_owned(),
            key,
        });
    }

    fn live(&mut self, pattern: &'static str, key: String) {
        self.live.push(Live { pattern, key });
    }

    /// `count` distinct library names not in `taken`.
    fn names(&mut self, count: usize, taken: &mut BTreeSet<&'static str>) -> Vec<&'static str> {
        let mut out = Vec::new();
        while out.len() < count && taken.len() < LIBRARIES.len() {
            let name = LIBRARIES[self.rng.below_usize(LIBRARIES.len())];
            if taken.insert(name) {
                out.push(name);
            }
        }
        out
    }

    /// At least `min` (1 for the first package, so every category is seeded), at most `max`.
    fn count(&mut self, max: usize) -> usize {
        let min = usize::from(self.index == 0);
        self.rng.range_usize(min, max)
    }
}

impl Package {
    /// The manifest and tsconfig: used, unused and optional dependencies. Returns the used and the optional ones.
    fn manifests(&mut self) -> (Vec<&'static str>, &'static str) {
        let i = self.index;
        let manifest_path = self.path("package.json");
        let mut taken = BTreeSet::new();

        // Dependencies: used ones are imported by `src/index.ts`, unused ones never.
        let used_deps = self.names(2, &mut taken);
        let count = self.count(2);
        let dead_deps = self.names(count, &mut taken);
        let count = self.count(2);
        let dead_dev = self.names(count, &mut taken);
        let optional = self.names(1, &mut taken);
        for name in &dead_deps {
            self.expect("dependencies", format!("{manifest_path}|{name}"));
        }
        for name in &dead_dev {
            self.expect("devDependencies", format!("{manifest_path}|{name}"));
        }
        for name in &optional {
            self.live("optional-dependency", format!("{manifest_path}|{name}"));
        }

        let mut dependencies: BTreeMap<String, Value> = used_deps
            .iter()
            .chain(&dead_deps)
            .map(|name| ((*name).to_owned(), json!("^1.0.0")))
            .collect();
        if i > 0 {
            dependencies.insert(format!("@dc/p{}", i - 1), json!("workspace:*"));
        }
        let mut dev_dependencies: BTreeMap<String, Value> = dead_dev
            .iter()
            .map(|name| ((*name).to_owned(), json!("^1.0.0")))
            .collect();
        dev_dependencies.insert("vitest".to_owned(), json!("^3.0.0"));
        let optional_dependencies: BTreeMap<String, Value> = optional
            .iter()
            .map(|name| ((*name).to_owned(), json!("^1.0.0")))
            .collect();
        let manifest = json!({
            "name": format!("@dc/p{i}"),
            "version": "1.0.0",
            "private": true,
            "type": "module",
            "main": "src/index.ts",
            "imports": { "#internal/*": "./src/internal/*.ts" },
            "scripts": { "generate": "node scripts/generate.mjs" },
            "dependencies": dependencies,
            "devDependencies": dev_dependencies,
            "optionalDependencies": optional_dependencies,
        });
        self.file("package.json", to_json(&manifest));
        self.file(
            "tsconfig.json",
            to_json(&json!({
                "compilerOptions": {
                    "target": "ES2022",
                    "module": "ESNext",
                    "moduleResolution": "Bundler",
                    "strict": true,
                    "baseUrl": ".",
                    "paths": { "~/*": ["./src/*"] },
                },
                "include": ["src", "scripts"],
            })),
        );
        (used_deps, optional[0])
    }

    /// Unused exports and types in a module the entry imports from, and unused files.
    fn dead_code(&mut self) {
        let i = self.index;
        // Unused exports and types in a module the entry imports from.
        let dead_exports = self.count(3);
        let dead_types = self.count(2);
        let mut util = format!("export function used{i}(value: number): number {{\n  return value + {i};\n}}\n");
        for k in 0..dead_exports {
            let _ = writeln!(util, "\nexport const deadValue{i}x{k} = {k};");
            self.expect("exports", format!("{}|deadValue{i}x{k}", self.path("src/util.ts")));
        }
        for k in 0..dead_types {
            if k % 2 == 0 {
                let _ = writeln!(util, "\nexport interface DeadShape{i}x{k} {{\n  id: number;\n}}");
                self.expect("types", format!("{}|DeadShape{i}x{k}", self.path("src/util.ts")));
            } else {
                let _ = writeln!(util, "\nexport type DeadAlias{i}x{k} = string | number;");
                self.expect("types", format!("{}|DeadAlias{i}x{k}", self.path("src/util.ts")));
            }
        }
        self.file("src/util.ts", util);

        // Unused files: one standalone, one that imports live code (still unreachable).
        let dead_files = self.count(2);
        for k in 0..dead_files {
            let relative = format!("src/orphan{k}.ts");
            let text = if k % 2 == 0 {
                format!("export const orphan{i}x{k} = {k};\n")
            } else {
                format!("import {{ used{i} }} from \"./util\";\n\nexport const orphan{i}x{k} = used{i}({k});\n")
            };
            self.file(&relative, text);
            self.expect("files", self.path(&relative));
        }
    }

    /// Live code reached only through one of the [`PATTERNS`].
    fn live_code(&mut self) {
        let i = self.index;
        // namespace-member: two exports, both read through the namespace.
        self.file(
            "src/ns-members.ts",
            "export const alpha = 1;\nexport function beta(): number {\n  return 2;\n}\n".to_owned(),
        );
        for name in ["alpha", "beta"] {
            self.live("namespace-member", format!("{}|{name}", self.path("src/ns-members.ts")));
        }
        // namespace-bare: the namespace object escapes, so every export is used.
        self.file(
            "src/ns-bare.ts",
            "export const one = 1;\nexport const two = 2;\n".to_owned(),
        );
        for name in ["one", "two"] {
            self.live("namespace-bare", format!("{}|{name}", self.path("src/ns-bare.ts")));
        }
        // ts-namespace
        self.file(
            "src/shapes.ts",
            "export namespace Shapes {\n  export const circle = \"circle\";\n  export const square = \"square\";\n}\n"
                .to_owned(),
        );
        self.live("ts-namespace", format!("{}|Shapes", self.path("src/shapes.ts")));
        // declare-module
        self.file(
            "src/env.d.ts",
            format!("declare module \"virtual:config-{i}\" {{\n  export const configValue: number;\n}}\n"),
        );
        self.live("declare-module", self.path("src/env.d.ts"));
        // test-mock: a manual mock in `__mocks__`, loaded by the test runner in place of `src/client.ts` when a test calls
        // `vi.mock("./client")`; nothing imports it.
        self.file(
            "src/client.ts",
            "export const fetchRemote = (): string => \"remote\";\n".to_owned(),
        );
        self.file(
            "src/__mocks__/client.ts",
            "export const fetchRemote = (): string => \"mock\";\n".to_owned(),
        );
        self.live("test-mock", self.path("src/__mocks__/client.ts"));
        self.file(
            "src/index.test.ts",
            format!(
                "import {{ expect, test, vi }} from \"vitest\";\nimport {{ fetchRemote }} from \"./client\";\nimport {{ used{i} }} from \"./util\";\n\nvi.mock(\"./client\");\n\ntest(\"used\", () => {{\n  expect(used{i}(1)).toBe({});\n  expect(fetchRemote()).toBe(\"mock\");\n}});\n",
                i + 1
            ),
        );
        self.live("test-mock", self.path("src/index.test.ts"));
        // typeof-import
        self.file(
            "src/typed.ts",
            "export const settings = { retries: 3, verbose: false };\n".to_owned(),
        );
        self.live("typeof-import", format!("{}|settings", self.path("src/typed.ts")));
        // dynamic-import
        self.file("src/lazy.ts", "export const lazyValue = 42;\n".to_owned());
        self.live("dynamic-import", self.path("src/lazy.ts"));
        // re-export-chain: barrel -> inner -> leaf; leaf also has a dead export.
        self.file("src/barrel/index.ts", "export * from \"./inner\";\n".to_owned());
        self.file(
            "src/barrel/inner.ts",
            "export { chained } from \"./leaf\";\nexport const innerValue = 7;\n".to_owned(),
        );
        self.file(
            "src/barrel/leaf.ts",
            format!("export const chained = \"chained\";\n\nexport const leafDead{i} = 0;\n"),
        );
        self.expect("exports", format!("{}|leafDead{i}", self.path("src/barrel/leaf.ts")));
        self.live(
            "re-export-chain",
            format!("{}|chained", self.path("src/barrel/leaf.ts")),
        );
        self.live(
            "re-export-chain",
            format!("{}|innerValue", self.path("src/barrel/inner.ts")),
        );
        // import-meta-glob
        for name in ["home", "about"] {
            let relative = format!("src/pages/{name}.ts");
            self.file(&relative, format!("export default \"{name}\";\n"));
            self.live("import-meta-glob", self.path(&relative));
        }
        // script-entry
        self.file(
            "scripts/generate.mjs",
            "import { writeFileSync } from \"node:fs\";\n\nwriteFileSync(\"generated.txt\", \"ok\\n\");\n".to_owned(),
        );
        self.live("script-entry", self.path("scripts/generate.mjs"));
        // tsconfig-paths
        self.file("src/aliased.ts", "export const aliased = \"aliased\";\n".to_owned());
        self.live("tsconfig-paths", self.path("src/aliased.ts"));
        // fixture-dir
        self.file(
            "fixtures/sample.ts",
            "export const sample = { input: 1, output: 2 };\n".to_owned(),
        );
        self.live("fixture-dir", self.path("fixtures/sample.ts"));
        // subpath-import
        self.file(
            "src/internal/helper.ts",
            "export const internalHelper = (): string => \"internal\";\n".to_owned(),
        );
        self.live("subpath-import", self.path("src/internal/helper.ts"));
    }

    /// The entry, `src/index.ts`: reaches everything live and nothing dead.
    fn entry(&mut self, used_deps: &[&str], optional: &str) {
        let i = self.index;
        let mut entry = String::new();
        for (k, name) in used_deps.iter().enumerate() {
            let _ = writeln!(entry, "import dep{k} from \"{name}\";");
        }
        if i > 0 {
            let _ = writeln!(entry, "import {{ run as previous }} from \"@dc/p{}\";", i - 1);
        }
        let _ = writeln!(entry, "import {{ used{i} }} from \"./util\";");
        entry.push_str(concat!(
            "import * as members from \"./ns-members\";\n",
            "import * as bare from \"./ns-bare\";\n",
            "import { Shapes } from \"./shapes\";\n",
        ));
        let _ = writeln!(entry, "import {{ configValue }} from \"virtual:config-{i}\";");
        entry.push_str(concat!(
            "import { chained, innerValue } from \"./barrel\";\n",
            "import { fetchRemote } from \"./client\";\n",
            "import { aliased } from \"~/aliased\";\n",
            "import { internalHelper } from \"#internal/helper\";\n",
            "\n",
            "type Settings = typeof import(\"./typed\")[\"settings\"];\n",
            "\n",
            "export const pages = import.meta.glob(\"./pages/*.ts\");\n",
            "\n",
            "export function describe(value: unknown): string {\n",
            "  return String(value);\n",
            "}\n",
            "\n",
            "export async function load(): Promise<number> {\n",
            "  const lazy = await import(\"./lazy\");\n",
            "  return lazy.lazyValue;\n",
            "}\n",
            "\n",
            "export async function optional(): Promise<unknown> {\n",
            "  try {\n",
        ));
        let _ = writeln!(entry, "    return await import(\"{optional}\");");
        entry.push_str(concat!(
            "  } catch {\n",
            "    return undefined;\n",
            "  }\n",
            "}\n",
            "\n",
            "export function run(settings?: Settings): string {\n",
            "  const parts = [\n",
        ));
        let _ = writeln!(entry, "    used{i}(1),");
        entry.push_str(concat!(
            "    members.alpha + members.beta(),\n",
            "    describe(bare),\n",
            "    Shapes.circle + Shapes.square,\n",
            "    configValue,\n",
            "    chained,\n",
            "    fetchRemote(),\n",
            "    innerValue,\n",
            "    aliased,\n",
            "    internalHelper(),\n",
            "    settings?.retries,\n",
        ));
        for k in 0..used_deps.len() {
            let _ = writeln!(entry, "    describe(dep{k}),");
        }
        if i > 0 {
            entry.push_str("    previous(),\n");
        }
        entry.push_str("  ];\n  return parts.join(\",\");\n}\n");
        self.file("src/index.ts", entry);
        // The specifiers of the entry that only resolve through a pattern: an unlisted or unresolved finding on them is a
        // false finding caused by that pattern.
        let index = self.path("src/index.ts");
        self.live("declare-module", format!("{index}|virtual:config-{i}"));
        self.live("tsconfig-paths", format!("{index}|~/aliased"));
        self.live("subpath-import", format!("{index}|#internal/helper"));
        self.live("optional-dependency", format!("{index}|{optional}"));
        self.live("import-meta-glob", format!("{index}|./pages/*.ts"));
    }
}

fn package(index: usize, options: &Options) -> Package {
    let mut package = Package {
        index,
        dir: format!("packages/p{index}"),
        rng: Rng::for_stream(options.seed ^ 0xDEAD_C0DE, index as u64 + 1),
        files: Vec::new(),
        expected: Vec::new(),
        live: Vec::new(),
    };
    let (used_deps, optional) = package.manifests();
    package.dead_code();
    package.live_code();
    package.entry(&used_deps, optional);
    package
}

/// Builds the workspace and its answer key in memory.
///
/// # Errors
/// Fails when `packages` is zero.
pub fn build(options: &Options) -> Result<DeadCode> {
    if options.packages == 0 {
        bail!("--packages must be at least 1");
    }
    let mut files = vec![
        (
            PathBuf::from("package.json"),
            to_json(&json!({
                "name": "deadcode-bench",
                "version": "0.0.0",
                "private": true,
                "type": "module",
                "packageManager": "pnpm@10.0.0",
            })),
        ),
        (
            PathBuf::from("pnpm-workspace.yaml"),
            "packages:\n  - \"packages/*\"\n".to_owned(),
        ),
        (PathBuf::from("pnpm-lock.yaml"), "lockfileVersion: '9.0'\n".to_owned()),
    ];
    let mut expected = Vec::new();
    let mut live = Vec::new();
    for index in 0..options.packages {
        let package = package(index, options);
        files.extend(package.files);
        expected.extend(package.expected);
        live.extend(package.live);
    }
    expected.sort();
    live.sort();
    Ok(DeadCode {
        files,
        answer_key: AnswerKey {
            schema: ANSWER_KEY_SCHEMA,
            seed: options.seed,
            packages: options.packages,
            expected,
            live,
        },
    })
}

/// Precision and recall of one tool in one category.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CategoryScore {
    pub expected: usize,
    pub reported: usize,
    pub true_positives: usize,
    /// Reported but not in the answer key.
    pub false_positives: Vec<String>,
    /// In the answer key but not reported.
    pub false_negatives: Vec<String>,
}

impl CategoryScore {
    /// `None` when nothing was reported.
    #[must_use]
    #[allow(clippy::cast_precision_loss, reason = "counts are far below 2^52")]
    pub fn precision(&self) -> Option<f64> {
        (self.reported > 0).then(|| self.true_positives as f64 / self.reported as f64)
    }

    /// `None` when nothing was expected.
    #[must_use]
    #[allow(clippy::cast_precision_loss, reason = "counts are far below 2^52")]
    pub fn recall(&self) -> Option<f64> {
        (self.expected > 0).then(|| self.true_positives as f64 / self.expected as f64)
    }
}

/// The score of one tool: per category, plus the false findings attributed to a live pattern.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Score {
    pub categories: BTreeMap<String, CategoryScore>,
    pub total: CategoryScore,
    /// False findings per live pattern (`other` when the key names no live code).
    pub false_by_pattern: BTreeMap<String, usize>,
}

/// The common category of a knip issue type: findings labelled through a namespace (`nsExports`, `nsTypes`) are
/// still unused exports and types.
#[must_use]
pub fn common_category(category: &str) -> &str {
    match category {
        "nsExports" => "exports",
        "nsTypes" => "types",
        other => other,
    }
}

/// Scores a tool's findings, keyed as in the parity harness, against the answer key. Every category the tool reports
/// is scored; one the key does not seed has nothing expected, so all its findings are false.
#[must_use]
pub fn score(key: &AnswerKey, found: &BTreeMap<String, BTreeSet<String>>) -> Score {
    let mut reported: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (category, keys) in found {
        reported
            .entry(common_category(category).to_owned())
            .or_default()
            .extend(keys.iter().cloned());
    }
    let expected = key.keys();
    let categories: BTreeSet<&String> = expected.keys().chain(reported.keys()).collect();
    let empty = BTreeSet::new();
    let mut score = Score::default();
    for category in categories {
        let want = expected.get(category).unwrap_or(&empty);
        let got = reported.get(category).unwrap_or(&empty);
        let entry = CategoryScore {
            expected: want.len(),
            reported: got.len(),
            true_positives: want.intersection(got).count(),
            false_positives: got.difference(want).cloned().collect(),
            false_negatives: want.difference(got).cloned().collect(),
        };
        for false_key in &entry.false_positives {
            let pattern = key.pattern_of(false_key).unwrap_or("other");
            *score.false_by_pattern.entry(pattern.to_owned()).or_default() += 1;
        }
        score.total.expected += entry.expected;
        score.total.reported += entry.reported;
        score.total.true_positives += entry.true_positives;
        score
            .total
            .false_positives
            .extend(entry.false_positives.iter().map(|k| format!("{category}:{k}")));
        score
            .total
            .false_negatives
            .extend(entry.false_negatives.iter().map(|k| format!("{category}:{k}")));
        score.categories.insert(category.clone(), entry);
    }
    score
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn options(packages: usize, seed: u64) -> Options {
        Options { packages, seed }
    }

    #[test]
    fn is_deterministic_and_seeded() {
        let a = build(&options(4, 7)).unwrap();
        let b = build(&options(4, 7)).unwrap();
        assert_eq!(a.files, b.files);
        assert_eq!(a.answer_key.expected, b.answer_key.expected);
        assert_ne!(a.files, build(&options(4, 8)).unwrap().files);
    }

    #[test]
    fn seeds_every_category_and_every_pattern() {
        let key = build(&options(1, 3)).unwrap().answer_key;
        let keys = key.keys();
        for category in SEEDED_CATEGORIES {
            assert!(!keys[category].is_empty(), "{category} not seeded");
        }
        let patterns: BTreeSet<&str> = key.live.iter().map(|live| live.pattern).collect();
        for (pattern, _) in PATTERNS {
            assert!(patterns.contains(pattern), "{pattern} missing");
        }
        assert_eq!(patterns.len(), PATTERNS.len());
    }

    #[test]
    fn expected_files_exist_and_live_code_is_not_expected() {
        let generated = build(&options(3, 5)).unwrap();
        let paths: BTreeSet<String> = generated
            .files
            .iter()
            .map(|(path, _)| path.to_string_lossy().into_owned())
            .collect();
        for expected in &generated.answer_key.expected {
            let file = expected.key.split('|').next().unwrap();
            assert!(paths.contains(file), "{file} not generated");
        }
        let expected: BTreeSet<&str> = generated.answer_key.expected.iter().map(|e| e.key.as_str()).collect();
        for live in &generated.answer_key.live {
            assert!(!expected.contains(live.key.as_str()), "{} both live and dead", live.key);
        }
    }

    #[test]
    fn scores_precision_recall_and_attributes_false_findings() {
        let key = build(&options(1, 3)).unwrap().answer_key;
        let mut found = key.keys();
        let missed = found["exports"].iter().next().unwrap().clone();
        found.get_mut("exports").unwrap().remove(&missed);
        found
            .entry("files".to_owned())
            .or_default()
            .insert("packages/p0/src/lazy.ts".to_owned());
        found
            .entry("nsExports".to_owned())
            .or_default()
            .insert("packages/p0/src/ns-members.ts|alpha".to_owned());
        found
            .entry("unlisted".to_owned())
            .or_default()
            .insert("packages/p0/src/index.ts|virtual:config-0".to_owned());
        let score = score(&key, &found);
        assert_eq!(score.categories["exports"].false_negatives, [missed]);
        assert_eq!(score.categories["files"].false_positives.len(), 1);
        assert_eq!(score.categories["unlisted"].expected, 0);
        assert_eq!(score.false_by_pattern["dynamic-import"], 1);
        assert_eq!(score.false_by_pattern["namespace-member"], 1);
        assert_eq!(score.false_by_pattern["declare-module"], 1);
        assert_eq!(score.categories["dependencies"].precision(), Some(1.0));
        assert!(score.categories["unlisted"].recall().is_none());
    }
}
