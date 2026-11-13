//! mise: the `run` commands of the tasks of `mise.toml` and its other config files are scripts, run in the task's
//! `dir` (or `task_config.dir`) relative to the config root.
//!
//! File tasks are read too: the files under the directories `task_config.includes` names or, when no config sets
//! `includes`, under `mise-tasks/`, `.mise-tasks/`, `mise/tasks/`, `.mise/tasks/` and `.config/mise/tasks/`. A
//! JavaScript or TypeScript file task is an entry, and the command its shebang runs it with
//! (`#!/usr/bin/env -S npx tsx`) a script. An extensionless one whose shebang runs it with `node`, `bun`, `deno`,
//! `tsx`, `ts-node` or one of those through `npx` (`mise-tasks/gen`) cannot be a module of the graph, which knows a
//! file's language by its extension, so the relative modules it imports are entries in its place, and those it only
//! resolves (`require.resolve`) are used but no entries, so their exports are judged. A shell one (a shebang of `sh`,
//! `bash`, `zsh`, `dash` or `ksh`, or without a shebang one of those extensions) is a script, run in the workspace or
//! in the directory of its `#MISE dir="…"` header (`# [MISE]`, `//MISE` and `:: MISE` too). A task for another
//! interpreter, and a file without a shebang whose extension is not a shell's (`README`, `NOTES.md`), is not read.
//!
//! The binaries a task runs by name are machine tools (mise installs those its `[tools]` names), so only those it
//! reaches through a package manager are judged (`optional_binaries`); a file task's are recorded with the task file.

use std::path::Path;

use skif_js_syntax::{ConfigValue, ImportKind, ParseOptions, parse_module};

use crate::config_load::load_config;
use crate::files::find_files;
use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{get_in, join_rel, parent_dir, script_in, strings};

/// The `mise` plugin.
pub struct Mise;

static SPEC: PluginSpec = PluginSpec {
    id: "mise",
    enablers: &[Enabler::File("mise.toml"), Enabler::File(".mise.toml")],
    config: &[
        "{mise,.mise}{,.*}.toml",
        "{mise,.mise}/config{,.*}.toml",
        ".config/mise{,.*}.toml",
        ".config/mise/config{,.*}.toml",
        ".config/mise/mise{,.local}.toml",
        "{mise,.mise}/conf.d/[!.]*.toml",
        ".config/mise/conf.d/[!.]*.toml",
    ],
    binaries: &["mise"],
    optional_binaries: true,
    ..PluginSpec::DEFAULT
};

impl Plugin for Mise {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let root = config_root(config.base, config.file_name());
        let includes: Vec<String> = get_in(config.value, &["task_config", "includes"])
            .map_or_default(strings)
            .into_iter()
            .filter(|include| {
                !std::path::Path::new(include)
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("toml"))
            })
            .filter_map(|include| resolve_dir(include, &root))
            .collect();
        let mut out = file_tasks(cx, &includes);
        let Some(ConfigValue::Object(tasks)) = config.value.get("tasks") else {
            return out;
        };
        let default_dir = get_in(config.value, &["task_config", "dir"]).and_then(ConfigValue::as_str);
        for (_, task) in tasks {
            let (runs, dir): (Vec<&str>, Option<&str>) = match task {
                ConfigValue::Object(_) => (
                    ["run", "run_windows"]
                        .iter()
                        .filter_map(|key| task.get(key))
                        .flat_map(strings)
                        .collect(),
                    task.get("dir").and_then(ConfigValue::as_str),
                ),
                other => (strings(other), None),
            };
            let Some(dir) = resolve_dir(dir.or(default_dir).unwrap_or("."), &root) else {
                continue;
            };
            for run in runs.into_iter().filter(|run| !has_template(run)) {
                out.extend(script_in(&dir, run.to_owned()));
            }
        }
        out
    }

    fn resolve_workspace(&self, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        if sets_includes(cx) {
            return Vec::new();
        }
        let dirs = FILE_TASK_DIRS.map(str::to_owned);
        file_tasks(cx, &dirs)
    }
}

/// Whether a config sets `task_config.includes`, which replaces the default file task directories.
fn sets_includes(cx: &PluginContext<'_>) -> bool {
    let patterns: Vec<String> = SPEC.config.iter().map(|pattern| (*pattern).to_owned()).collect();
    find_files(cx.fs, cx.globs, cx.dir, &patterns).into_iter().any(|file| {
        let path = cx.dir.join(file);
        cx.fs
            .read_to_string(&path)
            .ok()
            .and_then(|source| load_config(&path, &source))
            .is_some_and(|config| get_in(&config, &["task_config", "includes"]).is_some())
    })
}

/// The directories mise looks for file tasks in by default, relative to the config root.
const FILE_TASK_DIRS: [&str; 5] = [
    "mise-tasks",
    ".mise-tasks",
    "mise/tasks",
    ".mise/tasks",
    ".config/mise/tasks",
];

/// Extensions of the file tasks that are JavaScript or TypeScript modules.
const SOURCE_EXTENSIONS: &[&str] = &["js", "mjs", "cjs", "jsx", "ts", "mts", "cts", "tsx"];

/// Interpreters whose file tasks are read as shell scripts, and the extensions of such files.
const SHELLS: &[&str] = &["sh", "bash", "zsh", "dash", "ksh"];

/// The file tasks under `dirs`, directories relative to the workspace, at any depth.
fn file_tasks(cx: &PluginContext<'_>, dirs: &[String]) -> Vec<PluginInput> {
    let patterns: Vec<String> = dirs.iter().map(|dir| join_rel(dir, "**/*")).collect();
    if patterns.is_empty() {
        return Vec::new();
    }
    find_files(cx.fs, cx.globs, cx.dir, &patterns)
        .into_iter()
        .flat_map(|file| file_task(cx, file))
        .collect()
}

/// What one file task contributes: an entry for a JavaScript or TypeScript module, with the command its shebang runs
/// it with, and a script for a shell script.
fn file_task(cx: &PluginContext<'_>, file: String) -> Vec<PluginInput> {
    let name = file.rsplit('/').next().unwrap_or(&file);
    let extension = name
        .rsplit_once('.')
        .filter(|(stem, _)| !stem.is_empty())
        .map(|(_, extension)| extension);
    let is_source = extension.is_some_and(|extension| SOURCE_EXTENSIONS.contains(&extension));
    let named_as_shell = extension.is_some_and(|extension| SHELLS.contains(&extension));
    let source = cx.fs.read_to_string(&cx.dir.join(&file)).ok();
    let text = source.as_deref().unwrap_or_default();
    let shebang = text.lines().next().and_then(|line| line.strip_prefix("#!"));
    let words = shebang.map_or_default(shebang_words);
    let javascript = extension.is_none() && runs_javascript(&words);
    if is_source || javascript {
        let runner = (!words.is_empty()).then(|| PluginInput::SourceScript {
            script: format!("{} {file}", words.join(" ")),
            file: file.clone(),
        });
        let mut out = if is_source {
            vec![PluginInput::Entry(file)]
        } else {
            imported_modules(&file, text, &words)
        };
        out.extend(runner);
        return out;
    }
    let is_shell = match shebang {
        Some(shebang) => shebang_words(shebang).first().is_some_and(|name| SHELLS.contains(name)),
        // Without a shebang, only a file named as a shell script is one (`build.sh`, not `build` or `README`).
        None => named_as_shell,
    };
    if source.is_none() || !is_shell {
        return Vec::new();
    }
    let Some(dir) = header_dir(text).map_or_else(|| Some(String::new()), |dir| resolve_dir(dir, "")) else {
        return Vec::new();
    };
    // A task run in the workspace records its binaries with the task file; one run elsewhere leaves them to the
    // package living there.
    match script_in(&dir, text.to_owned()) {
        Some(PluginInput::Script(script)) => vec![PluginInput::SourceScript { file, script }],
        other => other.into_iter().collect(),
    }
}

/// Programs that run a file as JavaScript or TypeScript.
const JS_RUNNERS: &[&str] = &["node", "bun", "deno", "tsx", "ts-node"];

/// Whether the shebang command `words` ([`shebang_words`]) runs the file as JavaScript or TypeScript: one of
/// [`JS_RUNNERS`], directly or through `npx`, `pnpx` or `bunx` (`npx -y tsx`).
fn runs_javascript(words: &[&str]) -> bool {
    let mut words = words.iter().copied();
    let Some(program) = words.next() else {
        return false;
    };
    if JS_RUNNERS.contains(&program) {
        return true;
    }
    ["npx", "pnpx", "bunx"].contains(&program)
        && words
            .find(|word| !word.starts_with('-'))
            .is_some_and(|runner| JS_RUNNERS.contains(&runner))
}

/// Extensions a relative specifier without one may resolve to, as a glob alternation.
const MODULE_GLOB_EXTENSIONS: &str = "{js,mjs,cjs,jsx,ts,mts,cts,tsx}";

/// The relative modules the extensionless JavaScript file task `file` imports or requires, as entries relative to the
/// workspace, and those it only resolves (`require.resolve`), as build sources: the file itself for a specifier with a
/// code extension, else the files and `index` files it may resolve to. Parsed as TypeScript when its runner is not
/// plain `node`.
fn imported_modules(file: &str, text: &str, words: &[&str]) -> Vec<PluginInput> {
    let as_typescript = words.iter().any(|word| *word != "node" && JS_RUNNERS.contains(word));
    let parsed = format!("{file}.{}", if as_typescript { "ts" } else { "js" });
    let Ok(module) = parse_module(Path::new(&parsed), text, &ParseOptions::default()) else {
        return Vec::new();
    };
    let dir = parent_dir(file);
    let mut out: Vec<PluginInput> = Vec::new();
    for import in &module.imports {
        // A resolved module is used by its path alone, so its exports stay judged.
        let input: fn(String) -> PluginInput = match import.kind {
            ImportKind::Static
            | ImportKind::TypeOnly
            | ImportKind::SideEffect
            | ImportKind::Dynamic
            | ImportKind::Require
            | ImportKind::ExportFrom => PluginInput::Entry,
            ImportKind::Resolve => PluginInput::BuildSource,
            _ => continue,
        };
        let specifier = import.specifier.as_str();
        if !(specifier.starts_with("./") || specifier.starts_with("../")) {
            continue;
        }
        let path = join_rel(dir, specifier);
        let has_extension = path
            .rsplit('/')
            .next()
            .and_then(|name| name.rsplit_once('.'))
            .is_some_and(|(stem, extension)| !stem.is_empty() && SOURCE_EXTENSIONS.contains(&extension));
        let glob = if has_extension {
            path
        } else {
            format!("{path}{{,/index}}.{MODULE_GLOB_EXTENSIONS}")
        };
        let input = input(glob);
        if !out.contains(&input) {
            out.push(input);
        }
    }
    out
}

/// The command a shebang line runs: its program and arguments, or the words after `env` and its options
/// (`npx tsx` for `/usr/bin/env -S npx tsx`).
fn shebang_words(shebang: &str) -> Vec<&str> {
    let mut words = shebang.split_whitespace();
    let Some(program) = words.next().and_then(|program| program.rsplit('/').next()) else {
        return Vec::new();
    };
    if program == "env" {
        words.skip_while(|word| word.starts_with('-')).collect()
    } else {
        std::iter::once(program).chain(words).collect()
    }
}

/// The `dir` a file task's header sets: `#MISE dir="…"`, with or without a space after the comment marker (`#`,
/// `//` or `::`) and with `MISE` or `[MISE]`.
fn header_dir(text: &str) -> Option<&str> {
    text.lines().find_map(|line| {
        let comment = ["//", "#", "::"]
            .iter()
            .find_map(|marker| line.strip_prefix(marker))?
            .trim_start();
        let header = comment
            .strip_prefix("[MISE]")
            .or_else(|| comment.strip_prefix("MISE"))?;
        let value = header
            .trim()
            .strip_prefix("dir")?
            .trim_start()
            .strip_prefix('=')?
            .trim();
        Some(value.trim_matches(['"', '\'']))
    })
}

/// The directory a config file's paths are relative to: the directory that holds `mise.toml`, `.mise/` or
/// `.config/`.
fn config_root(base: &str, file_name: &str) -> String {
    let name = |dir: &str| dir.rsplit('/').next().unwrap_or(dir).to_owned();
    let is_fragment = name(base) == "conf.d";
    let grouped = if is_fragment { parent_dir(base) } else { base };
    let grouped_name = name(grouped);
    let root =
        if (is_fragment || file_name.starts_with("config.")) && (grouped_name == "mise" || grouped_name == ".mise") {
            parent_dir(grouped)
        } else {
            base
        };
    if name(root) == ".config" {
        parent_dir(root).to_owned()
    } else {
        root.to_owned()
    }
}

fn has_template(value: &str) -> bool {
    value.contains("{{") || value.contains("{%")
}

/// A task directory relative to the workspace: `{{config_root}}` is the config root, a relative path is relative to
/// it; a templated, home-relative or variable path is unknown.
fn resolve_dir(dir: &str, root: &str) -> Option<String> {
    let mut expanded = dir.to_owned();
    for variable in ["{{config_root}}", "{{ config_root }}"] {
        expanded = expanded.replace(variable, ".");
    }
    if has_template(&expanded) || expanded.starts_with('~') || expanded.contains('$') || expanded.starts_with('/') {
        return None;
    }
    Some(join_rel(root, &expanded))
}

#[cfg(test)]
mod tests {
    use super::header_dir;

    #[test]
    fn headers_take_any_comment_marker_and_spacing() {
        for header in [
            "#MISE dir=\"web\"",
            "# MISE dir=\"web\"",
            "# [MISE] dir=\"web\"",
            "#[MISE] dir='web'",
            "//MISE dir=web",
            "// MISE dir=\"web\"",
            "// [MISE] dir=\"web\"",
            ":: MISE dir=\"web\"",
        ] {
            assert_eq!(header_dir(&format!("echo\n{header}\n")), Some("web"), "{header}");
        }
        assert_eq!(header_dir("#!/bin/sh\n# MISE description=\"x\"\n"), None);
    }
}
