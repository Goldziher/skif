//! What the commands of toolchain files say about package managers: the program a command runs past its wrappers,
//! the versions `corepack prepare` and a global install pin, and `corepack enable`.

use skif_config::PackageManager;
use skif_core::shell::{RESERVED_WORDS, Word};

use super::parse::{Pin, Tool};

/// The name a package manager is invoked by.
pub(crate) const fn manager_name(manager: PackageManager) -> &'static str {
    match manager {
        PackageManager::Npm => "npm",
        PackageManager::Pnpm => "pnpm",
        PackageManager::Yarn => "yarn",
        PackageManager::Bun => "bun",
        _ => "",
    }
}

/// The package manager published under `name`.
pub(crate) fn manager_named(name: &str) -> Option<PackageManager> {
    match name {
        "npm" => Some(PackageManager::Npm),
        "pnpm" => Some(PackageManager::Pnpm),
        "yarn" => Some(PackageManager::Yarn),
        "bun" => Some(PackageManager::Bun),
        _ => None,
    }
}

/// The last path segment of a program word (`/usr/local/bin/npm` is `npm`).
pub(crate) fn basename(text: &str) -> &str {
    text.rsplit(['/', '\\']).next().unwrap_or(text)
}

/// Programs that run the command after them (with their own flags first).
const WRAPPERS: [&str; 6] = ["sudo", "doas", "exec", "time", "nohup", "command"];

/// The index of the word that names the program a command runs, past reserved words (`then`, `do`), `sudo`,
/// `env NAME=value` and the like. `None` for a command whose program is computed (`$NPM ci`) or that is only
/// wrappers.
pub(crate) fn program(words: &[Word]) -> Option<usize> {
    let mut index = 0;
    while let Some(word) = words.get(index) {
        if word.dynamic {
            return None;
        }
        if RESERVED_WORDS.contains(&word.text.as_str()) {
            index += 1;
            continue;
        }
        let name = basename(&word.text);
        let is_env = name == "env";
        if !is_env && !WRAPPERS.contains(&name) {
            return Some(index);
        }
        index += 1;
        while words.get(index).is_some_and(|word| {
            word.text.starts_with('-') || (is_env && skif_core::shell::is_assignment_text(&word.text))
        }) {
            index += 1;
        }
    }
    None
}

/// `name` and `version` of a `name@version` argument; a scoped name keeps its leading `@`.
pub(crate) fn split_spec(spec: &str) -> (&str, Option<&str>) {
    let at = spec
        .char_indices()
        .skip(1)
        .find(|&(_, c)| c == '@')
        .map(|(index, _)| index);
    match at {
        Some(at) => (&spec[..at], Some(&spec[at + 1..]).filter(|version| !version.is_empty())),
        None => (spec, None),
    }
}

/// Whether `word` asks for a global install (`-g`, `--global`, `--location=global`).
pub(crate) fn is_global_flag(word: &str) -> bool {
    matches!(word, "-g" | "--global" | "--location=global")
}

/// The arguments of `words` that are not flags.
fn positionals(words: &[Word]) -> impl Iterator<Item = &Word> {
    words.iter().filter(|word| !word.text.starts_with('-'))
}

/// A pin of a package manager's version in a `name@version` argument.
fn spec_pin(word: &Word, label: &str) -> Option<Pin> {
    if word.dynamic {
        return None;
    }
    let (name, version) = split_spec(&word.text);
    Some(Pin {
        tool: Tool::Manager(manager_named(name)?),
        version: version?.to_owned(),
        span: word.span.clone(),
        label: label.to_owned(),
    })
}

/// What a command does with corepack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Corepack {
    /// `corepack enable`: the shims run whatever version corepack resolves.
    Enable,
    /// `corepack prepare`, `corepack use` or `corepack install`: a version is fetched and activated.
    Prepare,
}

/// The corepack action of a command, if it is one.
pub(crate) fn corepack(words: &[Word]) -> Option<Corepack> {
    let index = program(words)?;
    if basename(&words[index].text) != "corepack" {
        return None;
    }
    match positionals(&words[index + 1..]).next()?.text.as_str() {
        "enable" => Some(Corepack::Enable),
        "prepare" | "use" | "install" => Some(Corepack::Prepare),
        _ => None,
    }
}

/// The package-manager versions a command pins: `corepack prepare pnpm@9.1.0`, `corepack use pnpm@9`, and a global
/// install such as `npm install -g pnpm@9.1.0`.
pub(crate) fn command_pins(words: &[Word]) -> Vec<Pin> {
    let Some(index) = program(words) else {
        return Vec::new();
    };
    let program = basename(&words[index].text);
    let args = &words[index + 1..];
    let mut positional = positionals(args);
    let Some(subcommand) = positional.next() else {
        return Vec::new();
    };
    let label = match (program, subcommand.text.as_str()) {
        ("corepack", "prepare" | "use" | "install") => format!("corepack {}", subcommand.text),
        ("npm", "install" | "i" | "add") if args.iter().any(|word| is_global_flag(&word.text)) => {
            "npm install -g".to_owned()
        }
        _ => return Vec::new(),
    };
    positional.filter_map(|word| spec_pin(word, &label)).collect()
}

/// npm subcommands that install, run or change dependencies; the others (`view`, `whoami`, `publish`, ...) are not
/// reported.
const NPM_COMMANDS: &[&str] = &[
    "add", "cit", "clean-install", "ci", "dedupe", "ddp", "exec", "i", "ic", "in", "install", "install-ci-test",
    "install-clean", "install-test", "isntall", "isntall-clean", "it", "link", "ln", "prune", "r", "rb", "rebuild",
    "remove", "restart", "rm", "rum", "run", "run-script", "start", "stop", "t", "test", "tst", "un", "uninstall",
    "unlink", "up", "update", "upgrade", "urn", "x",
];

/// bun subcommands that install or change dependencies; `bun <file>` and `bun run` also run code as a runtime.
const BUN_COMMANDS: &[&str] = &["a", "add", "i", "install", "link", "remove", "rm", "unlink", "update"];

/// yarn and pnpm subcommands that only read or talk to the registry; every other subcommand, a script name
/// included, runs the manager.
const READ_ONLY: &[&str] = &[
    "adduser", "audit", "bin", "cache", "config", "doctor", "help", "info", "la", "licenses", "list", "ll", "login",
    "logout", "ls", "npm", "outdated", "owner", "pack", "publish", "root", "search", "show", "store", "team", "view",
    "whoami", "why",
];

/// Flags that print and exit.
const INFO_FLAGS: &[&str] = &["--version", "-v", "--help", "-h"];

/// Flags of `manager` that take the next word as their value.
fn takes_value(manager: PackageManager, flag: &str) -> bool {
    let common = matches!(flag, "--registry" | "--loglevel" | "--userconfig" | "--cwd" | "--prefix");
    common
        || match manager {
            PackageManager::Npm => matches!(flag, "-w" | "--workspace" | "-C"),
            PackageManager::Pnpm => matches!(flag, "--filter" | "-F" | "--dir" | "-C"),
            _ => false,
        }
}

/// The index in `args` of the first word that is neither a flag nor a flag's value.
fn subcommand(manager: PackageManager, args: &[Word]) -> Option<usize> {
    let mut index = 0;
    while let Some(word) = args.get(index) {
        if !word.text.starts_with('-') {
            return Some(index);
        }
        index += if takes_value(manager, &word.text) { 2 } else { 1 };
    }
    None
}

/// A command that runs a package manager other than the workspace's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Invocation {
    /// The manager it runs.
    pub manager: PackageManager,
    /// The indices of the command's first and last word to quote: the program through its subcommand.
    pub words: (usize, usize),
}

/// Whether the arguments from the subcommand at `sub` install only `chosen` (or corepack) globally:
/// `npm install -g pnpm@9`, `yarn global add pnpm`.
fn installs_chosen_globally(manager: PackageManager, args: &[Word], sub: usize, chosen: PackageManager) -> bool {
    let rest = &args[sub + 1..];
    let mut packages: Vec<&str> = positionals(rest).map(|word| split_spec(&word.text).0).collect();
    let installs = if manager == PackageManager::Yarn {
        let global_add = args[sub].text == "global" && packages.first() == Some(&"add");
        if global_add {
            packages.remove(0);
        }
        global_add
    } else {
        matches!(args[sub].text.as_str(), "install" | "i" | "in" | "add" | "a")
            && args.iter().any(|word| is_global_flag(&word.text))
    };
    installs
        && !packages.is_empty()
        && packages
            .iter()
            .all(|name| *name == manager_name(chosen) || *name == "corepack")
}

/// The invocation of a package manager other than `chosen` that `words` runs, if any.
///
/// Reported: npm's install, run and exec subcommands and `npx`; yarn bare, or with any subcommand or script name
/// that is not [`READ_ONLY`]; pnpm likewise (`pnpx`, `pnpm dlx`); bun's install subcommands and `bunx`. Not
/// reported: only `--version` or `--help`, a read-only subcommand, a global install of `chosen` itself, and a runner
/// (`npx pnpm@9 install`) or `corepack <manager>` that runs `chosen`.
pub(crate) fn invocation(words: &[Word], chosen: PackageManager) -> Option<Invocation> {
    let first = program(words)?;
    let mut start = first;
    if basename(&words[first].text) == "corepack" {
        start += 1;
        manager_named(&words.get(start)?.text)?;
    }
    let (manager, runner) = match basename(&words[start].text) {
        "npm" => (PackageManager::Npm, false),
        "npx" => (PackageManager::Npm, true),
        "pnpm" => (PackageManager::Pnpm, false),
        "pnpx" => (PackageManager::Pnpm, true),
        "yarn" | "yarnpkg" => (PackageManager::Yarn, false),
        "bun" => (PackageManager::Bun, false),
        "bunx" => (PackageManager::Bun, true),
        _ => return None,
    };
    if manager == chosen {
        return None;
    }
    let args = &words[start + 1..];
    let found = |last: usize| Some(Invocation { manager, words: (first, last) });
    let runs_chosen = |target: Option<&Word>| {
        target.is_some_and(|word| split_spec(&word.text).0 == manager_name(chosen))
    };
    if runner {
        let target = args.iter().position(|word| !word.text.starts_with('-'));
        if runs_chosen(target.map(|index| &args[index])) {
            return None;
        }
        return found(target.map_or(start, |index| start + 1 + index));
    }
    let Some(sub) = subcommand(manager, args) else {
        let bare_install = manager == PackageManager::Yarn
            && !args.iter().any(|word| INFO_FLAGS.contains(&word.text.as_str()));
        return if bare_install { found(start) } else { None };
    };
    let word = &args[sub];
    if word.dynamic || installs_chosen_globally(manager, args, sub, chosen) {
        return None;
    }
    let text = word.text.as_str();
    if matches!(text, "dlx" | "x") && runs_chosen(positionals(&args[sub + 1..]).next()) {
        return None;
    }
    let reported = match manager {
        PackageManager::Npm => NPM_COMMANDS.contains(&text),
        PackageManager::Bun => BUN_COMMANDS.contains(&text) || text == "x",
        _ => !READ_ONLY.contains(&text),
    };
    if reported { found(start + 1 + sub) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(line: &str) -> Vec<Word> {
        skif_core::shell::tokenize(line)
            .segments
            .into_iter()
            .next()
            .unwrap()
            .words
    }

    fn pins(line: &str) -> Vec<(String, String, &str)> {
        command_pins(&words(line))
            .into_iter()
            .map(|pin| {
                let Tool::Manager(manager) = pin.tool else { panic!("not a manager pin") };
                (manager_name(manager).to_owned(), pin.version, &line[pin.span])
            })
            .collect()
    }

    #[test]
    fn the_program_is_found_past_wrappers() {
        let cases = [
            ("npm ci", Some("npm")),
            ("sudo -E npm i -g pnpm", Some("npm")),
            ("env CI=1 -i /usr/bin/pnpm install", Some("/usr/bin/pnpm")),
            ("time nohup yarn", Some("yarn")),
            ("then npm ci", Some("npm")),
            ("$NPM ci", None),
            ("sudo", None),
        ];
        for (line, expected) in cases {
            let words = words(line);
            assert_eq!(program(&words).map(|index| words[index].text.as_str()), expected, "{line}");
        }
    }

    #[test]
    fn specs_split_at_the_version() {
        assert_eq!(split_spec("pnpm@9.1.0"), ("pnpm", Some("9.1.0")));
        assert_eq!(split_spec("@scope/tool@1"), ("@scope/tool", Some("1")));
        assert_eq!(split_spec("pnpm"), ("pnpm", None));
        assert_eq!(split_spec("pnpm@"), ("pnpm", None));
    }

    #[test]
    fn corepack_and_global_installs_pin_manager_versions() {
        assert_eq!(
            pins("corepack prepare pnpm@9.1.0 --activate"),
            [("pnpm".to_owned(), "9.1.0".to_owned(), "pnpm@9.1.0")]
        );
        assert_eq!(pins("corepack use yarn@4"), [("yarn".to_owned(), "4".to_owned(), "yarn@4")]);
        assert_eq!(
            pins("npm install --global corepack pnpm@8.15.0"),
            [("pnpm".to_owned(), "8.15.0".to_owned(), "pnpm@8.15.0")]
        );
        assert_eq!(pins("sudo npm i -g pnpm@9"), [("pnpm".to_owned(), "9".to_owned(), "pnpm@9")]);
        assert!(pins("npm install pnpm@9").is_empty());
        assert!(pins("npm i -g pnpm").is_empty());
        assert!(pins("corepack prepare pnpm@$PNPM_VERSION").is_empty());
        assert_eq!(corepack(&words("corepack enable pnpm")), Some(Corepack::Enable));
        assert_eq!(corepack(&words("corepack prepare --activate")), Some(Corepack::Prepare));
        assert_eq!(corepack(&words("npm ci")), None);
    }

    fn foreign(line: &str, chosen: PackageManager) -> Option<(String, String)> {
        let words = words(line);
        let found = invocation(&words, chosen)?;
        let quoted: Vec<&str> = words[found.words.0..=found.words.1]
            .iter()
            .map(|word| word.text.as_str())
            .collect();
        Some((manager_name(found.manager).to_owned(), quoted.join(" ")))
    }

    #[test]
    fn invocations_of_another_manager_are_found() {
        let pnpm = PackageManager::Pnpm;
        let cases = [
            ("npm ci", Some(("npm", "npm ci"))),
            ("npm install --omit=dev", Some(("npm", "npm install"))),
            ("npm --prefix web run build", Some(("npm", "npm --prefix web run"))),
            ("npm run-script build", Some(("npm", "npm run-script"))),
            ("npm test", Some(("npm", "npm test"))),
            ("npm start", Some(("npm", "npm start"))),
            ("npm exec vite", Some(("npm", "npm exec"))),
            ("npx tsc --noEmit", Some(("npm", "npx tsc"))),
            ("npx", Some(("npm", "npx"))),
            ("yarn", Some(("yarn", "yarn"))),
            ("yarn --frozen-lockfile", Some(("yarn", "yarn"))),
            ("yarn add left-pad", Some(("yarn", "yarn add"))),
            ("yarn install", Some(("yarn", "yarn install"))),
            ("yarn run build", Some(("yarn", "yarn run"))),
            ("yarn build", Some(("yarn", "yarn build"))),
            ("sudo npm i -g typescript", Some(("npm", "npm i"))),
            ("corepack yarn install", Some(("yarn", "corepack yarn install"))),
            ("bunx prettier", Some(("bun", "bunx prettier"))),
            ("bun install", Some(("bun", "bun install"))),
            ("npm view pnpm version", None),
            ("npm whoami", None),
            ("npm publish --access public", None),
            ("npm --version", None),
            ("yarn --version", None),
            ("yarn info react", None),
            ("npm install -g pnpm@9.1.0 corepack", None),
            ("npm i --global pnpm", None),
            ("yarn global add pnpm", None),
            ("bun add -g pnpm", None),
            ("npx pnpm@9 install", None),
            ("npx -y pnpm install", None),
            ("corepack pnpm install", None),
            ("pnpm install --frozen-lockfile", None),
            ("pnpm", None),
            ("bun run build.ts", None),
            ("$PM install", None),
        ];
        for (line, expected) in cases {
            assert_eq!(
                foreign(line, pnpm),
                expected.map(|(manager, text)| (manager.to_owned(), text.to_owned())),
                "{line}"
            );
        }
    }

    #[test]
    fn the_chosen_manager_decides_what_is_foreign() {
        assert_eq!(foreign("pnpm install", PackageManager::Yarn), Some(("pnpm".to_owned(), "pnpm install".to_owned())));
        assert_eq!(foreign("pnpm dlx create-vite", PackageManager::Npm), Some(("pnpm".to_owned(), "pnpm dlx".to_owned())));
        assert_eq!(foreign("pnpm why react", PackageManager::Npm), None);
        assert_eq!(foreign("yarn dlx yarn@4 --version", PackageManager::Npm), Some(("yarn".to_owned(), "yarn dlx".to_owned())));
        assert_eq!(foreign("pnpm dlx npm@10 ci", PackageManager::Npm), None);
        assert_eq!(foreign("npm ci", PackageManager::Npm), None);
        assert_eq!(foreign("yarn", PackageManager::Npm), Some(("yarn".to_owned(), "yarn".to_owned())));
        assert_eq!(foreign("npm install -g yarn", PackageManager::Yarn), None);
    }
}
