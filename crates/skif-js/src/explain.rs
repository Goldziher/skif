//! Why a file, or one export of it, counts as used: the entry rule, the importers and the chain from an entry.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt::Write as _;
use std::path::Path;

use skif_js_syntax::{ExportKind, Import, ImportKind, ImportedKind, ImportedName};

use crate::analysis::JsAnalysis;
use crate::discover::relative;
use crate::graph::{Module, ModuleId, Target};

/// Re-export hops followed when explaining an export.
const MAX_DEPTH: usize = 8;
/// Importers listed before the rest is summarised.
const MAX_LISTED: usize = 12;

fn describe_name(name: &ImportedName) -> String {
    match &name.kind {
        ImportedKind::Default => "default".to_owned(),
        ImportedKind::Named(export) => export.clone(),
        ImportedKind::Namespace if name.escapes => "the namespace, used as a whole".to_owned(),
        ImportedKind::Namespace => format!("the namespace, members {}", name.accessed_members.join(", ")),
    }
}

fn describe_import(import: &Import) -> String {
    let what = if import.names.is_empty() {
        "the whole module".to_owned()
    } else {
        import.names.iter().map(describe_name).collect::<Vec<_>>().join(", ")
    };
    format!("{:?}: {what}", import.kind)
}

/// The modules an import reaches.
fn reached(target: &Target) -> &[ModuleId] {
    match target {
        Target::Module(id) => std::slice::from_ref(id),
        Target::Modules(ids) => ids.as_slice(),
        _ => &[],
    }
}

/// Whether `import` uses export `export` of the module it points at, or the module as a whole.
fn uses_export(import: &Import, export: &str) -> Option<String> {
    if matches!(import.kind, ImportKind::ExportFrom | ImportKind::SideEffect) {
        return None;
    }
    if import.names.is_empty() {
        return Some("the whole module".to_owned());
    }
    import.names.iter().find_map(|name| {
        let hit = match &name.kind {
            ImportedKind::Default => export == "default",
            ImportedKind::Named(imported) => imported == export,
            ImportedKind::Namespace => name.escapes || name.accessed_members.iter().any(|member| member == export),
        };
        hit.then(|| describe_name(name))
    })
}

impl JsAnalysis<'_> {
    /// A report that says why `file` (relative to the workspace root) is reachable and, with `export`, why that
    /// export of it counts as used or what would have to use it.
    #[must_use]
    pub fn explain_usage(&self, file: &Path, export: Option<&str>) -> String {
        let mut out = String::new();
        let Some(module) = self
            .graph
            .modules()
            .iter()
            .find(|module| module.path == file || module.abs == file)
        else {
            self.explain_missing(file, &mut out);
            return out;
        };
        let _ = writeln!(out, "{}", module.path.display());
        self.explain_reachability(module, &mut out);
        if let Some(name) = export {
            self.explain_export(module, name, 0, &mut out);
        }
        out
    }

    fn explain_missing(&self, file: &Path, out: &mut String) {
        let _ = writeln!(out, "{}", file.display());
        if self.unused_files.iter().any(|unused| unused.path == file) {
            let _ = writeln!(out, "  no entry file reaches it: reported by `unused-files`");
        } else {
            let _ = writeln!(
                out,
                "  not in the module graph, and not reported as unused: it is ignored, outside the project globs, a \
                 declaration file, in a package excluded from the run, or in a package whose entry files are not \
                 fully known"
            );
        }
    }

    fn explain_reachability(&self, module: &Module, out: &mut String) {
        if module.is_entry {
            let _ = writeln!(out, "  entry file: {}", self.entry_reason(module));
        }
        let importers = self.importer_lines(module);
        if importers.is_empty() {
            let _ = writeln!(out, "  imported by no module");
        } else {
            let _ = writeln!(out, "  imported by {} place(s):", importers.len());
            for line in importers.iter().take(MAX_LISTED) {
                let _ = writeln!(out, "    {line}");
            }
            if importers.len() > MAX_LISTED {
                let _ = writeln!(out, "    ... and {} more", importers.len() - MAX_LISTED);
            }
        }
        match self.chain_to_entry(module.id) {
            Some(chain) => {
                let names: Vec<String> = chain
                    .iter()
                    .map(|id| self.graph.module(*id).path.display().to_string())
                    .collect();
                let _ = writeln!(out, "  reached from an entry through: {}", names.join(" -> "));
            }
            None => {
                let _ = writeln!(out, "  no chain of imports from an entry file reaches it");
            }
        }
    }

    /// Which entry rule makes `module` an entry file.
    fn entry_reason(&self, module: &Module) -> &'static str {
        let Some(package) = self.setup.package(module.owner) else {
            return "an entry glob";
        };
        if relative(&module.abs, &package.dir).is_some_and(|rel| package.entry.is_match(&rel)) {
            "matches an entry glob of its package (defaults, configuration, test conventions or a plugin)"
        } else if package.config_files.contains(&module.abs) {
            "a configuration file of a tool"
        } else if relative(&module.abs, &package.dir).is_some_and(|rel| package.scripts.files.contains(&rel)) {
            "run by a script of package.json"
        } else {
            "published by package.json (main, bin, exports) or matched by an entry glob of another package"
        }
    }

    fn importer_lines(&self, module: &Module) -> Vec<String> {
        let mut lines = Vec::new();
        for importer_id in self.graph.imported_by(module.id) {
            let importer = self.graph.module(*importer_id);
            for resolved in &importer.imports {
                if !reached(&resolved.target).contains(&module.id) {
                    continue;
                }
                if let Some(import) = importer.info.imports.get(resolved.import) {
                    lines.push(format!(
                        "{}:{} ({})",
                        importer.path.display(),
                        import.span.start_line,
                        describe_import(import)
                    ));
                }
            }
        }
        lines
    }

    /// The shortest chain of imports from an entry file to `id`, entry first.
    fn chain_to_entry(&self, id: ModuleId) -> Option<Vec<ModuleId>> {
        let modules = self.graph.modules();
        let mut previous: HashMap<ModuleId, ModuleId> = HashMap::new();
        let mut queue: VecDeque<ModuleId> = modules
            .iter()
            .filter(|module| module.is_entry)
            .map(|module| module.id)
            .collect();
        let mut seen: HashSet<ModuleId> = queue.iter().copied().collect();
        while let Some(current) = queue.pop_front() {
            if current == id {
                let mut chain = vec![id];
                let mut at = id;
                while let Some(before) = previous.get(&at) {
                    chain.push(*before);
                    at = *before;
                }
                chain.reverse();
                return Some(chain);
            }
            for resolved in &modules[current.index()].imports {
                for next in reached(&resolved.target) {
                    if seen.insert(*next) {
                        previous.insert(*next, current);
                        queue.push_back(*next);
                    }
                }
            }
        }
        None
    }

    fn explain_export(&self, module: &Module, name: &str, depth: usize, out: &mut String) {
        let pad = "  ".repeat(depth + 1);
        let Some(export) = module.info.exports.iter().find(|export| export.name == name) else {
            let _ = writeln!(out, "{pad}`{name}` is not an export of {}", module.path.display());
            return;
        };
        let _ = writeln!(out, "{pad}export `{name}` of {}:", module.path.display());
        if module.is_entry && !self.setup.include_entry_exports {
            let _ = writeln!(out, "{pad}  the file is an entry, so its exports are public");
        }
        if self.is_tagged_public(export) {
            let _ = writeln!(out, "{pad}  tagged `@public` (or another ignored tag)");
        }
        let mut users = 0;
        for importer_id in self.graph.imported_by(module.id) {
            let importer = self.graph.module(*importer_id);
            for resolved in &importer.imports {
                if !reached(&resolved.target).contains(&module.id) {
                    continue;
                }
                let Some(import) = importer.info.imports.get(resolved.import) else {
                    continue;
                };
                if let Some(how) = uses_export(import, name) {
                    users += 1;
                    let _ = writeln!(
                        out,
                        "{pad}  used by {}:{} ({how})",
                        importer.path.display(),
                        import.span.start_line
                    );
                }
            }
            if depth < MAX_DEPTH {
                users += self.explain_reexports(importer, module, name, depth, out);
            }
        }
        if users == 0 {
            let _ = writeln!(out, "{pad}  no module imports it by name or as a whole");
        }
        if !self.exports.is_used(module.id, name) && self.in_used_signature(module, export, &mut HashSet::new()) {
            let _ = writeln!(out, "{pad}  mentioned in the signature of a used export of the same file");
        }
        if matches!(export.kind, ExportKind::ReExport | ExportKind::StarReExport) {
            let _ = writeln!(out, "{pad}  a re-export; the lines above and below follow where it leads");
        }
        let verdict = if self.exports.is_used(module.id, name) {
            "used by another module"
        } else {
            "not used by another module"
        };
        let _ = writeln!(out, "{pad}=> {verdict}");
    }

    /// Follows the re-exports of `importer` that forward export `name` of `module`; returns how many it found.
    fn explain_reexports(
        &self,
        importer: &Module,
        module: &Module,
        name: &str,
        depth: usize,
        out: &mut String,
    ) -> usize {
        let pad = "  ".repeat(depth + 1);
        let mut found = 0;
        for export in &importer.info.exports {
            let Some(from) = export.from.as_deref() else {
                continue;
            };
            if importer.reexport_source(from) != Some(module.id) {
                continue;
            }
            let forwarded = match export.kind {
                ExportKind::ReExport => export.imported_name.as_deref().unwrap_or(&export.name) == name,
                ExportKind::StarReExport if export.name == "*" => {
                    name != "default" && !importer.info.exports.iter().any(|own| own.name == name && own.from.is_none())
                }
                ExportKind::StarReExport => true,
                _ => false,
            };
            if !forwarded {
                continue;
            }
            found += 1;
            let exposed = if export.kind == ExportKind::StarReExport && export.name == "*" {
                name
            } else {
                export.name.as_str()
            };
            let _ = writeln!(
                out,
                "{pad}  re-exported by {} as `{exposed}`:",
                importer.path.display()
            );
            self.explain_export(importer, exposed, depth + 1, out);
        }
        found
    }
}
