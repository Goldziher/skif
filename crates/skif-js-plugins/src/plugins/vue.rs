//! Vue: the `configureWebpack` object of `vue.config.*` is a webpack config, and `vue-cli-service lint` runs
//! `@vue/cli-plugin-eslint`. A function `configureWebpack` is called statically with a base config, as knip calls it:
//! the object it returns, or the base config as it leaves it (`config.entry = { … }`), is the webpack config.

use skif_js_syntax::{ConfigValue, extract_property_calls};

use crate::files::find_files;
use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::plugins::webpack::bundler_inputs;
use crate::support::objects;

/// The `vue` plugin.
pub struct Vue;

static SPEC: PluginSpec = PluginSpec {
    id: "vue",
    enablers: &[Enabler::Dependency("vue")],
    config: &["vue.config.{js,ts,mjs}"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Vue {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let Some(webpack) = config.value.get("configureWebpack") else {
            return Vec::new();
        };
        if !webpack.is_unknown() {
            return objects(webpack)
                .into_iter()
                .flat_map(|value| bundler_inputs(&ConfigAst { value, ..*config }, cx))
                .collect();
        }
        // What the function does not let be read (a plugin pushed onto the config) leaves the entries as they were:
        // Vue CLI takes its entries from `pages` and `src/main`, not from the webpack config. Only an `entry` the
        // function sets to something unknown (`config.entry = entries()`) hides entries; a change the evaluator does
        // not follow inside `entry` (`config.entry.app = x`) is not seen.
        extract_property_calls(config.source, config.path, "configureWebpack", &base_config())
            .iter()
            .flat_map(|value| {
                let sets_unknown_entry = value.get("entry").is_some_and(ConfigValue::has_unknown);
                bundler_inputs(&ConfigAst { value, ..*config }, cx)
                    .into_iter()
                    .filter(move |input| sets_unknown_entry || *input != PluginInput::EntriesIncomplete)
            })
            .collect()
    }

    fn resolve_workspace(&self, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        if cx.scripts.values().any(|script| runs_lint(script)) {
            vec![PluginInput::Dependency("@vue/cli-plugin-eslint".to_owned())]
        } else {
            Vec::new()
        }
    }
}

/// The webpack config a function `configureWebpack` is called with: knip's base config, a development build with no
/// entries, aliases, plugins or rules.
fn base_config() -> ConfigValue {
    let object = |entries: Vec<(&str, ConfigValue)>| {
        ConfigValue::Object(
            entries
                .into_iter()
                .map(|(key, value)| (key.to_owned(), value))
                .collect(),
        )
    };
    object(vec![
        ("mode", ConfigValue::String("development".to_owned())),
        ("entry", object(Vec::new())),
        ("resolve", object(Vec::new())),
        ("plugins", ConfigValue::Array(Vec::new())),
        ("module", object(vec![("rules", ConfigValue::Array(Vec::new()))])),
    ])
}

/// Whether a command of `script` is `vue-cli-service lint …`: `lint` is its first positional argument, the command.
fn runs_lint(script: &str) -> bool {
    let words: Vec<&str> = script.split_whitespace().collect();
    words.iter().enumerate().any(|(index, word)| {
        word.rsplit('/').next() == Some("vue-cli-service")
            && words[index + 1..]
                .iter()
                .take_while(|word| !matches!(**word, "&&" | "||" | ";" | "|" | "&"))
                .find(|word| !word.starts_with('-'))
                == Some(&"lint")
    })
}

/// The declaration files a Vue code generator wrote in the workspace (`components.d.ts`, `auto-imports.d.ts`): the
/// `.d.ts` files at the top level or under `src` whose header carries `signature`. They import what the generated
/// globals stand for, so they are entries.
pub(crate) fn generated_declarations(cx: &PluginContext<'_>, signature: &str) -> Vec<String> {
    let patterns = ["*.d.ts".to_owned(), "src/**/*.d.ts".to_owned()];
    find_files(cx.fs, cx.dir, &patterns)
        .into_iter()
        .filter(|file| {
            cx.fs.read_to_string(&cx.dir.join(file)).is_ok_and(|text| {
                let end = text.char_indices().nth(300).map_or(text.len(), |(index, _)| index);
                text[..end].contains(signature)
            })
        })
        .collect()
}
