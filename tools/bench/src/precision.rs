//! Dead-code precision and recall against an answer key: generates a workspace with seeded dead code
//! ([`skif_bench::deadcode`]), runs skif without and with the type-aware analysis and optionally the pinned knip on it,
//! and scores each run's findings.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use skif_bench::deadcode::{self, AnswerKey, CategoryScore, PATTERNS, Score};
use skif_bench::generate;

use crate::corpus;
use crate::diff::Keys;
use crate::parity::{empty_knip_config, knip_keys, normalize, parse_knip_report, skif_keys};
use crate::proc::capture;
use crate::tools_install;

pub struct Options {
    pub skif: PathBuf,
    pub packages: usize,
    pub seed: u64,
    pub out: PathBuf,
    pub knip: bool,
    pub corpus_file: PathBuf,
    /// Where the locked comparison tools are installed ([`tools_install::install`]).
    pub tools_dir: PathBuf,
    pub json: bool,
    pub timeout: Duration,
    /// Replace `out/repo` even when `out` holds no `answer-key.json` from an earlier run.
    pub force: bool,
}

/// The file a run writes next to `repo/`; its presence marks `out` as an earlier run's output.
const ANSWER_KEY: &str = "answer-key.json";

/// Removes `out/repo` before a new run writes it, but only when `out` is an earlier run's output (it holds
/// [`ANSWER_KEY`]) or `force` is set, so a mistyped `--out` cannot delete an unrelated `repo` directory.
fn clear_repo(out: &Path, force: bool) -> Result<()> {
    let repo = out.join("repo");
    if !repo.exists() {
        return Ok(());
    }
    if !force && !out.join(ANSWER_KEY).is_file() {
        bail!(
            "{} exists but {} holds no {ANSWER_KEY} from an earlier run (use --force to replace it)",
            repo.display(),
            out.display()
        );
    }
    fs::remove_dir_all(&repo).with_context(|| format!("cleaning {}", repo.display()))
}

/// The rule of the class-member findings, which the parity keys leave out because knip 6 has no class-member check.
const CLASS_MEMBER_RULE: &str = "unused-class-members";

/// skif's findings keyed as in the parity harness, plus its `unused-class-members` findings under `classMembers` as
/// `<file>|<Class>.<member>`.
fn skif_findings(document: &Value, root: &Path) -> Keys {
    let mut keys = skif_keys(document, root);
    for diagnostic in document["diagnostics"].as_array().into_iter().flatten() {
        if diagnostic["rule"].as_str() != Some(CLASS_MEMBER_RULE) {
            continue;
        }
        let path = normalize(diagnostic["location"]["path"].as_str().unwrap_or_default(), root);
        let message = diagnostic["message"].as_str().unwrap_or_default();
        let name = message.split('`').nth(1).unwrap_or_default();
        keys.entry("classMembers".to_owned())
            .or_default()
            .insert(format!("{path}|{name}"));
    }
    keys
}

/// The type-aware coverage of a skif report: the checker and the packages per state (`partial 5`). Fails for a
/// `typed` run in which no package was typed, so the typed row never silently measures the name-based analysis.
fn coverage(document: &Value, typed: bool) -> Result<String> {
    let summary = &document["type-aware"];
    let mut states: BTreeMap<&str, usize> = BTreeMap::new();
    for package in summary["packages"].as_array().into_iter().flatten() {
        *states
            .entry(package["state"].as_str().unwrap_or("unknown"))
            .or_default() += 1;
    }
    if typed && (summary["enabled"].as_bool() != Some(true) || states.keys().all(|state| *state == "untyped")) {
        bail!("skif --type-aware typed no package (a build without the `type-aware` feature?): {summary}");
    }
    let engine = summary["engine"].as_str().unwrap_or("no checker");
    let states: Vec<String> = states.iter().map(|(state, n)| format!("{state} {n}")).collect();
    Ok(format!("{engine}: {}", states.join(", ")))
}

fn run_skif(skif: &Path, root: &Path, scratch: &Path, timeout: Duration, typed: bool) -> Result<Value> {
    let mode = if typed { "--type-aware" } else { "--no-type-aware" };
    let run = capture(
        Command::new(skif)
            .args([".", "--format", "json", "--no-install", mode])
            .current_dir(root),
        scratch,
        "skif",
        timeout,
    )?;
    if run.timed_out {
        bail!("skif timed out");
    }
    let document: Value = serde_json::from_slice(&run.stdout)
        .with_context(|| format!("skif output is not JSON; stderr: {}", run.stderr_summary()))?;
    if document.get("diagnostics").is_none() {
        bail!(
            "skif failed: {}",
            document["error"]["message"]
                .as_str()
                .unwrap_or("no diagnostics in the output")
        );
    }
    Ok(document)
}

fn run_knip(program: &Path, root: &Path, scratch: &Path, timeout: Duration) -> Result<Value> {
    let config = empty_knip_config(scratch)?;
    let run = capture(
        Command::new(program)
            .args(["--reporter", "json", "--config"])
            .arg(config)
            .current_dir(root)
            .env("CI", "1")
            .env("DOTENV_CONFIG_QUIET", "true"),
        scratch,
        "knip",
        timeout,
    )?;
    if run.timed_out {
        bail!("knip timed out");
    }
    parse_knip_report(&run.stdout).map_err(|reason| {
        anyhow::anyhow!(
            "knip failed (exit {}): {reason}; stderr: {}",
            run.code.map_or_else(|| "none".to_owned(), |c| c.to_string()),
            run.stderr_summary()
        )
    })
}

fn ratio(value: Option<f64>) -> String {
    value.map_or_else(|| "-".to_owned(), |v| format!("{v:.3}"))
}

fn cell(score: Option<&CategoryScore>) -> String {
    score.map_or_else(
        || "- | - | -".to_owned(),
        |s| {
            format!(
                "{}/{} | {} | {}",
                s.true_positives,
                s.reported,
                ratio(s.precision()),
                ratio(s.recall())
            )
        },
    )
}

/// The report as Markdown: one row per category, the base and member subtotals and the total, one column group per
/// tool, then the false findings by pattern and every false finding and miss.
fn markdown(key: &AnswerKey, tools: &[(String, Score)]) -> String {
    let mut out = format!(
        "# Dead-code precision and recall\n\nSeed {}, {} packages, {} expected findings, {} live items reached through {} patterns.\n\n| Category | Expected |",
        key.seed,
        key.packages,
        key.expected.len(),
        key.live.len(),
        PATTERNS.len()
    );
    let mut rule = String::from("| --- | ---: |");
    for (tool, _) in tools {
        let _ = write!(out, " {tool} TP/reported | precision | recall |");
        rule.push_str(" ---: | ---: | ---: |");
    }
    let _ = writeln!(out, "\n{rule}");
    let mut categories: Vec<&String> = tools.iter().flat_map(|(_, s)| s.categories.keys()).collect();
    categories.sort();
    categories.dedup();
    let row = |label: &str, expected: usize, pick: &dyn Fn(&Score) -> Option<&CategoryScore>| {
        let mut line = format!("| {label} | {expected} |");
        for (_, score) in tools {
            let _ = write!(line, " {} |", cell(pick(score)));
        }
        line
    };
    for category in categories {
        let expected = tools
            .iter()
            .find_map(|(_, s)| s.categories.get(category))
            .map_or(0, |s| s.expected);
        let _ = writeln!(out, "{}", row(category, expected, &|s| s.categories.get(category)));
    }
    for (label, members) in [("**base**", false), ("**members**", true)] {
        let scores: Vec<CategoryScore> = tools
            .iter()
            .map(|(_, s)| if members { s.members() } else { s.base() })
            .collect();
        let expected = scores.first().map_or(0, |s| s.expected);
        let mut line = format!("| {label} | {expected} |");
        for score in &scores {
            let _ = write!(line, " {} |", cell(Some(score)));
        }
        let _ = writeln!(out, "{line}");
    }
    let _ = writeln!(out, "{}", row("**all**", key.expected.len(), &|s| Some(&s.total)));
    for (tool, score) in tools {
        let _ = writeln!(out, "\n## {tool}\n");
        if score.false_by_pattern.is_empty() {
            out.push_str("No false findings.\n");
        } else {
            out.push_str("False findings by live pattern:\n\n");
            for (pattern, count) in &score.false_by_pattern {
                let _ = writeln!(out, "- `{pattern}`: {count}");
            }
        }
        for (label, keys) in [
            ("False findings", &score.total.false_positives),
            ("Missed", &score.total.false_negatives),
        ] {
            if !keys.is_empty() {
                let _ = writeln!(out, "\n{label}:\n");
                for finding in keys {
                    let _ = writeln!(out, "- `{finding}`");
                }
            }
        }
    }
    out
}

/// The type-aware coverage of each skif run ([`coverage`]), by run label.
type Coverages = BTreeMap<&'static str, String>;

/// A subtotal ([`Score::base`], [`Score::members`]) in the JSON report.
fn subtotal_json(s: &CategoryScore) -> Value {
    json!({
        "expected": s.expected,
        "reported": s.reported,
        "true_positives": s.true_positives,
        "precision": s.precision(),
        "recall": s.recall(),
    })
}

/// The scores of skif without and with types, and the type-aware coverage of each run.
fn skif_scores(
    skif: &Path,
    root: &Path,
    scratch: &Path,
    timeout: Duration,
    key: &AnswerKey,
) -> Result<(Vec<(String, Score)>, Coverages)> {
    let mut scores = Vec::new();
    let mut coverages = BTreeMap::new();
    for (label, typed) in [("skif", false), ("skif --type-aware", true)] {
        let document = run_skif(skif, root, scratch, timeout, typed)?;
        coverages.insert(label, coverage(&document, typed)?);
        scores.push((label.to_owned(), deadcode::score(key, &skif_findings(&document, root))));
    }
    Ok((scores, coverages))
}

/// Generates the workspace, runs the tools and prints the scores.
///
/// # Errors
/// Fails when the output directory cannot be written, skif is missing or fails, or knip was asked for and fails.
pub fn run(options: &Options) -> Result<()> {
    let skif = fs::canonicalize(&options.skif).with_context(|| {
        format!(
            "skif binary not found at {} (build it or pass --skif)",
            options.skif.display()
        )
    })?;
    let generated = deadcode::build(&deadcode::Options {
        packages: options.packages,
        seed: options.seed,
    })?;
    clear_repo(&options.out, options.force)?;
    let repo = options.out.join("repo");
    generate::write_files(&generated.files, &repo)?;
    fs::write(
        options.out.join(ANSWER_KEY),
        serde_json::to_string_pretty(&generated.answer_key)? + "\n",
    )?;
    let root = fs::canonicalize(&repo)?;
    let scratch = options.out.join("scratch");

    let (mut scores, coverages) = skif_scores(&skif, &root, &scratch, options.timeout, &generated.answer_key)?;
    if options.knip {
        let manifest = corpus::load_manifest(&options.corpus_file)?;
        let knip = tools_install::require(&options.corpus_file, &options.tools_dir, "knip")?;
        let knip_doc = run_knip(&knip, &root, &scratch, options.timeout)?;
        let knip_found = knip_keys(&knip_doc, &root);
        scores.push((
            format!("knip@{}", manifest.tools.knip),
            deadcode::score(&generated.answer_key, &knip_found),
        ));
    }

    let mut table = markdown(&generated.answer_key, &scores);
    table.push_str("\nType-aware coverage:\n\n");
    for (label, text) in &coverages {
        let _ = writeln!(table, "- {label}: {text}");
    }
    fs::write(options.out.join("deadcode.md"), &table)?;
    let report = json!({
        "seed": options.seed,
        "packages": options.packages,
        "expected": generated.answer_key.expected.len(),
        "type_aware_coverage": coverages,
        "tools": scores.iter().map(|(tool, score)| {
            let categories: serde_json::Map<String, Value> = score.categories.iter().map(|(category, s)| {
                (category.clone(), json!({
                    "expected": s.expected,
                    "reported": s.reported,
                    "true_positives": s.true_positives,
                    "precision": s.precision(),
                    "recall": s.recall(),
                    "false_positives": s.false_positives,
                    "false_negatives": s.false_negatives,
                }))
            }).collect();
            (tool.clone(), json!({
                "precision": score.total.precision(),
                "recall": score.total.recall(),
                "base": subtotal_json(&score.base()),
                "members": subtotal_json(&score.members()),
                "categories": categories,
                "false_by_pattern": score.false_by_pattern,
            }))
        }).collect::<serde_json::Map<String, Value>>(),
    });
    let json_text = serde_json::to_string_pretty(&report)? + "\n";
    fs::write(options.out.join("deadcode.json"), &json_text)?;
    if options.json {
        print!("{json_text}");
    } else {
        println!("{table}");
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use skif::{ConfigOverrides, Linter, MemoryFs, Workspace, load_config};
    use skif_bench::deadcode::{DeadCode, Options as DeadCodeOptions};

    use super::*;

    const ROOT: &str = "/repo";

    /// skif's report on a generated workspace, run in process, with or without types, as the CLI's JSON report.
    fn skif_in_process(generated: &DeadCode, typed: bool) -> Value {
        let mut fs = MemoryFs::new();
        for (path, text) in &generated.files {
            fs.insert(format!("{ROOT}/{}", path.to_string_lossy()), text.clone());
        }
        let fs = Arc::new(fs);
        let root = Path::new(ROOT);
        let overrides = ConfigOverrides::default()
            .with_type_aware(typed)
            .with_no_type_aware(!typed);
        let config = load_config(root, &*fs, &overrides).unwrap();
        let workspace = Workspace::discover_with_fs(root, &config, &*fs).unwrap();
        let report = Linter::with_fs(&config, fs).unwrap().run(&workspace).unwrap();
        serde_json::to_value(&report).unwrap()
    }

    /// The `<category>:<file>|<name>` keys of `names` in the member files of every package.
    fn in_every_package(packages: usize, names: &[(&str, &str, &str)]) -> Vec<String> {
        let mut keys: Vec<String> = (0..packages)
            .flat_map(|i| {
                names
                    .iter()
                    .map(move |(category, file, name)| format!("{category}:packages/p{i}/src/members/{file}|{name}"))
            })
            .collect();
        keys.sort();
        keys
    }

    /// Dead members that both analyses miss: the enum object is assigned to a variable (`Tone.Neon`) or indexed by a
    /// key that is no literal (`Size.Huge`), which uses the enum as a whole (ADR 0009); types never remove a use
    /// (ADR 0010).
    const WHOLE_ENUM_MISSES: [(&str, &str, &str); 2] =
        [("enumMembers", "enums.ts", "Huge"), ("enumMembers", "enums.ts", "Neon")];

    /// Scores skif on two seeds and asserts its false findings and misses are exactly the given ones in every package.
    fn assert_scores(typed: bool, false_findings: &[(&str, &str, &str)], misses: &[(&str, &str, &str)]) {
        for seed in [1, 26] {
            let packages = 3;
            let generated = deadcode::build(&DeadCodeOptions { packages, seed }).unwrap();
            let document = skif_in_process(&generated, typed);
            let typed_packages = document["type-aware"]["packages"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|package| package["state"] != "untyped")
                .count();
            assert_eq!(
                typed_packages,
                if typed { packages } else { 0 },
                "seed {seed}: {}",
                document["type-aware"]
            );
            let score = deadcode::score(&generated.answer_key, &skif_findings(&document, Path::new(ROOT)));
            assert_eq!(
                score.total.false_negatives,
                in_every_package(packages, misses),
                "seed {seed}: misses"
            );
            assert_eq!(
                score.total.false_positives,
                in_every_package(packages, false_findings),
                "seed {seed}: false findings"
            );
            assert_eq!(score.base().recall(), Some(1.0), "seed {seed}");
            assert_eq!(score.base().precision(), Some(1.0), "seed {seed}");
        }
    }

    /// By name, `shape.area()` on a `Circle` keeps `Square.area` used.
    #[test]
    fn skif_without_types_finds_every_seeded_finding_but_the_name_and_whole_enum_misses() {
        let mut misses = vec![("classMembers", "classes.ts", "Square.area")];
        misses.extend(WHOLE_ENUM_MISSES);
        misses.sort_unstable();
        assert_scores(false, &[], &misses);
    }

    /// Known false findings with types: the checker credits a read through a structurally satisfied interface
    /// (`Badge.label`), a type parameter's constraint (`Parcel.weight`) or a key of literal type (`Frame.height`) to
    /// that interface, constraint or nothing, never to the class member the read reaches at runtime.
    #[test]
    fn skif_with_types_finds_the_same_named_member_and_misreads_three_live_members() {
        let false_findings = [
            ("classMembers", "classes.ts", "Badge.label"),
            ("classMembers", "classes.ts", "Frame.height"),
            ("classMembers", "classes.ts", "Parcel.weight"),
        ];
        assert_scores(true, &false_findings, &WHOLE_ENUM_MISSES);
    }

    #[test]
    fn clear_repo_deletes_only_an_earlier_runs_output_unless_forced() {
        let out = std::env::temp_dir().join(format!("skif-bench-precision-{}", std::process::id()));
        let repo = out.join("repo");
        fs::create_dir_all(&repo).unwrap();
        fs::write(repo.join("keep.txt"), "x").unwrap();

        let error = clear_repo(&out, false).unwrap_err().to_string();
        assert!(error.contains("--force"), "{error}");
        assert!(repo.join("keep.txt").exists());

        fs::write(out.join(ANSWER_KEY), "{}").unwrap();
        clear_repo(&out, false).unwrap();
        assert!(!repo.exists());

        fs::create_dir_all(&repo).unwrap();
        fs::remove_file(out.join(ANSWER_KEY)).unwrap();
        clear_repo(&out, true).unwrap();
        assert!(!repo.exists());

        clear_repo(&out, false).unwrap();
        fs::remove_dir_all(&out).unwrap();
    }

    #[test]
    fn the_report_has_a_column_group_per_tool_and_lists_false_findings() {
        let generated = deadcode::build(&DeadCodeOptions { packages: 1, seed: 1 }).unwrap();
        let mut found = generated.answer_key.keys();
        found
            .entry("files".to_owned())
            .or_default()
            .insert("packages/p0/src/lazy.ts".to_owned());
        let scores = [
            (
                "skif".to_owned(),
                deadcode::score(&generated.answer_key, &generated.answer_key.keys()),
            ),
            ("knip".to_owned(), deadcode::score(&generated.answer_key, &found)),
        ];
        let table = markdown(&generated.answer_key, &scores);
        assert!(table.contains("| skif TP/reported | precision | recall | knip TP/reported |"));
        assert!(table.contains("`dynamic-import`: 1"));
        assert!(table.contains("- `files:packages/p0/src/lazy.ts`"));
        assert!(table.contains("No false findings."));
    }
}
