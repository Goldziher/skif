//! Deterministic synthetic monorepo generator.
//!
//! [`build`] is pure: it returns every file of the workspace as `(relative path, contents)` plus the number of
//! injected issues, so tests never touch the disk. [`write`] materialises the result.

use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::ValueEnum;
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::rng::{Rng, hash_str};

const POOL: &str = include_str!("pool.txt");
const CATALOG_SIZE: usize = 40;
const WORKSPACE_GLOBS: [&str; 3] = ["apps/*", "packages/*", "packages/group/*"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum PackageManager {
    Pnpm,
    Npm,
    Yarn,
    Bun,
}

impl PackageManager {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pnpm => "pnpm",
            Self::Npm => "npm",
            Self::Yarn => "yarn",
            Self::Bun => "bun",
        }
    }

    const fn package_manager_field(self) -> &'static str {
        match self {
            Self::Pnpm => "pnpm@12.15.0",
            Self::Npm => "npm@11.0.0",
            Self::Yarn => "yarn@4.5.0",
            Self::Bun => "bun@1.2.0",
        }
    }

    const fn internal_specifier(self) -> &'static str {
        match self {
            Self::Npm => "*",
            _ => "workspace:*",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Options {
    pub packages: usize,
    pub seed: u64,
    pub package_manager: PackageManager,
    pub catalog: bool,
    pub mismatch_rate: f64,
}

/// Issues injected into the workspace, one counter per kind.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct IssueCounts {
    /// Dependency entries whose specifier differs from the workspace-wide one for that name.
    pub mismatched_dependencies: usize,
    /// Packages whose `dependencies` are not alphabetical.
    pub unsorted_packages: usize,
    /// Packages with an `@types/*` package in `dependencies`.
    pub types_in_dependencies: usize,
    /// Packages with an empty `optionalDependencies` object.
    pub empty_dependency_objects: usize,
}

#[derive(Debug)]
pub struct Generated {
    pub files: Vec<(PathBuf, String)>,
    pub issues: IssueCounts,
    pub packages: usize,
    pub dependency_entries: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    Caret,
    Tilde,
    Exact,
    Alias,
    Git,
    File,
    Link,
}

impl Shape {
    const fn is_semver(self) -> bool {
        matches!(self, Self::Caret | Self::Tilde | Self::Exact)
    }
}

type Version = (u32, u32, u32);

/// The workspace-wide specifier of a dependency name; every package agrees on it unless an issue is injected.
#[derive(Debug, Clone, Copy)]
struct Canon {
    shape: Shape,
    version: Version,
}

fn pool() -> Vec<&'static str> {
    POOL.lines().filter(|line| !line.is_empty()).collect()
}

fn canon(name: &str) -> Canon {
    let hash = hash_str(name);
    let shape = if name.starts_with("@types/") {
        Shape::Caret
    } else {
        match hash % 1000 {
            650..=799 => Shape::Tilde,
            800..=939 => Shape::Exact,
            940..=959 => Shape::Alias,
            960..=969 => Shape::Git,
            970..=979 => Shape::File,
            980..=989 => Shape::Link,
            _ => Shape::Caret,
        }
    };
    let part = |shift: u32, modulus: u64| u32::try_from((hash >> shift) % modulus).unwrap_or(0);
    Canon {
        shape,
        version: (part(10, 20), part(20, 30), part(30, 40)),
    }
}

fn slug(name: &str) -> String {
    name.trim_start_matches('@').replace('/', "-")
}

/// The `package.json` key and value for a dependency in the given shape.
fn entry(name: &str, shape: Shape, (major, minor, patch): Version) -> (String, String) {
    let version = format!("{major}.{minor}.{patch}");
    match shape {
        Shape::Caret => (name.to_owned(), format!("^{version}")),
        Shape::Tilde => (name.to_owned(), format!("~{version}")),
        Shape::Exact => (name.to_owned(), version),
        Shape::Alias => (format!("{}-alias", slug(name)), format!("npm:{name}@^{version}")),
        Shape::Git => (slug(name), format!("github:acme/{}#v{version}", slug(name))),
        Shape::File => (slug(name), format!("file:../../vendor/{}", slug(name))),
        Shape::Link => (slug(name), format!("link:../../vendor/{}", slug(name))),
    }
}

fn bump((major, minor, patch): Version, rng: &mut Rng) -> Version {
    match rng.below(3) {
        0 => (major + 1, 0, 0),
        1 => (major, minor + 1, patch),
        _ => (major, minor, patch + 1),
    }
}

struct Layout {
    dir: PathBuf,
    name: String,
    is_app: bool,
}

fn layout(index: usize) -> Layout {
    let (dir, is_app) = match index % 10 {
        0 | 1 => (format!("apps/app-{index}"), true),
        2..=7 => (format!("packages/pkg-{index}"), false),
        _ => (format!("packages/group/grp-{index}"), false),
    };
    let leaf = dir.rsplit('/').next().unwrap_or_default().to_owned();
    Layout {
        dir: dir.into(),
        name: format!("@bench/{leaf}"),
        is_app,
    }
}

struct Generator<'a> {
    options: &'a Options,
    pool: &'a [&'static str],
    catalog: &'a BTreeMap<&'static str, Canon>,
}

impl Generator<'_> {
    /// A pool name, skewed towards the front of the pool so popular packages recur across the workspace.
    fn pick(&self, rng: &mut Rng, types: Option<bool>) -> &'static str {
        loop {
            let index = rng.below_usize(self.pool.len()).min(rng.below_usize(self.pool.len()));
            let name = self.pool[index];
            let is_types = name.starts_with("@types/");
            if types.is_none_or(|want| want == is_types) {
                return name;
            }
        }
    }

    /// Specifier for `name` in one package, injecting a version mismatch at the configured rate.
    fn specifier(&self, name: &'static str, rng: &mut Rng, issues: &mut IssueCounts) -> (String, String) {
        let canon = canon(name);
        if canon.shape.is_semver() && rng.chance(self.options.mismatch_rate) {
            issues.mismatched_dependencies += 1;
            return entry(name, canon.shape, bump(canon.version, rng));
        }
        if self.catalog.contains_key(name) {
            return (name.to_owned(), "catalog:".to_owned());
        }
        entry(name, canon.shape, canon.version)
    }
}

fn object(entries: &[(String, String)]) -> Value {
    Value::Object(entries.iter().map(|(k, v)| (k.clone(), json!(v))).collect())
}

fn sorted(mut entries: Vec<(String, String)>) -> Vec<(String, String)> {
    entries.sort();
    entries
}

fn to_json(value: &Value) -> Result<String> {
    Ok(serde_json::to_string_pretty(value)? + "\n")
}

fn package_json(index: usize, ctx: &Generator<'_>, issues: &mut IssueCounts, entries: &mut usize) -> Result<(PathBuf, String)> {
    let layout = layout(index);
    let options = ctx.options;
    let mut rng = Rng::for_stream(options.seed, index as u64 + 1);
    let mut seen: HashSet<String> = HashSet::new();

    let mut dependencies: Vec<(String, String)> = Vec::new();
    for _ in 0..rng.range_usize(3, 14) {
        let (key, value) = ctx.specifier(ctx.pick(&mut rng, Some(false)), &mut rng, issues);
        if seen.insert(key.clone()) {
            dependencies.push((key, value));
        }
    }
    let mut dev_dependencies: Vec<(String, String)> = Vec::new();
    for _ in 0..rng.range_usize(2, 8) {
        let want_types = rng.chance(0.5);
        let (key, value) = ctx.specifier(ctx.pick(&mut rng, Some(want_types)), &mut rng, issues);
        if seen.insert(key.clone()) {
            dev_dependencies.push((key, value));
        }
    }
    for _ in 0..rng.range_usize(0, 3) {
        let target = rng.below_usize(index.max(1));
        let candidate = layout(target);
        if index > 0 && !candidate.is_app && seen.insert(candidate.name.clone()) {
            dependencies.push((candidate.name, options.package_manager.internal_specifier().to_owned()));
        }
    }

    if rng.chance(options.mismatch_rate) {
        let moved = dev_dependencies.iter().position(|(key, _)| key.starts_with("@types/"));
        if let Some(position) = moved {
            dependencies.push(dev_dependencies.remove(position));
            issues.types_in_dependencies += 1;
        } else if seen.insert("@types/node".to_owned()) {
            let (key, value) = ctx.specifier("@types/node", &mut rng, issues);
            dependencies.push((key, value));
            issues.types_in_dependencies += 1;
        }
    }

    let mut dependencies = sorted(dependencies);
    if dependencies.len() >= 2 && rng.chance(options.mismatch_rate) {
        dependencies.reverse();
        issues.unsorted_packages += 1;
    }
    let dev_dependencies = sorted(dev_dependencies);
    *entries += dependencies.len() + dev_dependencies.len();

    let mut manifest = Map::new();
    manifest.insert("name".into(), json!(layout.name));
    manifest.insert("version".into(), json!("1.0.0"));
    manifest.insert("private".into(), json!(true));
    manifest.insert("description".into(), json!(format!("Synthetic benchmark package {index}")));
    manifest.insert("main".into(), json!("src/index.ts"));
    manifest.insert("scripts".into(), json!({ "build": "tsc -b", "test": "vitest run" }));
    manifest.insert("dependencies".into(), object(&dependencies));
    manifest.insert("devDependencies".into(), object(&dev_dependencies));
    if rng.chance(options.mismatch_rate) {
        manifest.insert("optionalDependencies".into(), json!({}));
        issues.empty_dependency_objects += 1;
    }
    Ok((layout.dir.join("package.json"), to_json(&Value::Object(manifest))?))
}

fn pnpm_workspace_yaml(catalog: &BTreeMap<&'static str, Canon>) -> String {
    let mut out = String::from("packages:\n");
    for glob in WORKSPACE_GLOBS {
        let _ = writeln!(out, "  - \"{glob}\"");
    }
    if !catalog.is_empty() {
        out.push_str("catalog:\n");
        for (name, canon) in catalog {
            let (_, value) = entry(name, canon.shape, canon.version);
            let _ = writeln!(out, "  \"{name}\": \"{value}\"");
        }
    }
    out
}

fn lockfile(package_manager: PackageManager) -> (&'static str, &'static str) {
    match package_manager {
        PackageManager::Pnpm => ("pnpm-lock.yaml", "lockfileVersion: '9.0'\n"),
        PackageManager::Npm => (
            "package-lock.json",
            "{\n  \"name\": \"bench-monorepo\",\n  \"lockfileVersion\": 3,\n  \"requires\": true,\n  \"packages\": {}\n}\n",
        ),
        PackageManager::Yarn => ("yarn.lock", "# This file is generated by running \"yarn install\"\n\n__metadata:\n  version: 8\n"),
        PackageManager::Bun => ("bun.lock", "{\n  \"lockfileVersion\": 1,\n  \"workspaces\": {}\n}\n"),
    }
}

fn root_manifest(options: &Options, catalog: &BTreeMap<&'static str, Canon>) -> Result<String> {
    let mut manifest = Map::new();
    manifest.insert("name".into(), json!("bench-monorepo"));
    manifest.insert("version".into(), json!("0.0.0"));
    manifest.insert("private".into(), json!(true));
    manifest.insert("packageManager".into(), json!(options.package_manager.package_manager_field()));
    manifest.insert("scripts".into(), json!({ "build": "turbo build", "test": "turbo test", "lint": "turbo lint" }));
    let globs: Vec<&str> = WORKSPACE_GLOBS.to_vec();
    match options.package_manager {
        PackageManager::Pnpm => {}
        PackageManager::Bun if !catalog.is_empty() => {
            let catalog: Map<String, Value> = catalog
                .iter()
                .map(|(name, canon)| ((*name).to_owned(), json!(entry(name, canon.shape, canon.version).1)))
                .collect();
            manifest.insert("workspaces".into(), json!({ "packages": globs, "catalog": catalog }));
        }
        _ => {
            manifest.insert("workspaces".into(), json!(globs));
        }
    }
    let dev: Vec<(String, String)> = ["typescript", "prettier", "eslint", "turbo"]
        .into_iter()
        .map(|name| {
            if catalog.contains_key(name) {
                (name.to_owned(), "catalog:".to_owned())
            } else {
                let canon = canon(name);
                entry(name, Shape::Caret, canon.version)
            }
        })
        .collect();
    manifest.insert("devDependencies".into(), object(&sorted(dev)));
    to_json(&Value::Object(manifest))
}

/// Builds the workspace in memory.
///
/// # Errors
/// Fails when the options are inconsistent (zero packages, a catalog for a package manager without one, or a
/// mismatch rate outside `0..=1`).
pub fn build(options: &Options) -> Result<Generated> {
    if options.packages == 0 {
        bail!("--packages must be at least 1");
    }
    if !(0.0..=1.0).contains(&options.mismatch_rate) {
        bail!("--mismatch-rate must be within 0..=1");
    }
    if options.catalog && !matches!(options.package_manager, PackageManager::Pnpm | PackageManager::Bun) {
        bail!("--catalog requires --pm pnpm or --pm bun");
    }
    let pool = pool();
    let catalog: BTreeMap<&'static str, Canon> = if options.catalog {
        pool.iter()
            .copied()
            .filter(|name| canon(name).shape.is_semver())
            .take(CATALOG_SIZE)
            .map(|name| (name, canon(name)))
            .collect()
    } else {
        BTreeMap::new()
    };
    let ctx = Generator {
        options,
        pool: &pool,
        catalog: &catalog,
    };

    let mut issues = IssueCounts::default();
    let mut entries = 0;
    let mut files = Vec::with_capacity(options.packages + 3);
    files.push((PathBuf::from("package.json"), root_manifest(options, &catalog)?));
    if options.package_manager == PackageManager::Pnpm {
        files.push((PathBuf::from("pnpm-workspace.yaml"), pnpm_workspace_yaml(&catalog)));
    }
    let (lock_name, lock_contents) = lockfile(options.package_manager);
    files.push((PathBuf::from(lock_name), lock_contents.to_owned()));
    for index in 0..options.packages {
        files.push(package_json(index, &ctx, &mut issues, &mut entries).with_context(|| format!("package {index}"))?);
    }
    Ok(Generated {
        files,
        issues,
        packages: options.packages,
        dependency_entries: entries,
    })
}

/// Writes a generated workspace under `out`.
///
/// # Errors
/// Fails when a directory or file cannot be created.
pub fn write(generated: &Generated, out: &Path) -> Result<()> {
    for (relative, contents) in &generated.files {
        let path = out.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        fs::write(&path, contents).with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::cast_precision_loss)]
mod tests {
    use super::*;

    fn options(packages: usize, seed: u64) -> Options {
        Options {
            packages,
            seed,
            package_manager: PackageManager::Pnpm,
            catalog: false,
            mismatch_rate: 0.1,
        }
    }

    fn package_files(generated: &Generated) -> Vec<(&PathBuf, Value)> {
        generated
            .files
            .iter()
            .filter(|(path, _)| path.starts_with("apps") || path.starts_with("packages"))
            .map(|(path, contents)| (path, serde_json::from_str(contents).expect("valid JSON")))
            .collect()
    }

    fn deps(manifest: &Value, field: &str) -> Vec<(String, String)> {
        manifest
            .get(field)
            .and_then(Value::as_object)
            .map(|map| map.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_owned())).collect())
            .unwrap_or_default()
    }

    #[test]
    fn pool_has_no_duplicates_and_is_large() {
        let names = pool();
        let unique: HashSet<_> = names.iter().collect();
        assert_eq!(unique.len(), names.len());
        assert!(names.len() >= 250, "pool has {} names", names.len());
    }

    #[test]
    fn same_seed_is_byte_identical() {
        let a = build(&options(60, 42)).unwrap();
        let b = build(&options(60, 42)).unwrap();
        assert_eq!(a.files, b.files);
        assert_eq!(a.issues, b.issues);
    }

    #[test]
    fn different_seed_differs() {
        assert_ne!(build(&options(60, 1)).unwrap().files, build(&options(60, 2)).unwrap().files);
    }

    #[test]
    fn generates_requested_package_count_across_all_levels() {
        let generated = build(&options(200, 5)).unwrap();
        let packages = package_files(&generated);
        assert_eq!(packages.len(), 200);
        for prefix in ["apps/", "packages/pkg-", "packages/group/"] {
            assert!(
                packages.iter().any(|(path, _)| path.to_string_lossy().starts_with(prefix)),
                "no package under {prefix}"
            );
        }
        let names: HashSet<_> = packages.iter().filter_map(|(_, m)| m["name"].as_str()).collect();
        assert_eq!(names.len(), 200);
    }

    #[test]
    fn every_package_manager_emits_a_lockfile_and_valid_json() {
        for package_manager in [PackageManager::Pnpm, PackageManager::Npm, PackageManager::Yarn, PackageManager::Bun] {
            let generated = build(&Options {
                package_manager,
                ..options(20, 3)
            })
            .unwrap();
            let root = generated.files.iter().find(|(p, _)| p == Path::new("package.json")).unwrap();
            let root: Value = serde_json::from_str(&root.1).unwrap();
            assert_eq!(root["workspaces"].is_array(), package_manager != PackageManager::Pnpm);
            let lock = lockfile(package_manager).0;
            assert!(generated.files.iter().any(|(p, _)| p == Path::new(lock)), "{lock} missing");
        }
    }

    #[test]
    fn catalog_is_emitted_and_used() {
        let generated = build(&Options {
            catalog: true,
            ..options(100, 9)
        })
        .unwrap();
        let yaml = &generated.files.iter().find(|(p, _)| p == Path::new("pnpm-workspace.yaml")).unwrap().1;
        assert!(yaml.contains("catalog:\n"));
        let uses = package_files(&generated)
            .iter()
            .flat_map(|(_, m)| deps(m, "dependencies").into_iter().chain(deps(m, "devDependencies")))
            .filter(|(_, v)| v == "catalog:")
            .count();
        assert!(uses > 50, "only {uses} catalog: specifiers");
    }

    #[test]
    fn catalog_needs_a_package_manager_with_one() {
        let result = build(&Options {
            catalog: true,
            package_manager: PackageManager::Npm,
            ..options(5, 1)
        });
        assert!(result.is_err());
    }

    #[test]
    fn zero_rate_injects_nothing() {
        let generated = build(&Options {
            mismatch_rate: 0.0,
            ..options(100, 4)
        })
        .unwrap();
        assert_eq!(generated.issues, IssueCounts::default());
    }

    #[test]
    fn reported_issue_counts_match_the_files() {
        let generated = build(&options(500, 11)).unwrap();
        let mut found = IssueCounts::default();
        for (_, manifest) in package_files(&generated) {
            let dependencies = deps(&manifest, "dependencies");
            let keys: Vec<_> = dependencies.iter().map(|(k, _)| k.clone()).collect();
            let mut sorted_keys = keys.clone();
            sorted_keys.sort();
            found.unsorted_packages += usize::from(keys != sorted_keys);
            found.types_in_dependencies += usize::from(keys.iter().any(|k| k.starts_with("@types/")));
            found.empty_dependency_objects += usize::from(manifest.get("optionalDependencies").is_some());
            for (key, value) in dependencies.into_iter().chain(deps(&manifest, "devDependencies")) {
                let canon = canon(&key);
                if pool().contains(&key.as_str())
                    && canon.shape.is_semver() && value != entry(&key, canon.shape, canon.version).1 {
                    found.mismatched_dependencies += 1;
                }
            }
        }
        assert_eq!(found, generated.issues);
    }

    #[test]
    fn injected_issue_rates_are_close_to_the_requested_rate() {
        let generated = build(&options(1000, 21)).unwrap();
        let expected_packages = 100.0;
        for (label, count) in [
            ("unsorted", generated.issues.unsorted_packages),
            ("empty", generated.issues.empty_dependency_objects),
        ] {
            let ratio = count as f64 / expected_packages;
            assert!((0.6..=1.4).contains(&ratio), "{label}: {count} injected, expected about 100");
        }
        let types = generated.issues.types_in_dependencies as f64 / expected_packages;
        assert!((0.5..=1.5).contains(&types), "types: {types}");
        let per_entry = generated.issues.mismatched_dependencies as f64 / generated.dependency_entries as f64;
        assert!((0.04..=0.12).contains(&per_entry), "mismatch rate {per_entry}");
    }
}
