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
}
