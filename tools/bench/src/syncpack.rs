//! Policy diff against syncpack: runs skif's policy rules and a pinned syncpack on the policy fixtures and on the
//! corpus repositories and diffs the findings.
//!
//! syncpack has no JSON lint report, but `syncpack json` prints every instance with its status code, and skif's
//! `detail` carries the same status code names ([ADR 0009](../../adrs/0009-version-and-catalog-policies.md)). A
//! finding is keyed `<package.json>|<dependency>` within its status code. A fixture whose `skif.toml` holds anything
//! that does not translate to a syncpack configuration is skipped with the reason.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};

use crate::corpus::{self, Tools};
use crate::diff::{self, BASELINE_SCHEMA, Baseline, CaseSummary, Counts, Entry, Gate, Keys, Known, Samples};
use crate::evidence::{Evidence, SYNCPACK_DEVIATIONS};
use crate::parity::normalize;
use crate::proc::{capture, find_on_path};

/// The skif rules that report syncpack's findings.
const POLICY_RULES: [&str; 9] = [
    "version-mismatch",
    "semver-range-mismatch",
    "banned-dependency",
    "invalid-local-version",
    "policy-conflict",
    "catalog-mismatch",
    "catalog-missing-entry",
    "catalog-not-used",
    "unresolved-catalog-references",
];

/// The dependency types syncpack is told to read when skif's `[policy] dependency-types` does not say otherwise: the
/// production and development sections skif governs by default, plus the `local` instance that carries each
/// package's own version. Plain `prod,dev` would drop `local` and with it the own-version alignment.
const DEFAULT_DEPENDENCY_TYPES: &str = "prod,dev,local";

pub const FIXTURES_ENTRY: &str = "fixtures";

pub struct Options {
    pub skif: PathBuf,
    pub corpus_file: PathBuf,
    pub corpus_dir: PathBuf,
    pub fixtures: PathBuf,
    pub out: PathBuf,
    pub only: Vec<String>,
    pub no_fixtures: bool,
    pub no_corpus: bool,
    pub timeout: Duration,
    pub gate: Gate,
}

/// What a `skif.toml` becomes for syncpack.
#[derive(Debug, PartialEq)]
struct Translated {
    config: Option<Value>,
    dependency_types: Option<String>,
}

fn camel(key: &str) -> &'static str {
    match key {
        "dependencies" => "dependencies",
        "packages" => "packages",
        "label" => "label",
        "dependency-types" => "dependencyTypes",
        _ => "specifierTypes",
    }
}

fn json_of(value: &toml::Value) -> Result<Value, String> {
    serde_json::to_value(value).map_err(|e| e.to_string())
}

fn version_group(table: &toml::Table) -> Result<Value, String> {
    let mut group = Map::new();
    for (key, value) in table {
        match key.as_str() {
            "dependencies" | "packages" | "label" | "dependency-types" | "specifier-types" => {
                group.insert(camel(key).to_owned(), json_of(value)?);
            }
            "policy" | "pin" | "snap-to" | "prefer" => {}
            other => return Err(format!("unsupported version-groups key `{other}`")),
        }
    }
    let policy = table
        .get("policy")
        .and_then(toml::Value::as_str)
        .unwrap_or("preferred-semver");
    match policy {
        "banned" => group.insert("isBanned".to_owned(), json!(true)),
        "ignored" => group.insert("isIgnored".to_owned(), json!(true)),
        "same-range" => group.insert("policy".to_owned(), json!("sameRange")),
        "same-minor" => group.insert("policy".to_owned(), json!("sameMinor")),
        "semver-range-only" => group.insert("policy".to_owned(), json!("semverRangeOnly")),
        "pinned" => group.insert(
            "pinVersion".to_owned(),
            json_of(table.get("pin").ok_or("pinned group without `pin`")?)?,
        ),
        "snapped-to" => group.insert(
            "snapTo".to_owned(),
            json_of(table.get("snap-to").ok_or("snapped-to group without `snap-to`")?)?,
        ),
        "preferred-semver" => match table.get("prefer").and_then(toml::Value::as_str) {
            Some("lowest") => group.insert("preferVersion".to_owned(), json!("lowestSemver")),
            _ => None,
        },
        other => return Err(format!("unsupported policy `{other}`")),
    };
    Ok(Value::Object(group))
}

fn semver_group(table: &toml::Table) -> Result<Value, String> {
    let mut group = Map::new();
    for (key, value) in table {
        match key.as_str() {
            "dependencies" | "packages" | "label" | "dependency-types" | "specifier-types" => {
                group.insert(camel(key).to_owned(), json_of(value)?);
            }
            "range" => {
                group.insert("range".to_owned(), json_of(value)?);
            }
            other => return Err(format!("unsupported semver-groups key `{other}`")),
        }
    }
    Ok(Value::Object(group))
}

fn tables<'a>(doc: &'a toml::Table, key: &str) -> Result<Vec<&'a toml::Table>, String> {
    match doc.get(key) {
        None => Ok(Vec::new()),
        Some(toml::Value::Array(items)) => items
            .iter()
            .map(|item| item.as_table().ok_or_else(|| format!("`{key}` holds a non-table")))
            .collect(),
        Some(_) => Err(format!("`{key}` is not an array of tables")),
    }
}

/// Translates the subset of `skif.toml` that has a syncpack equivalent: version groups, semver groups and
/// `[policy] prefer` and `dependency-types`. Anything else is an error naming it.
fn translate(text: &str) -> Result<Translated, String> {
    let doc: toml::Table = toml::from_str(text).map_err(|e| format!("skif.toml: {e}"))?;
    let mut version_groups = Vec::new();
    for table in tables(&doc, "version-groups")? {
        version_groups.push(version_group(table)?);
    }
    let mut semver_groups = Vec::new();
    for table in tables(&doc, "semver-groups")? {
        semver_groups.push(semver_group(table)?);
    }
    let mut dependency_types = None;
    for (key, value) in &doc {
        match key.as_str() {
            "version-groups" | "semver-groups" => {}
            "policy" => {
                let policy = value.as_table().ok_or("`policy` is not a table")?;
                for (name, setting) in policy {
                    match name.as_str() {
                        "prefer" => {
                            if setting.as_str() == Some("lowest") {
                                version_groups.push(json!({ "dependencies": ["**"], "preferVersion": "lowestSemver" }));
                            }
                        }
                        "dependency-types" => {
                            let items = setting.as_array().ok_or("`dependency-types` is not a list")?;
                            let names: Vec<&str> = items.iter().filter_map(toml::Value::as_str).collect();
                            dependency_types = Some(names.join(","));
                        }
                        other => return Err(format!("unsupported [policy] key `{other}`")),
                    }
                }
            }
            other => return Err(format!("unsupported table `{other}`")),
        }
    }
    let mut config = Map::new();
    if !version_groups.is_empty() {
        config.insert("versionGroups".to_owned(), Value::Array(version_groups));
    }
    if !semver_groups.is_empty() {
        config.insert("semverGroups".to_owned(), Value::Array(semver_groups));
    }
    Ok(Translated {
        config: (!config.is_empty()).then_some(Value::Object(config)),
        dependency_types,
    })
}

/// The `--dependency-types` value for a fixture's `[policy] dependency-types`: skif's default scope (with `local`) when
/// the list is absent or empty, the list itself otherwise, which takes the own versions out on both sides when it
/// does not name `local` (ADR 0072).
fn dependency_types(translated: Option<&str>) -> String {
    match translated {
        None | Some("") => DEFAULT_DEPENDENCY_TYPES.to_owned(),
        Some(types) => types.to_owned(),
    }
}

/// The `package.json` of every project in a Rush `rush.json` (JSONC), relative to the repository root. syncpack has
/// no Rush support, so these are fed to it as sources. `Ok(empty)` when there is no `rush.json`.
fn rush_manifests(root: &Path) -> Result<Vec<String>, String> {
    let Ok(text) = fs::read_to_string(root.join("rush.json")) else {
        return Ok(Vec::new());
    };
    Ok(rush_project_folders(&text)?
        .into_iter()
        .map(|folder| format!("{folder}/package.json"))
        .filter(|manifest| root.join(manifest).is_file())
        .collect())
}

/// The `projectFolder` of every project in the text of a `rush.json`.
fn rush_project_folders(text: &str) -> Result<Vec<String>, String> {
    let document: Value = serde_json::from_str(&strip_jsonc(text)).map_err(|e| format!("rush.json: {e}"))?;
    let projects = document
        .get("projects")
        .and_then(Value::as_array)
        .ok_or("rush.json: no `projects` list")?;
    Ok(projects
        .iter()
        .filter_map(|project| project.get("projectFolder").and_then(Value::as_str))
        .map(|folder| folder.trim_matches('/').to_owned())
        .collect())
}

/// JSONC as JSON: line and block comments and trailing commas removed, strings left alone.
fn strip_jsonc(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '"' => {
                out.push('"');
                i += 1;
                while i < chars.len() {
                    out.push(chars[i]);
                    if chars[i] == '\\' && i + 1 < chars.len() {
                        out.push(chars[i + 1]);
                        i += 2;
                        continue;
                    }
                    i += 1;
                    if chars[i - 1] == '"' {
                        break;
                    }
                }
            }
            '/' if chars.get(i + 1) == Some(&'/') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '/' if chars.get(i + 1) == Some(&'*') => {
                i += 2;
                while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                    i += 1;
                }
                i += 2;
            }
            ',' if chars[i + 1..]
                .iter()
                .find(|c| !c.is_whitespace())
                .is_some_and(|c| matches!(c, '}' | ']')) =>
            {
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// The workspace-relative `package.json` paths syncpack read, valid or not.
pub fn syncpack_packages(output: &str, root: &Path) -> BTreeSet<String> {
    output
        .lines()
        .filter(|l| l.starts_with('{'))
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|instance| {
            instance
                .get("package")
                .and_then(Value::as_str)
                .map(|p| normalize(p, root))
        })
        .collect()
}

/// syncpack's invalid instances from the lines of `syncpack json`, by status code.
pub fn syncpack_keys(output: &str, root: &Path) -> Keys {
    let mut keys = Keys::new();
    for line in output.lines().filter(|l| l.starts_with('{')) {
        let Ok(instance) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if instance.get("statusType").and_then(Value::as_str) == Some("Valid") {
            continue;
        }
        let (Some(code), Some(dependency), Some(package)) = (
            instance.get("statusCode").and_then(Value::as_str),
            instance.get("dependency").and_then(Value::as_str),
            instance.get("package").and_then(Value::as_str),
        ) else {
            continue;
        };
        keys.entry(code.to_owned())
            .or_default()
            .insert(format!("{}|{dependency}", normalize(package, root)));
    }
    keys
}

/// skif's policy findings that carry a syncpack status code, by status code.
pub fn skif_policy_keys(document: &Value, root: &Path) -> Keys {
    let mut keys = Keys::new();
    let Some(diagnostics) = document.get("diagnostics").and_then(Value::as_array) else {
        return keys;
    };
    for diagnostic in diagnostics {
        let rule = diagnostic.get("rule").and_then(Value::as_str).unwrap_or_default();
        let Some(code) = diagnostic.get("detail").and_then(Value::as_str) else {
            continue;
        };
        if !POLICY_RULES.contains(&rule) {
            continue;
        }
        let path = diagnostic
            .pointer("/location/path")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let dependency = diagnostic.get("dependency").and_then(Value::as_str).unwrap_or_default();
        keys.entry(code.to_owned())
            .or_default()
            .insert(format!("{}|{dependency}", normalize(path, root)));
    }
    keys
}

/// What both tools found on one workspace.
struct Pair {
    skif: Keys,
    syncpack: Keys,
    /// The `package.json` files syncpack read.
    read: BTreeSet<String>,
}

/// Both tools on one workspace: skif's policy rules with `skif_config` (or none), syncpack with `syncpack_args`.
/// `Err(reason)` when one of them cannot run.
fn run_pair(
    options: &Options,
    skif: &Path,
    tools: &Tools,
    workspace: &Path,
    skif_config: Option<&Path>,
    syncpack_args: &[String],
    scratch: &Path,
) -> Result<Result<Pair, String>> {
    let mut skif_command = Command::new(skif);
    skif_command
        .args([".", "--format", "json", "--no-js", "--no-install"])
        .current_dir(workspace);
    if let Some(config) = skif_config {
        skif_command.arg("--config").arg(config);
    }
    let skif_run = capture(&mut skif_command, scratch, "skif", options.timeout)?;
    if skif_run.timed_out {
        return Ok(Err("skif timed out".to_owned()));
    }
    let Ok(skif_doc) = serde_json::from_slice::<Value>(&skif_run.stdout) else {
        return Ok(Err(format!("skif output is not JSON: {}", skif_run.stderr_summary())));
    };
    if skif_doc.get("diagnostics").is_none() {
        let message = skif_doc["error"]["message"]
            .as_str()
            .unwrap_or("no diagnostics in the output");
        return Ok(Err(format!("skif failed: {message}")));
    }

    let spec = format!("syncpack@{}", tools.syncpack);
    let syncpack_run = capture(
        Command::new("npx")
            .args(["--yes", spec.as_str(), "json", "--no-ansi"])
            .args(syncpack_args)
            .current_dir(workspace)
            .env("CI", "1"),
        scratch,
        "syncpack",
        options.timeout,
    )?;
    if syncpack_run.timed_out {
        return Ok(Err("syncpack timed out".to_owned()));
    }
    let stdout = String::from_utf8_lossy(&syncpack_run.stdout);
    if !stdout.lines().any(|l| l.starts_with('{')) && syncpack_run.code != Some(0) {
        return Ok(Err(format!("syncpack failed: {}", syncpack_run.stderr_summary())));
    }
    Ok(Ok(Pair {
        skif: skif_policy_keys(&skif_doc, workspace),
        syncpack: syncpack_keys(&stdout, workspace),
        read: syncpack_packages(&stdout, workspace),
    }))
}

/// A comparison with the known deviations taken out of the one-sided counts.
struct Comparison {
    counts: BTreeMap<String, Counts>,
    samples: BTreeMap<String, Samples>,
    known: Known,
    known_samples: BTreeMap<String, Vec<String>>,
}

fn compare_known(workspace: &Path, found: Pair) -> Comparison {
    let evidence = Evidence::load(
        workspace,
        found.read,
        found.skif.values().chain(found.syncpack.values()).flatten(),
    );
    let split = diff::split_known(&found.skif, &found.syncpack, &|side, status, key| {
        evidence.classify(side, status, key)
    });
    let (counts, samples) = diff::compare(&split.skif, &split.tool);
    Comparison {
        counts,
        samples,
        known: split.known,
        known_samples: split.samples,
    }
}

fn add_known(total: &mut Known, known: &Known) {
    for (id, found) in known {
        let sum = total.entry(id.clone()).or_default();
        sum.skif_only += found.skif_only;
        sum.tool_only += found.tool_only;
    }
}

/// A syncpack configuration that is empty, so a repository's own (possibly legacy) syncpack settings neither break
/// the run nor bias it: skif ignores them.
fn empty_config(scratch: &Path) -> Result<String> {
    fs::create_dir_all(scratch)?;
    let path = fs::canonicalize(scratch)?.join("empty-syncpackrc.json");
    fs::write(&path, "{}\n")?;
    Ok(path.to_string_lossy().into_owned())
}

fn add(total: &mut BTreeMap<String, Counts>, counts: &BTreeMap<String, Counts>) {
    for (category, found) in counts {
        let sum = total.entry(category.clone()).or_default();
        sum.skif_only += found.skif_only;
        sum.tool_only += found.tool_only;
        sum.both += found.both;
    }
}

fn fixture_cases(root: &Path) -> Result<Vec<(String, PathBuf)>> {
    let mut cases = Vec::new();
    for group in sorted_dirs(root)? {
        for case in sorted_dirs(&group)? {
            if case.join("workspace").is_dir() {
                let name = format!(
                    "{}/{}",
                    group.file_name().unwrap_or_default().to_string_lossy(),
                    case.file_name().unwrap_or_default().to_string_lossy()
                );
                cases.push((name, case));
            }
        }
    }
    Ok(cases)
}

fn sorted_dirs(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut dirs: Vec<PathBuf> = fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    Ok(dirs)
}

fn run_fixtures(options: &Options, skif: &Path, tools: &Tools) -> Result<(Entry, Value)> {
    let fixtures = fs::canonicalize(&options.fixtures)
        .with_context(|| format!("fixtures not found at {}", options.fixtures.display()))?;
    let mut totals = BTreeMap::new();
    let mut summary = CaseSummary::default();
    let mut skipped: BTreeMap<String, String> = BTreeMap::new();
    let mut differing: BTreeMap<String, Value> = BTreeMap::new();
    let mut known_total = Known::new();
    let mut known_cases: BTreeMap<String, Value> = BTreeMap::new();
    for (name, case) in fixture_cases(&fixtures)? {
        let scratch = options.out.join("scratch").join(name.replace('/', "-"));
        fs::create_dir_all(&scratch)?;
        let scratch = fs::canonicalize(&scratch)?;
        let text = fs::read_to_string(case.join("skif.toml")).unwrap_or_default();
        let translated = match translate(&text) {
            Ok(translated) => translated,
            Err(reason) => {
                summary.skipped += 1;
                skipped.insert(name, reason);
                continue;
            }
        };
        let config = if let Some(config) = &translated.config {
            let path = scratch.join("syncpackrc.json");
            fs::write(&path, serde_json::to_string_pretty(config)?)?;
            path.to_string_lossy().into_owned()
        } else {
            empty_config(&scratch)?
        };
        let syncpack_args = [
            "--config".to_owned(),
            config,
            "--dependency-types".to_owned(),
            dependency_types(translated.dependency_types.as_deref()),
        ];
        let config_file = case.join("skif.toml");
        let skif_config = config_file.is_file().then_some(config_file.as_path());
        let workspace = fs::canonicalize(case.join("workspace"))?;
        match run_pair(options, skif, tools, &workspace, skif_config, &syncpack_args, &scratch)? {
            Err(reason) => {
                summary.skipped += 1;
                skipped.insert(name, reason);
            }
            Ok(found) => {
                summary.compared += 1;
                let Comparison {
                    counts,
                    samples,
                    known,
                    known_samples,
                } = compare_known(&workspace, found);
                add_known(&mut known_total, &known);
                if !known.is_empty() {
                    known_cases.insert(name.clone(), json!({ "counts": known, "samples": known_samples }));
                }
                if counts.values().all(|c| c.skif_only == 0 && c.tool_only == 0) {
                    summary.identical += 1;
                } else {
                    summary.differing.push(name.clone());
                    let per_category: Map<String, Value> = counts
                        .iter()
                        .map(|(category, found)| {
                            let sample = samples.get(category);
                            let detail = json!({
                                "skif_only": found.skif_only,
                                "syncpack_only": found.tool_only,
                                "both": found.both,
                                "samples": {
                                    "skif_only": sample.map(|s| &s.skif_only),
                                    "syncpack_only": sample.map(|s| &s.tool_only),
                                },
                            });
                            (category.clone(), detail)
                        })
                        .collect();
                    differing.insert(name, Value::Object(per_category));
                }
                add(&mut totals, &counts);
            }
        }
    }
    let detail = json!({
        "status": diff::COMPARED,
        "cases": {
            "compared": summary.compared,
            "identical": summary.identical,
            "skipped": summary.skipped,
        },
        "skipped": skipped,
        "differing": differing,
        "known_deviations": known_cases,
    });
    let mut entry = Entry::compared(totals).with_known(known_total);
    entry.cases = Some(summary);
    Ok((entry, detail))
}

fn run_repo(options: &Options, skif: &Path, tools: &Tools, name: &str, dir: &Path) -> Result<(Entry, Value)> {
    let root = fs::canonicalize(dir).with_context(|| format!("resolving {}", dir.display()))?;
    let scratch = options.out.join("scratch").join(name);
    let mut syncpack_args = vec![
        "--config".to_owned(),
        empty_config(&scratch)?,
        "--dependency-types".to_owned(),
        dependency_types(None),
    ];
    match rush_manifests(&root) {
        Ok(manifests) => syncpack_args.extend(manifests.into_iter().flat_map(|m| ["--source".to_owned(), m])),
        Err(reason) => {
            return Ok((
                Entry::skipped(reason.clone()),
                json!({ "status": diff::SKIPPED, "reason": reason }),
            ));
        }
    }
    Ok(
        match run_pair(options, skif, tools, &root, None, &syncpack_args, &scratch)? {
            Err(reason) => (
                Entry::skipped(reason.clone()),
                json!({ "status": diff::SKIPPED, "reason": reason }),
            ),
            Ok(found) => {
                let Comparison {
                    counts,
                    known,
                    known_samples,
                    ..
                } = compare_known(&root, found);
                let detail = json!({
                    "status": diff::COMPARED,
                    "categories": counts,
                    "known_deviations": known,
                    "known_samples": known_samples,
                });
                (Entry::compared(counts).with_known(known), detail)
            }
        },
    )
}

fn markdown(tool: &str, entries: &BTreeMap<String, Entry>) -> String {
    let mut out = format!(
        "# skif policy diff against {tool}\n\n| Target | Status | skif only | {tool} only | Both | Known deviations | Differing cases |\n| --- | --- | ---: | ---: | ---: | ---: | ---: |\n"
    );
    for (name, entry) in entries {
        if entry.status == diff::COMPARED {
            let sum = |f: fn(&Counts) -> usize| entry.categories.values().map(f).sum::<usize>();
            let differing = entry
                .cases
                .as_ref()
                .map_or_else(|| "-".to_owned(), |c| c.differing.len().to_string());
            let _ = writeln!(
                out,
                "| {name} | compared | {} | {} | {} | {} | {differing} |",
                sum(|c| c.skif_only),
                sum(|c| c.tool_only),
                sum(|c| c.both),
                diff::known_total(&entry.known_deviations)
            );
        } else {
            let _ = writeln!(
                out,
                "| {name} | skipped: {} | - | - | - | - | - |",
                entry.reason.as_deref().unwrap_or("")
            );
        }
    }
    out + &diff::known_section(tool, &SYNCPACK_DEVIATIONS, entries)
}

/// Runs the diff on the policy fixtures and the corpus and applies the baseline gate.
///
/// # Errors
/// Fails when npx or skif is missing, the corpus cannot be fetched, a command cannot start, the report cannot be
/// written, or the gate finds drift.
pub fn run(options: &Options) -> Result<()> {
    if find_on_path("npx").is_none() {
        bail!("npx is required to run the pinned syncpack: install Node.js");
    }
    let skif = fs::canonicalize(&options.skif).with_context(|| {
        format!(
            "skif binary not found at {} (build it or pass --skif)",
            options.skif.display()
        )
    })?;
    let manifest = corpus::load_manifest(&options.corpus_file)?;
    let tools = manifest.tools;
    let tool = format!("syncpack@{}", tools.syncpack);
    let mut entries = BTreeMap::new();
    let mut details = Map::new();
    if !options.no_fixtures {
        eprintln!("diff:syncpack: {FIXTURES_ENTRY}");
        let (entry, detail) = run_fixtures(options, &skif, &tools)?;
        details.insert(FIXTURES_ENTRY.to_owned(), detail);
        entries.insert(FIXTURES_ENTRY.to_owned(), entry);
    }
    if !options.no_corpus {
        let (_, repos) = corpus::fetch_suite(&options.corpus_file, "parity", &options.only, &options.corpus_dir)?;
        for (repo, dir) in &repos {
            eprintln!("diff:syncpack: {}", repo.name);
            let (entry, detail) = run_repo(options, &skif, &tools, &repo.name, dir)?;
            details.insert(repo.name.clone(), detail);
            entries.insert(repo.name.clone(), entry);
        }
    }
    let table = markdown(&tool, &entries);
    fs::create_dir_all(&options.out).with_context(|| format!("creating {}", options.out.display()))?;
    let full = json!({ "tool": tool, "targets": details });
    fs::write(
        options.out.join("syncpack.json"),
        serde_json::to_string_pretty(&full)? + "\n",
    )?;
    fs::write(options.out.join("syncpack.md"), &table)?;
    println!("{table}");
    let current = Baseline {
        schema: BASELINE_SCHEMA,
        tool,
        installed: false,
        entries,
    };
    diff::conclude(&options.gate, &current)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_skif_toml_means_syncpack_defaults() {
        let got = translate("# nothing\n").unwrap();
        assert_eq!(
            got,
            Translated {
                config: None,
                dependency_types: None
            }
        );
    }

    #[test]
    fn translates_groups_and_policy() {
        let got = translate(
            r#"
[policy]
prefer = "lowest"
dependency-types = ["prod", "dev"]

[[version-groups]]
dependencies = ["react"]
policy = "pinned"
pin = "18.3.1"

[[version-groups]]
packages = ["lib"]
dependencies = ["left-pad"]
policy = "banned"

[[version-groups]]
dependencies = ["a"]
policy = "same-minor"

[[version-groups]]
dependencies = ["b"]
policy = "semver-range-only"

[[semver-groups]]
dependency-types = ["dev"]
range = "~"
"#,
        )
        .unwrap();
        assert_eq!(got.dependency_types.as_deref(), Some("prod,dev"));
        assert_eq!(
            got.config.unwrap(),
            json!({
                "versionGroups": [
                    { "dependencies": ["react"], "pinVersion": "18.3.1" },
                    { "packages": ["lib"], "dependencies": ["left-pad"], "isBanned": true },
                    { "dependencies": ["a"], "policy": "sameMinor" },
                    { "dependencies": ["b"], "policy": "semverRangeOnly" },
                    { "dependencies": ["**"], "preferVersion": "lowestSemver" },
                ],
                "semverGroups": [{ "dependencyTypes": ["dev"], "range": "~" }],
            })
        );
    }

    #[test]
    fn reports_what_has_no_syncpack_equivalent() {
        assert!(
            translate("[catalog]\nmode = \"prefer\"\n")
                .unwrap_err()
                .contains("catalog")
        );
        assert!(
            translate("[[version-groups]]\npolicy = \"mystery\"\n")
                .unwrap_err()
                .contains("mystery")
        );
        assert!(translate("[[version-groups]]\npolicy = \"pinned\"\n").is_err());
    }

    #[test]
    fn keys_use_the_shared_status_code_names() {
        let output = concat!(
            "{\"dependency\":\"react\",\"package\":\"/w/packages/app/package.json\",\"statusCode\":\"DiffersToHighestOrLowestSemver\",\"statusType\":\"Fixable\"}\n",
            "{\"dependency\":\"react\",\"package\":\"/w/packages/lib/package.json\",\"statusCode\":\"IsHighestOrLowestSemver\",\"statusType\":\"Valid\"}\n",
            "x Issues found\n",
        );
        let syncpack = syncpack_keys(output, Path::new("/w"));
        let skif = skif_policy_keys(
            &json!({ "diagnostics": [
                { "rule": "version-mismatch", "detail": "DiffersToHighestOrLowestSemver", "dependency": "react",
                  "location": { "path": "packages/app/package.json" } },
                { "rule": "workspace-protocol", "detail": "X", "location": { "path": "a" } },
                { "rule": "version-mismatch", "location": { "path": "a" } },
            ]}),
            Path::new("/w"),
        );
        assert_eq!(syncpack, skif);
        assert_eq!(syncpack.len(), 1);
    }

    #[test]
    fn syncpack_is_asked_for_the_local_instance_when_the_scope_selects_it() {
        assert_eq!(dependency_types(None), "prod,dev,local");
        assert_eq!(dependency_types(Some("")), "prod,dev,local");
        assert_eq!(dependency_types(Some("prod,dev")), "prod,dev");
        assert_eq!(dependency_types(Some("dev,local")), "dev,local");
    }

    #[test]
    fn rush_json_comments_and_trailing_commas_are_tolerated() {
        let text = r#"
// header
{
  /* block, with "quotes" and // slashes */
  "rushVersion": "5.0.0", // trailing
  "url": "https://example.test/a//b",
  "projects": [
    { "packageName": "a", "projectFolder": "libs/a", },
    { "packageName": "b", "projectFolder": "apps/b/" },
  ],
}
"#;
        assert_eq!(rush_project_folders(text).unwrap(), ["libs/a", "apps/b"]);
        assert!(rush_project_folders("{}").is_err());
        assert!(rush_project_folders("{ not json").is_err());
    }

    #[test]
    fn lists_the_packages_syncpack_read() {
        let output = concat!(
            "{\"dependency\":\"a\",\"package\":\"/w/x/package.json\",\"statusType\":\"Valid\"}\n",
            "{\"dependency\":\"a\",\"package\":\"/w/y/package.json\",\"statusType\":\"Fixable\"}\n",
            "noise\n",
        );
        assert_eq!(
            syncpack_packages(output, Path::new("/w"))
                .into_iter()
                .collect::<Vec<_>>(),
            ["x/package.json", "y/package.json"]
        );
    }
}
