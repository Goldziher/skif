//! Where a dependency is listed and where the code uses it: `skif --trace-dependency`.

use skif_core::HashSet;
use std::fmt::Write as _;

use serde::Serialize;
use skif_js_syntax::ImportKind;

use crate::analysis::JsAnalysis;
use crate::graph::ModuleId;

/// A manifest that lists the dependency.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub struct DependencyListing {
    /// The package whose manifest lists it, by name or, without one, by directory.
    pub package: String,
    /// The manifest, relative to the workspace root.
    pub manifest: String,
    /// The section: `dependencies`, `devDependencies`, `peerDependencies` or `optionalDependencies`.
    pub section: &'static str,
    /// The version specifier as written.
    pub specifier: String,
}

/// One import of the dependency.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub struct DependencyImport {
    /// The importing file, relative to the workspace root.
    pub file: String,
    /// 1-based line of the import.
    pub line: u32,
    /// The specifier as written (`pkg`, `pkg/sub`).
    pub specifier: String,
    /// How the import is written.
    pub kind: ImportKind,
    /// The import brings in types only.
    pub type_only: bool,
    /// The package that owns the importing file.
    pub package: String,
    /// Whether an entry file reaches the importing file.
    pub reachable: bool,
}

/// What [`JsAnalysis::trace_dependency`] returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub struct DependencyTrace {
    /// The dependency traced.
    pub dependency: String,
    /// Every manifest that lists it.
    pub listed_in: Vec<DependencyListing>,
    /// Every import of it, or of a subpath of it, in the module graph.
    pub imported_by: Vec<DependencyImport>,
    /// The packages that count it as used: through an import, a script, a tool's configuration or its `@types` twin.
    pub used_by: Vec<String>,
}

impl DependencyTrace {
    /// The trace as text for a terminal.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "{}", self.dependency);
        if self.listed_in.is_empty() {
            let _ = writeln!(out, "  listed by no manifest of the workspace");
        } else {
            let _ = writeln!(out, "  listed in:");
            for listing in &self.listed_in {
                let _ = writeln!(
                    out,
                    "    {} {} \"{}\" ({})",
                    listing.manifest, listing.section, listing.specifier, listing.package
                );
            }
        }
        if self.imported_by.is_empty() {
            let _ = writeln!(out, "  imported by no file of the module graph");
        } else {
            let _ = writeln!(out, "  imported by:");
            for import in &self.imported_by {
                let mut notes = Vec::new();
                if import.specifier != self.dependency {
                    notes.push(format!("as `{}`", import.specifier));
                }
                if import.type_only {
                    notes.push("types only".to_owned());
                }
                if !import.reachable {
                    notes.push("not reached from an entry file".to_owned());
                }
                let notes = if notes.is_empty() {
                    String::new()
                } else {
                    format!(", {}", notes.join(", "))
                };
                let _ = writeln!(out, "    {}:{} ({}{notes})", import.file, import.line, import.package);
            }
        }
        if self.used_by.is_empty() {
            let _ = writeln!(out, "  used by no package");
        } else {
            let _ = writeln!(out, "  used by: {}", self.used_by.join(", "));
        }
        out
    }
}

fn slashed(path: &std::path::Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

impl JsAnalysis<'_> {
    /// Where the dependency `name` is listed, imported and counted as used.
    #[must_use]
    pub fn trace_dependency(&self, name: &str) -> DependencyTrace {
        let label = |id| {
            self.workspace.package(id).map_or_else(String::new, |package| {
                package.name.clone().unwrap_or_else(|| {
                    let dir = slashed(&package.dir);
                    if dir.is_empty() { ".".to_owned() } else { dir }
                })
            })
        };
        let listed_in = self
            .workspace
            .iter()
            .flat_map(|(id, package)| {
                let manifest = slashed(&package.dir.join("package.json"));
                package
                    .dependencies
                    .iter()
                    .filter(|entry| entry.name == name)
                    .map(move |entry| DependencyListing {
                        package: label(id),
                        manifest: manifest.clone(),
                        section: entry.kind.field(),
                        specifier: entry.specifier.clone(),
                    })
            })
            .collect();
        let reachable: HashSet<ModuleId> = self.reachable_from_entries();
        let mut imported_by = Vec::new();
        for module in self.graph.modules() {
            for resolved in &module.imports {
                if resolved.package.as_deref() != Some(name) {
                    continue;
                }
                let Some(import) = module.info.imports.get(resolved.import) else {
                    continue;
                };
                imported_by.push(DependencyImport {
                    file: slashed(&module.path),
                    line: import.span.start_line,
                    specifier: import.specifier.clone(),
                    kind: import.kind,
                    type_only: resolved.type_only,
                    package: label(module.owner),
                    reachable: reachable.contains(&module.id),
                });
            }
        }
        imported_by.sort_by(|a, b| a.file.cmp(&b.file).then(a.line.cmp(&b.line)));
        let used_by = self
            .workspace
            .package_ids()
            .filter(|id| self.dependencies.is_referenced(*id, name))
            .map(label)
            .collect();
        DependencyTrace {
            dependency: name.to_owned(),
            listed_in,
            imported_by,
            used_by,
        }
    }
}
