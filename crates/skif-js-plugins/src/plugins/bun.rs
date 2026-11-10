//! Bun: the test files `bun test` finds, the modules `bunfig.toml` preloads, and what source files hand to Bun's
//! APIs: the commands of Bun Shell (`` $`…` `` imported from `bun`, or `` Bun.$`…` ``) and of `Bun.spawn`, which are
//! scripts parsed like the scripts of `package.json`, and the `entrypoints` of `Bun.build`, which are production
//! entries relative to the package (Bun resolves them from the working directory a script starts in). A package whose
//! sources use the `Bun` global uses the type package it lists for it, `@types/bun` or `bun-types`.

use skif_js_syntax::{ConfigValue, ImportedCall, command_in_call_dir, extract_command_calls, literal_command};

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{get_in, module_input, production_entries, sources_containing, strings};

/// The `bun` plugin.
pub struct Bun;

/// The modules whose types declare the `Bun` global.
const TYPE_PACKAGES: [&str; 2] = ["@types/bun", "bun-types"];

/// The keys of `bunfig.toml` that name a module to load: a package, or a file relative to the config.
const MODULE_KEYS: [&[&str]; 3] = [&["preload"], &["test", "preload"], &["install", "security", "scanner"]];

static SPEC: PluginSpec = PluginSpec {
    id: "bun",
    enablers: &[
        Enabler::File("bun.lock"),
        Enabler::File("bun.lockb"),
        Enabler::File("bunfig.toml"),
        Enabler::Script("bun test"),
        Enabler::Dependency("@types/bun"),
        Enabler::Dependency("bun-types"),
    ],
    config: &["bunfig.toml"],
    entry: &[
        "**/*.{test,spec}.{js,jsx,ts,tsx,mjs,cjs,mts,cts}",
        "**/*_{test,spec}.{js,jsx,ts,tsx,mjs,cjs,mts,cts}",
    ],
    binaries: &["bun", "bunx"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Bun {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    /// The modules of `preload` (the runtime's) and `[test] preload`, the package `jsxImportSource` names, and the
    /// security scanner of `[install.security] scanner`, a package or a file.
    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let value = config.value;
        let mut out: Vec<PluginInput> = MODULE_KEYS
            .into_iter()
            .filter_map(|path| get_in(value, path))
            .flat_map(strings)
            .filter_map(|specifier| module_input(config.base, specifier, false))
            .collect();
        out.extend(
            get_in(value, &["jsxImportSource"])
                .and_then(ConfigValue::as_str)
                .and_then(|source| module_input(config.base, source, false))
                .filter(|input| matches!(input, PluginInput::Dependency(_))),
        );
        out
    }

    fn resolve_workspace(&self, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        let mut uses_global = false;
        for (source, text) in sources_containing(cx, "") {
            let global = text.contains("Bun.");
            if !global && !text.contains("\"bun\"") && !text.contains("'bun'") {
                continue;
            }
            uses_global |= global;
            let path = cx.dir.join(&source);
            for call in extract_command_calls(&text, &path, &["bun"], &[("Bun.*", "bun")]) {
                api_inputs(&call, &source, &mut out);
            }
        }
        if uses_global {
            out.extend(
                TYPE_PACKAGES
                    .iter()
                    .filter(|name| cx.has_dependency(name))
                    .map(|name| PluginInput::Dependency((*name).to_owned())),
            );
        }
        out
    }
}

/// What one call of Bun's API in the file `source` names.
fn api_inputs(call: &ImportedCall, source: &str, out: &mut Vec<PluginInput>) {
    let script = match (call.export.as_str(), &call.template) {
        ("$", Some(template)) => Some(template.clone()),
        ("spawn" | "spawnSync", None) => spawn_command(&call.arguments),
        ("build", None) => {
            let entrypoints = call.arguments.first().and_then(|options| options.get("entrypoints"));
            out.extend(production_entries("", entrypoints.map(strings).unwrap_or_default()));
            None
        }
        _ => None,
    };
    out.extend(script.map(|script| PluginInput::SourceScript {
        file: source.to_owned(),
        script,
    }));
}

/// The command line of `Bun.spawn(["cmd", ...args], options)` or `Bun.spawn({ cmd: [...], cwd })`, in the directory
/// its `cwd` option names; `None` unless every word is a literal plain word.
fn spawn_command(arguments: &[ConfigValue]) -> Option<String> {
    match arguments.first()? {
        ConfigValue::Array(words) => words_command(words, arguments.get(1)),
        options @ ConfigValue::Object(_) => match options.get("cmd")? {
            ConfigValue::Array(words) => words_command(words, Some(options)),
            _ => None,
        },
        _ => None,
    }
}

/// The command of a list of words run with `options`; `None` when a word is computed, which would change what the
/// command means, or is not a plain word ([`literal_command`]).
fn words_command(words: &[ConfigValue], options: Option<&ConfigValue>) -> Option<String> {
    let (program, rest) = words.split_first()?;
    if rest.iter().any(|word| word.as_str().is_none()) {
        return None;
    }
    let command = literal_command(&[program.clone(), ConfigValue::Array(rest.to_vec())])?;
    Some(command_in_call_dir(command, options))
}
