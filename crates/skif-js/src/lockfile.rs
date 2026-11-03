//! Package facts read from a lockfile, for projects whose dependencies are not installed.

use std::collections::BTreeSet;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use serde_json::Value;
use skif_core::FileSystem;

use crate::deps::{Installed, bins_of, peers_of};

#[derive(Default)]
struct Entry {
    bins: BTreeSet<String>,
    peers: BTreeSet<String>,
}

type Entries = HashMap<String, Entry>;

/// The binaries and peer dependencies each locked package declares, by package name.
///
/// Reads `pnpm-lock.yaml`, else `package-lock.json`, from `root`. Empty when neither exists or is understood.
pub(crate) fn read(fs: &dyn FileSystem, root: &Path) -> HashMap<String, Arc<Installed>> {
    let entries = if let Ok(text) = fs.read_to_string(&root.join("pnpm-lock.yaml")) {
        pnpm(&text)
    } else if let Ok(text) = fs.read_to_string(&root.join("package-lock.json")) {
        npm(&text)
    } else {
        Entries::new()
    };
    entries
        .into_iter()
        .map(|(name, entry)| {
            let installed = Installed {
                bins: entry.bins.into_iter().collect(),
                peers: entry.peers.into_iter().collect(),
            };
            (name, Arc::new(installed))
        })
        .collect()
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
        Other,
    }
    let mut entries = Entries::new();
    let mut in_packages = false;
    let mut current: Option<String> = None;
    let mut section = Section::Other;
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
                section = if body == "peerDependencies:" {
                    Section::Peers
                } else {
                    Section::Other
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
            _ => {}
        }
    }
    entries
}

fn npm(text: &str) -> Entries {
    let mut entries = Entries::new();
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
        assert_eq!(entries["oxlint"].bins.iter().next().map(String::as_str), Some("oxlint"));
        assert!(!entries.contains_key("other"));
    }

    #[test]
    fn npm_lock_peers_and_bins() {
        let entries = npm(
            r#"{"packages":{"":{"name":"app"},
              "node_modules/a/node_modules/@s/tool":{"bin":{"tool-cli":"cli.js"},"peerDependencies":{"p":"1"}},
              "node_modules/solo":{"bin":"x.js"}}}"#,
        );
        assert!(entries["@s/tool"].bins.contains("tool-cli") && entries["@s/tool"].peers.contains("p"));
        assert!(entries["solo"].bins.contains("solo"));
    }
}
