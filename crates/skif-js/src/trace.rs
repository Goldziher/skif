//! Where a dependency is listed and where the code uses it: `skif --trace-dependency`, and per package why it is used
//! or not: `skif explain-usage --dependency`.
//!
//! [`JsAnalysis::explain_dependency`] reads the verdict and the evidence from the same record of uses that
//! `unused-dependencies` judges, and [`DependencyExplanation::render`] is the only place that turns it into text.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use serde::Serialize;
use skif_core::{DependencyKind, HashSet};
use skif_js_syntax::ImportKind;
use skif_workspace::{DependencyEntry, Package, PackageId};

use crate::analysis::JsAnalysis;
use crate::deps::{Credit, Use};
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

/// The verdict on a dependency in one package, as `unused-dependencies` and `unused-dev-dependencies` judge it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum DependencyVerdict {
    /// The package uses it; [`PackageDependencyUsage::credit`] says how.
    Used,
    /// The manifest lists it and nothing uses it: the rule reports it.
    Unused,
    /// Unused but not reported: an `ignore-dependencies` entry or a plugin covers it.
    Ignored,
    /// Unused but not reported: `peerDependenciesMeta` marks the peer dependency optional.
    OptionalPeer,
    /// Not judged: the package is excluded from the analysis.
    PackageExcluded,
    /// Not judged: the entry files of the package are not fully known.
    EntriesIncomplete,
    /// Unused but not reported: the dependency is on a workspace member that nothing can import or run, so it only
    /// orders tasks.
    OrdersTasks,
    /// The manifest does not list it and nothing credits it to the package.
    NotListed,
}

/// Why `unused-dependencies` does not report an unused dependency ([`JsAnalysis::unused_dependency_exemption`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Exemption {
    Ignored,
    OptionalPeer,
    PackageExcluded,
    EntriesIncomplete,
    OrdersTasks,
}

impl From<Exemption> for DependencyVerdict {
    fn from(exemption: Exemption) -> Self {
        match exemption {
            Exemption::Ignored => Self::Ignored,
            Exemption::OptionalPeer => Self::OptionalPeer,
            Exemption::PackageExcluded => Self::PackageExcluded,
            Exemption::EntriesIncomplete => Self::EntriesIncomplete,
            Exemption::OrdersTasks => Self::OrdersTasks,
        }
    }
}

/// Why a package counts the dependency as used.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
#[non_exhaustive]
pub enum DependencyCredit {
    /// [`PackageDependencyUsage::uses`] lists what uses it.
    Uses,
    /// A tool that is used without an import (`typescript`).
    AlwaysUsed,
    /// The types of a runtime that needs no package of its own (`@types/node`).
    RuntimeTypes,
    /// It is the `@types` package of `package`, which the package uses.
    TypesOf {
        /// The typed package.
        package: String,
    },
    /// It is a peer dependency of `host`, which the package uses.
    PeerOf {
        /// The package that needs it as a peer.
        host: String,
    },
}

/// What a use is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum UseKind {
    /// The file imports it, re-exports it, declares it as a module or imports the package it types.
    Import,
    /// The test file mocks it.
    Mock,
    /// It compiles the file's type (`.mdx`, `.marko`).
    Compiler,
    /// The file has JSX, which the automatic runtime compiles to an import of it.
    JsxRuntime,
    /// A command that a `node:child_process` call of the file runs names it.
    Command,
    /// It provides the binary that a script, a command or a plugin runs.
    Binary,
    /// A script of `package.json` or a plugin of the package names it.
    ScriptOrPlugin,
    /// The tool configuration file names it.
    Config,
    /// A `tsconfig.json` includes every visible `@types` package, and it declares only globals.
    AmbientTypes,
}

/// One use that credits the dependency to a package's manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub struct DependencyUse {
    /// What the use is.
    pub kind: UseKind,
    /// The file, relative to the workspace root, for the uses that have one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// The binary, for a [`UseKind::Binary`] use.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binary: Option<String>,
}

/// The dependency in one package: whether its manifest lists it, the verdict and the evidence for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub struct PackageDependencyUsage {
    /// The package, by name or, without one, by directory.
    pub package: String,
    /// The package directory, relative to the workspace root; `.` for the root.
    pub dir: String,
    /// The manifest entry, when the package lists it.
    pub listed: Option<DependencyListing>,
    /// The verdict.
    pub verdict: DependencyVerdict,
    /// Why the package counts it as used, when it does.
    pub credit: Option<DependencyCredit>,
    /// What credits it to this package's manifest, sorted; also the files of packages inside this one that use it
    /// through this manifest.
    pub uses: Vec<DependencyUse>,
    /// Imports of it by files of this package that credit no manifest: a type-only import in strict mode, or a file
    /// of another project.
    pub not_counted: Vec<DependencyImport>,
}

/// What [`JsAnalysis::explain_dependency`] returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub struct DependencyExplanation {
    /// The dependency explained.
    pub dependency: String,
    /// The analysis ran in production mode: tool configuration is not judged and only production entries count.
    pub production: bool,
    /// The analysis ran in strict mode: type-only imports use nothing, and production code uses only its own
    /// package's `dependencies` and `peerDependencies`.
    pub strict: bool,
    /// Every package that lists it or that something credits it to, in workspace order; with a package asked for,
    /// that package alone.
    pub packages: Vec<PackageDependencyUsage>,
}

impl DependencyExplanation {
    /// The explanation as text for a terminal.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "{}", self.dependency);
        if self.production {
            let _ = writeln!(
                out,
                "  production mode: tool configuration is not judged and only production entries count"
            );
        }
        if self.strict {
            let _ = writeln!(
                out,
                "  strict mode: type-only imports use nothing, production code only its own package's dependencies"
            );
        }
        if self.packages.is_empty() {
            let _ = writeln!(out, "  no package lists it or uses it");
        }
        for package in &self.packages {
            render_package(&mut out, package);
        }
        out
    }
}

fn render_package(out: &mut String, package: &PackageDependencyUsage) {
    let verdict = match (&package.verdict, &package.credit) {
        (DependencyVerdict::Used, Some(DependencyCredit::AlwaysUsed)) => {
            "used: a tool that is used without an import".to_owned()
        }
        (DependencyVerdict::Used, Some(DependencyCredit::RuntimeTypes)) => {
            "used: the types of a runtime that needs no package".to_owned()
        }
        (DependencyVerdict::Used, Some(DependencyCredit::TypesOf { package })) => {
            format!("used: the types of `{package}`, which the package uses")
        }
        (DependencyVerdict::Used, Some(DependencyCredit::PeerOf { host })) => {
            format!("used: a peer dependency of `{host}`, which the package uses")
        }
        (DependencyVerdict::Used, _) => "used".to_owned(),
        (DependencyVerdict::Unused, _) => "unused: nothing uses it".to_owned(),
        (DependencyVerdict::Ignored, _) => "unused, not reported: ignored by the configuration or a plugin".to_owned(),
        (DependencyVerdict::OptionalPeer, _) => "unused, not reported: an optional peer dependency".to_owned(),
        (DependencyVerdict::PackageExcluded, _) => "not judged: the package is excluded".to_owned(),
        (DependencyVerdict::EntriesIncomplete, _) => {
            "not judged: the entry files of the package are not fully known".to_owned()
        }
        (DependencyVerdict::OrdersTasks, _) => {
            "unused, not reported: a workspace member nothing can import or run, listed to order tasks".to_owned()
        }
        (DependencyVerdict::NotListed, _) => "not listed".to_owned(),
    };
    let _ = writeln!(out, "  {} ({}): {verdict}", package.dir, package.package);
    if let Some(listing) = &package.listed {
        let _ = writeln!(
            out,
            "    listed in {} {} \"{}\"",
            listing.manifest, listing.section, listing.specifier
        );
    }
    if !package.uses.is_empty() {
        let _ = writeln!(out, "    used by:");
    }
    for use_ in &package.uses {
        let text = match use_.kind {
            UseKind::Import => "import in",
            UseKind::Mock => "mock in",
            UseKind::Compiler => "compiles",
            UseKind::JsxRuntime => "JSX runtime of",
            UseKind::Command => "command in",
            UseKind::Binary => "provides the binary",
            UseKind::ScriptOrPlugin => "a script or plugin of the package",
            UseKind::Config => "named by the config",
            UseKind::AmbientTypes => "global types a tsconfig.json includes",
        };
        let subject = use_
            .file
            .as_deref()
            .map(str::to_owned)
            .or_else(|| use_.binary.as_deref().map(|binary| format!("`{binary}`")));
        match subject {
            Some(subject) => {
                let _ = writeln!(out, "      {text} {subject}");
            }
            None => {
                let _ = writeln!(out, "      {text}");
            }
        }
    }
    if !package.not_counted.is_empty() {
        let _ = writeln!(out, "    imports that count for no manifest:");
    }
    for import in &package.not_counted {
        let types = if import.type_only { ", types only" } else { "" };
        let _ = writeln!(
            out,
            "      {}:{} (as `{}`{types})",
            import.file, import.line, import.specifier
        );
    }
}

fn slashed(path: &std::path::Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// The directory of `package` relative to the workspace root, `.` for the root.
pub(crate) fn dir_of(package: &Package) -> String {
    let dir = slashed(&package.dir);
    if dir.is_empty() { ".".to_owned() } else { dir }
}

/// A package by name or, without one, by directory.
fn label(package: &Package) -> String {
    package.name.clone().unwrap_or_else(|| dir_of(package))
}

impl JsAnalysis<'_> {
    /// Where the dependency `name` is listed, imported and counted as used.
    #[must_use]
    pub fn trace_dependency(&self, name: &str) -> DependencyTrace {
        let listed_in = self
            .workspace
            .iter()
            .flat_map(|(_, package)| {
                package
                    .dependencies
                    .iter()
                    .filter(|entry| &*entry.name == name)
                    .map(move |entry| listing(package, entry))
            })
            .collect();
        let imported_by = self.imports_of(name).into_iter().map(|(_, import)| import).collect();
        let used_by = self
            .workspace
            .iter()
            .filter(|(id, _)| self.dependencies.is_referenced(*id, name))
            .map(|(_, package)| label(package))
            .collect();
        DependencyTrace {
            dependency: name.to_owned(),
            listed_in,
            imported_by,
            used_by,
        }
    }

    /// Why the dependency `name` is used or unused in every package that lists it or that something credits it to,
    /// or with `package` in that package alone: the verdict `unused-dependencies` reaches, the uses that credit it
    /// and the imports of it that credit nothing.
    #[must_use]
    pub fn explain_dependency(&self, name: &str, package: Option<PackageId>) -> DependencyExplanation {
        let imports = self.imports_of(name);
        let counted: BTreeSet<ModuleId> = self
            .workspace
            .package_ids()
            .filter_map(|id| self.dependencies.uses(id, name))
            .flatten()
            .filter_map(|use_| match use_ {
                Use::Import(module) | Use::Mock(module) => Some(*module),
                _ => None,
            })
            .collect();
        let packages = self
            .workspace
            .iter()
            .filter(|(id, candidate)| match package {
                Some(asked) => *id == asked,
                None => {
                    candidate.dependencies.iter().any(|entry| &*entry.name == name)
                        || self.dependencies.uses(*id, name).is_some()
                }
            })
            .map(|(id, candidate)| self.package_usage(id, candidate, name, &imports, &counted))
            .collect();
        DependencyExplanation {
            dependency: name.to_owned(),
            production: self.setup.production,
            strict: self.setup.isolate_workspaces,
            packages,
        }
    }

    fn package_usage(
        &self,
        id: PackageId,
        package: &Package,
        name: &str,
        imports: &[(ModuleId, DependencyImport)],
        counted: &BTreeSet<ModuleId>,
    ) -> PackageDependencyUsage {
        let entry = package.dependencies.iter().find(|entry| &*entry.name == name);
        let credit = self.dependencies.credit(id, name);
        let verdict = match (&credit, entry) {
            (Some(_), _) => DependencyVerdict::Used,
            (None, None) => DependencyVerdict::NotListed,
            (None, Some(entry)) => self
                .unused_dependency_exemption(id, package, entry)
                .map_or(DependencyVerdict::Unused, DependencyVerdict::from),
        };
        let uses = match &credit {
            Some(Credit::Uses(uses)) => uses.iter().map(|use_| self.describe_use(use_)).collect(),
            _ => Vec::new(),
        };
        let credit = credit.map(|credit| match credit {
            Credit::Uses(_) => DependencyCredit::Uses,
            Credit::AlwaysUsed => DependencyCredit::AlwaysUsed,
            Credit::UnpairedTypes => DependencyCredit::RuntimeTypes,
            Credit::TypesOf(package) => DependencyCredit::TypesOf { package },
            Credit::PeerOf(host) => DependencyCredit::PeerOf { host },
        });
        let not_counted = imports
            .iter()
            .filter(|(module, _)| self.graph.module(*module).owner == id && !counted.contains(module))
            .map(|(_, import)| import.clone())
            .collect();
        PackageDependencyUsage {
            package: label(package),
            dir: dir_of(package),
            listed: entry.map(|entry| listing(package, entry)),
            verdict,
            credit,
            uses,
            not_counted,
        }
    }

    fn describe_use(&self, use_: &Use) -> DependencyUse {
        let module_file = |module: &ModuleId| Some(slashed(&self.graph.module(*module).path));
        let (kind, file, binary) = match use_ {
            Use::Import(module) => (UseKind::Import, module_file(module), None),
            Use::Mock(module) => (UseKind::Mock, module_file(module), None),
            Use::Compiler(module) => (UseKind::Compiler, module_file(module), None),
            Use::JsxRuntime(module) => (UseKind::JsxRuntime, module_file(module), None),
            Use::Command(file) => (UseKind::Command, Some(slashed(file)), None),
            Use::Binary(binary) => (UseKind::Binary, None, Some(binary.clone())),
            Use::Tooling => (UseKind::ScriptOrPlugin, None, None),
            Use::Config(file) => (UseKind::Config, Some(slashed(file)), None),
            Use::AmbientTypes => (UseKind::AmbientTypes, None, None),
        };
        DependencyUse { kind, file, binary }
    }

    /// Every import of `name`, or of a subpath of it, in the module graph, with the importing module, sorted by file
    /// and line.
    fn imports_of(&self, name: &str) -> Vec<(ModuleId, DependencyImport)> {
        let reachable: HashSet<ModuleId> = self.reachable_from_entries();
        let owner = |id| self.workspace.package(id).map_or_else(String::new, label);
        let mut imported_by = Vec::new();
        for module in self.graph.modules() {
            for resolved in &module.imports {
                if resolved.package.as_deref() != Some(name) {
                    continue;
                }
                let Some(import) = module.info.imports.get(resolved.import) else {
                    continue;
                };
                imported_by.push((
                    module.id,
                    DependencyImport {
                        file: slashed(&module.path),
                        line: import.span.start_line,
                        specifier: import.specifier.clone(),
                        kind: import.kind,
                        type_only: resolved.type_only,
                        package: owner(module.owner),
                        reachable: reachable.contains(&module.id),
                    },
                ));
            }
        }
        imported_by.sort_by(|(_, a), (_, b)| a.file.cmp(&b.file).then(a.line.cmp(&b.line)));
        imported_by
    }

    /// Why `unused-dependencies` does not report `entry` of package `id` (`package`) when nothing uses it; `None` when
    /// it reports it.
    pub(crate) fn unused_dependency_exemption(
        &self,
        id: PackageId,
        package: &Package,
        entry: &DependencyEntry,
    ) -> Option<Exemption> {
        if self.is_skipped(id) {
            Some(Exemption::PackageExcluded)
        } else if self.entries_incomplete(id) {
            Some(Exemption::EntriesIncomplete)
        } else if self.ignores_dependency(id, &entry.name) {
            Some(Exemption::Ignored)
        } else if self.only_orders_tasks(&entry.name) {
            Some(Exemption::OrdersTasks)
        } else if entry.kind == DependencyKind::PeerDependencies && is_optional_peer(package, &entry.name) {
            Some(Exemption::OptionalPeer)
        } else {
            None
        }
    }
}

/// The manifest entry of `package` for a dependency.
fn listing(package: &Package, entry: &DependencyEntry) -> DependencyListing {
    DependencyListing {
        package: label(package),
        manifest: slashed(&package.dir.join("package.json")),
        section: entry.kind.field(),
        specifier: entry.specifier.to_string(),
    }
}

/// Whether `peerDependenciesMeta` marks peer dependency `name` of `package` optional.
fn is_optional_peer(package: &Package, name: &str) -> bool {
    package
        .manifest
        .value()
        .get("peerDependenciesMeta")
        .and_then(|meta| meta.get(name))
        .and_then(|entry| entry.get("optional"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}
