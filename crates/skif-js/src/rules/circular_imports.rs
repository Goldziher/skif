use std::collections::BTreeSet;

use skif_core::{Category, Fixability, Location, Related, RuleMeta, diagnostic};
use skif_js_syntax::ImportKind;
use skif_lint::Emitter;

use super::JsRule;
use super::support::module_location;
use crate::analysis::JsAnalysis;
use crate::graph::Target;

/// Metadata of `circular-imports`.
pub static META: RuleMeta = RuleMeta::new(
    "circular-imports",
    Category::Js,
    None,
    Fixability::None,
    "Modules import each other in a cycle.",
);

/// Reports strongly connected components of value imports. Type-only imports and the imports skif adds for registered
/// Vue components (ADR 0020; mutually recursive components are common) do not count, nor do resolve calls
/// (`require.resolve`), which load nothing, and dynamic imports count only with `[js.cycles] dynamic-imports`. A
/// component whose files are exactly those of a `[js.cycles] allow` entry is not reported.
pub struct CircularImports;

/// Strongly connected components by Tarjan's algorithm, without recursion.
fn tarjan(adjacency: &[Vec<usize>]) -> Vec<Vec<usize>> {
    let count = adjacency.len();
    let mut index = vec![usize::MAX; count];
    let mut low = vec![0_usize; count];
    let mut on_stack = vec![false; count];
    let mut stack = Vec::new();
    let mut next = 0_usize;
    let mut components = Vec::new();
    for root in 0..count {
        if index[root] != usize::MAX {
            continue;
        }
        let mut work = vec![(root, 0_usize)];
        while let Some(&(node, edge)) = work.last() {
            if edge == 0 && index[node] == usize::MAX {
                index[node] = next;
                low[node] = next;
                next += 1;
                stack.push(node);
                on_stack[node] = true;
            }
            if let Some(&target) = adjacency[node].get(edge) {
                if let Some(top) = work.last_mut() {
                    top.1 += 1;
                }
                if index[target] == usize::MAX {
                    work.push((target, 0));
                } else if on_stack[target] {
                    low[node] = low[node].min(index[target]);
                }
            } else {
                work.pop();
                if let Some(&(parent, _)) = work.last() {
                    low[parent] = low[parent].min(low[node]);
                }
                if low[node] == index[node] {
                    let mut component = Vec::new();
                    while let Some(member) = stack.pop() {
                        on_stack[member] = false;
                        component.push(member);
                        if member == node {
                            break;
                        }
                    }
                    components.push(component);
                }
            }
        }
    }
    components
}

/// A workspace-relative path in one spelling: `/`-separated, without a leading `./`.
fn normalize(path: &str) -> String {
    path.replace('\\', "/").trim_start_matches("./").to_owned()
}

impl JsRule for CircularImports {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn check(&self, analysis: &JsAnalysis<'_>, out: &mut Emitter<'_>) {
        let modules = analysis.graph.modules();
        let cycles_config = &analysis.setup.cycles;
        let dynamic_imports = cycles_config.dynamic_imports;
        let allowed: Vec<BTreeSet<String>> = cycles_config
            .allow
            .iter()
            .map(|files| files.iter().map(|file| normalize(file)).collect())
            .collect();
        let adjacency: Vec<Vec<usize>> = modules
            .iter()
            .map(|module| {
                let mut targets: Vec<usize> = module
                    .imports
                    .iter()
                    .filter(|resolved| !resolved.type_only && !resolved.component)
                    .filter(|resolved| {
                        module
                            .info
                            .imports
                            .get(resolved.import)
                            .is_some_and(|import| match import.kind {
                                ImportKind::Resolve => false,
                                ImportKind::Dynamic => dynamic_imports,
                                _ => true,
                            })
                    })
                    .filter_map(|resolved| match resolved.target {
                        Target::Module(id) => Some(id.index()),
                        _ => None,
                    })
                    .collect();
                targets.sort_unstable();
                targets.dedup();
                targets
            })
            .collect();

        let mut cycles: Vec<Vec<usize>> = tarjan(&adjacency)
            .into_iter()
            .filter(|component| {
                component.len() > 1 || component.first().is_some_and(|node| adjacency[*node].contains(node))
            })
            .map(|mut component| {
                component.sort_by(|a, b| modules[*a].path.cmp(&modules[*b].path));
                component
            })
            .collect();
        cycles.sort_by(|a, b| modules[a[0]].path.cmp(&modules[b[0]].path));

        for cycle in cycles {
            let first = &modules[cycle[0]];
            if first.ignored {
                continue;
            }
            if !allowed.is_empty() {
                let files: BTreeSet<String> = cycle
                    .iter()
                    .map(|member| normalize(&modules[*member].path.to_string_lossy()))
                    .collect();
                if allowed.contains(&files) {
                    continue;
                }
            }
            let span = first.imports.iter().find_map(|resolved| {
                let Target::Module(id) = resolved.target else {
                    return None;
                };
                cycle
                    .contains(&id.index())
                    .then(|| first.info.imports.get(resolved.import).map(|import| import.span))
                    .flatten()
            });
            let mut builder = diagnostic(
                &META,
                format!("{} modules import each other in a cycle", cycle.len()),
                module_location(first, span),
            )
            .help("move the shared code into a module both sides can import");
            for member in cycle.iter().skip(1) {
                builder = builder.related(Related::new(
                    "part of the same cycle".to_owned(),
                    Location::new(modules[*member].path.clone(), None),
                ));
            }
            if let Some(package) = analysis.workspace.package(first.owner) {
                builder = builder.package(package.display_name());
            }
            out.emit(builder.build());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::tarjan;

    #[test]
    fn finds_components() {
        let adjacency = vec![vec![1], vec![2], vec![0], vec![0], vec![4]];
        let mut components = tarjan(&adjacency);
        for component in &mut components {
            component.sort_unstable();
        }
        components.sort();
        assert_eq!(components, [vec![0, 1, 2], vec![3], vec![4]]);
    }
}
