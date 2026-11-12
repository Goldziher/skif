//! AWS CDK: the `app` command of `cdk.json` and of `cdk -a "<command>"` is a script whose file is the entry, and so is
//! the `entry` of each `NodejsFunction` construct.

use skif_js_syntax::{extract_constructor_options, is_declaration_file};

use crate::files::find_files;
use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::script_config::flag_values;
use crate::support::join_rel;

/// The `aws-cdk` plugin.
pub struct AwsCdk;

static SPEC: PluginSpec = PluginSpec {
    id: "aws-cdk",
    enablers: &[Enabler::Dependency("aws-cdk"), Enabler::Dependency("aws-cdk-lib")],
    config: &["cdk.json"],
    binaries: &["cdk"],
    optional_binaries: true,
    ..PluginSpec::DEFAULT
};

impl Plugin for AwsCdk {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        config
            .value
            .get("app")
            .and_then(|app| app.as_str())
            .map(|command| PluginInput::Script(command.to_string()))
            .into_iter()
            .collect()
    }

    fn resolve_workspace(&self, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out: Vec<PluginInput> = cx
            .scripts
            .values()
            .flat_map(|script| flag_values(script, &["cdk"], APP_FLAGS))
            .map(PluginInput::Script)
            .collect();
        out.extend(lambda_entries(cx));
        out
    }
}

/// The options of `cdk` that take the command that runs the app.
const APP_FLAGS: &[&str] = &["-a", "--app"];

/// The `entry` of every `new NodejsFunction(…, { entry })` in the workspace's source files. A function with an entry
/// that cannot be evaluated leaves the entries incomplete.
fn lambda_entries(cx: &PluginContext<'_>) -> Vec<PluginInput> {
    let patterns = ["**/*.{ts,mts,cts,tsx,js,mjs,cjs}".to_owned()];
    let mut out = Vec::new();
    for source in find_files(cx.fs, cx.globs, cx.dir, &patterns) {
        let path = cx.dir.join(&source);
        let Ok(text) = cx.fs.read_to_string(&path) else {
            continue;
        };
        if is_declaration_file(&source) || !text.contains("NodejsFunction") {
            continue;
        }
        let dir = source.rsplit_once('/').map_or("", |(dir, _)| dir);
        for entry in extract_constructor_options(&text, &path, "NodejsFunction", "entry") {
            match entry.as_str() {
                Some(entry) if !entry.starts_with('/') => out.push(PluginInput::Entry(join_rel(dir, entry))),
                _ => out.push(PluginInput::EntriesIncomplete),
            }
            out.push(PluginInput::OptionalDependency("esbuild".to_owned()));
        }
    }
    out
}
