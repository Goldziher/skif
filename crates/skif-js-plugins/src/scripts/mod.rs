//! Parses `package.json` scripts into the binaries, files and dependencies they reference.
//!
//! The parser is a pure, never-panicking, best-effort interpreter of POSIX-style command lines. It
//! understands quoting, separators (`&&`, `||`, `;`, `|`, `&`), subshells, command substitution,
//! redirections, environment assignments and the argument conventions of the common package
//! managers (`npm`, `npx`, `pnpm`, `yarn`, `bun`), process wrappers (`cross-env`, `dotenv`, ...),
//! task runners (`concurrently`, `npm-run-all`, `turbo`) and script runners (`node`, `tsx`,
//! `ts-node`, ...). Nothing is executed and the file system is never consulted.

mod args;
mod tables;
mod tokenize;
mod walk;

#[cfg(test)]
mod tests;

use std::ops::Range;

use indexmap::IndexMap;

pub use tables::{IGNORED_BINARIES, SHELL_BUILTINS, is_ignored_binary, is_shell_builtin};

pub use skif_config::PackageManager;

/// What the parser knows about the package that owns the script.
#[derive(Debug, Clone, Default)]
pub struct ScriptContext {
    /// The package's scripts by name, in `package.json` order. Used to follow `npm run x`,
    /// `pnpm x`, `yarn x`, `bun run x`, `node --run x` and `npm-run-all` patterns.
    pub scripts: IndexMap<String, String>,
    /// The package manager declared by the package; informational, commands are resolved from the
    /// binary that is actually invoked.
    pub package_manager: Option<PackageManager>,
    /// The name of the script being parsed. It is never expanded again, which breaks cycles that
    /// lead back to it.
    pub current_script: Option<String>,
}

impl ScriptContext {
    /// A context with no scripts.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds the script `name` with the command line `body`.
    #[must_use]
    pub fn with_script(mut self, name: impl Into<String>, body: impl Into<String>) -> Self {
        self.scripts.insert(name.into(), body.into());
        self
    }

    /// Sets the package manager the package declares.
    #[must_use]
    pub fn with_package_manager(mut self, package_manager: PackageManager) -> Self {
        self.package_manager = Some(package_manager);
        self
    }

    /// Sets the script being parsed, which is not expanded again.
    #[must_use]
    pub fn with_current_script(mut self, name: impl Into<String>) -> Self {
        self.current_script = Some(name.into());
        self
    }
}

/// How a binary was reached.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum BinaryVia {
    /// Invoked by name (or by a `node_modules/.bin` path), directly or behind a wrapper.
    #[default]
    Direct,
    /// `npx <bin>`.
    Npx,
    /// `npm exec <bin>`.
    NpmExec,
    /// `pnpm exec <bin>`, or `pnpm <bin>` when `<bin>` is neither a script nor a pnpm command.
    PnpmExec,
    /// `pnpm dlx`, `pnpx` or `yarn dlx`.
    Dlx,
    /// `bunx <bin>` or `bun x <bin>`.
    Bunx,
    /// `yarn run <bin>` or `yarn <bin>`.
    YarnRun,
    /// `yarn exec <bin>`.
    YarnExec,
    /// `bun run <bin>` or `bun <bin>`.
    BunRun,
}

impl BinaryVia {
    /// Whether the tool is fetched on demand, so it need not be listed in the manifest.
    #[must_use]
    pub const fn is_ephemeral(self) -> bool {
        matches!(self, Self::Npx | Self::Dlx | Self::Bunx)
    }
}

/// A binary the script runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinaryUse {
    /// The binary's name as the script spells it.
    pub name: String,
    /// Byte range in the parsed script. For binaries reached through another script (`npm run x`)
    /// or a string that could not be located exactly, the range of the referencing word.
    pub span: Range<usize>,
    /// How the binary was reached.
    pub via: BinaryVia,
    /// The command targets another workspace (`pnpm --filter web exec vitest`), so the binary
    /// belongs to that workspace's manifest rather than the current one.
    pub filtered: bool,
    /// The workspaces the command names (`yarn workspace web`, `pnpm --filter web`, `npm -w web`): package names,
    /// patterns or directories; empty when it targets others without naming them.
    pub workspaces: Vec<String>,
}

/// A file the script runs, loads or passes to a runner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRef {
    /// The file path as the script spells it.
    pub path: String,
    /// Byte range of the path in the parsed script.
    pub span: Range<usize>,
    /// How a test runner reads the path when it is a directory (`mocha build/test`); `None` for a plain file.
    pub tree: Option<FileTree>,
}

/// How a test runner reads a directory it is given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileTree {
    /// The runner descends into subdirectories (`mocha --recursive`); otherwise it reads only the files directly in
    /// the directory.
    pub recursive: bool,
    /// The extensions, without the dot, of the files the runner reads (`mocha --extension ts`).
    pub extensions: Vec<String>,
}

/// A package the script needs that is not a binary name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepRef {
    /// The package name, without version or subpath.
    pub name: String,
    /// Byte range of the package in the parsed script.
    pub span: Range<usize>,
    /// Fetched on demand (`npx -p pkg`, `pnpm dlx pkg`).
    pub ephemeral: bool,
}

/// Everything a script references.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScriptInfo {
    /// The binaries the script runs.
    pub binaries: Vec<BinaryUse>,
    /// The system binaries the script runs ([`is_ignored_binary`] but not a [`is_shell_builtin`]), which are assumed
    /// to be installed globally and are only reported when a caller asks for them (knip's `ignoreGlobalBinaries:
    /// false`).
    pub global_binaries: Vec<BinaryUse>,
    /// The files the script runs, loads or passes to a runner.
    pub files: Vec<FileRef>,
    /// The packages the script needs that are not binary names.
    pub dependencies: Vec<DepRef>,
    /// Names of the package's scripts this script runs, in order of first reference. Names that
    /// are not in [`ScriptContext::scripts`] are included so callers can report them.
    pub run_scripts: Vec<String>,
    /// Task names passed to `turbo`; they refer to scripts of other workspaces.
    pub turbo_tasks: Vec<String>,
    /// Package managers (`npm`, `pnpm`, `yarn`) the script invokes, which a manifest may list as a dependency.
    pub package_managers: Vec<String>,
    /// The script runs the Node.js test runner (`node --test`), which finds test files by convention.
    pub node_test: bool,
}

/// Splits `script` into its simple commands, each as its words with quotes and escapes resolved. Separators
/// (`&&`, `||`, `;`, `|`, `&`) end a command; leading `NAME=value` assignments and redirections are dropped.
/// Nothing is executed and no wrapper is interpreted, so a caller looks for the program word itself.
#[must_use]
pub fn split_commands(script: &str) -> Vec<Vec<String>> {
    tokenize::tokenize(script)
        .segments
        .into_iter()
        .map(|segment| segment.words.into_iter().map(|word| word.text).collect())
        .filter(|words: &Vec<String>| !words.is_empty())
        .collect()
}

/// Parses `script` and follows references to other scripts of the same package.
#[must_use]
pub fn parse_script(script: &str, ctx: &ScriptContext) -> ScriptInfo {
    walk::Walker::new(ctx).run(script)
}

fn dedupe<T: PartialEq>(items: &mut Vec<T>) {
    let mut unique: Vec<T> = Vec::with_capacity(items.len());
    for item in items.drain(..) {
        if !unique.contains(&item) {
            unique.push(item);
        }
    }
    *items = unique;
}

impl ScriptInfo {
    fn dedupe(&mut self) {
        dedupe(&mut self.binaries);
        dedupe(&mut self.global_binaries);
        dedupe(&mut self.files);
        dedupe(&mut self.dependencies);
    }
}
