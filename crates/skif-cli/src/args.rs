//! The command line, as specified by ADR 0016.

use std::path::PathBuf;

use clap::{ColorChoice, Parser, Subcommand, ValueEnum};
use skif::Level;

/// Fast dependency linter for JavaScript and TypeScript monorepos.
#[derive(Debug, Parser)]
#[command(
    name = "skif",
    bin_name = "skif",
    version,
    about,
    args_conflicts_with_subcommands = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
    #[command(flatten)]
    pub lint: LintArgs,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// List every rule with its category, default level and fixability.
    Rules(RulesArgs),
    /// Print the JSON Schema of `skif.toml`.
    Schema,
    /// Write a starter `skif.toml`; refuses to overwrite an existing configuration.
    Init(InitArgs),
    /// Show the configuration files a root is built from, the merged result, and where each key was set.
    Config(ConfigArgs),
    /// Explain why the JavaScript analysis counts a file, or one export of it, as used.
    ExplainUsage(ExplainUsageArgs),
    /// Translate the configuration of sherif, syncpack and knip into a `skif.toml`; never deletes a file.
    Migrate(MigrateArgs),
    /// List every dependency the version groups govern, by version group, with its specifiers and states.
    List(ListArgs),
    /// Print every dependency instance the version groups govern as one JSON object per line (NDJSON).
    Json(InstancesArgs),
}

/// What `skif list` and `skif json` read.
#[derive(Debug, clap::Args)]
pub struct InstancesArgs {
    /// A path inside a project, resolved to its workspace root as for linting.
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Cover every workspace and standalone package at or below PATH, each with its own configuration.
    #[arg(short, long)]
    pub recursive: bool,

    /// Use this configuration file (and the files it extends) instead of discovering `skif.toml` files.
    #[arg(long, value_name = "FILE", conflicts_with = "no_config")]
    pub config: Option<PathBuf>,

    /// Read no configuration file: the defaults plus the flags.
    #[arg(long)]
    pub no_config: bool,

    #[command(flatten)]
    pub filter: FilterArgs,
}

/// syncpack's instance filters: what the version-group policies see in this run.
#[derive(Debug, Default, clap::Args)]
pub struct FilterArgs {
    /// Only the dependencies whose name matches this name or glob (`react`, `@types/**`, `!eslint`), in the
    /// version-group and semver-group rules only. Repeatable. syncpack's `--dependencies`.
    // Not `--dependencies`: that is knip's issue-type shortcut on the same command.
    #[arg(long = "dependency", value_name = "NAME")]
    pub dependencies: Vec<String>,

    /// Only these dependency types (`prod,dev`, `peer`, `!local`, a custom type), in the version-group and
    /// semver-group rules only; also their default scope for this run. Comma separated.
    #[arg(long, value_name = "TYPES", value_delimiter = ',')]
    pub dependency_types: Vec<String>,

    /// Only instances with these specifier types (`exact,range`, `!workspace-protocol`), in the version-group
    /// and semver-group rules only. Comma separated.
    #[arg(long, value_name = "TYPES", value_delimiter = ',')]
    pub specifier_types: Vec<String>,

    /// Only instances declared in files matching this glob, relative to the root (`packages/app/package.json`), in
    /// the version-group and semver-group rules only. Repeatable.
    #[arg(long, value_name = "GLOB")]
    pub source: Vec<String>,
}

#[derive(Debug, clap::Args)]
pub struct ListArgs {
    #[command(flatten)]
    pub instances: InstancesArgs,

    /// What to show, comma separated: `instances` (every instance of each dependency), `statuses` (the state names),
    /// `hints` (mark dependencies that are workspace packages), `ignored` (the dependencies of `ignored` groups),
    /// `all` or `none`.
    #[arg(long, value_enum, value_delimiter = ',', default_value = "hints,statuses")]
    pub show: Vec<Show>,
}

/// The details `skif list --show` turns on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Show {
    /// Every instance of every dependency.
    Instances,
    /// The state of each dependency and instance.
    Statuses,
    /// A mark on dependencies that are packages of the workspace.
    Hints,
    /// The dependencies of `ignored` version groups.
    Ignored,
    /// Everything above.
    All,
    /// Nothing above.
    None,
}

#[derive(Debug, clap::Args)]
pub struct RulesArgs {
    /// Output format.
    #[arg(long, value_enum, default_value_t = RulesFormat::Human)]
    pub format: RulesFormat,
}

/// Output format of `skif rules` and `skif config`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
pub enum RulesFormat {
    /// An aligned table.
    #[default]
    Human,
    /// One JSON document on stdout.
    Json,
}

impl From<RulesFormat> for Format {
    fn from(format: RulesFormat) -> Self {
        match format {
            RulesFormat::Human => Self::Human,
            RulesFormat::Json => Self::Json,
        }
    }
}

#[derive(Debug, clap::Args)]
pub struct InitArgs {
    /// Directory to write `skif.toml` into.
    #[arg(default_value = ".")]
    pub path: PathBuf,
}

#[derive(Debug, clap::Args)]
pub struct ExplainUsageArgs {
    /// A file, or `FILE#EXPORT` for one of its exports, relative to the current directory (or absolute).
    pub target: String,

    /// A path inside a project, resolved to its workspace root as for linting.
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Explain the production-only analysis, as `--production` does for a lint run.
    #[arg(long)]
    pub production: bool,

    /// Use this configuration file (and the files it extends) instead of discovering `skif.toml` files.
    #[arg(long, value_name = "FILE", conflicts_with = "no_config")]
    pub config: Option<PathBuf>,

    /// Read no configuration file: the defaults plus the flags.
    #[arg(long)]
    pub no_config: bool,

    /// Output format.
    #[arg(long, value_enum, default_value_t = RulesFormat::Human)]
    pub format: RulesFormat,
}

/// The tools `skif migrate --from` accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum MigrateFrom {
    /// The `sherif` key of `package.json` and the flags of the scripts that run sherif.
    Sherif,
    /// `.syncpackrc*`, `syncpack.config.*`, the `syncpack` key of `package.json` and the scripts that run syncpack.
    Syncpack,
    /// `knip.json(c)`, `knip.{js,ts,...}`, the `knip` key of `package.json` and the scripts that run knip.
    Knip,
    /// Every tool that has a configuration file, a `package.json` key or a script in the project.
    Auto,
}

/// Which checks the migrated configuration keeps.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
pub enum MigrateProfile {
    /// Only the checks the old tools ran: the rules skif enables by default beyond them are turned off.
    #[default]
    Exact,
    /// The checks of the old tools plus skif's defaults for everything else.
    Full,
}

#[derive(Debug, clap::Args)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "each bool is an independent command-line switch"
)]
pub struct MigrateArgs {
    /// The project to migrate: the directory with the root `package.json`.
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// The tools to migrate, comma separated.
    #[arg(long, value_enum, value_delimiter = ',', default_value = "auto")]
    pub from: Vec<MigrateFrom>,

    /// Read this configuration file instead of looking for one; needs exactly one tool in `--from`.
    #[arg(long, value_name = "FILE")]
    pub source: Option<PathBuf>,

    /// Which checks to keep.
    #[arg(long, value_enum, default_value_t = MigrateProfile::Exact)]
    pub profile: MigrateProfile,

    /// Write the configuration here instead of `PATH/skif.toml`; `-` prints it to stdout.
    #[arg(long, value_name = "FILE")]
    pub output: Option<PathBuf>,

    /// Combine with the existing `skif.toml` instead of refusing to overwrite it.
    #[arg(long, conflicts_with = "force")]
    pub merge: bool,

    /// Replace an existing `skif.toml`.
    #[arg(long)]
    pub force: bool,

    /// Print the configuration to stdout and the report to stderr; write nothing.
    #[arg(long)]
    pub dry_run: bool,

    /// Output format of the report. With `json`, stdout holds exactly one document (and the configuration when
    /// nothing is written).
    #[arg(long, value_enum, default_value_t = RulesFormat::Human)]
    pub format: RulesFormat,

    /// Exit 1 when any construct needs a human.
    #[arg(long)]
    pub strict: bool,

    /// Write `stale-js-config = "error"` for knip's `treatConfigHintsAsErrors`; by default it is written as `warn`,
    /// because skif's configuration hints differ from knip's.
    #[arg(long)]
    pub strict_hints: bool,
}

#[derive(Debug, clap::Args)]
pub struct ConfigArgs {
    /// A path inside a project, resolved to its root as for linting.
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Report every workspace and standalone package at or below PATH, each with its own configuration.
    #[arg(short, long)]
    pub recursive: bool,

    /// Print the configuration files in the order they apply, then the effective configuration.
    #[arg(long, conflicts_with = "explain", required_unless_present = "explain")]
    pub show: bool,

    /// Print every assignment of a configuration key (`lint.fail-on-warnings`, `rules.ID.level`, `version-groups[0]`)
    /// and its final value.
    #[arg(long, value_name = "KEY")]
    pub explain: Option<String>,

    /// With --explain, also list the overrides that apply to this package.
    #[arg(long, value_name = "NAME", requires = "explain", conflicts_with = "show")]
    pub package: Option<String>,

    /// Output format.
    #[arg(long, value_enum, default_value_t = RulesFormat::Human)]
    pub format: RulesFormat,
}

/// Output format.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
pub enum Format {
    /// Grouped, optionally coloured text.
    #[default]
    Human,
    /// One JSON document on stdout.
    Json,
    /// SARIF 2.1.0 for code scanning.
    Sarif,
    /// GitHub Actions workflow-command annotations.
    Github,
}

/// Which version a version alignment fix targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Select {
    Highest,
    Lowest,
}

#[derive(Debug, clap::Args)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "each bool is an independent command-line switch"
)]
pub struct LintArgs {
    /// A path inside a project; the workspace root is the nearest ancestor workspace that lists the project as a
    /// member, else the project itself. Without a `package.json` here or above, every project below is linted.
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Lint every workspace and standalone package at or below PATH, each with the `skif.toml` files that govern it.
    #[arg(short, long)]
    pub recursive: bool,

    /// Output format [default: human, or human plus GitHub annotations when `GITHUB_ACTIONS=true`].
    #[arg(long, value_enum)]
    pub format: Option<Format>,

    /// Write the rendering to this file instead of stdout.
    #[arg(long, value_name = "FILE", requires = "format")]
    pub output: Option<PathBuf>,

    /// Apply safe fixes, then lint again and run the package manager's install command.
    #[arg(long)]
    pub fix: bool,

    /// Apply safe and unsafe fixes; implies --fix.
    #[arg(long)]
    pub fix_unsafe: bool,

    /// With --fix or --fix-unsafe: print the planned changes as a unified diff (JSON: the `fix` object with
    /// `changes`); no manifest is written and no install runs. An --output report file is still written.
    #[arg(long)]
    pub dry_run: bool,

    /// Which version a version alignment fix targets, overriding `[policy] prefer` for this run.
    #[arg(long, value_enum)]
    pub select: Option<Select>,

    /// Do not run the package manager after fixing.
    #[arg(long)]
    pub no_install: bool,

    /// Exit 1 when there are more than N warnings (counted over all roots).
    #[arg(long, value_name = "N")]
    pub max_warnings: Option<usize>,

    /// Same as --max-warnings 0.
    #[arg(long, conflicts_with = "max_warnings")]
    pub fail_on_warnings: bool,

    /// Never ask which alternative fix to apply; the diagnostic stays unfixed unless --select decides.
    #[arg(long)]
    pub no_prompt: bool,

    /// Set a rule's level, as ID=off|info|warn|error. Repeatable.
    #[arg(long = "rule", value_name = "ID=LEVEL", value_parser = parse_rule_level)]
    pub rules: Vec<(String, Level)>,

    /// Turn a rule off. Repeatable.
    #[arg(long, value_name = "ID")]
    pub ignore_rule: Vec<String>,

    /// Ignore a package by name or glob in every rule; replaces the file's list. Repeatable.
    #[arg(long, value_name = "NAME")]
    pub ignore_package: Vec<String>,

    /// Ignore a dependency in every rule. Repeatable.
    #[arg(long, value_name = "NAME")]
    pub ignore_dependency: Vec<String>,

    /// Ignore one rule for one package, as RULE:PACKAGE. Repeatable.
    #[arg(long, value_name = "RULE:PACKAGE", value_parser = parse_rule_package)]
    pub ignore_rule_for_package: Vec<(String, String)>,

    /// Fail when an `[[ignore]]` entry of a configuration file has no `reason`.
    #[arg(long)]
    pub require_ignore_reasons: bool,

    /// Skip JavaScript analysis.
    #[arg(long)]
    pub no_js: bool,

    /// Use this configuration file (and the files it extends) instead of discovering `skif.toml` files.
    #[arg(long, value_name = "FILE", conflicts_with = "no_config")]
    pub config: Option<PathBuf>,

    /// Read no configuration file: the defaults plus the flags.
    #[arg(long)]
    pub no_config: bool,

    /// Only analyse production code in JavaScript analysis.
    #[arg(long)]
    pub production: bool,

    /// When to colour human output.
    #[arg(long, value_enum, default_value_t = ColorChoice::Auto)]
    pub color: ColorChoice,

    #[command(flatten)]
    pub filter: FilterArgs,
}

impl LintArgs {
    /// Whether any fixing flag was given.
    pub const fn fixing(&self) -> bool {
        self.fix || self.fix_unsafe
    }

    /// Whether fixes are planned and shown but not written.
    pub const fn dry_run(&self) -> bool {
        self.dry_run && self.fixing()
    }

    pub fn format(&self) -> Format {
        self.format.unwrap_or_default()
    }
}

fn parse_rule_level(value: &str) -> Result<(String, Level), String> {
    let (rule, level) = value
        .split_once('=')
        .filter(|(rule, _)| !rule.is_empty())
        .ok_or_else(|| "expected `ID=LEVEL`, for example `unsorted-dependencies=off`".to_owned())?;
    let level = match level {
        "off" => Level::Off,
        "info" => Level::Info,
        "warn" => Level::Warn,
        "error" => Level::Error,
        other => return Err(format!("unknown level `{other}`; expected off, info, warn or error")),
    };
    Ok((rule.to_owned(), level))
}

fn parse_rule_package(value: &str) -> Result<(String, String), String> {
    match value.split_once(':') {
        Some((rule, package)) if !rule.is_empty() && !package.is_empty() => Ok((rule.to_owned(), package.to_owned())),
        _ => Err("expected `RULE:PACKAGE`".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_levels_parse() {
        assert_eq!(parse_rule_level("a-b=off"), Ok(("a-b".to_owned(), Level::Off)));
        assert_eq!(parse_rule_level("a-b=info"), Ok(("a-b".to_owned(), Level::Info)));
        assert!(parse_rule_level("a-b=loud").is_err());
        assert!(parse_rule_level("=warn").is_err());
        assert!(parse_rule_level("nolevel").is_err());
    }

    #[test]
    fn rule_package_pairs_parse() {
        assert_eq!(
            parse_rule_package("r:@scope/p"),
            Ok(("r".to_owned(), "@scope/p".to_owned()))
        );
        assert!(parse_rule_package("r:").is_err());
        assert!(parse_rule_package("r").is_err());
    }

    #[test]
    fn command_line_is_consistent() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }

    fn long_flags(command: &clap::Command, flags: &mut std::collections::BTreeSet<String>) {
        flags.extend(
            command
                .get_arguments()
                .filter_map(|arg| arg.get_long().map(str::to_owned)),
        );
        for subcommand in command.get_subcommands() {
            long_flags(subcommand, flags);
        }
    }

    /// Whether `text` names `--flag` as a whole word, not as the beginning of a longer flag.
    fn mentions(text: &str, flag: &str) -> bool {
        let needle = format!("--{flag}");
        text.match_indices(&needle).any(|(start, _)| {
            !text[start + needle.len()..]
                .chars()
                .next()
                .is_some_and(|next| next.is_ascii_alphanumeric() || next == '-' || next == '_')
        })
    }

    #[test]
    fn a_flag_is_not_mentioned_by_a_longer_flag() {
        assert!(mentions("use `--fix` here", "fix"));
        assert!(mentions("--fix", "fix"));
        assert!(mentions("--fix-unsafe and --fix.", "fix"));
        assert!(!mentions("only --fix-unsafe", "fix"));
        assert!(!mentions("--ignore-rule-for-package", "ignore-rule"));
        assert!(!mentions("--fixture", "fix"));
    }

    /// The CLI reference is written by hand; this fails when a flag is added to the command line and not to it.
    #[test]
    fn every_long_flag_is_in_the_cli_reference() {
        use clap::CommandFactory;
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        // A source tarball of this crate holds neither the website nor the other crates.
        if !root.join("crates/skif-core/src").is_dir() {
            return;
        }
        let reference = std::fs::read_to_string(root.join("website/src/content/docs/reference/cli.mdx")).unwrap();
        let mut flags = std::collections::BTreeSet::new();
        long_flags(&Cli::command(), &mut flags);
        flags.remove("help");
        flags.remove("version");
        let missing: Vec<&String> = flags.iter().filter(|flag| !mentions(&reference, flag)).collect();
        assert_eq!(missing, Vec::<&String>::new(), "flags missing from reference/cli.mdx");
    }
}
