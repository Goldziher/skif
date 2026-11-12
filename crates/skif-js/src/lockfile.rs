//! Package facts read from a lockfile, for projects whose dependencies are not installed.

use skif_core::HashMap;
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use serde_json::Value;
use skif_core::FileSystem;

use crate::deps::{Installed, bins_of, optional_peers_of, peers_of};

#[derive(Default)]
struct Entry {
    bins: BTreeSet<String>,
    peers: BTreeSet<String>,
    optional_peers: BTreeSet<String>,
}

type Entries = HashMap<String, Entry>;

/// What the lockfile of a workspace records.
#[derive(Default)]
pub(crate) struct Locked {
    /// The binaries and (optional) peer dependencies each locked package declares, by package name.
    pub packages: HashMap<String, Arc<Installed>>,
    /// The `configVersion` of `bun.lock`, which sets the linker Bun defaults to; `None` without a `bun.lock`, or when
    /// it records none or does not parse.
    pub bun_config_version: Option<u64>,
}

/// Reads the lockfile at `root`.
///
/// The packages come from `pnpm-lock.yaml` (its project's document, not the package manager's environment), else
/// `package-lock.json`, `bun.lock`, `yarn.lock` (Berry, which records peers and binaries); none when no lockfile exists
/// or is understood. A classic (v1) `yarn.lock` records no binaries, so reading it would make every locked package look
/// like one without binaries; it is skipped. `bun.lock` is parsed once, for its packages and for its `configVersion`,
/// which is read whichever lockfile the packages come from.
pub(crate) fn read(fs: &dyn FileSystem, root: &Path) -> Locked {
    let bun_lock = fs
        .read_to_string(&root.join("bun.lock"))
        .ok()
        .map(|text| parse_bun(&text));
    let bun_config_version = bun_lock.as_ref().and_then(|lock| lock.get("configVersion")?.as_u64());
    let entries = if let Ok(text) = fs.read_to_string(&root.join("pnpm-lock.yaml")) {
        pnpm(skif_workspace::pnpm_project_document(&text))
    } else if let Ok(text) = fs.read_to_string(&root.join("package-lock.json")) {
        npm(&text)
    } else if let Some(lock) = &bun_lock {
        bun(lock)
    } else if let Ok(text) = fs.read_to_string(&root.join("yarn.lock")) {
        yarn(&text)
    } else {
        Entries::default()
    };
    let packages = entries
        .into_iter()
        .map(|(name, entry)| {
            let installed = Installed {
                bins: entry.bins.into_iter().collect(),
                peers: entry.peers.into_iter().collect(),
                optional_peers: entry.optional_peers.into_iter().collect(),
                version: None,
            };
            (name, Arc::new(installed))
        })
        .collect();
    Locked {
        packages,
        bun_config_version,
    }
}

fn unquote(text: &str) -> &str {
    let text = text.trim();
    for quote in ['\'', '"'] {
        if let Some(inner) = text.strip_prefix(quote) {
            return inner.split(quote).next().unwrap_or(inner);
        }
    }
    text
}

/// The key of a `pnpm-lock.yaml` mapping line: `'@scope/name': ^1` is `@scope/name`.
fn yaml_key(line: &str) -> Option<&str> {
    let line = line.trim();
    if line.starts_with(['\'', '"']) {
        return Some(unquote(line));
    }
    line.split_once(':').map(|(key, _)| key.trim())
}

/// The package name of a `packages:` key: `'/@a/b@1.0.0(peer@2)'` is `@a/b`.
fn package_of_key(key: &str) -> Option<&str> {
    let key = unquote(key.trim().trim_end_matches(':')).trim_start_matches('/');
    let key = key.split('(').next().unwrap_or(key);
    let at = key.rfind('@').filter(|at| *at > 0)?;
    Some(&key[..at])
}

fn pnpm(text: &str) -> Entries {
    #[derive(PartialEq)]
    enum Section {
        Peers,
        PeersMeta,
        Other,
    }
    let mut entries = Entries::default();
    let mut in_packages = false;
    let mut current: Option<String> = None;
    let mut section = Section::Other;
    // The peer whose `peerDependenciesMeta` entry is being read.
    let mut meta_peer: Option<String> = None;
    for line in text.lines() {
        let body = line.trim();
        if body.is_empty() || body.starts_with('#') {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        match indent {
            0 => {
                in_packages = body == "packages:";
                current = None;
            }
            2 if in_packages => {
                current = package_of_key(body).map(str::to_owned);
                if let Some(name) = &current {
                    entries.entry(name.clone()).or_default();
                }
                section = Section::Other;
            }
            4 if in_packages => {
                section = match body {
                    "peerDependencies:" => Section::Peers,
                    "peerDependenciesMeta:" => Section::PeersMeta,
                    _ => Section::Other,
                };
                if body == "hasBin: true"
                    && let Some(name) = &current
                {
                    entries
                        .entry(name.clone())
                        .or_default()
                        .bins
                        .insert(name.rsplit('/').next().unwrap_or(name).to_owned());
                }
            }
            6 if in_packages && section == Section::Peers => {
                if let (Some(name), Some(peer)) = (&current, yaml_key(body)) {
                    entries.entry(name.clone()).or_default().peers.insert(peer.to_owned());
                }
            }
            6 if in_packages && section == Section::PeersMeta => {
                meta_peer = yaml_key(body).map(str::to_owned);
            }
            8 if in_packages && section == Section::PeersMeta && is_optional_flag(body) => {
                if let (Some(name), Some(peer)) = (&current, &meta_peer) {
                    entries
                        .entry(name.clone())
                        .or_default()
                        .optional_peers
                        .insert(peer.clone());
                }
            }
            _ => {}
        }
    }
    entries
}

/// Whether a YAML lockfile line under a `peerDependenciesMeta` entry marks the peer optional.
fn is_optional_flag(body: &str) -> bool {
    body.split_once(':')
        .is_some_and(|(key, value)| key.trim() == "optional" && value.trim() == "true")
}

fn npm(text: &str) -> Entries {
    let mut entries = Entries::default();
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return entries;
    };
    let Some(packages) = value.get("packages").and_then(Value::as_object) else {
        return entries;
    };
    for (key, package) in packages {
        let Some((_, name)) = key.rsplit_once("node_modules/") else {
            continue;
        };
        let entry = entries.entry(name.to_owned()).or_default();
        entry.bins.extend(bins_of(package, name));
        entry.peers.extend(peers_of(package));
        entry.optional_peers.extend(optional_peers_of(package));
    }
    entries
}

/// `bun.lock` as JSON; `null` when it does not parse.
fn parse_bun(text: &str) -> Value {
    skif_workspace::parse_bun_lock(text).unwrap_or(Value::Null)
}

fn bun(value: &Value) -> Entries {
    let mut entries = Entries::default();
    let Some(packages) = value.get("packages").and_then(Value::as_object) else {
        return entries;
    };
    for package in packages.values() {
        let Some(parts) = package.as_array() else {
            continue;
        };
        let Some(name) = parts.first().and_then(Value::as_str).and_then(package_of_key) else {
            continue;
        };
        let entry = entries.entry(name.to_owned()).or_default();
        if let Some(meta) = parts.iter().find(|part| part.is_object()) {
            entry.bins.extend(bins_of(meta, name));
            entry.peers.extend(peers_of(meta));
            entry.optional_peers.extend(
                meta.get("optionalPeers")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned),
            );
        }
    }
    entries
}

/// Whether a `yarn.lock` is a Berry lock: only Berry writes the top-level `__metadata:` block.
pub(crate) fn is_yarn_berry(text: &str) -> bool {
    text.lines().any(|line| line.trim_end() == "__metadata:")
}

fn yarn(text: &str) -> Entries {
    #[derive(PartialEq)]
    enum Section {
        Peers,
        PeersMeta,
        Bins,
        Other,
    }
    let mut entries = Entries::default();
    if !is_yarn_berry(text) {
        return entries;
    }
    let mut current: Option<String> = None;
    let mut section = Section::Other;
    // The peer whose `peerDependenciesMeta` entry is being read.
    let mut meta_peer: Option<String> = None;
    for line in text.lines() {
        let body = line.trim();
        if body.is_empty() || body.starts_with('#') {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        match indent {
            0 => {
                section = Section::Other;
                current = body
                    .trim_end_matches(':')
                    .split(',')
                    .next()
                    .and_then(package_of_key)
                    .map(str::to_owned);
                if let Some(name) = &current {
                    entries.entry(name.clone()).or_default();
                }
            }
            2 => {
                section = match body {
                    "peerDependencies:" => Section::Peers,
                    "peerDependenciesMeta:" => Section::PeersMeta,
                    "bin:" => Section::Bins,
                    _ => Section::Other,
                };
            }
            4 => {
                if let (Some(name), Some(key)) = (&current, yaml_key(body)) {
                    let entry = entries.entry(name.clone()).or_default();
                    match section {
                        Section::Peers => {
                            entry.peers.insert(key.to_owned());
                        }
                        Section::Bins => {
                            entry.bins.insert(key.to_owned());
                        }
                        Section::PeersMeta => meta_peer = Some(key.to_owned()),
                        Section::Other => {}
                    }
                }
            }
            6 if section == Section::PeersMeta && is_optional_flag(body) => {
                if let (Some(name), Some(peer)) = (&current, &meta_peer) {
                    entries
                        .entry(name.clone())
                        .or_default()
                        .optional_peers
                        .insert(peer.clone());
                }
            }
            _ => {}
        }
    }
    entries
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn pnpm_lock_peers_and_bins() {
        let entries = pnpm(
            "lockfileVersion: '9.0'\n\nimporters:\n  .:\n    dependencies:\n      react:\n        specifier: 18.0.0\n\n\
             packages:\n\n  '@testing-library/react@16.0.0(@testing-library/dom@10.0.0)(react@18.0.0)':\n    \
             resolution: {integrity: sha512-x}\n    peerDependencies:\n      '@testing-library/dom': ^10.0.0\n      \
             react: ^18.0.0\n    peerDependenciesMeta:\n      '@types/react':\n        optional: true\n\n  \
             oxlint@1.0.0:\n    resolution: {integrity: sha512-y}\n    hasBin: true\n\nsnapshots:\n\n  \
             other@1.0.0:\n    peerDependencies:\n      nope: '1'\n",
        );
        let react = &entries["@testing-library/react"];
        assert_eq!(
            react.peers.iter().map(String::as_str).collect::<Vec<_>>(),
            ["@testing-library/dom", "react"]
        );
        assert_eq!(
            react.optional_peers.iter().map(String::as_str).collect::<Vec<_>>(),
            ["@types/react"]
        );
        assert_eq!(entries["oxlint"].bins.iter().next().map(String::as_str), Some("oxlint"));
        assert!(!entries.contains_key("other"));
    }

    #[test]
    fn a_two_document_pnpm_lock_is_read_from_the_project_document() {
        // pnpm 11 and later: the package manager's environment, pnpm with its binary, then the project's lockfile.
        let text = "---\nlockfileVersion: '9.0'\n\nimporters:\n\n  .:\n    configDependencies: {}\n    \
                    packageManagerDependencies:\n      pnpm:\n        specifier: 12.9.1\n        version: 12.9.1\n\n\
                    packages:\n\n  pnpm@12.9.1:\n    resolution: {integrity: sha512-pnpm}\n    hasBin: true\n\n\
                    snapshots:\n\n  pnpm@12.9.1: {}\n\n---\nlockfileVersion: '9.0'\n\nimporters:\n\n  .:\n    \
                    devDependencies:\n      oxlint:\n        specifier: 1.0.0\n        version: 1.0.0\n\n\
                    packages:\n\n  oxlint@1.0.0:\n    resolution: {integrity: sha512-oxlint}\n    hasBin: true\n    \
                    peerDependencies:\n      typescript: ^5.0.0\n\nsnapshots:\n\n  oxlint@1.0.0: {}\n";
        let fs = skif_core::MemoryFs::new().with_file("/r/pnpm-lock.yaml", text);
        let packages = read(&fs, Path::new("/r")).packages;
        assert_eq!(
            packages.keys().map(String::as_str).collect::<BTreeSet<_>>(),
            BTreeSet::from(["oxlint"])
        );
        assert_eq!(packages["oxlint"].bins, ["oxlint"]);
        assert_eq!(packages["oxlint"].peers, ["typescript"]);
    }

    #[test]
    fn lockfiles_record_optional_peers() {
        let pnpm_lock = pnpm(
            "lockfileVersion: '9.0'\n\npackages:\n\n  runner@1.0.0:\n    resolution: {integrity: sha512-x}\n    \
             peerDependencies:\n      otel: ^1.9.0\n      ui: ^1.0.0\n    peerDependenciesMeta:\n      otel:\n        \
             optional: true\n      ui:\n        optional: false\n",
        );
        let npm_lock = npm(
            r#"{"packages":{"node_modules/runner":{"peerDependencies":{"otel":"^1.9.0","ui":"1"},
              "peerDependenciesMeta":{"otel":{"optional":true},"ui":{"optional":false}}}}}"#,
        );
        let bun_lock = bun(&parse_bun(
            r#"{"lockfileVersion":1,"packages":{"runner":["runner@1.0.0","",{"peerDependencies":{"otel":"^1.9.0","ui":"1",},"optionalPeers":["otel"],},"sha512-x"],}}"#,
        ));
        let yarn_lock = yarn(
            "__metadata:\n  version: 8\n\n\"runner@npm:^1.0.0\":\n  version: 1.0.0\n  peerDependencies:\n    \
             otel: ^1.9.0\n    ui: ^1.0.0\n  peerDependenciesMeta:\n    otel:\n      optional: true\n    ui:\n      \
             optional: false\n  checksum: abc\n",
        );
        for (lockfile, entries) in [
            ("pnpm", pnpm_lock),
            ("npm", npm_lock),
            ("bun", bun_lock),
            ("yarn", yarn_lock),
        ] {
            let runner = &entries["runner"];
            assert_eq!(
                runner.peers.iter().map(String::as_str).collect::<Vec<_>>(),
                ["otel", "ui"],
                "{lockfile}"
            );
            assert_eq!(
                runner.optional_peers.iter().map(String::as_str).collect::<Vec<_>>(),
                ["otel"],
                "{lockfile}"
            );
        }
    }

    #[test]
    fn npm_lock_peers_and_bins() {
        let entries = npm(r#"{"packages":{"":{"name":"app"},
              "node_modules/a/node_modules/@s/tool":{"bin":{"tool-cli":"cli.js"},"peerDependencies":{"p":"1"}},
              "node_modules/solo":{"bin":"x.js"}}}"#);
        assert!(entries["@s/tool"].bins.contains("tool-cli") && entries["@s/tool"].peers.contains("p"));
        assert!(entries["solo"].bins.contains("solo"));
    }

    #[test]
    fn bun_lock_peers_and_bins() {
        let entries = bun(&parse_bun(
            r#"{
  "lockfileVersion": 1,
  "workspaces": { "": { "name": "app", }, },
  "packages": {
    "@s/react": ["@s/react@16.0.0", "", { "peerDependencies": { "@s/dom": "^10", "react": "^18", }, }, "sha512-x"],
    "oxlint": ["oxlint@1.0.0", "", { "bin": { "oxlint": "bin.js" } }, "sha512-y"],
    "app": ["app@workspace:."],
  },
}"#,
        ));
        assert!(entries["@s/react"].peers.contains("@s/dom") && entries["@s/react"].peers.contains("react"));
        assert!(entries["oxlint"].bins.contains("oxlint"));
    }

    #[test]
    fn read_records_the_config_version_of_bun_lock() {
        let lock = |config: &str| format!("{{\n  \"lockfileVersion\": 1,{config}\n  \"workspaces\": {{}},\n}}\n");
        let cases = [
            (lock("\n  \"configVersion\": 1,"), Some(1)),
            (lock("\n  \"configVersion\": 0,"), Some(0)),
            (lock(""), None),
            (format!("\u{feff}{}", lock("\n  \"configVersion\": 1,")), Some(1)),
            ("not json".to_owned(), None),
        ];
        for (text, expected) in cases {
            let fs = skif_core::MemoryFs::new().with_file("/r/bun.lock", &text);
            assert_eq!(read(&fs, Path::new("/r")).bun_config_version, expected, "{text}");
        }
        // A lockfile read before `bun.lock` for the packages leaves its `configVersion` read.
        let fs = skif_core::MemoryFs::new()
            .with_file("/r/pnpm-lock.yaml", "lockfileVersion: '9.0'\n")
            .with_file("/r/bun.lock", lock("\n  \"configVersion\": 1,"));
        assert_eq!(read(&fs, Path::new("/r")).bun_config_version, Some(1));
        assert_eq!(
            read(&skif_core::MemoryFs::new(), Path::new("/r")).bun_config_version,
            None
        );
    }

    #[test]
    fn yarn_berry_lock_peers_and_bins() {
        let entries = yarn(
            "__metadata:\n  version: 8\n\n\"@s/react@npm:^16.0.0, @s/react@npm:^16.1.0\":\n  version: 16.0.0\n  \
             peerDependencies:\n    \"@s/dom\": ^10.0.0\n    react: ^18.0.0\n  checksum: abc\n\n\
             \"oxlint@npm:1.0.0\":\n  version: 1.0.0\n  bin:\n    oxlint: bin.js\n  languageName: node\n",
        );
        assert!(entries["@s/react"].peers.contains("@s/dom") && entries["@s/react"].peers.contains("react"));
        assert!(entries["oxlint"].bins.contains("oxlint"));
    }

    #[test]
    fn yarn_classic_lock_is_not_read() {
        let entries = yarn(
            "# THIS IS AN AUTOGENERATED FILE. DO NOT EDIT THIS FILE DIRECTLY.\n# yarn lockfile v1\n\n\n             vitest@^1.0.0:\n  version \"1.0.0\"\n  resolved \"https://registry.yarnpkg.com/vitest/-/vitest-1.0.0.tgz\"\n               integrity sha512-x\n  dependencies:\n    tinypool \"^0.8.0\"\n\n             prettier@^3.0.0:\n  version \"3.0.0\"\n  resolved \"https://registry.yarnpkg.com/prettier/-/prettier-3.0.0.tgz\"\n",
        );
        assert!(
            entries.is_empty(),
            "classic lock read: {:?}",
            entries.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn read_skips_a_classic_yarn_lock_but_reads_berry() {
        let classic = "# yarn lockfile v1\n\n\nvitest@^1.0.0:\n  version \"1.0.0\"\n";
        let berry =
            "__metadata:\n  version: 8\n\n\"vitest@npm:^1.0.0\":\n  version: 1.0.0\n  bin:\n    vitest: vitest.mjs\n";
        let fs = skif_core::MemoryFs::new().with_file("/r/yarn.lock", classic);
        assert!(read(&fs, Path::new("/r")).packages.is_empty());
        let fs = skif_core::MemoryFs::new().with_file("/r/yarn.lock", berry);
        let entries = read(&fs, Path::new("/r")).packages;
        assert_eq!(entries["vitest"].bins, ["vitest"]);
    }
}
