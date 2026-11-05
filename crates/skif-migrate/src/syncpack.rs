//! syncpack: `.syncpackrc*`, `syncpack.config.*`, the `syncpack` key of `package.json`, and the flags of the scripts
//! that run it.

use skif_js_syntax::ConfigValue;

use crate::doc::Val;
use crate::keys;
use crate::notes::{NoteClass, Tool};
use crate::scripts::{self, Invocation};
use crate::state::{Src, State};
use crate::value::{at, boolean, entries, join, read, string, string_list};
use crate::{MigrateError, ToolInput};

/// The rules that check what `syncpack lint` checks.
const RULES: &[&str] = &[
    "version-mismatch",
    "semver-range-mismatch",
    "banned-dependency",
    "invalid-local-version",
    "policy-conflict",
    "catalog-mismatch",
    "catalog-missing-entry",
    "catalog-not-used",
];

/// The dependency types syncpack lints when nothing narrows them and that skif can read.
const DEFAULT_TYPES: &[&str] = &["prod", "dev", "peer", keys::TYPE_LOCAL];

/// Dependency types syncpack reads from places skif does not look at.
const UNSUPPORTED_TYPES: &[&str] = &["overrides", "resolutions", "pnpmOverrides"];

/// `semverGroups[].range` values.
const RANGES: &[&str] = &["", "*", "^", "~", ">", ">=", "<", "<="];

/// Keys that only matter to `syncpack format`.
const FORMAT_KEYS: &[&str] = &[
    "sortAz",
    "sortFirst",
    "sortExports",
    "sortPackages",
    "indent",
    "formatBugs",
    "formatRepository",
];

/// Keys of the registry commands.
const REGISTRY_KEYS: &[&str] = &["updateGroups", "minimumReleaseAge", "maxConcurrentRequests"];

/// Keys syncpack 15 rejects.
const RETIRED_KEYS: &[&str] = &[
    "lintFormatting",
    "lintSemverRanges",
    "lintVersions",
    "dependencyTypes",
    "specifierTypes",
];

/// The manifest sections that `sortAz` can name and that `unsorted-dependencies` covers.
const DEPENDENCY_MAPS: &[&str] = &[
    "dependencies",
    "devDependencies",
    "peerDependencies",
    "optionalDependencies",
];

/// Flags of `syncpack` that do not change what is checked.
const INERT_FLAGS: &[&str] = &[
    "--help",
    "--version",
    "--config",
    "--reporter",
    "--no-ansi",
    "--log-levels",
    "--show",
    "--sort",
    "--dry-run",
    "--check",
    "--indent",
];

#[derive(Debug, Clone, PartialEq, Eq)]
enum CustomKind {
    /// `optionalDependencies`: skif has the `optional` token.
    Optional,
    /// Written to `[[custom-types]]` under its own name.
    Custom,
    /// skif cannot read it.
    Unsupported,
}

#[derive(Debug, Default)]
struct Types {
    custom: Vec<(String, CustomKind)>,
}

impl Types {
    fn kind(&self, name: &str) -> Option<&CustomKind> {
        self.custom
            .iter()
            .find(|(known, _)| known == name)
            .map(|(_, kind)| kind)
    }

    fn default_scope(&self) -> Vec<String> {
        let mut scope: Vec<String> = DEFAULT_TYPES.iter().map(|token| (*token).to_owned()).collect();
        for (name, kind) in &self.custom {
            let token = match kind {
                CustomKind::Optional => "optional",
                CustomKind::Custom => name.as_str(),
                CustomKind::Unsupported => continue,
            };
            if !scope.iter().any(|known| known == token) {
                scope.push(token.to_owned());
            }
        }
        scope
    }
}

/// A list of dependency type tokens translated for skif.
struct MappedTypes {
    tokens: Vec<String>,
    /// Every positive token named something skif cannot read, so the group would cover nothing skif checks.
    covers_nothing: bool,
}

#[derive(Default)]
struct Run {
    types: Types,
    /// The scope given by `--dependency-types`, which replaces the default one.
    flag_types: Option<Vec<String>>,
}

pub(crate) fn translate(input: &ToolInput, state: &mut State<'_>) -> Result<(), MigrateError> {
    state.keep(RULES.iter().copied());
    let package = input
        .package_json
        .as_ref()
        .map(|file| read(file, Tool::Syncpack))
        .transpose()?;
    let package_name = input
        .package_json
        .as_ref()
        .map_or("package.json", |file| file.path.as_str());
    let found = match (&input.config, &package) {
        (Some(file), _) => Some((
            read(file, Tool::Syncpack)?,
            Src::new(Tool::Syncpack, file.path.clone()),
            String::new(),
        )),
        (None, Some(package)) => scripts::config_key(Tool::Syncpack, package).map(|config| {
            let prefix = if package.get("syncpack").is_some() {
                "syncpack"
            } else {
                "config.syncpack"
            };
            (
                config.clone(),
                Src::new(Tool::Syncpack, package_name),
                prefix.to_owned(),
            )
        }),
        (None, None) => None,
    };
    let mut run = Run::default();
    let src = found
        .as_ref()
        .map_or_else(|| Src::new(Tool::Syncpack, package_name), |(_, src, _)| src.clone());
    if let Some((config, src, prefix)) = &found {
        read_config(state, src, prefix, config, &input.workspace_patterns, &mut run);
    }
    if let Some(package) = &package {
        for invocation in scripts::invocations(package, Tool::Syncpack) {
            read_flags(state, &src, &invocation, &input.workspace_patterns, &mut run);
        }
    }
    emit_scope(state, &src, &run);
    Ok(())
}

fn read_config(
    state: &mut State<'_>,
    src: &Src,
    root: &str,
    config: &ConfigValue,
    workspace_patterns: &[String],
    run: &mut Run,
) {
    let pairs = entries(state, src, root, config);
    if let Some((_, custom)) = pairs.iter().find(|(key, _)| *key == "customTypes") {
        custom_types(state, src, &join(root, "customTypes"), custom, &mut run.types);
    }
    for (key, item) in pairs {
        let path = join(root, key);
        match key {
            "customTypes" | "$schema" => {}
            _ if key.starts_with("//") => {}
            "versionGroups" => version_groups(state, src, &path, item, run),
            "semverGroups" => semver_groups(state, src, &path, item, &run.types),
            "dependencyGroups" => state.note_with(
                src,
                NoteClass::Unsupported,
                path,
                "skif has no dependency groups (an alias that bundles dependencies under one name)",
                "express the grouping with `dependencies` selectors on a version group",
            ),
            "source" => {
                let sources = string_list(state, src, &path, item);
                compare_source(state, src, &path, &sources, workspace_patterns);
            }
            "sourceMode" => state.note(
                src,
                NoteClass::Unsupported,
                path,
                "skif always reads the members of the workspace and has no source modes",
            ),
            "strict" => state.note_with(
                src,
                NoteClass::Human,
                path,
                "syncpack's strict mode has no skif equivalent",
                "see `skif rules` for the rules that decide what is reported",
            ),
            _ if FORMAT_KEYS.contains(&key) => format_key(state, src, &path, key, item),
            _ if REGISTRY_KEYS.contains(&key) => state.note(
                src,
                NoteClass::Approximate,
                path,
                "dropped: skif never queries a registry",
            ),
            _ if RETIRED_KEYS.contains(&key) => state.note(
                src,
                NoteClass::Human,
                path,
                "syncpack 15 rejects this key, so the file was not valid for it",
            ),
            other => state.note(
                src,
                NoteClass::Unsupported,
                path,
                format!("`{other}` is not a syncpack option"),
            ),
        }
    }
}

fn format_key(state: &mut State<'_>, src: &Src, path: &str, key: &str, item: &ConfigValue) {
    if key == "sortAz" {
        let names = string_list(state, src, path, item);
        if names.iter().any(|name| DEPENDENCY_MAPS.contains(&name.as_str())) {
            state.keep(["unsorted-dependencies"]);
            state.note(
                src,
                NoteClass::Approximate,
                path,
                "dependency maps stay sorted: `unsorted-dependencies` is kept; other manifest formatting is not checked",
            );
            return;
        }
    }
    state.note(
        src,
        NoteClass::Approximate,
        path,
        "dropped: it only affects `syncpack format`, and skif does not format manifests",
    );
}

fn custom_types(state: &mut State<'_>, src: &Src, path: &str, value: &ConfigValue, types: &mut Types) {
    for (name, definition) in entries(state, src, path, value) {
        let type_path = join(path, name);
        let mut strategy = None;
        let mut location = None;
        let mut external = false;
        for (key, item) in entries(state, src, &type_path, definition) {
            let key_path = join(&type_path, key);
            match key {
                "strategy" => strategy = string(state, src, &key_path, item),
                "path" => location = string(state, src, &key_path, item),
                "namePath" => {}
                "source" => external = true,
                other => state.note(
                    src,
                    NoteClass::Unsupported,
                    key_path,
                    format!("`{other}` is not a custom type option"),
                ),
            }
        }
        let kind = custom_type(
            state,
            src,
            &type_path,
            name,
            strategy.as_deref(),
            location.as_deref(),
            external,
        );
        types.custom.push((name.to_owned(), kind));
    }
}

fn custom_type(
    state: &mut State<'_>,
    src: &Src,
    path: &str,
    name: &str,
    strategy: Option<&str>,
    location: Option<&str>,
    external: bool,
) -> CustomKind {
    if external {
        state.note(
            src,
            NoteClass::Unsupported,
            path,
            "reads a file other than package.json, which skif cannot",
        );
        return CustomKind::Unsupported;
    }
    let (Some(strategy), Some(location)) = (strategy, location) else {
        state.note(src, NoteClass::Human, path, "needs both a `strategy` and a `path`");
        return CustomKind::Unsupported;
    };
    if strategy == "versionsByName" && location == "optionalDependencies" {
        state.note(
            src,
            NoteClass::Exact,
            path,
            "skif has the dependency type `optional`; the custom type is not needed",
        );
        return CustomKind::Optional;
    }
    let Some(target) = keys::strategy(strategy) else {
        state.note(
            src,
            NoteClass::Unsupported,
            path,
            format!("the strategy `{strategy}` has no skif equivalent"),
        );
        return CustomKind::Unsupported;
    };
    let element = state
        .doc
        .element(&[keys::CUSTOM_TYPES], vec![format!("syncpack customTypes.{name}")]);
    element.set(keys::CUSTOM_TYPE_NAME, Val::str(name));
    element.set(keys::CUSTOM_TYPE_PATH, Val::str(location));
    element.set(keys::CUSTOM_TYPE_STRATEGY, Val::str(target));
    state.note(src, NoteClass::Exact, path, "written to [[custom-types]]");
    CustomKind::Custom
}

fn map_types(state: &mut State<'_>, src: &Src, path: &str, tokens: &[String], types: &Types) -> MappedTypes {
    let mut kept: Vec<String> = Vec::new();
    let mut dropped: Vec<String> = Vec::new();
    let mut positive = false;
    let mut kept_positive = false;
    for token in tokens {
        let (negated, name) = token
            .strip_prefix('!')
            .map_or((false, token.as_str()), |name| (true, name));
        positive |= !negated;
        let mapped = match name {
            "prod" | "dev" | "peer" | "local" | "optional" => Some(name),
            _ if UNSUPPORTED_TYPES.contains(&name) => None,
            other => match types.kind(other) {
                Some(CustomKind::Optional) => Some("optional"),
                Some(CustomKind::Custom) => Some(other),
                _ => None,
            },
        };
        match mapped {
            Some(mapped) => {
                let entry = format!("{}{mapped}", if negated { "!" } else { "" });
                kept_positive |= !negated;
                if !kept.contains(&entry) {
                    kept.push(entry);
                }
            }
            None if !negated => dropped.push(name.to_owned()),
            None => {}
        }
    }
    let covers_nothing = positive && !kept_positive;
    if !dropped.is_empty() && !covers_nothing {
        state.note(
            src,
            NoteClass::Approximate,
            path,
            format!(
                "the dependency types {} are not covered by skif and were dropped",
                dropped.join(", ")
            ),
        );
    }
    MappedTypes {
        tokens: kept,
        covers_nothing,
    }
}

/// The selectors of a group, collected from its keys.
#[derive(Default)]
struct Selectors {
    label: Option<String>,
    dependencies: Vec<String>,
    packages: Vec<String>,
    dependency_types: Option<Vec<String>>,
    specifier_types: Vec<String>,
}

impl Selectors {
    /// Reads the key if it is a selector; `false` when it is not one.
    fn read(&mut self, state: &mut State<'_>, src: &Src, key: &str, path: &str, item: &ConfigValue) -> bool {
        match key {
            "label" => self.label = string(state, src, path, item),
            "dependencies" => self.dependencies = string_list(state, src, path, item),
            "packages" => self.packages = string_list(state, src, path, item),
            "dependencyTypes" => self.dependency_types = Some(string_list(state, src, path, item)),
            "specifierTypes" => self.specifier_types = string_list(state, src, path, item),
            _ => return false,
        }
        true
    }

    fn comment(&self, path: &str) -> Vec<String> {
        let origin = format!("syncpack {path}");
        vec![
            self.label
                .as_ref()
                .map_or(origin.clone(), |label| format!("{origin}: {label}")),
        ]
    }

    fn write(&self, section: &mut crate::doc::Section, types: &[String]) {
        if let Some(label) = &self.label {
            section.set("label", Val::str(label.clone()));
        }
        if !self.dependencies.is_empty() {
            section.set("dependencies", Val::strs(self.dependencies.iter().cloned()));
        }
        if !self.packages.is_empty() {
            section.set("packages", Val::strs(self.packages.iter().cloned()));
        }
        if !types.is_empty() {
            section.set("dependency-types", Val::strs(types.iter().cloned()));
        }
        if !self.specifier_types.is_empty() {
            section.set("specifier-types", Val::strs(self.specifier_types.iter().cloned()));
        }
    }

    fn types(&self, state: &mut State<'_>, src: &Src, path: &str, types: &Types) -> Option<Vec<String>> {
        let Some(tokens) = &self.dependency_types else {
            return Some(Vec::new());
        };
        let mapped = map_types(state, src, &join(path, "dependencyTypes"), tokens, types);
        if mapped.covers_nothing {
            state.note_with(
                src,
                NoteClass::Unsupported,
                join(path, "dependencyTypes"),
                format!(
                    "the group only covers {}, which skif does not read; the group was not written",
                    tokens.join(", ")
                ),
                "skif checks dependencies, devDependencies, peerDependencies and optionalDependencies",
            );
            return None;
        }
        Some(mapped.tokens)
    }
}

enum Policy {
    Preferred,
    Pinned(String),
    Banned,
    Ignored,
    SameRange,
    SameMinor,
    Snapped(Vec<String>),
    Catalog,
}

#[derive(Default)]
struct GroupKeys {
    banned: bool,
    ignored: bool,
    pin: Option<String>,
    snap_to: Vec<String>,
    prefer: Option<String>,
    policy: Option<String>,
}

fn version_groups(state: &mut State<'_>, src: &Src, path: &str, value: &ConfigValue, run: &mut Run) {
    let Some(groups) = value.as_array() else {
        state.note(src, NoteClass::Human, path, "expected an array of groups");
        return;
    };
    for (index, group) in groups.iter().enumerate() {
        version_group(state, src, &at(path, index), group, run);
    }
}

fn version_group(state: &mut State<'_>, src: &Src, path: &str, group: &ConfigValue, run: &mut Run) {
    let mut selectors = Selectors::default();
    let mut options = GroupKeys::default();
    for (key, item) in entries(state, src, path, group) {
        let key_path = join(path, key);
        if selectors.read(state, src, key, &key_path, item) {
            continue;
        }
        match key {
            "isBanned" => options.banned = boolean(state, src, &key_path, item).unwrap_or(false),
            "isIgnored" => options.ignored = boolean(state, src, &key_path, item).unwrap_or(false),
            "pinVersion" => options.pin = string(state, src, &key_path, item),
            "snapTo" => options.snap_to = string_list(state, src, &key_path, item),
            "preferVersion" => options.prefer = string(state, src, &key_path, item),
            "policy" => options.policy = string(state, src, &key_path, item),
            "severity" => state.note_with(
                src,
                NoteClass::Human,
                key_path,
                "syncpack let a group set the severity of each state; skif sets levels per rule",
                "use [rules] or an [[overrides]] entry with max-level",
            ),
            other => state.note(
                src,
                NoteClass::Unsupported,
                key_path,
                format!("`{other}` is not a version group option"),
            ),
        }
    }
    let Some(types) = selectors.types(state, src, path, &run.types) else {
        return;
    };
    let Some(policy) = group_policy(state, src, path, options.policy.as_deref(), &options) else {
        return;
    };
    let mut prefer = None;
    if matches!(policy, Policy::Preferred) {
        prefer = options.prefer.as_deref().and_then(|text| match text {
            "highestSemver" => Some("highest"),
            "lowestSemver" => Some("lowest"),
            other => {
                state.note(
                    src,
                    NoteClass::Human,
                    join(path, "preferVersion"),
                    format!("`{other}` is not `highestSemver` or `lowestSemver`"),
                );
                None
            }
        });
    }
    if matches!(policy, Policy::Catalog) {
        state.doc.table(&["catalog"]).set("mode", Val::str("prefer"));
        state.note_with(
            src,
            NoteClass::Approximate,
            join(path, "policy"),
            "`catalog` is a global setting in skif: [catalog] mode = \"prefer\" was written and the group was not",
            "list dependencies to exclude in [catalog] exclude",
        );
        return;
    }
    let comments = selectors.comment(path);
    let element = state.doc.element(&["version-groups"], comments);
    selectors.write(element, &types);
    match policy {
        Policy::Preferred => {
            if let Some(prefer) = prefer {
                element.set("prefer", Val::str(prefer));
            }
        }
        Policy::Pinned(pin) => {
            element.set("policy", Val::str("pinned"));
            element.set("pin", Val::str(pin));
        }
        Policy::Banned => element.set("policy", Val::str("banned")),
        Policy::Ignored => element.set("policy", Val::str("ignored")),
        Policy::SameRange => element.set("policy", Val::str("same-range")),
        Policy::SameMinor => element.set("policy", Val::str("same-minor")),
        Policy::Snapped(packages) => {
            element.set("policy", Val::str("snapped-to"));
            element.set("snap-to", Val::strs(packages));
        }
        Policy::Catalog => {}
    }
    state.note(src, NoteClass::Exact, path, "written to [[version-groups]]");
}

fn group_policy(
    state: &mut State<'_>,
    src: &Src,
    path: &str,
    named: Option<&str>,
    options: &GroupKeys,
) -> Option<Policy> {
    if options.banned {
        return Some(Policy::Banned);
    }
    if options.ignored {
        return Some(Policy::Ignored);
    }
    if let Some(pin) = &options.pin {
        return Some(Policy::Pinned(pin.clone()));
    }
    if !options.snap_to.is_empty() {
        return Some(Policy::Snapped(options.snap_to.clone()));
    }
    let policy_path = join(path, "policy");
    match named {
        None => Some(Policy::Preferred),
        Some("sameRange") => Some(Policy::SameRange),
        Some("sameMinor") => Some(Policy::SameMinor),
        Some("banned") => Some(Policy::Banned),
        Some("ignored") => Some(Policy::Ignored),
        Some("catalog") => Some(Policy::Catalog),
        Some("semverRangeOnly") => {
            state.note_with(
                src,
                NoteClass::Unsupported,
                policy_path,
                "skif cannot check ranges while letting versions differ; the group was written with policy = \"ignored\"",
                "the semver groups still apply to these dependencies",
            );
            Some(Policy::Ignored)
        }
        Some(other) => {
            state.note(
                src,
                NoteClass::Human,
                policy_path,
                format!("`{other}` is not a policy skif knows; the group uses the default policy"),
            );
            Some(Policy::Preferred)
        }
    }
}

fn semver_groups(state: &mut State<'_>, src: &Src, path: &str, value: &ConfigValue, types: &Types) {
    let Some(groups) = value.as_array() else {
        state.note(src, NoteClass::Human, path, "expected an array of groups");
        return;
    };
    for (index, group) in groups.iter().enumerate() {
        semver_group(state, src, &at(path, index), group, types);
    }
}

fn semver_group(state: &mut State<'_>, src: &Src, path: &str, group: &ConfigValue, types: &Types) {
    let mut selectors = Selectors::default();
    let mut range = None;
    let mut ignored = false;
    for (key, item) in entries(state, src, path, group) {
        let key_path = join(path, key);
        if selectors.read(state, src, key, &key_path, item) {
            continue;
        }
        match key {
            "range" => range = string(state, src, &key_path, item),
            "isIgnored" | "isDisabled" => ignored |= boolean(state, src, &key_path, item).unwrap_or(false),
            other => state.note(
                src,
                NoteClass::Unsupported,
                key_path,
                format!("`{other}` is not a semver group option"),
            ),
        }
    }
    let Some(dependency_types) = selectors.types(state, src, path, types) else {
        return;
    };
    if !ignored {
        match range.as_deref() {
            Some(range) if RANGES.contains(&range) => {}
            Some(other) => {
                state.note(
                    src,
                    NoteClass::Human,
                    join(path, "range"),
                    format!("`{other}` is not a range skif knows; the group was not written"),
                );
                return;
            }
            None => {
                state.note(
                    src,
                    NoteClass::Human,
                    path,
                    "the group has no `range`; it was not written",
                );
                return;
            }
        }
    }
    let comments = selectors.comment(path);
    let element = state.doc.element(&["semver-groups"], comments);
    selectors.write(element, &dependency_types);
    if ignored {
        element.set(keys::SEMVER_IGNORED, Val::Bool(true));
    } else if let Some(range) = range {
        element.set("range", Val::str(range));
    }
    state.note(src, NoteClass::Exact, path, "written to [[semver-groups]]");
}

fn normalize_source(pattern: &str) -> String {
    let trimmed = pattern.trim_start_matches("./").trim_end_matches('/');
    let trimmed = trimmed
        .strip_suffix("package.json")
        .unwrap_or(trimmed)
        .trim_end_matches('/');
    if trimmed.is_empty() {
        ".".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// A `source` that lists the workspace members is what skif does anyway.
fn compare_source(state: &mut State<'_>, src: &Src, path: &str, sources: &[String], workspace_patterns: &[String]) {
    let mut wanted: Vec<String> = sources
        .iter()
        .map(|source| normalize_source(source))
        .filter(|s| s != ".")
        .collect();
    let mut members: Vec<String> = workspace_patterns
        .iter()
        .map(|pattern| normalize_source(pattern))
        .collect();
    wanted.sort();
    wanted.dedup();
    members.sort();
    members.dedup();
    if wanted == members && !workspace_patterns.is_empty() {
        state.note(
            src,
            NoteClass::Default,
            path,
            "lists the workspace patterns, which skif always reads",
        );
    } else {
        state.note_with(
            src,
            NoteClass::Human,
            path,
            "syncpack read other package.json files than the workspace members",
            "skif reads the members of the workspace; add them there or lint the other directory separately",
        );
    }
}

fn read_flags(state: &mut State<'_>, src: &Src, invocation: &Invocation, workspace_patterns: &[String], run: &mut Run) {
    let path = invocation.path();
    if let Some(command @ ("update" | "format")) = invocation.subcommand() {
        state.note(
            src,
            NoteClass::Unsupported,
            format!("{path}: syncpack {command}"),
            format!("skif does not {command}; it lints and, with --fix, aligns"),
        );
        return;
    }
    let types = invocation.values("dependency-types", None);
    if !types.is_empty() {
        let flag_path = format!("{path}: --dependency-types");
        let mapped = map_types(state, src, &flag_path, &types, &run.types);
        if mapped.covers_nothing {
            state.note(
                src,
                NoteClass::Unsupported,
                flag_path,
                "only dependency types skif does not read; the default scope is kept",
            );
        } else {
            state.note(src, NoteClass::Exact, flag_path, "written to [policy] dependency-types");
            run.flag_types = Some(mapped.tokens);
        }
    }
    let sources = invocation.values("source", None);
    if !sources.is_empty() {
        compare_source(state, src, &format!("{path}: --source"), &sources, workspace_patterns);
    }
    for (flag, what) in [
        ("dependencies", "dependency names"),
        ("specifier-types", "specifier types"),
    ] {
        if invocation.has(flag, None) {
            state.note_with(
                src,
                NoteClass::Human,
                format!("{path}: --{flag}"),
                format!("limited the run to some {what}; skif checks every one"),
                "silence the rest with [[ignore]] if the narrowing was intended",
            );
        }
    }
    for flag in invocation.flags() {
        let handled = ["--dependency-types", "--source", "--dependencies", "--specifier-types"];
        if !handled.contains(&flag.as_str()) && !INERT_FLAGS.contains(&flag.as_str()) {
            state.note(
                src,
                NoteClass::Human,
                format!("{path}: {flag}"),
                "this syncpack flag is not translated",
            );
        }
    }
}

fn emit_scope(state: &mut State<'_>, src: &Src, run: &Run) {
    let (tokens, class, message) = match &run.flag_types {
        Some(tokens) => (tokens.clone(), None, ""),
        None => (
            run.types.default_scope(),
            Some(NoteClass::Approximate),
            "[policy] dependency-types was widened to the scope syncpack lints by default; resolutions, overrides and \
             pnpm overrides are not covered",
        ),
    };
    state.doc.table(&["policy"]).set("dependency-types", Val::strs(tokens));
    if let Some(class) = class {
        state.note(src, class, "dependencyTypes", message);
    }
}
