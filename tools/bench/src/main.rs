//! Benchmark harness for skif: generates synthetic monorepos, fetches the pinned corpus and runs hyperfine.

mod corpus;
mod run;

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};

use skif_bench::generate::{self, Options, PackageManager};
use crate::run::RunOptions;

const DEFAULT_CORPUS_FILE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/corpus.toml");

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
    /// Benchmark skif (and any installed comparison tools) with hyperfine.
    Run(RunArgs),
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
}

#[derive(Subcommand)]
enum CorpusAction {
    /// Clone every repository of the corpus at its pinned commit (idempotent).
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
    #[arg(long, default_value_t = 10)]
    runs: u32,
}

fn generate_command(args: &GenerateArgs) -> Result<()> {
    if args.out.exists() {
        if args.force {
            fs::remove_dir_all(&args.out).with_context(|| format!("cleaning {}", args.out.display()))?;
        } else if fs::read_dir(&args.out)?.next().is_some() {
            bail!("{} is not empty (use --force to replace it)", args.out.display());
        }
    }
    let generated = generate::build(&Options {
        packages: args.packages,
        seed: args.seed,
        package_manager: args.pm,
        catalog: args.catalog,
        mismatch_rate: args.mismatch_rate,
    })?;
    generate::write(&generated, &args.out)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "out": args.out,
            "packages": generated.packages,
            "dependency-entries": generated.dependency_entries,
            "injected-issues": generated.issues,
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
            for (repo, dir) in corpus::fetch_all(&args.file, &args.dir)? {
                println!("{} {} {}", repo.name, &repo.rev[..12], dir.display());
            }
            Ok(())
        }
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
        }),
    }
}
