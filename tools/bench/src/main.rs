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
const DEFAULT_SYNCPACK_BASELINE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/baselines/syncpack.json");
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
    /// Benchmark skif and the pinned comparison tools with hyperfine.
    Run(RunArgs),
    /// Compare skif's findings with a pinned knip on the corpus.
    Parity(ParityArgs),
    /// Compare skif's policy findings with a pinned syncpack on the policy fixtures and the corpus.
    SyncpackDiff(SyncpackArgs),
    /// Measure dead-code precision and recall of skif (and the pinned knip) against a generated answer key.
    Deadcode(DeadcodeArgs),
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
    /// Also run the knip version pinned in the corpus manifest (needs npx and the npm registry).
    #[arg(long)]
    knip: bool,
    /// Corpus manifest (for the pinned knip version).
    #[arg(long, default_value = DEFAULT_CORPUS_FILE)]
    corpus_file: PathBuf,
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
        Command::Parity(args) => parity::run(&parity::Options {
            skif: args.skif,
            corpus_file: args.corpus_file,
            corpus_dir: args.corpus_dir,
            out: args.out,
            only: args.only,
            install: args.install,
            recursive: args.recursive,
            repo_knip_config: args.repo_knip_config,
            include_test_files: args.include_test_files,
            timeout: Duration::from_secs(args.timeout_secs),
            gate: args.gate.gate(args.baseline),
        }),
        Command::SyncpackDiff(args) => syncpack::run(&syncpack::Options {
            skif: args.skif,
            corpus_file: args.corpus_file,
            corpus_dir: args.corpus_dir,
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
            json: args.json,
            timeout: Duration::from_secs(args.timeout_secs),
            force: args.force,
        }),
        Command::Run(args) => run::run(&RunOptions {
            skif: args.skif,
            skif_args: args.skif_args,
            sizes: args.sizes,
            package_manager: args.pm,
            corpus_file: args.corpus_file,
            corpus_dir: args.corpus_dir,
            use_corpus: !args.no_corpus,
            generated_dir: args.generated_dir,
            out: args.out,
            warmup: args.warmup,
            runs: args.runs,
            graph_files: args.graph_files,
            graph_packages: args.graph_packages,
            enforce: !args.no_enforce,
            install: args.install,
        }),
    }
}
