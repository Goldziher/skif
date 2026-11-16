//! Benchmark harness for skif: generates synthetic monorepos, fetches the pinned corpus and runs hyperfine.

mod corpus;
mod diff;
mod evidence;
mod parity;
mod precision;
mod proc;
mod provenance;
mod run;
mod sources;
mod syncpack;
mod tools;
mod tools_install;
mod type_aware;
mod typed_diff;

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};

use crate::diff::{Gate, Tolerance};
use crate::run::RunOptions;
use skif_bench::generate::{self, Options, PackageManager};

const DEFAULT_CORPUS_FILE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/corpus.toml");
const DEFAULT_PARITY_BASELINE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/baselines/parity.json");
const DEFAULT_TYPE_AWARE_BASELINE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/baselines/type-aware.json");
const DEFAULT_SYNCPACK_BASELINE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/baselines/syncpack.json");
const DEFAULT_TOOLS_DIR: &str = "target/bench-generated/tools";
const DEFAULT_POLICY_FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/policy");

#[derive(Parser)]
#[command(name = "skif-bench", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Generate a deterministic synthetic monorepo.
    Generate(GenerateArgs),
    /// Manage the pinned corpus of real monorepos.
    Corpus {
        #[command(subcommand)]
        action: CorpusAction,
    },
    /// Keep the pinned comparison tools at the latest npm releases.
    Tools {
        #[command(subcommand)]
        action: ToolsAction,
    },
    /// Benchmark skif and the pinned comparison tools with hyperfine.
    Run(RunArgs),
    /// Compare skif's findings with a pinned knip on the corpus.
    Parity(ParityArgs),
    /// Compare skif's policy findings with a pinned syncpack on the policy fixtures and the corpus.
    SyncpackDiff(SyncpackArgs),
    /// Measure dead-code precision and recall of skif (and the pinned knip) against a generated answer key.
    Deadcode(DeadcodeArgs),
    /// Compare the JSON output of a reference skif binary with the binary under test on the bench corpus, typed and
    /// untyped; fails on any difference the allow list does not cover.
    TypedDiff(TypedDiffArgs),
}

#[derive(Args)]
struct TypedDiffArgs {
    /// The reference skif binary (the build before the change).
    #[arg(long)]
    reference: PathBuf,
    /// The skif binary under test.
    #[arg(long, default_value = "target/release/skif")]
    skif: PathBuf,
    /// Only these repositories of the `bench` suite.
    #[arg(long, value_delimiter = ',')]
    only: Vec<String>,
    /// Corpus manifest.
    #[arg(long, default_value = DEFAULT_CORPUS_FILE)]
    corpus_file: PathBuf,
    /// Cache directory holding the corpus checkouts (fetched when missing, installed checkouts are used as they are).
    #[arg(long, default_value = "target/bench-corpus")]
    corpus_dir: PathBuf,
    /// JSON file listing expected differences: `{"allow": [{"kind": "added|removed|package|section", "reason": "...",
    /// "repo", "mode", "rule", "file", "package", "section"}]}`; fields left out match anything.
    #[arg(long)]
    allow: Option<PathBuf>,
    /// Also write the comparison as JSON to this file.
    #[arg(long)]
    out: Option<PathBuf>,
    /// Where the captured output of each run is written.
    #[arg(long, default_value = "target/bench-typed-diff")]
    scratch: PathBuf,
    /// Seconds each skif run may take.
    #[arg(long, default_value_t = 900)]
    timeout_secs: u64,
}

#[derive(Args)]
struct DeadcodeArgs {
    /// The skif binary under test.
    #[arg(long, default_value = "target/release/skif")]
    skif: PathBuf,
    /// Number of packages to generate.
    #[arg(long, default_value_t = 5)]
    packages: usize,
    /// Seed of the pseudo-random generator.
    #[arg(long, default_value_t = 1)]
    seed: u64,
    /// Where the workspace (`repo/`), `answer-key.json` and the report are written.
    #[arg(long, default_value = "target/bench-deadcode")]
    out: PathBuf,
    /// Also run the knip version pinned in the corpus manifest (installed from the lock with npm, which needs the npm registry).
    #[arg(long)]
    knip: bool,
    /// Corpus manifest (for the pinned knip version and its lock).
    #[arg(long, default_value = DEFAULT_CORPUS_FILE)]
    corpus_file: PathBuf,
    /// Where the locked comparison tools are installed (the directory `run` uses), unless already there from the same
    /// lock.
    #[arg(long, default_value = DEFAULT_TOOLS_DIR)]
    tools_dir: PathBuf,
    /// Print the report as JSON instead of Markdown.
    #[arg(long)]
    json: bool,
    /// Seconds each skif run and knip run may take.
    #[arg(long, default_value_t = 600)]
    timeout_secs: u64,
    /// Replace `<out>/repo` even when `<out>` holds no `answer-key.json` from an earlier run.
    #[arg(long)]
    force: bool,
}

#[derive(Args)]
struct GenerateArgs {
    /// Number of packages to generate.
    #[arg(long)]
    packages: usize,
    /// Directory to write the workspace into; must be empty or absent unless --force is given.
    #[arg(long)]
    out: PathBuf,
    /// Seed of the pseudo-random generator.
    #[arg(long, default_value_t = 1)]
    seed: u64,
    /// Package manager whose workspace layout and lockfile to emit.
    #[arg(long, value_enum, default_value = "pnpm")]
    pm: PackageManager,
    /// Emit a catalog (pnpm and bun) and reference it with `catalog:` specifiers.
    #[arg(long)]
    catalog: bool,
    /// Probability, per dependency or package, of injecting each kind of issue.
    #[arg(long, default_value_t = 0.05)]
    mismatch_rate: f64,
    /// Delete the output directory first.
    #[arg(long)]
    force: bool,
    /// TypeScript modules to add to every package (an import chain ending in `src/index.ts`); 0 writes manifests only.
    #[arg(long, default_value_t = 0)]
    source_files_per_package: usize,
}

#[derive(Subcommand)]
enum CorpusAction {
    /// Fetch every repository of a suite at its pinned commit (idempotent).
    Fetch(CorpusArgs),
}

#[derive(Subcommand)]
enum ToolsAction {
    /// Move every `[tools]` pin behind its latest release to it, in corpus.toml, tools/package.json and the lock.
    Update(ToolsArgs),
    /// Fail when a `[tools]` pin is behind its latest release.
    Check(ToolsArgs),
}

#[derive(Args)]
struct ToolsArgs {
    /// Corpus manifest.
    #[arg(long, default_value = DEFAULT_CORPUS_FILE)]
    file: PathBuf,
}

#[derive(Args)]
struct CorpusArgs {
    /// Corpus manifest.
    #[arg(long, default_value = DEFAULT_CORPUS_FILE)]
    file: PathBuf,
    /// Cache directory the repositories are cloned into.
    #[arg(long, default_value = "target/bench-corpus")]
    dir: PathBuf,
    /// Suite to fetch: `bench` (timed) or `parity` (compared with knip and syncpack).
    #[arg(long, default_value = "parity")]
    suite: String,
    /// Only these repositories of the suite.
    #[arg(long, value_delimiter = ',')]
    only: Vec<String>,
}

#[derive(Args)]
struct GateArgs {
    /// Record this run in the baseline file.
    #[arg(long)]
    write_baseline: bool,
    /// Fail when this run drifts from the baseline file.
    #[arg(long)]
    check: bool,
    /// A count may move by this percentage of its baseline value...
    #[arg(long, default_value_t = 10)]
    tolerance_percent: usize,
    /// ...or by this many, whichever is larger.
    #[arg(long, default_value_t = 5)]
    tolerance_abs: usize,
}

impl GateArgs {
    fn gate(&self, baseline: PathBuf) -> Gate {
        Gate {
            baseline,
            write: self.write_baseline,
            check: self.check,
            tolerance: Tolerance {
                percent: self.tolerance_percent,
                absolute: self.tolerance_abs,
            },
        }
    }
}

// Independent command-line switches, one per flag.
#[allow(clippy::struct_excessive_bools)]
#[derive(Args)]
struct ParityArgs {
    /// The skif binary under test.
    #[arg(long, default_value = "target/release/skif")]
    skif: PathBuf,
    /// Corpus manifest.
    #[arg(long, default_value = DEFAULT_CORPUS_FILE)]
    corpus_file: PathBuf,
    /// Corpus cache directory.
    #[arg(long, default_value = "target/bench-corpus")]
    corpus_dir: PathBuf,
    /// Where the locked comparison tools are installed (the directory `run` uses), unless already there from the same
    /// lock.
    #[arg(long, default_value = DEFAULT_TOOLS_DIR)]
    tools_dir: PathBuf,
    /// Where the full report (with sample findings) is written.
    #[arg(long, default_value = "target/bench-results")]
    out: PathBuf,
    /// Baseline file (counts only).
    #[arg(long, default_value = DEFAULT_PARITY_BASELINE)]
    baseline: PathBuf,
    /// Only these repositories of the parity suite.
    #[arg(long, value_delimiter = ',')]
    only: Vec<String>,
    /// Install dependencies first (`--ignore-scripts`); knip reports far fewer false findings with them.
    #[arg(long)]
    install: bool,
    /// Run skif with --recursive on every repository, not only those marked `recursive` in the corpus manifest. Paths
    /// stay relative to the start directory, as in knip's report; nested roots that are not workspace members are
    /// analysed against their own manifests.
    #[arg(long)]
    recursive: bool,
    /// Run knip with the repository's own configuration instead of an empty one. The repository's ignores and
    /// disabled rules then hide findings, so a baseline cannot be recorded or checked this way.
    #[arg(long)]
    repo_knip_config: bool,
    /// Count test and fixture files in the knip-only `files` instead of reporting them as known deviations. Files
    /// outside skif's project set (css, mdx, json, ...) are always known deviations. Not usable with a baseline.
    #[arg(long)]
    include_test_files: bool,
    /// Seconds each skif run and knip run may take, and each install unless the corpus manifest sets its own.
    #[arg(long, default_value_t = 1800)]
    timeout_secs: u64,
    /// Skip the second skif run with `--no-type-aware`. The run is shorter, but the findings only one mode reports
    /// are not set apart as `type-resolved` and the type-aware baseline is not checked; a baseline cannot be
    /// recorded this way.
    #[arg(long)]
    no_untyped_pass: bool,
    /// Baseline file of the typed and untyped counts per rule (counts only).
    #[arg(long, default_value = DEFAULT_TYPE_AWARE_BASELINE)]
    type_aware_baseline: PathBuf,
    /// Sample findings the full report lists per category, known deviation and side: a count, or `all` to list every
    /// one-sided finding. Baselines hold counts only, so this changes no baseline.
    #[arg(long, default_value_t = diff::SAMPLE_LIMIT, value_parser = diff::parse_sample_limit)]
    samples: usize,
    #[command(flatten)]
    gate: GateArgs,
}

#[derive(Args)]
struct SyncpackArgs {
    /// The skif binary under test.
    #[arg(long, default_value = "target/release/skif")]
    skif: PathBuf,
    /// Corpus manifest.
    #[arg(long, default_value = DEFAULT_CORPUS_FILE)]
    corpus_file: PathBuf,
    /// Corpus cache directory.
    #[arg(long, default_value = "target/bench-corpus")]
    corpus_dir: PathBuf,
    /// Where the locked comparison tools are installed (the directory `run` uses), unless already there from the same
    /// lock.
    #[arg(long, default_value = DEFAULT_TOOLS_DIR)]
    tools_dir: PathBuf,
    /// The policy conformance fixtures.
    #[arg(long, default_value = DEFAULT_POLICY_FIXTURES)]
    fixtures: PathBuf,
    /// Where the full report (with sample findings) is written.
    #[arg(long, default_value = "target/bench-results")]
    out: PathBuf,
    /// Baseline file (counts only).
    #[arg(long, default_value = DEFAULT_SYNCPACK_BASELINE)]
    baseline: PathBuf,
    /// Only these repositories of the parity suite.
    #[arg(long, value_delimiter = ',')]
    only: Vec<String>,
    /// Skip the policy fixtures.
    #[arg(long)]
    no_fixtures: bool,
    /// Skip the corpus repositories.
    #[arg(long)]
    no_corpus: bool,
    /// Seconds each skif run and syncpack run may take.
    #[arg(long, default_value_t = 300)]
    timeout_secs: u64,
    #[command(flatten)]
    gate: GateArgs,
}

// Independent command-line switches, one per flag.
#[allow(clippy::struct_excessive_bools)]
#[derive(Args)]
struct RunArgs {
    /// The skif binary under test.
    #[arg(long, default_value = "target/release/skif")]
    skif: PathBuf,
    /// Extra argument passed to every skif invocation; repeatable.
    #[arg(long = "skif-arg", allow_hyphen_values = true)]
    skif_args: Vec<String>,
    /// Package counts of the generated workspaces.
    #[arg(long, value_delimiter = ',', default_values_t = [50, 500, 2000])]
    sizes: Vec<usize>,
    /// Package manager of the generated workspaces.
    #[arg(long, value_enum, default_value = "pnpm")]
    pm: PackageManager,
    /// Skip the corpus and benchmark only generated workspaces.
    #[arg(long)]
    no_corpus: bool,
    /// Only these repositories of the bench corpus; the summary records them under `corpus_only`, and such a partial
    /// run is not published.
    #[arg(long, value_delimiter = ',', conflicts_with = "no_corpus")]
    only: Vec<String>,
    /// The skif checkout the binary under test was built from, whose commit the summary records as `source_rev`
    /// (default: the checkout the harness was built from).
    #[arg(long)]
    skif_source: Option<PathBuf>,
    /// Corpus manifest.
    #[arg(long, default_value = DEFAULT_CORPUS_FILE)]
    corpus_file: PathBuf,
    /// Corpus cache directory.
    #[arg(long, default_value = "target/bench-corpus")]
    corpus_dir: PathBuf,
    /// Where generated workspaces are written.
    #[arg(long, default_value = "target/bench-generated")]
    generated_dir: PathBuf,
    /// Where the summary and the raw hyperfine exports are written.
    #[arg(long, default_value = "target/bench-results")]
    out: PathBuf,
    /// Hyperfine warmup runs.
    #[arg(long, default_value_t = 3)]
    warmup: u32,
    /// Hyperfine measured runs.
    #[arg(long, default_value_t = 15)]
    runs: u32,
    /// Source files of the module graph case (ADR 0001: 10,000 files); 0 skips it.
    #[arg(long, default_value_t = 10_000)]
    graph_files: usize,
    /// Packages the module graph case spreads its files over.
    #[arg(long, default_value_t = 100)]
    graph_packages: usize,
    /// Exit 0 even when an ADR 0001 target check fails.
    #[arg(long)]
    no_enforce: bool,
    /// Exit with an error when a check of the type-aware budget of ADR 0029 fails: types on against types off on the
    /// corpus, peak RSS with types, the module graph case with types and the speed against knip.
    #[arg(long)]
    check_budget: bool,
    /// Install the dependencies of the corpus repositories (`--ignore-scripts`, as `parity --install`) before timing
    /// knip, which fails to load their tool configurations without them.
    #[arg(long)]
    install: bool,
}

fn generate_command(args: &GenerateArgs) -> Result<()> {
    if args.out.exists() {
        if args.force {
            fs::remove_dir_all(&args.out).with_context(|| format!("cleaning {}", args.out.display()))?;
        } else if fs::read_dir(&args.out)?.next().is_some() {
            bail!("{} is not empty (use --force to replace it)", args.out.display());
        }
    }
    let mut generated = generate::build(&Options {
        packages: args.packages,
        seed: args.seed,
        package_manager: args.pm,
        catalog: args.catalog,
        mismatch_rate: args.mismatch_rate,
    })?;
    sources::add_sources(&mut generated, args.source_files_per_package);
    generate::write(&generated, &args.out)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "out": args.out,
            "packages": generated.packages,
            "dependency-entries": generated.dependency_entries,
            "injected-issues": generated.issues,
            "source-files-per-package": args.source_files_per_package,
        }))?
    );
    Ok(())
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Generate(args) => generate_command(&args),
        Command::Corpus {
            action: CorpusAction::Fetch(args),
        } => {
            let suite = corpus::fetch_suite(&args.file, &args.suite, &args.only, &args.dir)?;
            for (repo, dir) in &suite.repos {
                println!("{} {} {}", repo.name, &repo.rev[..12], dir.display());
            }
            Ok(())
        }
        Command::Tools { action } => match action {
            ToolsAction::Update(args) => tools::update(
                &args.file,
                &tools::npm_latest,
                &tools::npm_relock,
                &mut std::io::stdout(),
            ),
            ToolsAction::Check(args) => tools::check(&args.file, &tools::npm_latest, &mut std::io::stdout()),
        },
        Command::Parity(args) => parity::run(&parity::Options {
            skif: args.skif,
            corpus_file: args.corpus_file,
            corpus_dir: args.corpus_dir,
            tools_dir: args.tools_dir,
            out: args.out,
            only: args.only,
            install: args.install,
            recursive: args.recursive,
            repo_knip_config: args.repo_knip_config,
            include_test_files: args.include_test_files,
            timeout: Duration::from_secs(args.timeout_secs),
            gate: args.gate.gate(args.baseline),
            untyped_pass: !args.no_untyped_pass,
            type_aware_baseline: args.type_aware_baseline,
            samples: args.samples,
        }),
        Command::SyncpackDiff(args) => syncpack::run(&syncpack::Options {
            skif: args.skif,
            corpus_file: args.corpus_file,
            corpus_dir: args.corpus_dir,
            tools_dir: args.tools_dir,
            fixtures: args.fixtures,
            out: args.out,
            only: args.only,
            no_fixtures: args.no_fixtures,
            no_corpus: args.no_corpus,
            timeout: Duration::from_secs(args.timeout_secs),
            gate: args.gate.gate(args.baseline),
        }),
        Command::Deadcode(args) => precision::run(&precision::Options {
            skif: args.skif,
            packages: args.packages,
            seed: args.seed,
            out: args.out,
            knip: args.knip,
            corpus_file: args.corpus_file,
            tools_dir: args.tools_dir,
            json: args.json,
            timeout: Duration::from_secs(args.timeout_secs),
            force: args.force,
        }),
        Command::TypedDiff(args) => typed_diff::run(&typed_diff::Options {
            reference: args.reference,
            skif: args.skif,
            corpus_file: args.corpus_file,
            corpus_dir: args.corpus_dir,
            only: args.only,
            allow: args.allow,
            out: args.out,
            scratch: args.scratch,
            timeout: Duration::from_secs(args.timeout_secs),
        }),
        Command::Run(args) => run::run(&RunOptions {
            skif: args.skif,
            skif_args: args.skif_args,
            sizes: args.sizes,
            package_manager: args.pm,
            corpus_file: args.corpus_file,
            corpus_dir: args.corpus_dir,
            use_corpus: !args.no_corpus,
            only: args.only,
            skif_source: args.skif_source,
            generated_dir: args.generated_dir,
            out: args.out,
            warmup: args.warmup,
            runs: args.runs,
            graph_files: args.graph_files,
            graph_packages: args.graph_packages,
            enforce: !args.no_enforce,
            check_budget: args.check_budget,
            install: args.install,
        }),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn the_command_line_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn run_narrows_the_corpus_and_names_the_skif_source() {
        let cli = Cli::try_parse_from([
            "skif-bench",
            "run",
            "--only",
            "babel,vue-core",
            "--skif-source",
            "../skif",
        ])
        .unwrap();
        let Command::Run(args) = cli.command else {
            panic!("not the run command");
        };
        assert_eq!(args.only, ["babel", "vue-core"]);
        assert_eq!(args.skif_source, Some(PathBuf::from("../skif")));
        assert!(Cli::try_parse_from(["skif-bench", "run", "--only", "babel", "--no-corpus"]).is_err());
    }

    #[test]
    fn parity_runs_the_untyped_pass_unless_told_not_to() {
        let parity = |args: &[&str]| {
            let cli = Cli::try_parse_from(["skif-bench", "parity"].iter().chain(args)).unwrap();
            let Command::Parity(args) = cli.command else {
                panic!("not the parity command");
            };
            args
        };
        let default = parity(&[]);
        assert!(!default.no_untyped_pass);
        assert_eq!(default.type_aware_baseline, PathBuf::from(DEFAULT_TYPE_AWARE_BASELINE));
        let skipped = parity(&["--no-untyped-pass", "--type-aware-baseline", "/tmp/t.json"]);
        assert!(skipped.no_untyped_pass);
        assert_eq!(skipped.type_aware_baseline, PathBuf::from("/tmp/t.json"));
    }
}
