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
/// Reads `pnpm-lock.yaml`, else `package-lock.json`, `bun.lock`, `yarn.lock` (Berry, which records peers and binaries),
/// from `root`. Empty when none exists or is understood.
pub(crate) fn read(fs: &dyn FileSystem, root: &Path) -> HashMap<String, Arc<Installed>> {
    let entries = if let Ok(text) = fs.read_to_string(&root.join("pnpm-lock.yaml")) {
        pnpm(&text)
    } else if let Ok(text) = fs.read_to_string(&root.join("package-lock.json")) {
        npm(&text)
    } else if let Ok(text) = fs.read_to_string(&root.join("bun.lock")) {
        bun(&text)
    } else if let Ok(text) = fs.read_to_string(&root.join("yarn.lock")) {
        yarn(&text)
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

/// `bun.lock` is JSONC: strips the trailing commas outside of strings so it parses as JSON.
fn strip_trailing_commas(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut in_string = false;
    let mut escaped = false;
    for (index, &c) in chars.iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
        } else if c == '"' {
            in_string = true;
        } else if c == ','
            && chars[index + 1..]
                .iter()
                .find(|next| !next.is_whitespace())
                .is_some_and(|next| matches!(next, '}' | ']'))
        {
            continue;
        }
        out.push(c);
    }
    out
}

fn bun(text: &str) -> Entries {
    let mut entries = Entries::new();
    let Ok(value) = serde_json::from_str::<Value>(&strip_trailing_commas(text)) else {
        return entries;
    };
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
        }
    }
    entries
}

fn yarn(text: &str) -> Entries {
    #[derive(PartialEq)]
    enum Section {
        Peers,
        Bins,
        Other,
    }
    let mut entries = Entries::new();
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
                        Section::Other => {}
                    }
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
        assert_eq!(entries["oxlint"].bins.iter().next().map(String::as_str), Some("oxlint"));
        assert!(!entries.contains_key("other"));
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
        let entries = bun(r#"{
  "lockfileVersion": 1,
  "workspaces": { "": { "name": "app", }, },
  "packages": {
    "@s/react": ["@s/react@16.0.0", "", { "peerDependencies": { "@s/dom": "^10", "react": "^18", }, }, "sha512-x"],
    "oxlint": ["oxlint@1.0.0", "", { "bin": { "oxlint": "bin.js" } }, "sha512-y"],
    "app": ["app@workspace:."],
  },
}"#);
        assert!(entries["@s/react"].peers.contains("@s/dom") && entries["@s/react"].peers.contains("react"));
        assert!(entries["oxlint"].bins.contains("oxlint"));
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
}
