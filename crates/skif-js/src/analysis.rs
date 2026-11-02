//! The analysis the rules read: module graph, export usage and dependency usage.

use std::path::PathBuf;
use std::sync::Arc;

use skif_config::Config;
use skif_core::FileSystem;
use skif_js_syntax::{Export, ExportKind};
use skif_workspace::{PackageId, Workspace};

use crate::deps::DependencyUsage;
use crate::discover::{discover, relative};
use crate::entries::entry_files;
use crate::graph::{self, Inputs, Module, ModuleGraph};
use crate::input::InputSource;
use crate::resolve::ModuleResolver;
use crate::setup::Setup;
use crate::usage::ExportUsage;

/// A project file nothing reaches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnusedFile {
    /// Path relative to the workspace root.
    pub path: PathBuf,
    /// The package that owns the file.
    pub owner: PackageId,
}

/// Everything the JS rules need, built once per lint run.
pub struct JsAnalysis<'a> {
    /// The workspace analysed.
    pub workspace: &'a Workspace,
    /// Modules reachable from the entry files.
    pub graph: ModuleGraph,
    /// Which exports other modules use.
    pub exports: ExportUsage,
    /// Which packages and binaries the code and scripts use.
    pub dependencies: DependencyUsage,
    /// Project files outside the graph.
    pub unused_files: Vec<UnusedFile>,
    setup: Setup,
}

impl<'a> JsAnalysis<'a> {
    /// Discovers files, builds the graph and computes usage.
    ///
    /// `sources` supply plugin-provided entries, project files, ignores and references.
    #[must_use]
    pub fn build(
        workspace: &'a Workspace,
        config: &Config,
        fs: &Arc<dyn FileSystem>,
        sources: &[Arc<dyn InputSource>],
    ) -> Self {
        let setup = Setup::new(workspace, config, fs, sources);
        let candidates = discover(fs.as_ref(), workspace);
        let entries = entry_files(fs.as_ref(), workspace, &setup, &candidates);
        let resolver = ModuleResolver::new(Arc::clone(fs), workspace);
        let inputs = Inputs {
            fs: fs.as_ref(),
            resolver: &resolver,
            root: &workspace.root,
            candidates: &candidates,
            setup: &setup,
        };
        let graph = graph::build(&inputs, &entries);
        let exports = ExportUsage::compute(&graph, setup.include_entry_exports);
        let dependencies = DependencyUsage::compute(workspace, &setup, &graph, fs.as_ref());

        let unused_files = candidates
            .iter()
            .filter(|candidate| graph.id_of(&candidate.abs).is_none())
            .filter(|candidate| !candidate.rel.ends_with(".d.ts"))
            .filter(|candidate| {
                let Some(package) = setup.package(candidate.owner) else {
                    return false;
                };
                let Some(in_package) = relative(&candidate.abs, &package.dir) else {
                    return false;
                };
                package.project.is_match(&in_package)
                    && !package.ignore.matches_any(&in_package)
                    && !setup.global_ignore.matches_any(&candidate.rel)
            })
            .map(|candidate| UnusedFile {
                path: PathBuf::from(&candidate.rel),
                owner: candidate.owner,
            })
            .collect();
        Self {
            workspace,
            graph,
            exports,
            dependencies,
            unused_files,
            setup,
        }
    }

    /// Whether `[js] production` is on.
    #[must_use]
    pub fn production(&self) -> bool {
        self.setup.production
    }

    /// Whether dependency `name` of package `id` is on an `ignore-dependencies` list.
    #[must_use]
    pub fn ignores_dependency(&self, id: PackageId, name: &str) -> bool {
        self.setup.ignores_dependency(id, name)
    }

    /// Whether the package is excluded from analysis (`ignore-packages`).
    #[must_use]
    pub fn is_skipped(&self, id: PackageId) -> bool {
        self.setup.package(id).is_none()
    }

    /// Whether a module's own issues are reported: it is a project file, not ignored, and not a public entry.
    #[must_use]
    pub fn reports_exports_of(&self, module: &Module) -> bool {
        module.in_project && !module.ignored && (!module.is_entry || self.setup.include_entry_exports)
    }

    /// Exports of `module` that no other module uses, values and types alike, in source order.
    ///
    /// Exports tagged `@public`, `@beta` or `@alpha` (and `@internal` in production mode, and the `ignore-tags` of `unused-exports`) are not listed.
    #[must_use]
    pub fn unused_exports<'m>(&self, module: &'m Module) -> Vec<&'m Export> {
        if !self.reports_exports_of(module) {
            return Vec::new();
        }
        module
            .info
            .exports
            .iter()
            .filter(|export| !(export.kind == ExportKind::StarReExport && export.name == "*"))
            .filter(|export| export.name != "export=")
            .filter(|export| !self.is_tagged_public(export))
            .filter(|export| !self.exports.is_used(module.id, &export.name))
            .collect()
    }

    fn is_tagged_public(&self, export: &Export) -> bool {
        export.tags.iter().any(|tag| match tag.as_str() {
            "public" | "beta" | "alpha" => true,
            "internal" => self.setup.production,
            other => self.setup.ignore_tags.iter().any(|ignored| ignored == other),
        })
    }
}
