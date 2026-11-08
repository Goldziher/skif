//! Findings parity with knip: runs skif and a pinned knip on each corpus repository and diffs the findings.
//!
//! Both tools' findings are reduced to a key per finding, `<file>|<name>` within a knip issue type. The two sides
//! disagree where the tools model things differently, so the numbers are a measure of drift, not a verdict: the
//! committed baselines say what the agreement was, and `--check` fails when it moves.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::corpus::{self, Repo, Tools};
use crate::diff::{self, BASELINE_SCHEMA, Baseline, COMPARED, Counts, Deviation, Entry, Gate, Keys, Samples, Side};
use crate::proc::{capture, find_on_path};

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
/// (the counterparts of `unused-namespace-exports` and `unused-namespace-types`, ADR 0029), not names of
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

/// Categories that name the same thing, `<file>|<export>`, and differ only in how the tools label it: knip counts a
/// `const enum` as a type and files an export under `nsExports` when any importer uses a namespace, skif when all do.
const EXPORT_LABELS: [&str; 4] = ["exports", "types", "nsExports", "nsTypes"];

/// The known differences between skif and knip.
pub const KNIP_DEVIATIONS: [Deviation; 4] = [
    Deviation {
        id: NON_CODE_FILE,
        reason: "knip lists stylesheets, markup, data and docs as unused files; skif's project set is code files only",
    },
    Deviation {
        id: TEST_FILE,
        reason: "skif treats test files and test directories as entries, so it never reports them as unused; knip does without its test-runner plugin",
    },
    Deviation {
        id: FIXTURE_FILE,
        reason: "skif does not report files below `fixtures` or `__fixtures__` as unused unless `[js] include-fixtures` is set (ADR 0050); knip reports them, and the harness counts other fixture-like directories (`fixture`, `testdata`, `*-fixtures`) here too",
    },
    Deviation {
        id: KNIP_UNREAD,
        reason: "knip reads no files below these paths (a deny-all .gitignore hides them), so it reports every dependency there as unused and has no view of their files",
    },
];

/// Extensions skif parses and follows; every other file is outside its project set.
const CODE_EXTENSIONS: [&str; 11] = [
    "js", "mjs", "cjs", "jsx", "ts", "mts", "cts", "tsx", "vue", "svelte", "astro",
];

const TEST_DIRECTORIES: [&str; 6] = ["test", "tests", "__tests__", "e2e", "cypress", "__mocks__"];
const FIXTURE_DIRECTORIES: [&str; 5] = ["fixtures", "__fixtures__", "fixture", "test_root", "testdata"];

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
        let path = normalize(
            diagnostic
                .pointer("/location/path")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            root,
        );
        let message = diagnostic.get("message").and_then(Value::as_str).unwrap_or_default();
        let names = backticked(message);
        let key = match *category {
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
        };
        keys.entry((*category).to_owned()).or_default().insert(key);
    }
    keys
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

/// Whether a knip-reported unused file is a known deviation: outside skif's project set (not code), a fixture, or a
/// test. Fixtures and tests only count when `include_test_files` is off.
fn classify_file(path: &str, include_test_files: bool) -> Option<&'static str> {
    let normalized = path.replace('\\', "/");
    let (directories, name) = normalized.rsplit_once('/').unwrap_or(("", normalized.as_str()));
    let extension = name
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase());
    if !extension.is_some_and(|e| CODE_EXTENSIONS.contains(&e.as_str())) {
        return Some(NON_CODE_FILE);
    }
    if include_test_files {
        return None;
    }
    let mut parts = directories.split('/');
    if parts.any(|p| FIXTURE_DIRECTORIES.contains(&p) || p == "__testfixtures__" || p.ends_with("-fixtures")) {
        return Some(FIXTURE_FILE);
    }
    let in_test_directory = directories.split('/').any(|p| TEST_DIRECTORIES.contains(&p));
    let test_name = [".test.", ".spec.", "_spec.", "_test."]
        .iter()
        .any(|marker| name.contains(marker));
    (in_test_directory || test_name).then_some(TEST_FILE)
}

/// The knip report in its stdout, which may carry banners or log lines around the JSON (dotenv, console output of
/// config files). Tries every `{` that starts a line or a string key, and accepts the first object that is a report.
fn parse_knip_report(stdout: &[u8]) -> Result<Value, String> {
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

/// Installs the dependencies of a checkout without running lifecycle scripts; `Some(reason)` when that fails.
fn install(repo: &Repo, dir: &Path, scratch: &Path, default_timeout: Duration) -> Result<Option<String>> {
    let timeout = repo.install_timeout_secs.map_or(default_timeout, Duration::from_secs);
    let marker = dir.join(INSTALLED_MARKER);
    if fs::read_to_string(&marker).is_ok_and(|rev| rev.trim() == repo.rev) {
        return Ok(None);
    }
    let (program, args) = match install_command(repo, dir) {
        Ok(command) => command,
        Err(reason) => return Ok(Some(reason)),
    };
    if find_on_path(&program).is_none() {
        return Ok(Some(format!("{program} is not installed")));
    }
    eprintln!("parity: {} {program} {}", repo.name, args.join(" "));
    let run = capture(
        Command::new(&program)
            .args(&args)
            .current_dir(dir)
            .env("CI", "1")
            .env("COREPACK_ENABLE_DOWNLOAD_PROMPT", "0"),
        scratch,
        "install",
        timeout,
    )?;
    if run.timed_out {
        return Ok(Some(format!(
            "{program} install timed out after {} s",
            timeout.as_secs()
        )));
    }
    if run.code != Some(0) {
        return Ok(Some(format!("{program} install failed: {}", run.stderr_summary())));
    }
    fs::write(&marker, &repo.rev).with_context(|| format!("writing {}", marker.display()))?;
    Ok(None)
}

struct Outcome {
    entry: Entry,
    detail: Value,
}

fn skipped(reason: impl Into<String>) -> Outcome {
    let reason = reason.into();
    Outcome {
        detail: json!({ "status": diff::SKIPPED, "reason": reason }),
        entry: Entry::skipped(reason),
    }
}

fn parity_repo(options: &Options, skif: &Path, tools: &Tools, repo: &Repo, dir: &Path) -> Result<Outcome> {
    let root = fs::canonicalize(dir).with_context(|| format!("resolving {}", dir.display()))?;
    let scratch = options.out.join("scratch").join(&repo.name);
    if options.install
        && let Some(reason) = install(repo, &root, &scratch, options.timeout)?
    {
        return Ok(skipped(format!("install: {reason}")));
    }

    let mut skif_command = Command::new(skif);
    skif_command
        .args([".", "--format", "json", "--no-install"])
        .current_dir(&root);
    let recursive = options.recursive || repo.recursive;
    if recursive {
        skif_command.arg("--recursive");
    }
    let skif_run = capture(&mut skif_command, &scratch, "skif", options.timeout)?;
    if skif_run.timed_out {
        return Ok(skipped("skif timed out"));
    }
    let skif_doc: Value = match serde_json::from_slice(&skif_run.stdout) {
        Ok(doc) => doc,
        Err(error) => return Ok(skipped(format!("skif output is not JSON: {error}"))),
    };
    if skif_doc.get("diagnostics").is_none() {
        let message = skif_doc["error"]["message"]
            .as_str()
            .unwrap_or("no diagnostics in the output");
        return Ok(skipped(format!("skif failed: {message}")));
    }

    let spec = format!("knip@{}", tools.knip);
    let mut knip_command = Command::new("npx");
    knip_command
        .args(["--yes", spec.as_str(), "--reporter", "json"])
        .current_dir(&root)
        .env("CI", "1")
        .env("DOTENV_CONFIG_QUIET", "true");
    if !options.repo_knip_config {
        let config = empty_knip_config(&scratch)?;
        knip_command.arg("--config").arg(config);
    }
    let knip_run = capture(&mut knip_command, &scratch, "knip", options.timeout)?;
    if knip_run.timed_out {
        return Ok(skipped("knip timed out"));
    }
    let knip_doc = match parse_knip_report(&knip_run.stdout) {
        Ok(doc) => doc,
        Err(reason) => {
            return Ok(skipped(format!(
                "knip failed (exit {}): {reason}; stderr: {}",
                knip_run.code.map_or_else(|| "none".to_owned(), |c| c.to_string()),
                knip_run.stderr_summary()
            )));
        }
    };

    let mut skif_found = skif_keys(&skif_doc, &root);
    let knip_found = knip_keys(&knip_doc, &root);
    align_export_labels(&mut skif_found, &knip_found);
    let split = diff::split_known(&skif_found, &knip_found, &|side, category, key| {
        if repo.knip_unread.iter().any(|prefix| key.starts_with(prefix.as_str())) {
            return Some(KNIP_UNREAD);
        }
        (side == Side::Tool && category == "files")
            .then(|| classify_file(key, options.include_test_files))
            .flatten()
    });
    let (counts, samples) = diff::compare(&split.skif, &split.tool);
    let detail = json!({
        "status": COMPARED,
        "recursive": recursive,
        "known_deviations": split.known,
        "known_samples": split.samples,
        "skif_ms": skif_run.elapsed.as_millis(),
        "knip_ms": knip_run.elapsed.as_millis(),
        "skif_total": total(&skif_found),
        "knip_total": total(&knip_found),
        "categories": category_detail(&counts, &samples),
    });
    Ok(Outcome {
        entry: Entry::compared(counts).with_known(split.known),
        detail,
    })
}

/// An empty knip configuration, so the repository's own settings (ignores, `files: off`, ignored workspaces) do not
/// hide findings. Returns its absolute path.
fn empty_knip_config(scratch: &Path) -> Result<PathBuf> {
    fs::create_dir_all(scratch).with_context(|| format!("creating {}", scratch.display()))?;
    let path = fs::canonicalize(scratch)?.join("knip-empty.json");
    fs::write(&path, "{}\n").with_context(|| format!("writing {}", path.display()))?;
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

fn markdown(tool: &str, installed: bool, entries: &BTreeMap<String, Entry>) -> String {
    let mut out = format!(
        "# skif findings parity with {tool}\n\nDependencies installed: {}.\n\n| Repository | Status | skif only | {tool} only | Both | Known deviations |\n| --- | --- | ---: | ---: | ---: | ---: |\n",
        if installed { "yes" } else { "no" }
    );
    for (name, entry) in entries {
        if entry.status == COMPARED {
            let sum = |f: fn(&Counts) -> usize| entry.categories.values().map(f).sum::<usize>();
            let _ = writeln!(
                out,
                "| {name} | compared | {} | {} | {} | {} |",
                sum(|c| c.skif_only),
                sum(|c| c.tool_only),
                sum(|c| c.both),
                diff::known_total(&entry.known_deviations)
            );
        } else {
            let _ = writeln!(
                out,
                "| {name} | skipped: {} | - | - | - | - |",
                entry.reason.as_deref().unwrap_or("")
            );
        }
    }
    out + &diff::known_section(tool, &KNIP_DEVIATIONS, entries)
}

/// Runs the comparison on the parity suite and applies the baseline gate.
///
/// # Errors
/// Fails when npx or skif is missing, the corpus cannot be fetched, a command cannot start, the report cannot be
/// written, or the gate finds drift.
pub fn run(options: &Options) -> Result<()> {
    if (options.repo_knip_config || options.include_test_files) && (options.gate.write || options.gate.check) {
        bail!(
            "--repo-knip-config and --include-test-files change what is compared: the baselines are recorded without them"
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
    let (tools, repos) = corpus::fetch_suite(&options.corpus_file, "parity", &options.only, &options.corpus_dir)?;
    let tool = format!("knip@{}", tools.knip);
    let mut entries = BTreeMap::new();
    let mut details = serde_json::Map::new();
    for (repo, dir) in &repos {
        eprintln!("parity: {}", repo.name);
        let outcome = parity_repo(options, &skif, &tools, repo, dir)?;
        if let Some(reason) = &outcome.entry.reason {
            eprintln!("parity: {} skipped: {reason}", repo.name);
        }
        details.insert(repo.name.clone(), outcome.detail);
        entries.insert(repo.name.clone(), outcome.entry);
    }
    let table = markdown(&tool, options.install, &entries);
    fs::create_dir_all(&options.out).with_context(|| format!("creating {}", options.out.display()))?;
    let full = json!({
        "tool": tool,
        "installed": options.install,
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
        installed: options.install,
        entries,
    };
    diff::conclude(&options.gate, &current)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
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
        assert_eq!(classify_file("pkg/x/colorize-fixtures/a.ts", false), Some(FIXTURE_FILE));
        assert_eq!(classify_file("pkg/fixtures/test/a.ts", false), Some(FIXTURE_FILE));
        assert_eq!(classify_file("pkg/test/a.ts", true), None);
        assert_eq!(classify_file("pkg/fixtures/a.ts", true), None);
        assert_eq!(classify_file("pkg/src/a.ts", false), None);
        assert_eq!(classify_file("pkg/src/latest.tsx", false), None);
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
}
