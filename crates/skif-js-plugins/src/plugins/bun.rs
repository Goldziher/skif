//! Bun: the test files `bun test` finds, the modules `bunfig.toml` preloads, and what source files hand to Bun's
//! APIs: the commands of Bun Shell (`` $`…` `` imported from `bun`, or `` Bun.$`…` ``) and of `Bun.spawn`, which are
//! scripts parsed like the scripts of `package.json`, and the `entrypoints` of `Bun.build`, which are production
//! entries relative to the package (Bun resolves them from the working directory a script starts in). A package whose
//! sources use the `Bun` global uses the type package it lists for it, `@types/bun` or `bun-types`. Only the package's
//! own sources are read for commands and entries: a file below a directory with its own `package.json`, such as a
//! member below a workspace root, belongs to that package. A type package serves the packages below the one listing it
//! too (a workspace root lists Bun's types for its members), so their use of the global counts for it.

use skif_js_syntax::{ConfigValue, ImportedCall, command_in_call_dir, extract_command_calls, literal_command};

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{get_in, module_input, own_sources_where, production_entries, sources_containing, strings};

/// The `bun` plugin.
pub struct Bun;

/// The modules whose types declare the `Bun` global.
const TYPE_PACKAGES: [&str; 2] = ["@types/bun", "bun-types"];

/// The keys of `bunfig.toml` that name a module to load: a package, or a file relative to the config. `[lint] plugins`
/// lists the ESLint plugins `bun lint` loads.
const MODULE_KEYS: [&[&str]; 4] = [
    &["preload"],
    &["test", "preload"],
    &["install", "security", "scanner"],
    &["lint", "plugins"],
];

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
    // bun.com/docs/test lists only `js`, `jsx`, `ts` and `tsx` and the `.test`/`.spec` suffixes, but Bun 1.4 runs the
    // module extensions and the `_test`/`_spec` suffixes too.
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

    /// The modules of `preload` (the runtime's) and `[test] preload`, the package `jsxImportSource` names, the
    /// security scanner of `[install.security] scanner` and the plugins of `[lint] plugins`, each a package or a file.
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
        let names_bun = |text: &str| text.contains("Bun.") || text.contains("\"bun\"") || text.contains("'bun'");
        for (source, text) in own_sources_where(cx, names_bun) {
            uses_global |= text.contains("Bun.");
            let path = cx.dir.join(&source);
            for call in extract_command_calls(&text, &path, &["bun"], &[("Bun.*", "bun")]) {
                api_inputs(&call, &source, &mut out);
            }
        }
        let listed: Vec<&str> = TYPE_PACKAGES
            .into_iter()
            .filter(|name| cx.has_dependency(name))
            .collect();
        if !listed.is_empty() && (uses_global || !sources_containing(cx, "Bun.").is_empty()) {
            out.extend(listed.into_iter().map(|name| PluginInput::Dependency(name.to_owned())));
        }
        out
    }
}

/// What one call of Bun's API in the file `source` names.
fn api_inputs(call: &ImportedCall, source: &str, out: &mut Vec<PluginInput>) {
    let script = match (call.export.as_str(), &call.template) {
        ("$", Some(template)) => Some(command_in_call_dir(template.clone(), call.arguments.first())),
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
/// its `cwd` option names (see [`command_in_call_dir`]); see [`words_command`] for the words.
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

/// The command of a list of words run with `options`: the program and its literal arguments, a computed argument left
/// out as for `spawn` of `node:child_process`. `None` when the program is computed or a literal word is not a plain
/// word ([`literal_command`]).
fn words_command(words: &[ConfigValue], options: Option<&ConfigValue>) -> Option<String> {
    let (program, rest) = words.split_first()?;
    let command = literal_command(&[program.clone(), ConfigValue::Array(rest.to_vec())])?;
    Some(command_in_call_dir(command, options))
}
