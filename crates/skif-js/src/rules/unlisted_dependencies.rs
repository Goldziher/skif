use std::collections::HashSet;
use std::path::PathBuf;

use skif_core::{Category, Fixability, Location, Related, RuleMeta, Severity, diagnostic};
use skif_lint::Emitter;

use super::JsRule;
use super::support::{is_fixture, module_location};
use crate::analysis::JsAnalysis;

/// Metadata of `unlisted-dependencies`.
pub static META: RuleMeta = RuleMeta::new(
    "unlisted-dependencies",
    Category::Js,
    Some(Severity::Error),
    Fixability::None,
    "A file imports a package that neither its `package.json` nor any `package.json` above it lists. An import that \
     only the tsconfig `paths` of another package map has the detail `ForeignTsconfigPaths`: it needs a mapping, not \
     a dependency. A package a tool config names (a tsconfig `extends`, an ESLint or Oxlint plugin, a Prisma \
     generator) is reported at the config with the detail `ConfigReference`.",
);

/// The `detail` of a finding whose specifier only the `paths` of another package's TypeScript configuration map: the
/// fix is a mapping in the importing package's configuration or an import by package name, not a dependency.
const FOREIGN_TSCONFIG_PATHS: &str = "ForeignTsconfigPaths";

/// The `detail` of a finding for a package that a tool config names rather than a module imports.
const CONFIG_REFERENCE: &str = "ConfigReference";

/// Reports imports of packages the manifest does not list, once per file and package. In strict mode production code
/// must import from `dependencies` or `peerDependencies`, so a package listed only elsewhere is reported too. An
/// import that only another package's `tsconfig` `paths` map is reported as a missing mapping. A package a tool config
/// names and no manifest lists is reported at the config.
pub struct UnlistedDependencies;

impl JsRule for UnlistedDependencies {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn check(&self, analysis: &JsAnalysis<'_>, out: &mut Emitter<'_>) {
        let mut seen = HashSet::new();
        for unlisted in &analysis.dependencies.unlisted {
            let module = analysis.graph.module(unlisted.module);
            if module.ignored
                || is_fixture(module)
                || analysis.ignores_dependency(unlisted.package, &unlisted.name)
                || analysis.declares_import(&unlisted.specifier, &unlisted.name)
            {
                continue;
            }
            if !seen.insert((unlisted.package, unlisted.name.as_str(), unlisted.module)) {
                continue;
            }
            let Some(package) = analysis.workspace.package(unlisted.package) else {
                continue;
            };
            // Strict mode reports an import that the manifest lists only outside `dependencies` and
            // `peerDependencies`, such as in `devDependencies`.
            let listed_as = package
                .dependencies
                .iter()
                .find(|entry| entry.name == unlisted.name)
                .map(|entry| entry.kind.field());
            if listed_as.is_none()
                && let Some(foreign) = &unlisted.foreign_paths
            {
                let message = format!(
                    "`{}` resolves only through the `paths` of `{}`; add the mapping to this package's tsconfig or \
                     import it by package name",
                    unlisted.specifier, foreign.config
                );
                let help = format!(
                    "map `{}` in the tsconfig of `{}` as `{}` does, or import the module by the name of the package \
                     that exports it",
                    foreign.pattern,
                    package.display_name(),
                    foreign.config
                );
                out.emit(
                    diagnostic(&META, message, module_location(module, unlisted.span))
                        .package(package.display_name())
                        .dependency(unlisted.name.clone())
                        .detail(FOREIGN_TSCONFIG_PATHS)
                        .related(Related::new(
                            format!("`{}` maps `{}` here", foreign.pattern, unlisted.specifier),
                            Location::new(PathBuf::from(&foreign.config), None),
                        ))
                        .help(help)
                        .build(),
                );
                continue;
            }
            let (message, help) = match listed_as {
                Some(field) => (
                    format!(
                        "`{}` is imported by production code but listed only in `{field}`",
                        unlisted.name
                    ),
                    format!(
                        "move `{}` to the dependencies of `{}`",
                        unlisted.name,
                        package.display_name()
                    ),
                ),
                None => (
                    format!("`{}` is imported but not listed in `package.json`", unlisted.name),
                    format!(
                        "add `{}` to the dependencies of `{}`",
                        unlisted.name,
                        package.display_name()
                    ),
                ),
            };
            out.emit(
                diagnostic(&META, message, module_location(module, unlisted.span))
                    .package(package.display_name())
                    .dependency(unlisted.name.clone())
                    .help(help)
                    .build(),
            );
        }
        for unlisted in &analysis.dependencies.unlisted_config {
            if analysis.ignores_dependency(unlisted.package, &unlisted.name) {
                continue;
            }
            let Some(package) = analysis.workspace.package(unlisted.package) else {
                continue;
            };
            let file = unlisted.file.to_string_lossy().replace('\\', "/");
            out.emit(
                diagnostic(
                    &META,
                    format!(
                        "`{file}` references `{}`, which is not listed in `package.json`",
                        unlisted.name
                    ),
                    Location::new(unlisted.file.clone(), None),
                )
                .package(package.display_name())
                .dependency(unlisted.name.clone())
                .detail(CONFIG_REFERENCE)
                .help(format!(
                    "add `{}` to the devDependencies of `{}`, or add it to `ignore-dependencies`",
                    unlisted.name,
                    package.display_name()
                ))
                .build(),
            );
        }
    }
}
