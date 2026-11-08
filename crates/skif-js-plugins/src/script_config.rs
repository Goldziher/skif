//! Option values a shell command passes to a tool: `storybook dev -c ui/.storybook`, `vitest --config x.ts`,
//! `cdk deploy -a "npx ts-node bin/app.ts"`.

use indexmap::IndexMap;

use crate::support::join_rel;

/// The flags that name a config file or directory.
const CONFIG_FLAGS: &[&str] = &["-c", "--config", "--config-dir"];

/// The flags that name a TypeScript project.
const PROJECT_FLAGS: &[&str] = &["-p", "--project"];

/// The config flags of the tool `id`.
pub(crate) fn config_flags(id: &str) -> Vec<&'static str> {
    let mut flags = CONFIG_FLAGS.to_vec();
    if id == "typescript" || id == "tsrs" {
        flags.extend(PROJECT_FLAGS);
    }
    // `buf generate --template buf.gen.yaml`.
    if id == "buf" {
        flags.push("--template");
    }
    // `kysely-codegen --config-file x.ts`, `railway up --file x.ts`.
    if id == "kysely-codegen" {
        flags.push("--config-file");
    }
    if id == "railway" {
        flags.push("--file");
    }
    flags
}

/// Ends the word being read, if any, and adds it to the last command.
fn flush(commands: &mut [Vec<String>], word: &mut String, in_word: &mut bool) {
    if *in_word && let Some(last) = commands.last_mut() {
        last.push(std::mem::take(word));
    }
    *in_word = false;
}

/// Splits a command line into commands (on `&&`, `||`, `;`, `|`, `&` and newlines) of words, with quotes removed.
fn commands(script: &str) -> Vec<Vec<String>> {
    let mut commands = vec![Vec::new()];
    let mut word = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    for c in script.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => word.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                in_word = true;
            }
            (None, c) if c.is_whitespace() && c != '\n' => flush(&mut commands, &mut word, &mut in_word),
            (None, '&' | '|' | ';' | '\n') => {
                flush(&mut commands, &mut word, &mut in_word);
                commands.push(Vec::new());
            }
            (None, c) => {
                word.push(c);
                in_word = true;
            }
        }
    }
    flush(&mut commands, &mut word, &mut in_word);
    commands.retain(|command| !command.is_empty());
    commands
}

/// The last path segment of a command word: `node_modules/.bin/vitest` is `vitest`.
fn binary_name(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

/// The values `flags` take in every command of `script` that runs one of `binaries`, in order.
pub(crate) fn flag_values(script: &str, binaries: &[&str], flags: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    for command in commands(script) {
        let Some(position) = command.iter().position(|word| binaries.contains(&binary_name(word))) else {
            continue;
        };
        let mut words = command[position + 1..].iter();
        while let Some(word) = words.next() {
            if flags.contains(&word.as_str()) {
                if let Some(value) = words.next().filter(|value| !value.starts_with('-')) {
                    out.push(value.clone());
                }
            } else if let Some((flag, value)) = word.split_once('=')
                && flags.contains(&flag)
                && !value.is_empty()
            {
                out.push(value.to_owned());
            }
        }
    }
    out
}

/// The command after `--` of every command of `script` that runs one of `binaries` with `subcommand` as its first
/// word (`varlock run -- node app.js`), its words joined again.
pub(crate) fn commands_after_separator(script: &str, binaries: &[&str], subcommand: &str) -> Vec<String> {
    let mut out = Vec::new();
    for command in commands(script) {
        let Some(position) = command.iter().position(|word| binaries.contains(&binary_name(word))) else {
            continue;
        };
        let args = &command[position + 1..];
        if args.first().map(String::as_str) != Some(subcommand) {
            continue;
        }
        if let Some(separator) = args.iter().position(|word| word == "--") {
            let rest: Vec<String> = args[separator + 1..]
                .iter()
                .map(|word| {
                    if word.contains(char::is_whitespace) {
                        format!("'{word}'")
                    } else {
                        word.clone()
                    }
                })
                .collect();
            if !rest.is_empty() {
                out.push(rest.join(" "));
            }
        }
    }
    out
}

/// The positional words of every command of `script` that runs one of `binaries`, one list per command, in order.
///
/// Flags are skipped as `minimist` reads them: a flag takes the next word as its value unless it has an inline `=value`,
/// is listed in `boolean_flags` or the next word is a flag itself. Parsing a command ends at `--`.
pub(crate) fn positionals(script: &str, binaries: &[&str], boolean_flags: &[&str]) -> Vec<Vec<String>> {
    let mut found = Vec::new();
    for command in commands(script) {
        let Some(position) = command.iter().position(|word| binaries.contains(&binary_name(word))) else {
            continue;
        };
        let mut words = command[position + 1..].iter();
        let mut positional = Vec::new();
        while let Some(word) = words.next() {
            if word == "--" {
                break;
            }
            if let Some(flag) = word.strip_prefix('-').filter(|flag| !flag.is_empty()) {
                let name = flag.trim_start_matches('-');
                if !flag.contains('=') && !boolean_flags.contains(&name) {
                    let takes_value = words.clone().next().is_some_and(|next| !next.starts_with('-'));
                    if takes_value {
                        words.next();
                    }
                }
            } else {
                positional.push(word.clone());
            }
        }
        found.push(positional);
    }
    found
}

/// A config path or directory from a script, relative to the workspace; `None` when it is computed, absolute,
/// home-relative or outside the workspace. A backslash counts as a separator, so `..\x` and `C:\x` are outside too.
pub(crate) fn workspace_path(value: &str) -> Option<String> {
    let value = value.replace('\\', "/");
    let has_drive = value.as_bytes().get(1) == Some(&b':');
    if value.is_empty() || value.starts_with(['/', '~']) || has_drive || value.contains(['$', '`', '(', '*']) {
        return None;
    }
    let path = join_rel("", &value);
    (!path.is_empty() && path != ".." && !path.starts_with("../")).then_some(path)
}

/// The config files and directories the scripts of a workspace pass to the tool with these binaries.
pub(crate) fn config_paths(scripts: &IndexMap<String, String>, binaries: &[&str], flags: &[&str]) -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    for script in scripts.values() {
        for value in flag_values(script, binaries, flags) {
            if let Some(path) = workspace_path(&value)
                && !paths.contains(&path)
            {
                paths.push(path);
            }
        }
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flag_values_are_read_per_command() {
        let script = "build && storybook dev -p 6006 -c ui/.storybook; vitest --config=a.ts run && other -c no";
        assert_eq!(flag_values(script, &["storybook"], &["-c"]), ["ui/.storybook"]);
        assert_eq!(flag_values(script, &["vitest"], &["--config"]), ["a.ts"]);
        assert_eq!(flag_values(script, &["vite"], &["-c"]), [] as [std::string::String; 0]);
    }

    #[test]
    fn quotes_and_paths_to_binaries_are_understood() {
        let script = r#"npx cdk deploy -a "npx ts-node bin/app.ts" && ./node_modules/.bin/vitest -c 'x y.ts'"#;
        assert_eq!(
            flag_values(script, &["cdk"], &["-a", "--app"]),
            ["npx ts-node bin/app.ts"]
        );
        assert_eq!(flag_values(script, &["vitest"], &["-c"]), ["x y.ts"]);
    }

    #[test]
    fn a_flag_without_a_value_yields_nothing() {
        assert_eq!(
            flag_values("rollup -c", &["rollup"], &["-c"]),
            [] as [std::string::String; 0]
        );
        assert_eq!(
            flag_values("rollup -c -w", &["rollup"], &["-c"]),
            [] as [std::string::String; 0]
        );
    }

    #[test]
    fn positionals_skip_flags_and_their_values() {
        let script =
            "build && jasmine --fail-fast --reporter=x --config cfg.json spec/a.js spec/b.js -- extra; other y";
        assert_eq!(
            positionals(script, &["jasmine"], &["fail-fast"]),
            [vec!["spec/a.js".to_owned(), "spec/b.js".to_owned()]]
        );
        assert_eq!(
            positionals("jasmine --fail-fast spec/a.js", &["jasmine"], &[]),
            [Vec::<String>::new()],
            "an unlisted flag takes the next word"
        );
    }

    #[test]
    fn commands_after_the_separator_of_a_subcommand() {
        let script = "varlock run -- node 'my app.js' && varlock load -- x && other run -- y";
        assert_eq!(
            commands_after_separator(script, &["varlock"], "run"),
            ["node 'my app.js'"]
        );
    }

    #[test]
    fn computed_and_outside_paths_are_dropped() {
        assert_eq!(workspace_path("./ui/.storybook/"), Some("ui/.storybook".to_owned()));
        for value in [
            "$DIR/x",
            "../x",
            "a/../../x",
            "/abs/x",
            "~/x",
            "..\\x",
            "C:\\x",
            "C:/x",
            "",
            ".",
            "a/*.ts",
        ] {
            assert_eq!(workspace_path(value), None, "{value}");
        }
    }
}
