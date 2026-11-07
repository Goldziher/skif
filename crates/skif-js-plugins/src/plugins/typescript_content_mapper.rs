//! TypeScript Content Mapper: the command `package.json#typescript.contentMapper.exec` runs is a script.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};

/// The `typescript-content-mapper` plugin.
pub struct TypescriptContentMapper;

static SPEC: PluginSpec = PluginSpec {
    id: "typescript-content-mapper",
    enablers: &[Enabler::ManifestKey("typescript.contentMapper")],
    package_json_key: Some("typescript.contentMapper"),
    ..PluginSpec::DEFAULT
};

impl Plugin for TypescriptContentMapper {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let Some(exec) = config.value.get("exec").and_then(ConfigValue::as_array) else {
            return Vec::new();
        };
        let words: Option<Vec<String>> = exec
            .iter()
            .map(|word| {
                word.as_str().map(|word| {
                    if word.is_empty() || word.contains(|c: char| c.is_whitespace() || "'\"$`\\".contains(c)) {
                        format!("'{}'", word.replace('\'', r"'\''"))
                    } else {
                        word.to_owned()
                    }
                })
            })
            .collect();
        words
            .filter(|words| !words.is_empty())
            .map(|words| PluginInput::Script(words.join(" ")))
            .into_iter()
            .collect()
    }
}
