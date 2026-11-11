//! CODEOWNERS parsing and matching, and the renderer that groups diagnostics by owner.
//!
//! The caller reads the file; this module only parses text and matches paths.

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::Path;

use skif_core::{Diagnostic, Report};

use crate::compact::compact_line;
use crate::options::RenderOptions;

/// The places GitHub and GitLab look for a CODEOWNERS file, relative to the repository top-level, in the order they
/// look; the first that exists is used.
pub const CODEOWNERS_LOCATIONS: [&str; 3] = [".github/CODEOWNERS", "CODEOWNERS", "docs/CODEOWNERS"];

/// The heading of the group of diagnostics that no CODEOWNERS entry covers.
const UNOWNED: &str = "(unowned)";

/// A gitignore-like CODEOWNERS pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Pattern {
    /// `/`-separated segments; an unanchored pattern starts with `**`.
    segments: Vec<String>,
    /// The pattern ended in `/`, so it matches directories (and everything in them) but not files.
    dir_only: bool,
    /// The pattern ended in `/*`, so, as on GitHub, it matches the files directly in that directory and not the
    /// contents of its subdirectories.
    files_only: bool,
}

impl Pattern {
    fn parse(text: &str) -> Option<Self> {
        let (text, rooted) = text.strip_prefix('/').map_or((text, false), |rest| (rest, true));
        let (text, dir_only) = text.strip_suffix('/').map_or((text, false), |rest| (rest, true));
        if text.is_empty() {
            return None;
        }
        let anchored = rooted || text.contains('/');
        let mut segments: Vec<String> = Vec::new();
        if !anchored {
            segments.push("**".to_owned());
        }
        for segment in text.split('/').filter(|segment| !segment.is_empty()) {
            // `a/**/**/b` is `a/**/b`; collapsing avoids re-trying the same split once per redundant `**`.
            if segment == "**" && segments.last().is_some_and(|last| last == "**") {
                continue;
            }
            segments.push(segment.to_owned());
        }
        let files_only = segments.len() > 1 && segments.last().is_some_and(|last| last == "*");
        Some(Self {
            segments,
            dir_only,
            files_only,
        })
    }

    /// Whether the pattern matches the file `path` or one of the directories that contain it.
    fn matches(&self, path: &[&str]) -> bool {
        (1..=path.len())
            .filter(|&depth| {
                if depth < path.len() {
                    !self.files_only
                } else {
                    !self.dir_only
                }
            })
            .any(|depth| segments_match(&self.segments, &path[..depth]))
    }
}

fn segments_match(pattern: &[String], path: &[&str]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some((first, rest)) if first == "**" => (0..=path.len()).any(|skip| segments_match(rest, &path[skip..])),
        Some((first, rest)) => path
            .split_first()
            .is_some_and(|(name, tail)| wildcard(first, name) && segments_match(rest, tail)),
    }
}

/// Matches one path segment against `*` (any run of characters), `?` (one character) and `\x` (a literal `x`).
fn wildcard(pattern: &str, name: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let name: Vec<char> = name.chars().collect();
    let (mut p, mut n) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while n < name.len() {
        match pattern.get(p) {
            Some('*') => {
                star = Some((p, n));
                p += 1;
            }
            Some('?') => {
                p += 1;
                n += 1;
            }
            Some('\\') if pattern.get(p + 1) == Some(&name[n]) => {
                p += 2;
                n += 1;
            }
            Some(&c) if c != '\\' && c == name[n] => {
                p += 1;
                n += 1;
            }
            _ => match star {
                Some((star_p, star_n)) => {
                    p = star_p + 1;
                    n = star_n + 1;
                    star = Some((star_p, star_n + 1));
                }
                None => return false,
            },
        }
    }
    pattern[p..].iter().all(|&c| c == '*')
}

/// Splits a line into whitespace-separated tokens, honouring `\` escapes (`\ `, `\#`) and stopping at an unescaped
/// `#` that starts a token (a trailing comment).
fn tokens(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                // Keep the escape for wildcard characters so they stay literal; drop it for the rest.
                Some(next @ ('*' | '?' | '\\')) => {
                    current.push('\\');
                    current.push(next);
                }
                Some(next) => current.push(next),
                None => current.push('\\'),
            },
            '#' if current.is_empty() => break,
            c if c.is_whitespace() => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// The default owners of a GitLab section header (`[Section] @owner`, `^[Section][2] @owner`), or `None` when the
/// line is not a section header.
fn section_owners(line: &str) -> Option<Vec<String>> {
    let header = line.strip_prefix('^').unwrap_or(line).strip_prefix('[')?;
    let (_, rest) = header.split_once(']')?;
    // An optional approval count, `[2]`, follows the name.
    let rest = match rest.strip_prefix('[') {
        Some(count) => count.split_once(']').map_or(count, |(_, rest)| rest),
        None => rest,
    };
    Some(tokens(rest))
}

/// A parsed CODEOWNERS file: patterns with their owners, in file order.
///
/// Patterns follow the gitignore-like rules GitHub and GitLab document: a leading `/` or a `/` inside the pattern
/// anchors it to the repository top-level, otherwise it matches at any depth; `*` and `?` stay within a path
/// segment and `**` crosses segments; a trailing `/` matches a directory and everything in it; a pattern that
/// matches a directory covers the files inside, except that `dir/*` covers only the files directly in `dir`, as on
/// GitHub. The **last** matching line wins, and a line with no owners (and no section defaults) leaves its paths
/// unowned. Comments (`#`),
/// escaped spaces and `#` (`\ `, `\#`) and a leading byte order mark are understood. A GitLab section header
/// (`[Section] @owner`, `^[Section]`) gives its default owners, if any, to the entries below it that name none, as
/// GitLab does; otherwise sections are not kept apart, so their entries count as one list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CodeOwners {
    entries: Vec<(Pattern, Vec<String>)>,
}

impl CodeOwners {
    /// Parses the text of a CODEOWNERS file. A leading UTF-8 byte order mark is ignored. Lines that cannot be
    /// patterns are skipped; parsing never fails.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let mut entries = Vec::new();
        let mut defaults: Vec<String> = Vec::new();
        for line in text.lines().map(str::trim) {
            if let Some(owners) = section_owners(line) {
                defaults = owners;
                continue;
            }
            let mut tokens = tokens(line).into_iter();
            let Some(pattern) = tokens.next().as_deref().and_then(Pattern::parse) else {
                continue;
            };
            let owners: Vec<String> = tokens.collect();
            entries.push((pattern, if owners.is_empty() { defaults.clone() } else { owners }));
        }
        Self { entries }
    }

    /// The owners of a `/`-separated path relative to the repository top-level, in file order; empty when no line
    /// matches or the last matching line names no owner.
    #[must_use]
    pub fn owners(&self, path: &str) -> &[String] {
        let segments: Vec<&str> = path
            .split('/')
            .filter(|segment| !segment.is_empty() && *segment != ".")
            .collect();
        if segments.is_empty() {
            return &[];
        }
        self.entries
            .iter()
            .rev()
            .find(|(pattern, _)| pattern.matches(&segments))
            .map_or(&[], |(_, owners)| owners.as_slice())
    }
}

/// Renders the diagnostics grouped by owner, using the CODEOWNERS set with [`RenderOptions::with_codeowners`]
/// (without one, every diagnostic is unowned). Each group is a heading `<owner> (<count>)` followed by one
/// [compact](crate::render_compact) line per diagnostic; owners are in lexical order, `(unowned)` last, and a
/// diagnostic with several owners appears under each. Paths are matched relative to the repository top-level when
/// [`RenderOptions::with_repo_root`] is set.
///
/// # Errors
///
/// Returns any error produced by writing to `out`.
pub fn render_codeowners(report: &Report, options: &RenderOptions, out: &mut dyn Write) -> io::Result<()> {
    let mut owned: BTreeMap<&str, Vec<&Diagnostic>> = BTreeMap::new();
    let mut unowned: Vec<&Diagnostic> = Vec::new();
    // A report is sorted by path, so the diagnostics of one file follow each other and its owners are looked up
    // once for them.
    let mut last: Option<(&Path, &[String])> = None;
    for diagnostic in &report.diagnostics {
        let path = diagnostic.location.path.as_path();
        let owners = match last {
            Some((last_path, owners)) if last_path.as_os_str() == path.as_os_str() => owners,
            _ => {
                let owners = options
                    .codeowners()
                    .map_or(&[][..], |codeowners| codeowners.owners(&options.repo_path(path)));
                last = Some((path, owners));
                owners
            }
        };
        if owners.is_empty() {
            unowned.push(diagnostic);
        }
        for owner in owners {
            owned.entry(owner.as_str()).or_default().push(diagnostic);
        }
    }
    let groups = owned
        .into_iter()
        .chain((!unowned.is_empty()).then_some((UNOWNED, unowned)));
    for (index, (owner, diagnostics)) in groups.enumerate() {
        if index > 0 {
            writeln!(out)?;
        }
        writeln!(out, "{} ({})", crate::text::escape_controls(owner), diagnostics.len())?;
        for diagnostic in diagnostics {
            let path = options.display_path(&diagnostic.location.path);
            writeln!(out, "{}", compact_line(diagnostic, &path))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owners<'a>(codeowners: &'a CodeOwners, path: &str) -> Vec<&'a str> {
        codeowners.owners(path).iter().map(String::as_str).collect()
    }

    #[test]
    fn the_last_matching_line_wins() {
        let codeowners = CodeOwners::parse("* @all\n/packages/ @pkgs\npackages/web/** @web @design\n");
        assert_eq!(owners(&codeowners, "package.json"), ["@all"]);
        assert_eq!(owners(&codeowners, "packages/api/package.json"), ["@pkgs"]);
        assert_eq!(owners(&codeowners, "packages/web/src/a.ts"), ["@web", "@design"]);
    }

    #[test]
    fn unanchored_patterns_match_at_any_depth() {
        let codeowners = CodeOwners::parse("package.json @manifests\ndocs/ @docs\n*.ts @ts\n");
        assert_eq!(owners(&codeowners, "a/b/package.json"), ["@manifests"]);
        assert_eq!(owners(&codeowners, "a/docs/x.md"), ["@docs"]);
        assert_eq!(owners(&codeowners, "docs/x.md"), ["@docs"]);
        assert_eq!(owners(&codeowners, "src/deep/x.ts"), ["@ts"]);
        assert_eq!(owners(&codeowners, "src/x.tsx"), [] as [&str; 0]);
    }

    #[test]
    fn directory_patterns_need_a_directory() {
        let codeowners = CodeOwners::parse("build/ @build\nlib @lib\n");
        assert_eq!(owners(&codeowners, "build"), [] as [&str; 0]);
        assert_eq!(owners(&codeowners, "x/build/out.js"), ["@build"]);
        assert_eq!(owners(&codeowners, "lib/index.js"), ["@lib"]);
        assert_eq!(owners(&codeowners, "lib"), ["@lib"]);
    }

    #[test]
    fn wildcards_stay_in_their_segment() {
        let codeowners = CodeOwners::parse("/apps/*/package.json @apps\n/a/**/z.ts @z\n/v?.json @v\n");
        assert_eq!(owners(&codeowners, "apps/web/package.json"), ["@apps"]);
        assert_eq!(owners(&codeowners, "apps/web/nested/package.json"), [] as [&str; 0]);
        assert_eq!(owners(&codeowners, "a/z.ts"), ["@z"]);
        assert_eq!(owners(&codeowners, "a/b/c/z.ts"), ["@z"]);
        assert_eq!(owners(&codeowners, "v1.json"), ["@v"]);
        assert_eq!(owners(&codeowners, "v10.json"), [] as [&str; 0]);
        let codeowners = CodeOwners::parse("docs/* @docs\n");
        assert_eq!(owners(&codeowners, "docs/start.md"), ["@docs"]);
        assert_eq!(owners(&codeowners, "docs/build/notes.md"), [] as [&str; 0]);
    }

    #[test]
    fn comments_escapes_sections_and_ownerless_lines() {
        let text = "# heading\n\n[Frontend] @fe\n/my\\ dir/ @spaced # trailing\n\\#hash @hash\n* @all\n/vendor/\n";
        let codeowners = CodeOwners::parse(text);
        assert_eq!(owners(&codeowners, "my dir/a.ts"), ["@all"]);
        // `/vendor/` names no owners, so it takes the `[Frontend]` section's default.
        assert_eq!(owners(&codeowners, "vendor/x.js"), ["@fe"]);
        let codeowners = CodeOwners::parse(
            "* @all
/vendor/
",
        );
        assert_eq!(owners(&codeowners, "vendor/x.js"), [] as [&str; 0]);
        let codeowners = CodeOwners::parse("/my\\ dir/ @spaced # trailing\n\\#hash @hash\n");
        assert_eq!(owners(&codeowners, "my dir/a.ts"), ["@spaced"]);
        assert_eq!(owners(&codeowners, "#hash"), ["@hash"]);
    }

    #[test]
    fn gitlab_sections_give_default_owners_to_ownerless_entries() {
        let text = concat!(
            "/docs/\n[Web] @web @design\n/apps/web/\n/apps/web/api/ @api\n",
            "^[Optional][2] @opt\n*.md\n[Bare]\n/lib/\n",
        );
        let codeowners = CodeOwners::parse(text);
        assert_eq!(owners(&codeowners, "docs/a.txt"), [] as [&str; 0]);
        assert_eq!(owners(&codeowners, "apps/web/a.ts"), ["@web", "@design"]);
        assert_eq!(owners(&codeowners, "apps/web/api/a.ts"), ["@api"]);
        assert_eq!(owners(&codeowners, "README.md"), ["@opt"]);
        assert_eq!(owners(&codeowners, "lib/a.ts"), [] as [&str; 0]);
    }

    #[test]
    fn a_byte_order_mark_is_ignored() {
        let codeowners = CodeOwners::parse("\u{feff}/package.json @root\n");
        assert_eq!(owners(&codeowners, "package.json"), ["@root"]);
        let codeowners = CodeOwners::parse("\u{feff}[Web] @web\n/apps/\n");
        assert_eq!(owners(&codeowners, "apps/a.ts"), ["@web"]);
    }

    #[test]
    fn wildcard_table() {
        for (pattern, name, expected) in [
            ("*", "anything", true),
            ("*.json", "package.json", true),
            ("*.json", "package.jsonc", false),
            ("a*b*c", "aXbYc", true),
            ("a*b*c", "aXbY", false),
            ("?", "", false),
            ("\\*", "*", true),
            ("\\*", "x", false),
        ] {
            assert_eq!(wildcard(pattern, name), expected, "{pattern} {name}");
        }
    }
}
