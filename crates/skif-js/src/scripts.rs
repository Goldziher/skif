//! What the `scripts` of a package run.

use indexmap::IndexSet;
use serde_json::Value;
use skif_js_plugins::scripts::{ScriptContext, is_ignored_binary, parse_script};
use skif_workspace::Package;

/// A binary a script runs.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct ScriptBinary {
    pub name: String,
    /// The script that runs it.
    pub script: String,
}

/// A binary a script runs in other workspaces (`pnpm --filter web exec vitest`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct ForwardedBinary {
    /// The workspaces it names: package names, patterns or directories.
    pub targets: Vec<String>,
    pub binary: ScriptBinary,
}

/// Binaries, files and packages referenced by a package's scripts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ScriptFacts {
    pub binaries: Vec<ScriptBinary>,
    /// Binaries fetched on demand (`npx x`, `pnpm dlx x`): a listed provider is used, nothing is ever unlisted.
    pub ephemeral_binaries: Vec<ScriptBinary>,
    /// Binaries run in named workspaces; the package they belong to is only known with the whole workspace.
    pub forwarded: Vec<ForwardedBinary>,
    /// Package managers the scripts invoke, which a manifest may list as a dependency.
    pub package_managers: Vec<String>,
    /// Files the scripts run, relative to the package directory.
    pub files: Vec<String>,
    pub dependencies: Vec<String>,
}

/// Whether a `scripts` key is a comment: npm has no comment syntax, so projects write `"#note"` or `"//note"` keys
/// whose values are prose, not commands.
fn is_comment_key(name: &str) -> bool {
    name.starts_with('#') || name.starts_with("//")
}

pub(crate) fn analyze(package: &Package) -> ScriptFacts {
    let Some(scripts) = package.manifest.value().get("scripts").and_then(Value::as_object) else {
        return ScriptFacts::default();
    };
    let commands: Vec<(&String, &str)> = scripts
        .iter()
        .filter(|(name, _)| !is_comment_key(name))
        .filter_map(|(name, body)| Some((name, body.as_str()?)))
        .collect();
    let context = commands.iter().fold(ScriptContext::new(), |context, &(name, body)| {
        context.with_script(name, body)
    });
    let mut binaries = IndexSet::new();
    let mut ephemeral_binaries = IndexSet::new();
    let mut forwarded = IndexSet::new();
    let mut package_managers = IndexSet::new();
    let mut files = IndexSet::new();
    let mut dependencies = IndexSet::new();
    for (name, body) in commands {
        let info = parse_script(body, &context.clone().with_current_script(name));
        for binary in info.binaries {
            if is_ignored_binary(&binary.name) {
                continue;
            }
            if binary.filtered {
                if !binary.workspaces.is_empty() {
                    forwarded.insert(ForwardedBinary {
                        targets: binary.workspaces,
                        binary: ScriptBinary {
                            name: binary.name,
                            script: name.clone(),
                        },
                    });
                }
                continue;
            }
            let target = if binary.via.is_ephemeral() {
                &mut ephemeral_binaries
            } else {
                &mut binaries
            };
            target.insert(ScriptBinary {
                name: binary.name,
                script: name.clone(),
            });
        }
        package_managers.extend(info.package_managers);
        files.extend(info.files.into_iter().map(|file| file.path));
        dependencies.extend(
            info.dependencies
                .into_iter()
                .filter(|dependency| !dependency.ephemeral)
                .map(|dependency| dependency.name),
        );
    }
    ScriptFacts {
        binaries: binaries.into_iter().collect(),
        ephemeral_binaries: ephemeral_binaries.into_iter().collect(),
        forwarded: forwarded.into_iter().collect(),
        package_managers: package_managers.into_iter().collect(),
        files: files.into_iter().collect(),
        dependencies: dependencies.into_iter().collect(),
    }
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
        assert_eq!(facts.ephemeral_binaries.len(), 1);
        assert_eq!(facts.ephemeral_binaries[0].name, "cowsay");
        assert!(
            facts.files.iter().any(|f| f.ends_with("scripts/build.js")),
            "{:?}",
            facts.files
        );
    }

    #[test]
    fn binaries_run_in_named_workspaces_are_forwarded() {
        let facts = analyze(&package(
            r#"{"name":"p","scripts":{"a":"pnpm --filter web exec vitest","b":"yarn workspace sub run biome check","c":"pnpm -r exec tsc"}}"#,
        ));
        // `pnpm -r exec` runs in every package, the root included, so the root uses the binary itself.
        let own: Vec<&str> = facts.binaries.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(own, ["tsc"]);
        let forwarded: Vec<(&str, &[String])> = facts
            .forwarded
            .iter()
            .map(|f| (f.binary.name.as_str(), f.targets.as_slice()))
            .collect();
        assert_eq!(
            forwarded,
            [("vitest", &["web".to_owned()][..]), ("biome", &["sub".to_owned()][..])]
        );
    }

    #[test]
    fn comment_keys_are_not_commands() {
        let facts = analyze(&package(
            r##"{"name":"p","scripts":{"#note":"The build runs first","//test":"Ensure the suite passes","test":"vitest run"}}"##,
        ));
        let names: Vec<&str> = facts.binaries.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, ["vitest"]);
    }

    #[test]
    fn no_scripts() {
        assert_eq!(analyze(&package(r#"{"name":"p"}"#)), ScriptFacts::default());
    }
}
