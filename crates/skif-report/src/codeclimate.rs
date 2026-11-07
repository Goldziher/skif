//! The Code Climate renderer, in the subset GitLab Code Quality reads.

use std::collections::BTreeMap;
use std::io::{self, Write};

use serde::Serialize;
use skif_core::{Report, Severity};

use crate::options::RenderOptions;

#[derive(Serialize)]
struct Issue<'a> {
    description: &'a str,
    check_name: &'a str,
    fingerprint: String,
    severity: &'static str,
    location: IssueLocation,
}

#[derive(Serialize)]
struct IssueLocation {
    path: String,
    lines: Lines,
}

#[derive(Serialize)]
struct Lines {
    begin: u32,
}

/// GitLab's severity scale has five levels; skif's three map to the middle and bottom of it.
const fn severity(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "major",
        Severity::Warning => "minor",
        _ => "info",
    }
}

/// The 128-bit FNV-1a hash of `parts`, each followed by a NUL, as 32 hex digits. It is stable across platforms,
/// releases and runs, which is all a fingerprint needs; it is not a security hash.
fn fnv1a_128(parts: &[&str]) -> String {
    const OFFSET: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
    const PRIME: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;
    let mut hash = OFFSET;
    for part in parts {
        for byte in part.bytes().chain(std::iter::once(0)) {
            hash ^= u128::from(byte);
            hash = hash.wrapping_mul(PRIME);
        }
    }
    format!("{hash:032x}")
}

/// Renders a Code Climate JSON array, one issue per diagnostic, for GitLab Code Quality (`artifacts:reports:codequality`).
///
/// `check_name` is the rule ID and `severity` maps error to `major`, warning to `minor` and info to `info`. The
/// `fingerprint` hashes the rule, path, package, dependency, specifier and message, not the line, so an issue keeps its identity when code above
/// it moves; identical diagnostics in one file are told apart by their order. Paths are relative to the repository
/// top-level when [`RenderOptions::with_repo_root`] is set, as GitLab expects. A diagnostic without a span points
/// at line 1.
///
/// # Errors
///
/// Returns any error produced by writing to `out`.
pub fn render_codeclimate(report: &Report, options: &RenderOptions, out: &mut dyn Write) -> io::Result<()> {
    let mut seen: BTreeMap<(&str, String, &str), usize> = BTreeMap::new();
    let issues: Vec<Issue<'_>> = report
        .diagnostics
        .iter()
        .map(|diagnostic| {
            let path = options.repo_path(&diagnostic.location.path);
            let rule = diagnostic.rule.as_str();
            let occurrence = seen
                .entry((rule, path.clone(), diagnostic.message.as_str()))
                .or_insert(0);
            let fingerprint = fnv1a_128(&[
                rule,
                &path,
                diagnostic.package.as_deref().unwrap_or_default(),
                diagnostic.dependency.as_deref().unwrap_or_default(),
                diagnostic.specifier.as_deref().unwrap_or_default(),
                &diagnostic.message,
                &occurrence.to_string(),
            ]);
            *occurrence += 1;
            Issue {
                description: &diagnostic.message,
                check_name: rule,
                fingerprint,
                severity: severity(diagnostic.severity),
                location: IssueLocation {
                    path,
                    lines: Lines {
                        begin: diagnostic.location.span.map_or(1, |span| span.start_line),
                    },
                },
            }
        })
        .collect();
    serde_json::to_writer_pretty(&mut *out, &issues).map_err(io::Error::other)?;
    writeln!(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv1a_128_matches_the_reference_vectors() {
        // FNV-1a 128 of "" is the offset basis; of "a" it is the published test vector. A NUL follows each part.
        let mut hash: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
        hash ^= u128::from(b'a');
        hash = hash.wrapping_mul(0x0000_0000_0100_0000_0000_0000_0000_013b);
        assert_eq!(format!("{hash:032x}"), "d228cb696f1a8caf78912b704e4a8964");
        assert_ne!(fnv1a_128(&["ab", "c"]), fnv1a_128(&["a", "bc"]));
        assert_eq!(fnv1a_128(&["x"]).len(), 32);
    }
}
