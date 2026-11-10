//! Calls of imported functions, for plugins that read what source files pass to a library.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use skif_js_syntax::{ConfigValue, FILE_DIR, ImportedCall, extract_command_calls, extract_imported_calls};

fn calls(source: &str, modules: &[&str], globals: &[(&str, &str)]) -> Vec<ImportedCall> {
    extract_imported_calls(source, Path::new("src/run.ts"), modules, globals)
}

fn s(v: &str) -> ConfigValue {
    ConfigValue::String(v.to_string())
}

#[test]
fn named_default_and_member_calls_are_found_with_static_arguments() {
    let source = r"
        import pino from 'pino';
        import { execa as run, $ } from 'execa';
        const other = require('other');
        const target = 'pino-pretty';
        pino({ transport: { target } });
        pino.transport({ targets: [] });
        await run('tsx', ['scripts/build.ts', computed]);
        await $`node ./bin/cli.js ${flag}`;
        await $({ cwd: 'x' })`vite build`;
        other('ignored');
    ";
    let found = calls(source, &["pino", "execa"], &[]);
    let summary: Vec<(&str, &str)> = found.iter().map(|c| (c.module.as_str(), c.export.as_str())).collect();
    assert_eq!(
        summary,
        [
            ("pino", "default"),
            ("pino", "transport"),
            ("execa", "execa"),
            ("execa", "$"),
            ("execa", "$"),
            ("execa", "$"),
        ]
    );
    assert_eq!(
        found[0].arguments,
        [ConfigValue::Object(vec![(
            "transport".to_owned(),
            ConfigValue::Object(vec![("target".to_owned(), s("pino-pretty"))])
        )])]
    );
    assert_eq!(
        found[2].arguments,
        [
            s("tsx"),
            ConfigValue::Array(vec![s("scripts/build.ts"), ConfigValue::Unknown])
        ]
    );
    assert_eq!(found[3].template.as_deref(), Some("node ./bin/cli.js $__skif__"));
    // `$({ cwd })` is a call of `$` and then the tag of a template.
    assert_eq!(found[4].template.as_deref(), Some("vite build"));
    assert!(found[5].template.is_none());
}

#[test]
fn required_modules_and_globals_count() {
    let source = r"
        const { spawn } = require('nano-spawn');
        spawn('eslint', ['.']);
        $`prettier --check .`;
    ";
    let found = calls(source, &["nano-spawn"], &[("$", "zx")]);
    assert_eq!(found.len(), 2);
    assert_eq!(
        (found[0].module.as_str(), found[0].export.as_str()),
        ("nano-spawn", "spawn")
    );
    assert_eq!((found[1].module.as_str(), found[1].export.as_str()), ("zx", "$"));
    assert_eq!(calls("not valid (", &["x"], &[]), []);
    assert_eq!(calls("foo()", &["x"], &[]), []);
}

/// A global written `Bun.*` is the module object: its members are the module's exports, like those of an import.
#[test]
fn a_global_module_object_calls_its_exports() {
    let source = r#"
        import { $ as sh } from "bun";
        await Bun.build({ entrypoints: ["./src/worker.ts"] });
        await Bun.$`bun ./script/copy.ts`;
        await sh`bunx tsc`;
        Bun.spawn(["bun", "x.ts"]);
        Other.build({});
    "#;
    let found = calls(source, &["bun"], &[("Bun.*", "bun")]);
    let summary: Vec<(&str, &str)> = found.iter().map(|c| (c.module.as_str(), c.export.as_str())).collect();
    assert_eq!(summary, [("bun", "build"), ("bun", "$"), ("bun", "$"), ("bun", "spawn")]);
    assert_eq!(
        found[0].arguments,
        [ConfigValue::Object(vec![(
            "entrypoints".to_owned(),
            ConfigValue::Array(vec![s("./src/worker.ts")])
        )])]
    );
    assert_eq!(found[1].template.as_deref(), Some("bun ./script/copy.ts"));
    // Without the `.*` suffix a global is a function, and a member of it is no call of the module.
    assert_eq!(calls("Bun.build({});", &[], &[("Bun", "bun")]), []);
}

/// Only the calls that run a command read their `cwd` option as the directory the command runs in: any other call keeps
/// the option as the source evaluates it.
#[test]
fn the_cwd_option_is_read_as_a_directory_only_for_command_calls() {
    let source = "import { run } from 'lib';\nrun({ cwd: '.' });\nrun({ cwd: __dirname });\n";
    let cwd = |call: &ImportedCall| call.arguments.first().and_then(|options| options.get("cwd")).cloned();
    let plain = calls(source, &["lib"], &[]);
    assert_eq!(cwd(&plain[0]), Some(s(".")));
    let kept = cwd(&plain[1]);
    assert!(
        kept.as_ref()
            .and_then(ConfigValue::as_str)
            .is_some_and(|dir| !dir.starts_with(FILE_DIR)),
        "{kept:?}"
    );
    let commands = extract_command_calls(source, Path::new("src/run.ts"), &["lib"], &[]);
    assert_eq!(cwd(&commands[0]), None);
    let marked = cwd(&commands[1]);
    assert!(
        marked
            .as_ref()
            .and_then(ConfigValue::as_str)
            .is_some_and(|dir| dir.starts_with(FILE_DIR)),
        "{marked:?}"
    );
}
