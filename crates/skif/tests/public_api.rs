//! Audits the public surface of the facade for third-party types outside the allow-list that the `skif` crate
//! documentation states once (`<!-- allow-list: ... -->`), the amendment of ADR 0003 records.
//!
//! A rustdoc-JSON check needs a nightly toolchain, so this audit reads the sources instead. For the facade and for
//! every `skif_*` crate it re-exports from, each public item header (a `pub` function, type, field, constant) is
//! checked for a path or an imported name that comes from a crate outside the allow-list, the standard library and
//! skif itself. It is conservative: it also reads public items the facade does not re-export.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeSet;
use std::path::PathBuf;

const STANDARD: [&str; 6] = ["std", "core", "alloc", "crate", "self", "super"];

fn repo_file(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").join(relative)
}

fn is_ident(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn idents(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !is_ident(c))
        .filter(|word| word.chars().next().is_some_and(|first| !first.is_ascii_digit()))
}

/// The source without comment lines and without the unit tests at its end.
fn code(source: &str) -> String {
    let source = source.split("#[cfg(test)]").next().unwrap_or_default();
    source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The statements that start with `keyword`, as the text between the keyword and the closing `;`.
fn statements(code: &str, prefixes: &[&str]) -> Vec<String> {
    let lines: Vec<&str> = code.lines().collect();
    let mut found = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index].trim();
        if let Some(rest) = prefixes.iter().find_map(|prefix| line.strip_prefix(prefix)) {
            let mut text = rest.to_owned();
            while !text.contains(';') && index + 1 < lines.len() {
                index += 1;
                text.push(' ');
                text.push_str(lines[index].trim());
            }
            found.push(text.split(';').next().unwrap_or_default().to_owned());
        }
        index += 1;
    }
    found
}

/// The headers of the public items of `code`: signatures up to their body, field lines, and the variant lines of
/// public enums.
fn public_headers(code: &str) -> Vec<String> {
    const ITEMS: [&str; 7] = ["fn", "struct", "enum", "trait", "type", "static", "union"];
    let lines: Vec<&str> = code.lines().collect();
    let mut headers = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index].trim();
        let is_public = line.starts_with("pub ") && !line.starts_with("pub use") && !line.starts_with("pub mod");
        if is_public {
            let keyword = line
                .split_whitespace()
                .skip(1)
                .find(|word| !matches!(*word, "const" | "unsafe" | "async" | "extern"))
                .unwrap_or_default();
            if ITEMS.contains(&keyword) {
                let mut text = line.to_owned();
                while !text.contains(['{', ';']) && index + 1 < lines.len() {
                    index += 1;
                    text.push(' ');
                    text.push_str(lines[index].trim());
                }
                let opens_enum = keyword == "enum" && text.contains('{');
                headers.push(text.split(['{', ';']).next().unwrap_or_default().to_owned());
                if opens_enum {
                    let mut depth = brace_balance(lines[index]);
                    while depth > 0 && index + 1 < lines.len() {
                        index += 1;
                        let variant = lines[index].trim();
                        depth += brace_balance(variant);
                        if !variant.is_empty() && !variant.starts_with("#[") {
                            headers.push(variant.to_owned());
                        }
                    }
                }
            } else {
                headers.push(line.trim_end_matches(',').to_owned());
            }
        }
        index += 1;
    }
    headers
}

fn brace_balance(line: &str) -> i32 {
    let opens = line.matches('{').count();
    let closes = line.matches('}').count();
    i32::try_from(opens).unwrap_or(0) - i32::try_from(closes).unwrap_or(0)
}

/// The first segment of every `a::b` path in `text`.
fn path_roots(text: &str) -> Vec<&str> {
    text.match_indices("::")
        .filter_map(|(position, _)| {
            let before = &text[..position];
            let root = before.rsplit(|c: char| !is_ident(c)).next().unwrap_or_default();
            let start = before.len() - root.len();
            let first_segment = !before[..start].ends_with(':');
            (first_segment && root.chars().next().is_some_and(|c| c.is_ascii_lowercase())).then_some(root)
        })
        .collect()
}

/// What `source` takes from outside the allow-list: the names it imports from a foreign crate and the paths of its
/// public headers that start in one.
fn violations(source: &str, allowed: &BTreeSet<String>) -> Vec<String> {
    let code = code(source);
    let modules: BTreeSet<String> = statements(&code, &["mod ", "pub mod "])
        .iter()
        .filter_map(|body| idents(body).next().map(str::to_owned))
        .collect();
    let is_local = |root: &str| {
        STANDARD.contains(&root) || root.starts_with("skif") || allowed.contains(root) || modules.contains(root)
    };
    let mut foreign_names: BTreeSet<String> = BTreeSet::new();
    let mut local_names: BTreeSet<String> = BTreeSet::new();
    for body in statements(&code, &["use ", "pub use ", "pub(crate) use "]) {
        let root = idents(&body).next().unwrap_or_default();
        let names = idents(&body).filter(|name| !matches!(*name, "as" | "self" | "crate" | "super"));
        if is_local(root) {
            local_names.extend(names.map(str::to_owned));
        } else {
            foreign_names.extend(
                names
                    .filter(|name| name.starts_with(char::is_uppercase))
                    .map(str::to_owned),
            );
        }
    }
    let mut found = Vec::new();
    for header in public_headers(&code) {
        for root in path_roots(&header) {
            if !is_local(root) && !local_names.contains(root) {
                found.push(format!("{header}: path `{root}::`"));
            }
        }
        for name in idents(&header) {
            if foreign_names.contains(name) && !local_names.contains(name) {
                found.push(format!("{header}: imported `{name}`"));
            }
        }
    }
    found
}

fn allow_list() -> BTreeSet<String> {
    let lib = std::fs::read_to_string(repo_file("crates/skif/src/lib.rs")).unwrap();
    let marker = lib
        .lines()
        .find_map(|line| {
            line.trim_start_matches("//!")
                .trim()
                .strip_prefix("<!-- allow-list:")
                .and_then(|rest| rest.strip_suffix("-->"))
        })
        .expect("crates/skif/src/lib.rs states the allow-list as `<!-- allow-list: ... -->`");
    marker.split(',').map(|name| name.trim().to_owned()).collect()
}

/// The directories of `crates/` whose items the facade re-exports.
fn audited_crates() -> BTreeSet<String> {
    let lib = std::fs::read_to_string(repo_file("crates/skif/src/lib.rs")).unwrap();
    let mut crates: BTreeSet<String> = lib
        .lines()
        .filter_map(|line| line.trim().strip_prefix("pub use skif_"))
        .filter_map(|rest| rest.split("::").next())
        .map(|name| format!("skif-{}", name.replace('_', "-")))
        .collect();
    crates.insert("skif".to_owned());
    crates
}

fn sources(directory: &std::path::Path, found: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(&path, found);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            found.push(path);
        }
    }
}

/// The tests that read the sibling crates need the repository; a source tarball holds the facade alone.
fn in_repository() -> bool {
    repo_file("crates/skif-core/src").is_dir()
}

#[test]
fn the_public_surface_names_only_allow_listed_third_party_crates() {
    if !in_repository() {
        return;
    }
    let allowed = allow_list();
    assert!(
        allowed.contains("serde_json") && allowed.contains("nodejs_semver"),
        "{allowed:?}"
    );
    let crates = audited_crates();
    assert!(crates.len() >= 5, "{crates:?}");
    let mut leaks = Vec::new();
    let mut audited = 0;
    for name in &crates {
        let mut files = Vec::new();
        sources(&repo_file(&format!("crates/{name}/src")), &mut files);
        for file in files {
            let source = std::fs::read_to_string(&file).unwrap();
            audited += public_headers(&code(&source)).len();
            leaks.extend(
                violations(&source, &allowed)
                    .into_iter()
                    .map(|leak| format!("{}: {leak}", file.display())),
            );
        }
    }
    assert!(
        audited > 200,
        "only {audited} public items were audited; the scanner has stopped finding them"
    );
    assert_eq!(
        leaks,
        Vec::<String>::new(),
        "third-party types outside the allow-list in the public API"
    );
}

#[test]
fn the_audit_flags_a_type_from_a_crate_outside_the_allow_list() {
    let allowed = allow_list();
    let leaking = "use regex::Regex;\n\npub struct Matcher {\n    pub pattern: Regex,\n}\n";
    assert_eq!(
        violations(leaking, &allowed).len(),
        1,
        "{:?}",
        violations(leaking, &allowed)
    );
    let qualified = "pub fn compile(source: &str) -> globset::GlobSet {\n    todo!()\n}\n";
    assert_eq!(
        violations(qualified, &allowed).len(),
        1,
        "{:?}",
        violations(qualified, &allowed)
    );
}

#[test]
fn the_audit_accepts_the_allow_list_the_standard_library_and_skif() {
    let allowed = allow_list();
    let fine = "use std::path::PathBuf;\nuse serde_json::Value;\nuse skif_core::Fix;\nmod inner;\npub use inner::Thing;\n\
                pub fn load(path: PathBuf, value: Value, fix: Fix) -> std::io::Result<indexmap::IndexMap<String, Thing>> {\n    todo!()\n}\n";
    assert_eq!(violations(fine, &allowed), Vec::<String>::new());
    let private = "use regex::Regex;\n\nfn helper(pattern: Regex) {}\npub(crate) fn also(pattern: &Regex) {}\n";
    assert_eq!(violations(private, &allowed), Vec::<String>::new());
}

#[test]
fn the_documented_allow_list_is_the_one_the_adr_records() {
    if !in_repository() {
        return;
    }
    let records: Vec<String> = std::fs::read_dir(repo_file("adrs"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "md"))
        .map(|path| std::fs::read_to_string(path).unwrap())
        .filter(|text| text.contains("# Third-Party Types in the Facade API"))
        .collect();
    let [record] = records.as_slice() else {
        panic!(
            "expected exactly one ADR on the third-party allow-list, found {}",
            records.len()
        );
    };
    for name in allow_list() {
        assert!(record.contains(&format!("`{name}`")), "the ADR does not name `{name}`");
    }
}
