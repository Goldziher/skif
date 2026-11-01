//! What the `scripts` of a package run.

use serde_json::Value;
use skif_js_plugins::scripts::{ScriptContext, is_ignored_binary, parse_script};
use skif_workspace::Package;

/// A binary a script runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScriptBinary {
    pub name: String,
    /// The script that runs it.
    pub script: String,
}

/// Binaries, files and packages referenced by a package's scripts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ScriptFacts {
    pub binaries: Vec<ScriptBinary>,
    /// Files the scripts run, relative to the package directory.
    pub files: Vec<String>,
    pub dependencies: Vec<String>,
}

fn push_unique<T: PartialEq>(items: &mut Vec<T>, item: T) {
    if !items.contains(&item) {
        items.push(item);
    }
}

pub(crate) fn analyze(package: &Package) -> ScriptFacts {
    let mut facts = ScriptFacts::default();
    let Some(scripts) = package.manifest.value.get("scripts").and_then(Value::as_object) else {
        return facts;
    };
    let context = scripts
        .iter()
        .filter_map(|(name, body)| Some((name, body.as_str()?)))
        .fold(ScriptContext::new(), |context, (name, body)| context.with_script(name, body));
    for (name, body) in scripts {
        let Some(body) = body.as_str() else { continue };
        let info = parse_script(body, &context.clone().with_current_script(name));
        for binary in info.binaries {
            if binary.filtered || binary.via.is_ephemeral() || is_ignored_binary(&binary.name) {
                continue;
            }
            push_unique(
                &mut facts.binaries,
                ScriptBinary {
                    name: binary.name,
                    script: name.clone(),
                },
            );
        }
        for file in info.files {
            push_unique(&mut facts.files, file.path);
        }
        for dependency in info.dependencies {
            if !dependency.ephemeral {
                push_unique(&mut facts.dependencies, dependency.name);
            }
        }
    }
    facts
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn package(json: &str) -> Package {
        Package::parse("pkg".into(), Arc::from(json), false).unwrap()
    }

    #[test]
    fn binaries_files_and_dependencies() {
        let facts = analyze(&package(
            r#"{"name":"p","scripts":{"build":"tsc -p . && node scripts/build.js","test":"vitest run","dlx":"npx cowsay hi"}}"#,
        ));
        let names: Vec<&str> = facts.binaries.iter().map(|b| b.name.as_str()).collect();
        assert!(names.contains(&"tsc") && names.contains(&"vitest"), "{names:?}");
        assert!(!names.contains(&"cowsay"));
        assert!(facts.files.iter().any(|f| f.ends_with("scripts/build.js")), "{:?}", facts.files);
    }

    #[test]
    fn no_scripts() {
        assert_eq!(analyze(&package(r#"{"name":"p"}"#)), ScriptFacts::default());
    }
}
