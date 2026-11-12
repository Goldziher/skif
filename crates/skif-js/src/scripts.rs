//! What the `scripts` of a package run.

use indexmap::{IndexMap, IndexSet};
use serde_json::Value;
use skif_js_plugins::scripts::{FileTree, ScriptContext, ScriptInfo, is_ignored_binary, parse_script};
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

/// A binary a script runs in another directory (`cd sub && rspack`, `pnpm -C sub exec rspack`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct RelocatedBinary {
    /// The directory, relative to the package directory.
    pub dir: String,
    pub binary: ScriptBinary,
    /// Fetched on demand (`npx`).
    pub ephemeral: bool,
    /// The command also targets another workspace without naming it (`pnpm -C sub exec`), so the package itself does
    /// not run it.
    pub filtered: bool,
}

/// A package a script, or a command of a tool's file, needs for a command run in another directory, as
/// [`RelocatedBinary`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct RelocatedDependency {
    /// The directory, relative to the package directory.
    pub dir: String,
    pub name: String,
}

/// Binaries, files and packages referenced by a package's scripts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ScriptFacts {
    pub binaries: Vec<ScriptBinary>,
    /// System binaries the scripts run directly (`rm`, `curl`, `git`), assumed to be installed globally: reported
    /// only with `[js] ignore-global-binaries = false`.
    pub global_binaries: Vec<ScriptBinary>,
    /// Binaries fetched on demand (`npx x`, `pnpm dlx x`): a listed provider is used, nothing is ever unlisted.
    pub ephemeral_binaries: Vec<ScriptBinary>,
    /// Binaries run in named workspaces; the package they belong to is only known with the whole workspace.
    pub forwarded: Vec<ForwardedBinary>,
    /// Binaries run in another directory: they belong to the package or project that directory is in, known only with
    /// the whole workspace. Once the setup has credited the ones fetched on demand, the rest are judged there.
    pub relocated_binaries: Vec<RelocatedBinary>,
    /// Packages needed by commands run in another directory, as [`Self::relocated_binaries`].
    pub relocated_dependencies: Vec<RelocatedDependency>,
    /// Package managers the scripts invoke, which a manifest may list as a dependency.
    pub package_managers: Vec<String>,
    /// Files the scripts run, relative to the package directory.
    pub files: Vec<String>,
    /// How a test runner reads the `files` it is given, where a directory stands for the files the runner reads in it.
    pub file_trees: IndexMap<String, FileTree>,
    pub dependencies: Vec<String>,
}

/// Whether a `scripts` key is a comment: npm has no comment syntax, so projects write `"#note"` or `"//note"` keys
/// whose values are prose, not commands.
fn is_comment_key(name: &str) -> bool {
    name.starts_with('#') || name.starts_with("//")
}

/// Records how a test runner reads `path`; when several scripts pass it, the widest reading wins: recursive if any
/// is, and every extension any of them reads.
fn merge_tree(trees: &mut IndexMap<String, FileTree>, path: &str, tree: FileTree) {
    let Some(known) = trees.get_mut(path) else {
        trees.insert(path.to_owned(), tree);
        return;
    };
    known.recursive |= tree.recursive;
    for extension in tree.extensions {
        if !known.extensions.contains(&extension) {
            known.extensions.push(extension);
        }
    }
}

pub(crate) fn analyze(package: &Package) -> ScriptFacts {
    analyze_manifest(package.manifest.value())
}

/// The `scripts` of the parsed `package.json` `manifest` that are commands, in order: comment keys and values that
/// are not strings are left out.
fn commands(manifest: &Value) -> Vec<(&String, &str)> {
    manifest
        .get("scripts")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter(|(name, _)| !is_comment_key(name))
        .filter_map(|(name, body)| Some((name, body.as_str()?)))
        .collect()
}

/// The script parser's context for a command the package of `manifest` runs: its scripts, so that `pnpm build` or
/// `npm run build` follows the package's `build` script instead of naming a binary `build`.
pub(crate) fn script_context(manifest: &Value) -> ScriptContext {
    commands(manifest)
        .into_iter()
        .fold(ScriptContext::new(), |context, (name, body)| {
            context.with_script(name, body)
        })
}

/// What the `scripts` of the parsed `package.json` `manifest` run.
pub(crate) fn analyze_manifest(manifest: &Value) -> ScriptFacts {
    let commands = commands(manifest);
    let context = script_context(manifest);
    let mut collected = Collected::default();
    for (name, body) in commands {
        collected.add(name, parse_script(body, &context.clone().with_current_script(name)));
    }
    collected.into_facts()
}

/// [`ScriptFacts`] being gathered script by script, without duplicates and in script order.
#[derive(Default)]
struct Collected {
    binaries: IndexSet<ScriptBinary>,
    ephemeral_binaries: IndexSet<ScriptBinary>,
    global_binaries: IndexSet<ScriptBinary>,
    forwarded: IndexSet<ForwardedBinary>,
    relocated_binaries: IndexSet<RelocatedBinary>,
    relocated_dependencies: IndexSet<RelocatedDependency>,
    package_managers: IndexSet<String>,
    files: IndexSet<String>,
    file_trees: IndexMap<String, FileTree>,
    dependencies: IndexSet<String>,
}

impl Collected {
    /// Adds what the script `name` runs.
    fn add(&mut self, name: &str, info: ScriptInfo) {
        let binary = |binary: String| ScriptBinary {
            name: binary,
            script: name.to_owned(),
        };
        for used in info.binaries {
            if is_ignored_binary(&used.name) {
                continue;
            }
            if used.filtered && !used.workspaces.is_empty() {
                self.forwarded.insert(ForwardedBinary {
                    targets: used.workspaces,
                    binary: binary(used.name),
                });
                continue;
            }
            if let Some(dir) = used.relocation_dir().map(str::to_owned) {
                self.relocated_binaries.insert(RelocatedBinary {
                    dir,
                    binary: binary(used.name),
                    ephemeral: used.via.is_ephemeral(),
                    filtered: used.filtered,
                });
                continue;
            }
            if used.filtered {
                continue;
            }
            let target = if used.via.is_ephemeral() {
                &mut self.ephemeral_binaries
            } else {
                &mut self.binaries
            };
            target.insert(binary(used.name));
        }
        for used in info.global_binaries {
            if !used.filtered && !used.via.is_ephemeral() {
                self.global_binaries.insert(binary(used.name));
            }
        }
        self.package_managers.extend(info.package_managers);
        for file in info.files {
            if let Some(tree) = file.tree {
                merge_tree(&mut self.file_trees, &file.path, tree);
            }
            self.files.insert(file.path);
        }
        for dependency in info.dependencies.into_iter().filter(|dependency| !dependency.ephemeral) {
            match dependency.dir {
                Some(dir) => {
                    self.relocated_dependencies.insert(RelocatedDependency {
                        dir,
                        name: dependency.name,
                    });
                }
                None => {
                    self.dependencies.insert(dependency.name);
                }
            }
        }
    }

    fn into_facts(self) -> ScriptFacts {
        ScriptFacts {
            binaries: self.binaries.into_iter().collect(),
            ephemeral_binaries: self.ephemeral_binaries.into_iter().collect(),
            global_binaries: self.global_binaries.into_iter().collect(),
            forwarded: self.forwarded.into_iter().collect(),
            relocated_binaries: self.relocated_binaries.into_iter().collect(),
            relocated_dependencies: self.relocated_dependencies.into_iter().collect(),
            package_managers: self.package_managers.into_iter().collect(),
            files: self.files.into_iter().collect(),
            file_trees: self.file_trees,
            dependencies: self.dependencies.into_iter().collect(),
        }
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
    fn commands_run_in_another_directory_are_relocated() {
        let facts = analyze(&package(
            r#"{"name":"p","scripts":{"a":"cd sub && rspack serve && node -r @x/hook ./x.js","b":"pnpm -C web exec vite","c":"tsc"}}"#,
        ));
        let own: Vec<&str> = facts.binaries.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(own, ["tsc"]);
        let relocated: Vec<(&str, &str, bool)> = facts
            .relocated_binaries
            .iter()
            .map(|r| (r.dir.as_str(), r.binary.name.as_str(), r.filtered))
            .collect();
        assert_eq!(relocated, [("sub", "rspack", false), ("web", "vite", true)]);
        let dependencies: Vec<(&str, &str)> = facts
            .relocated_dependencies
            .iter()
            .map(|r| (r.dir.as_str(), r.name.as_str()))
            .collect();
        assert_eq!(dependencies, [("sub", "@x/hook")]);
        assert!(facts.dependencies.is_empty(), "{:?}", facts.dependencies);
        assert_eq!(facts.files, ["sub/x.js"]);
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
