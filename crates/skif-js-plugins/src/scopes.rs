//! Nx projects as scopes of the package that owns them (ADR 0109).
//!
//! An Nx project is a directory with a `project.json`. In an integrated workspace it has no `package.json`: its
//! dependencies are listed by the package that holds it, usually the root, and its tools (`vite.config.ts`,
//! `eslint.config.mjs`, `jest.config.ts`, `tsconfig.json`) are configured in its own directory. Each such project is a
//! scope: the plugins run there as they run for a package, and what they find is rebased onto the owning package.

use std::path::PathBuf;

use indexmap::IndexMap;
use skif_config::PluginConfig;

use crate::files::find_files;
use crate::plugin::{IncompleteScope, PluginContext, PluginOutput};
use crate::support::join_rel;

/// The Nx projects below a package (ADR 0109).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct NxProjects {
    /// The directory of each `project.json` the `nx` plugin reads, relative to the package directory and sorted; `""`
    /// for the package's own.
    pub dirs: Vec<String>,
    /// The project directories with no `package.json` in them or between them and the package directory: the
    /// package lists their dependencies, and every enabled plugin also runs in them.
    pub scopes: Vec<String>,
}

impl NxProjects {
    /// Every `project.json` below the package of `ctx` (outside `node_modules`), when the `nx` plugin is enabled
    /// there; none otherwise.
    #[must_use]
    pub fn find(ctx: &PluginContext<'_>) -> Self {
        if !crate::resolve::is_plugin_enabled(ctx, crate::plugins::nx::ID) {
            return Self::default();
        }
        let dirs: Vec<String> = project_dirs(ctx);
        let scopes = dirs
            .iter()
            .filter(|dir| !dir.is_empty() && !has_own_manifest(ctx, dir))
            .cloned()
            .collect();
        Self { dirs, scopes }
    }

    /// The projects whose directory `keep` accepts, such as those outside gitignored and build output directories.
    #[must_use]
    pub fn filter(mut self, keep: impl Fn(&str) -> bool) -> Self {
        self.dirs.retain(|dir| keep(dir));
        self.scopes.retain(|dir| keep(dir));
        self
    }
}

/// The directories, relative to the package directory and sorted, that hold a `project.json`; `""` for the package's
/// own.
pub(crate) fn project_dirs(ctx: &PluginContext<'_>) -> Vec<String> {
    find_files(ctx.fs, ctx.dir, &["**/project.json".to_owned()])
        .iter()
        .map(|path| path.rsplit_once('/').map_or("", |(dir, _)| dir).to_owned())
        .collect()
}

/// Whether `dir`, or a directory between it and the package directory, holds a `package.json`: the project belongs
/// to that package.
fn has_own_manifest(ctx: &PluginContext<'_>, dir: &str) -> bool {
    let mut current = String::new();
    dir.split('/').any(|part| {
        current = join_rel(&current, part);
        ctx.fs.is_file(&ctx.dir.join(&current).join("package.json"))
    })
}

/// A scope's own context: the project directory as the package directory, the owning package's dependencies, no
/// scripts (they belong to the owning package) and only the switches of the plugin table, since the globs a table
/// gives are relative to the owning package.
pub(crate) struct Scope {
    /// The project directory, relative to the owning package's directory.
    pub(crate) dir: String,
    abs: PathBuf,
    scripts: IndexMap<String, String>,
    plugins: IndexMap<String, PluginConfig>,
}

impl Scope {
    pub(crate) fn new(owner: &PluginContext<'_>, dir: &str) -> Self {
        Self {
            dir: dir.to_owned(),
            abs: crate::support::join_lexically(owner.dir, dir),
            scripts: IndexMap::new(),
            plugins: switches(owner.plugins),
        }
    }

    /// The context the plugins run in for this scope of the package `owner`.
    pub(crate) fn context<'a>(&'a self, owner: &PluginContext<'a>) -> PluginContext<'a> {
        PluginContext {
            dir: &self.abs,
            is_root: false,
            package_name: None,
            dependencies: owner.dependencies,
            inherited_dependencies: owner.inherited_dependencies,
            scripts: &self.scripts,
            fs: owner.fs,
            plugins: &self.plugins,
            modules: owner.modules,
        }
    }
}

/// The plugin table reduced to its switches: `true`, `false`, or the `enabled` of a table.
fn switches(plugins: &IndexMap<String, PluginConfig>) -> IndexMap<String, PluginConfig> {
    plugins
        .iter()
        .filter_map(|(id, setting)| {
            let enabled = match setting {
                PluginConfig::Enabled(enabled) => Some(*enabled),
                PluginConfig::Options(options) => options.enabled,
                _ => None,
            };
            enabled.map(|enabled| (id.clone(), PluginConfig::Enabled(enabled)))
        })
        .collect()
}

/// `output`, which a plugin produced for the scope in `scope`, with its paths, globs and expressions made relative to
/// the owning package. Entries a config of the scope hides lie in the scope, unless the plugin bounds them further.
pub(crate) fn rebase(mut output: PluginOutput, scope: &str) -> PluginOutput {
    let globs = |list: &mut Vec<String>| {
        for glob in list.iter_mut() {
            *glob = rebase_glob(scope, glob);
        }
    };
    globs(&mut output.entries);
    globs(&mut output.project);
    globs(&mut output.production_entries);
    globs(&mut output.ignore);
    globs(&mut output.module_dirs);
    for regex in &mut output.entry_regexes {
        *regex = rebase_regex(scope, regex);
    }
    for alias in &mut output.aliases {
        if let Some(target) = &mut alias.target {
            *target = join_rel(scope, target);
        }
    }
    for dir in &mut output.build_dirs {
        dir.out_dir = join_rel(scope, &dir.out_dir);
        dir.root_dir = join_rel(scope, &dir.root_dir);
    }
    for run in &mut output.run_binaries {
        if let Some(dir) = &mut run.dir {
            *dir = join_rel(scope, dir);
        }
    }
    for reason in &mut output.incomplete_reasons {
        reason.scope = match &reason.scope {
            IncompleteScope::Within(within) => {
                IncompleteScope::Within(within.iter().map(|glob| rebase_glob(scope, glob)).collect())
            }
            _ => IncompleteScope::Within(vec![format!("{scope}/**")]),
        };
    }
    output
}

/// Folds `from`, the rebased output of a scope, into `into`, the output of the same plugin for the owning package.
pub(crate) fn merge(into: &mut PluginOutput, from: PluginOutput) {
    let lists = [
        (&mut into.entries, from.entries),
        (&mut into.project, from.project),
        (&mut into.production_entries, from.production_entries),
        (&mut into.entry_regexes, from.entry_regexes),
        (&mut into.referenced_binaries, from.referenced_binaries),
        (&mut into.provided_binaries, from.provided_binaries),
        (&mut into.ignore, from.ignore),
        (&mut into.conditions, from.conditions),
        (&mut into.virtual_modules, from.virtual_modules),
        (&mut into.module_dirs, from.module_dirs),
        (&mut into.extensions, from.extensions),
    ];
    for (list, items) in lists {
        extend_unique(list, items);
    }
    extend_unique(&mut into.config_files, from.config_files);
    extend_unique(&mut into.run_binaries, from.run_binaries);
    extend_unique(&mut into.aliases, from.aliases);
    extend_unique(&mut into.build_dirs, from.build_dirs);
    extend_unique(&mut into.incomplete_reasons, from.incomplete_reasons);
    for reference in from.referenced_dependencies {
        match into
            .referenced_dependencies
            .iter_mut()
            .find(|existing| existing.name == reference.name && existing.from == reference.from)
        {
            Some(existing) => existing.merge(&reference),
            None => into.referenced_dependencies.push(reference),
        }
    }
    into.custom_elements = into.custom_elements.or(from.custom_elements);
    into.entries_incomplete |= from.entries_incomplete;
}

fn extend_unique<T: PartialEq>(list: &mut Vec<T>, items: Vec<T>) {
    for item in items {
        if !list.contains(&item) {
            list.push(item);
        }
    }
}

/// A glob relative to the scope `scope`, relative to the owning package; a negation stays one.
fn rebase_glob(scope: &str, glob: &str) -> String {
    match glob.strip_prefix('!') {
        Some(negated) => format!("!{}", rebase_glob(scope, negated)),
        None => join_rel(scope, glob),
    }
}

/// An entry expression of the scope `scope` (matched against `/` and the path relative to the scope, see
/// [`crate::PluginInput::EntryRegex`]) for a match against `/` and the path relative to the owning package. An
/// expression anchored with a leading `^` stays anchored at the scope; an unanchored one matches anywhere below it.
fn rebase_regex(scope: &str, regex: &str) -> String {
    let (flags, body) = split_inline_flags(regex);
    let prefix = format!("^/{}", regex::escape(scope));
    match body.strip_prefix('^') {
        Some(rest) if !has_top_level_alternation(rest) => format!("{flags}{prefix}(?:{rest})"),
        _ => format!("{flags}{prefix}(?:/.*)?(?:{body})"),
    }
}

/// The leading inline flag group of `regex` (`(?i)`) and the rest.
fn split_inline_flags(regex: &str) -> (&str, &str) {
    let Some(rest) = regex.strip_prefix("(?") else {
        return ("", regex);
    };
    match rest.find(')') {
        Some(end) if end > 0 && rest[..end].chars().all(|c| c.is_ascii_alphabetic()) => regex.split_at(end + 3),
        _ => ("", regex),
    }
}

/// Whether `regex` has a `|` outside every group and character class, so that a leading `^` anchors only its first
/// alternative.
fn has_top_level_alternation(regex: &str) -> bool {
    let mut depth = 0_u32;
    let mut in_class = false;
    let mut chars = regex.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                chars.next();
            }
            '[' if !in_class => in_class = true,
            ']' if in_class => in_class = false,
            '(' if !in_class => depth += 1,
            ')' if !in_class => depth = depth.saturating_sub(1),
            '|' if !in_class && depth == 0 => return true,
            _ => {}
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs_are_rebased_onto_the_scope_and_negations_stay_negations() {
        assert_eq!(rebase_glob("apps/web", "src/**/*.ts"), "apps/web/src/**/*.ts");
        assert_eq!(rebase_glob("apps/web", "./index.html"), "apps/web/index.html");
        assert_eq!(
            rebase_glob("apps/web", "!**/*.stories.tsx"),
            "!apps/web/**/*.stories.tsx"
        );
        assert_eq!(rebase_glob("apps/web", "../../jest.preset.js"), "jest.preset.js");
    }

    /// An anchored expression stays anchored at the scope, an unanchored one matches anywhere below it, and inline
    /// flags stay in front; neither matches a sibling directory.
    #[test]
    fn entry_expressions_match_below_the_scope_only() {
        let matches = |regex: &str, path: &str| regex::Regex::new(regex).is_ok_and(|compiled| compiled.is_match(path));
        let anchored = rebase_regex("apps/web", "^/e2e/.*\\.ts$");
        assert_eq!(anchored, "^/apps/web(?:/e2e/.*\\.ts$)");
        assert!(matches(&anchored, "/apps/web/e2e/a.ts"));
        assert!(!matches(&anchored, "/apps/web/src/e2e/a.ts"));

        let unanchored = rebase_regex("apps/web", "(/__tests__/.*|(\\.|/)(test|spec))\\.[jt]sx?$");
        assert!(matches(&unanchored, "/apps/web/src/a.test.ts"));
        assert!(matches(&unanchored, "/apps/web/__tests__/a.ts"));
        assert!(!matches(&unanchored, "/apps/api/src/a.test.ts"));
        assert!(!matches(&unanchored, "/apps/web2/__tests__/a.ts"));

        let flagged = rebase_regex("apps/web", "(?i)^/E2E/");
        assert_eq!(flagged, "(?i)^/apps/web(?:/E2E/)");
        assert!(matches(&flagged, "/apps/web/e2e/a.ts"));

        let alternation = rebase_regex("apps/web", "^/a/|/b/");
        assert!(matches(&alternation, "/apps/web/x/b/c.ts"));
        assert!(!matches(&alternation, "/b/c.ts"));
    }
}
