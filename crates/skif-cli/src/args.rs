//! The command line, as specified by ADR 0016.

use std::path::{Component, Path, PathBuf};

use clap::{ArgGroup, ColorChoice, Parser, Subcommand, ValueEnum};
use skif::Level;

use crate::error::{CliError, Kind};

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
    /// Update dependencies to newer published versions from the npm registry; the only command that uses the network.
    Update(UpdateArgs),
    /// List every dependency the version groups govern, by version group, with its specifiers and states.
    List(ListArgs),
    /// Print every dependency instance the version groups govern as one JSON object per line (NDJSON).
    Json(InstancesArgs),
    /// Format every `package.json` as `[format]` describes (the `package-json-format` rule alone); never installs.
    Format(FormatArgs),
    /// Run a Model Context Protocol server on stdin and stdout, for AI agents.
    Mcp,
    /// Run a Language Server Protocol server on stdin and stdout: diagnostics on open and save.
    Lsp,
}

/// How far `skif update` may move a version.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
pub enum Target {
    /// The highest published version.
    #[default]
    Latest,
    /// The highest version with the same major.
    Minor,
    /// The highest version with the same major and minor.
    Patch,
}

#[derive(Debug, clap::Args)]
pub struct UpdateArgs {
    /// A path inside a project, resolved to its workspace root as for linting.
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Write nothing; exit 1 when an update is available.
    #[arg(long)]
    pub check: bool,

    /// How far a version may move; an `[[update-groups]]` entry can narrow it further.
    #[arg(long, value_enum, default_value_t = Target::Latest)]
    pub target: Target,

    /// Only these dependencies, by name or glob. Repeatable.
    #[arg(long = "dependencies", value_name = "NAME")]
    pub dependencies: Vec<String>,

    /// Do not run the package manager after writing.
    #[arg(long)]
    pub no_install: bool,

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
    #[arg(id = "dependency", long = "dependency", value_name = "NAME")]
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
#[allow(
    clippy::struct_excessive_bools,
    reason = "each bool is an independent command-line switch"
)]
pub struct FormatArgs {
    /// A path inside a project, resolved to its workspace root as for linting.
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Report the manifests that are not formatted and exit 1, without writing.
    #[arg(long)]
    pub check: bool,

    /// Also write the unsafe change: a `sort-exports` longer than `types` and `default` reordering other conditions.
    #[arg(long = "unsafe", conflicts_with = "check")]
    pub allow_unsafe: bool,

    /// Format every workspace and standalone package at or below PATH.
    #[arg(short, long)]
    pub recursive: bool,

    /// Output format [default: human, or human plus GitHub annotations when `GITHUB_ACTIONS=true`].
    #[arg(long, value_enum)]
    pub format: Option<Format>,

    /// Use this configuration file (and the files it extends) instead of discovering `skif.toml` files.
    #[arg(long, value_name = "FILE", conflicts_with = "no_config")]
    pub config: Option<PathBuf>,

    /// Read no configuration file: the defaults plus the flags.
    #[arg(long)]
    pub no_config: bool,

    /// When to colour human output.
    #[arg(long, value_enum, default_value_t = ColorChoice::Auto)]
    pub color: ColorChoice,
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
    /// Code Climate JSON for GitLab Code Quality.
    Codeclimate,
    /// One `path:line:col rule message` line per diagnostic.
    Compact,
    /// Markdown tables per rule, for pull request comments.
    Markdown,
    /// Markdown tables per rule, each in a collapsed `<details>` section.
    Disclosure,
    /// Compact lines grouped by owner, from the repository's CODEOWNERS file.
    Codeowners,
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
#[command(group(ArgGroup::new("fixing").args(["fix", "fix_unsafe"]).multiple(true)))]
pub struct LintArgs {
    /// A path inside a project; the workspace root is the nearest ancestor workspace that lists the project as a
    /// member, else the project itself. Without a `package.json` here or above, every project below is linted.
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Lint every workspace and standalone package at or below PATH, each with the `skif.toml` files that govern it.
    #[arg(short, long)]
    pub recursive: bool,

    /// Output format, as FORMAT or FORMAT=FILE to write that rendering to FILE. Repeatable; at most one goes to
    /// stdout [default: human, or human plus GitHub annotations when `GITHUB_ACTIONS=true`].
    #[arg(long, value_name = "FORMAT[=FILE]", value_parser = FormatSpecParser)]
    pub format: Vec<FormatSpec>,

    /// Write the rendering to this file instead of stdout; needs exactly one --format, without `=FILE`.
    #[arg(long, value_name = "FILE", requires = "format")]
    pub output: Option<PathBuf>,

    /// Apply safe fixes, then lint again and run the package manager's install command.
    #[arg(long)]
    pub fix: bool,

    /// Apply safe and unsafe fixes; implies --fix.
    #[arg(long)]
    pub fix_unsafe: bool,

    /// With --fix or --fix-unsafe: print the planned changes as a unified diff (JSON: the `fix` object with
    /// `changes`); no manifest is written and no install runs. Reports still go to stdout, --output and FORMAT=FILE.
    #[arg(long)]
    pub dry_run: bool,

    /// Let fixes delete files (`unused-files`); without it they are skipped. Needs --fix or --fix-unsafe.
    #[arg(long, requires = "fixing")]
    pub allow_remove_files: bool,

    /// Only apply the fixes of these rules: rule ids or knip issue types (`dependencies`, `exports`, `types`,
    /// `enumMembers`, `files`, `catalog`, ...). Comma separated or repeatable; needs --fix or --fix-unsafe.
    #[arg(long, value_name = "TYPE", value_delimiter = ',', requires = "fixing")]
    pub fix_type: Vec<String>,

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

    /// Exit 1 only when there are more than N errors (counted over all roots); warnings still follow --max-warnings.
    #[arg(long, value_name = "N")]
    pub max_issues: Option<usize>,

    /// Show at most N findings of each rule in human and json output (not with --fix); the summary still counts all
    /// of them, and the exit code is unchanged.
    #[arg(long, value_name = "N")]
    pub max_show_issues: Option<usize>,

    /// Exit 0 when the run finds problems; exit 2 when skif could not run is kept.
    #[arg(long)]
    pub no_exit_code: bool,

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

    /// Also analyse the source files that `.gitignore` files hide (the git index is not used to list files then).
    #[arg(long)]
    pub no_gitignore: bool,

    /// Report unused exports of entry files too, overriding `[js] include-entry-exports`.
    #[arg(long)]
    pub include_entry_exports: bool,

    /// The TypeScript configuration of the root package, relative to the workspace root, used instead of its
    /// `tsconfig.json` (overrides `[js] tsconfig`).
    #[arg(short = 't', long = "tsconfig", visible_alias = "tsConfig", value_name = "FILE")]
    pub tsconfig: Option<String>,

    /// Take the project files of each package from its `tsconfig.json` (`files`, `include`, `exclude`) instead of the
    /// `project` globs.
    #[arg(long)]
    pub use_tsconfig_files: bool,

    /// knip's strict mode: --production, and each workspace package uses only the dependencies its own manifest
    /// lists, not those of a containing package or the root.
    #[arg(long)]
    pub strict: bool,

    /// When to colour human output.
    #[arg(long, value_enum, default_value_t = ColorChoice::Auto)]
    pub color: ColorChoice,

    /// Lint again whenever a file below the root changes, until interrupted.
    #[arg(long, conflicts_with_all = ["fix", "fix_unsafe"])]
    pub watch: bool,

    /// Instead of linting, print where this dependency is listed, every import of it and the packages that count it
    /// as used.
    #[arg(
        long,
        value_name = "NAME",
        conflicts_with_all = [
            "fix", "fix_unsafe", "workspace", "include", "exclude", "dependencies", "exports", "files", "cycles",
            "tags", "max_show_issues", "rules", "ignore_rule",
        ]
    )]
    pub trace_dependency: Option<String>,

    /// Print what the run loaded (roots, configuration files, packages) and the phase timings to stderr.
    #[arg(long)]
    pub debug: bool,

    /// Print the time each phase of the run took to stderr, as `SKIF_TIMINGS=1` does.
    #[arg(long)]
    pub performance: bool,

    /// Print the wall time of the run to stderr.
    #[arg(long)]
    pub duration: bool,

    /// Print the peak memory (resident set size) of the run to stderr; Linux only.
    #[arg(long)]
    pub memory: bool,

    /// Report only the findings of these workspace packages, by name or directory relative to the root (globs, `!`
    /// to exclude). The analysis still covers every package. Repeatable.
    #[arg(short = 'W', long, value_name = "NAME|DIR")]
    pub workspace: Vec<String>,

    /// Report only these knip issue types (`files`, `dependencies`, `exports`, ...) or skif rule ids; a rule that is
    /// off (by default or in the configuration) is turned on. Comma separated, repeatable.
    #[arg(long, value_name = "TYPE|RULE", value_delimiter = ',')]
    pub include: Vec<String>,

    /// Do not report these knip issue types or skif rule ids. Comma separated, repeatable.
    #[arg(long, value_name = "TYPE|RULE", value_delimiter = ',')]
    pub exclude: Vec<String>,

    /// Shortcut for `--include` of knip's dependency issue types.
    #[arg(long)]
    pub dependencies: bool,

    /// Shortcut for `--include` of knip's export issue types.
    #[arg(long)]
    pub exports: bool,

    /// Shortcut for `--include files`.
    #[arg(long)]
    pub files: bool,

    /// Shortcut for `--include cycles`.
    #[arg(long)]
    pub cycles: bool,

    /// Do not report exports tagged with these `JSDoc` tags, written `-TAG` as in knip. Comma separated, repeatable.
    #[arg(long, value_name = "-TAG", value_delimiter = ',', allow_hyphen_values = true)]
    pub tags: Vec<String>,

    #[command(flatten)]
    pub filter: FilterArgs,
}

/// The arguments of a bare `skif`: every flag at its command-line default. A command that lints on its own behalf
/// sets the fields it needs and takes the rest from here (`LintArgs { path, ..LintArgs::default() }`), so a flag
/// added later does not break it.
impl Default for LintArgs {
    fn default() -> Self {
        Self {
            path: PathBuf::from("."),
            recursive: false,
            format: Vec::new(),
            output: None,
            fix: false,
            fix_unsafe: false,
            dry_run: false,
            allow_remove_files: false,
            fix_type: Vec::new(),
            select: None,
            no_install: false,
            max_warnings: None,
            fail_on_warnings: false,
            max_issues: None,
            max_show_issues: None,
            no_exit_code: false,
            no_prompt: false,
            rules: Vec::new(),
            ignore_rule: Vec::new(),
            ignore_package: Vec::new(),
            ignore_dependency: Vec::new(),
            ignore_rule_for_package: Vec::new(),
            require_ignore_reasons: false,
            no_js: false,
            config: None,
            no_config: false,
            production: false,
            no_gitignore: false,
            include_entry_exports: false,
            tsconfig: None,
            use_tsconfig_files: false,
            strict: false,
            color: ColorChoice::Auto,
            watch: false,
            trace_dependency: None,
            debug: false,
            performance: false,
            duration: false,
            memory: false,
            workspace: Vec::new(),
            include: Vec::new(),
            exclude: Vec::new(),
            dependencies: false,
            exports: false,
            files: false,
            cycles: false,
            tags: Vec::new(),
            filter: FilterArgs::default(),
        }
    }
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

    /// The format that decides the run's behaviour (JSON-mode errors, colour, prompts, where the fix summary goes):
    /// the one written to stdout, else the first one given, else `human` (ADR 0054).
    pub fn format(&self) -> Format {
        self.format
            .iter()
            .find(|spec| spec.file.is_none())
            .or_else(|| self.format.first())
            .map_or_else(Format::default, |spec| spec.format)
    }

    /// Where each rendering goes, in the order given: `None` is stdout. Fails, as a usage error, when two
    /// renderings would go to stdout, two to the same file (however it is spelled), a `FORMAT=FILE` names an
    /// existing directory, or `--output` is combined with several formats or with `FORMAT=FILE`.
    pub fn targets(&self) -> Result<Vec<(Format, Option<PathBuf>)>, CliError> {
        if let Some(output) = &self.output {
            return match self.format.as_slice() {
                [FormatSpec { format, file: None }] => Ok(vec![(*format, Some(output.clone()))]),
                _ => Err(CliError::new(
                    Kind::Usage,
                    "--output takes exactly one --format without `=FILE`; to write several formats, give each its \
                     own file as --format FORMAT=FILE",
                )),
            };
        }
        if self.format.is_empty() {
            return Ok(vec![(Format::default(), None)]);
        }
        if self.format.iter().filter(|spec| spec.file.is_none()).count() > 1 {
            return Err(CliError::new(
                Kind::Usage,
                "at most one --format can go to stdout; write the others to files with --format FORMAT=FILE",
            ));
        }
        let mut files = std::collections::BTreeSet::new();
        for file in self.format.iter().filter_map(|spec| spec.file.as_ref()) {
            if file.is_dir() {
                return Err(CliError::new(
                    Kind::Usage,
                    format!("--format names {}, which is a directory, not a file", file.display()),
                ));
            }
            if !files.insert(file_identity(file)) {
                return Err(CliError::new(
                    Kind::Usage,
                    format!("--format names the file {} twice", file.display()),
                ));
            }
        }
        Ok(self
            .format
            .iter()
            .map(|spec| (spec.format, spec.file.clone()))
            .collect())
    }

    /// The files the run itself writes (`--output` and every `FORMAT=FILE` target), each spelled as
    /// [`file_identity`] resolves it, so `--watch` can leave them out of its scan instead of reacting to its own
    /// writes.
    pub fn written_files(&self) -> std::collections::BTreeSet<PathBuf> {
        self.output
            .iter()
            .chain(self.format.iter().filter_map(|spec| spec.file.as_ref()))
            .map(|file| file_identity(file))
            .collect()
    }
}

/// A key that is equal for two spellings of the same output file: the path made absolute with `.` and `..` resolved
/// lexically, and its parent directory canonicalised when it exists, so `a/../x`, `./x` and a symlinked parent all
/// name `x`. The parent is resolved as the OS would first; the lexical form is the fallback for a parent that does
/// not exist (yet).
fn file_identity(file: &Path) -> PathBuf {
    let absolute = std::path::absolute(file).unwrap_or_else(|_| file.to_path_buf());
    let canonical_parent = |path: &Path| {
        let name = path.file_name()?;
        let parent = path.parent()?.canonicalize().ok()?;
        Some(parent.join(name))
    };
    canonical_parent(&absolute).unwrap_or_else(|| {
        let lexical = lexical_normal(&absolute);
        canonical_parent(&lexical).unwrap_or(lexical)
    })
}

/// `path` with `.` dropped and each `..` removing the component before it, without asking the file system.
fn lexical_normal(path: &Path) -> PathBuf {
    let mut normal = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                // `..` above the root is the root; above the start of a relative path it stays.
                if !normal.pop() && !normal.has_root() {
                    normal.push(component);
                }
            }
            other => normal.push(other),
        }
    }
    normal
}

/// One `--format` value: a format, and the file its rendering goes to instead of stdout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatSpec {
    pub format: Format,
    pub file: Option<PathBuf>,
}

impl FormatSpec {
    /// Parses `FORMAT` or `FORMAT=FILE`; the file is everything after the first `=`.
    pub fn parse(value: &str) -> Result<Self, String> {
        let (name, file) = match value.split_once('=') {
            Some((_, "")) => {
                return Err("expected a file name after `=`, as in `codeclimate=gl-code-quality.json`".to_owned());
            }
            Some((name, file)) => (name, Some(PathBuf::from(file))),
            None => (value, None),
        };
        let format = Format::from_str(name, false).map_err(|_| {
            let names: Vec<String> = Format::value_variants()
                .iter()
                .filter_map(|format| format.to_possible_value().map(|value| value.get_name().to_owned()))
                .collect();
            format!("unknown format `{name}`; expected one of {}", names.join(", "))
        })?;
        Ok(Self { format, file })
    }
}

/// The value parser of `--format`, which keeps the format list in `--help`.
#[derive(Debug, Clone, Copy)]
struct FormatSpecParser;

impl clap::builder::TypedValueParser for FormatSpecParser {
    type Value = FormatSpec;

    fn parse_ref(
        &self,
        cmd: &clap::Command,
        arg: Option<&clap::Arg>,
        value: &std::ffi::OsStr,
    ) -> Result<Self::Value, clap::Error> {
        let text = value
            .to_str()
            .ok_or_else(|| clap::Error::new(clap::error::ErrorKind::InvalidUtf8).with_cmd(cmd))?;
        FormatSpec::parse(text).map_err(|message| {
            let arg = arg.map_or_else(|| "--format".to_owned(), ToString::to_string);
            clap::Error::raw(
                clap::error::ErrorKind::ValueValidation,
                format!("invalid value '{text}' for '{arg}': {message}\n"),
            )
            .with_cmd(cmd)
        })
    }

    fn possible_values(&self) -> Option<Box<dyn Iterator<Item = clap::builder::PossibleValue> + '_>> {
        Some(Box::new(
            Format::value_variants().iter().filter_map(ValueEnum::to_possible_value),
        ))
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
    fn the_default_lint_args_are_those_of_a_bare_run() {
        let parsed = Cli::try_parse_from(["skif"]).unwrap().lint;
        assert_eq!(format!("{:?}", LintArgs::default()), format!("{parsed:?}"));
    }

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
    fn format_specs_parse() {
        assert_eq!(
            FormatSpec::parse("json"),
            Ok(FormatSpec {
                format: Format::Json,
                file: None
            })
        );
        assert_eq!(
            FormatSpec::parse("codeclimate=out/gl=cq.json"),
            Ok(FormatSpec {
                format: Format::Codeclimate,
                file: Some(PathBuf::from("out/gl=cq.json"))
            })
        );
        assert!(FormatSpec::parse("json=").is_err());
        assert!(FormatSpec::parse("yaml=x").is_err());
    }

    #[test]
    fn written_files_name_the_output_and_every_format_file_canonically() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let sarif = dir.path().join("sub").join("..").join("a.sarif");
        let parse = |flags: Vec<String>| {
            Cli::try_parse_from(std::iter::once("skif".to_owned()).chain(flags))
                .unwrap()
                .lint
        };
        let several = parse(vec![
            "--format".to_owned(),
            "json".to_owned(),
            "--format".to_owned(),
            format!("sarif={}", sarif.display()),
        ]);
        assert_eq!(
            several.written_files(),
            std::collections::BTreeSet::from([root.join("a.sarif")])
        );
        let output = parse(vec![
            "--format".to_owned(),
            "json".to_owned(),
            "--output".to_owned(),
            root.join("r.json").display().to_string(),
        ]);
        assert_eq!(
            output.written_files(),
            std::collections::BTreeSet::from([root.join("r.json")])
        );
        assert!(parse(Vec::new()).written_files().is_empty());
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
