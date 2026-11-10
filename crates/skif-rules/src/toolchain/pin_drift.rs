use std::ops::Range;
use std::sync::Arc;

use serde_json::Value;
use skif_core::{Category, FileSystem, Fixability, Location, PathSeg, Related, RuleMeta, Severity, diagnostic};
use skif_lint::{Emitter, LintContext, OptionsError, Rule};
use skif_specifier::Specifier;
use skif_workspace::Package;

use super::commands::{manager_name, manager_named, split_spec};
use super::options::{self, ToolchainKind, ToolchainOptions};
use super::parse::{Pin, Tool};
use super::scan::{Scan, ToolchainFile};
use super::{Finding, emit, rule_ids};
use crate::manifest::support::{field_location, key};

/// Metadata of `toolchain-pin-drift`.
pub static META: RuleMeta = RuleMeta::new(
    "toolchain-pin-drift",
    Category::Workspace,
    Some(Severity::Warning),
    Fixability::None,
    "Node and package-manager versions pinned in version files, Dockerfiles and CI workflows must match the root's.",
);

/// The kinds of file read when the `kinds` option is not set.
const DEFAULT_KINDS: [ToolchainKind; 3] = [
    ToolchainKind::VersionFiles,
    ToolchainKind::Dockerfiles,
    ToolchainKind::Workflows,
];

/// The root version files, in the order the first one with a Node version becomes the source of truth.
const NODE_SOURCES: [&str; 5] = [".nvmrc", ".node-version", ".tool-versions", "mise.toml", ".mise.toml"];

const NODE_DETAIL: &str = "NodeVersion";
const MANAGER_DETAIL: &str = "PackageManagerVersion";
const COREPACK_DETAIL: &str = "CorepackUnpinned";

/// Reports Node and package-manager versions pinned outside `package.json` that disagree with the root.
///
/// Node is checked against the first root version file that pins it (`.nvmrc`, `.node-version`, `.tool-versions`,
/// `mise.toml`, `.mise.toml`), otherwise against the root's `engines.node`; a version file that is the source also
/// has `engines.node` checked against it. Package-manager pins are checked against `packageManager` (or the version
/// of `devEngines.packageManager`). Two pins agree when the versions they allow overlap: an exact version must equal
/// an exact source and satisfy a range, a range must admit an exact source. A Dockerfile that runs `corepack enable`
/// and never `corepack prepare` is reported at `info`, since the manager version is then whatever corepack resolves.
///
/// Options (`[rules.toolchain-pin-drift]`): `kinds` (default version files, Dockerfiles and workflows), `files` and
/// `ignore-files`, as for every toolchain rule.
pub struct ToolchainPinDrift {
    fs: Arc<dyn FileSystem>,
}

impl ToolchainPinDrift {
    /// The rule, reading toolchain files through `fs`.
    #[must_use]
    pub fn new(fs: Arc<dyn FileSystem>) -> Self {
        Self { fs }
    }
}

impl std::fmt::Debug for ToolchainPinDrift {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolchainPinDrift").finish_non_exhaustive()
    }
}

/// The root's version of a tool, which every other pin must agree with.
struct Source {
    tool: Tool,
    version: String,
    /// Where it is written, for messages: `` `.nvmrc` ``, `` `packageManager` ``.
    origin: String,
    /// The toolchain file and span that hold it, so the pin is not checked against itself.
    pin: Option<(String, Range<usize>)>,
    location: Location,
    from_version_file: bool,
}

impl Source {
    fn is(&self, file: &ToolchainFile, pin: &Pin) -> bool {
        self.pin
            .as_ref()
            .is_some_and(|(path, span)| *path == file.path && *span == pin.span)
    }
}

/// Whether `version` is a version or range [`compatible`] can compare.
fn is_version(version: &str) -> bool {
    compatible(version, version).is_some()
}

/// Whether two versions or ranges overlap; `None` when either is not a version or range (`lts/*`, `latest`).
fn compatible(left: &str, right: &str) -> Option<bool> {
    let range = |version: &str| {
        let trimmed = version.trim();
        Specifier::new_node_range(trimmed.strip_prefix(['v', 'V']).unwrap_or(trimmed))
    };
    Some(range(left)?.allows_any(&range(right)?))
}

fn node_source(scan: &Scan, root: &Package) -> Option<Source> {
    let from_file = NODE_SOURCES.iter().find_map(|name| {
        let file = scan
            .files
            .iter()
            .find(|file| file.path == *name && file.kind == ToolchainKind::VersionFiles)?;
        let pin = file
            .parsed
            .pins
            .iter()
            .find(|pin| pin.tool == Tool::Node && is_version(&pin.version))?;
        Some(Source {
            tool: Tool::Node,
            version: pin.version.clone(),
            origin: format!("`{name}`"),
            pin: Some((file.path.clone(), pin.span.clone())),
            location: file.location(&pin.span),
            from_version_file: true,
        })
    });
    from_file.or_else(|| {
        let engines = root
            .manifest
            .value()
            .pointer("/engines/node")?
            .as_str()
            .filter(|engines| is_version(engines))?;
        Some(Source {
            tool: Tool::Node,
            version: engines.to_owned(),
            origin: "`engines.node` of the root package.json".to_owned(),
            pin: None,
            location: field_location(root, &[key("engines"), key("node")]),
            from_version_file: false,
        })
    })
}

fn manager_source(root: &Package) -> Option<Source> {
    let manifest = root.manifest.value();
    if let Some(field) = manifest.get("packageManager").and_then(Value::as_str) {
        let (name, version) = split_spec(field);
        let version = version?.split('+').next()?;
        return Some(Source {
            tool: Tool::Manager(manager_named(name)?),
            version: version.to_owned(),
            origin: "`packageManager`".to_owned(),
            pin: None,
            location: field_location(root, &[key("packageManager")]),
            from_version_file: false,
        });
    }
    let declared = manifest.pointer("/devEngines/packageManager")?;
    let entries: Vec<(Option<usize>, &Value)> = match declared {
        Value::Array(entries) => entries
            .iter()
            .enumerate()
            .map(|(index, entry)| (Some(index), entry))
            .collect(),
        entry => vec![(None, entry)],
    };
    entries.into_iter().find_map(|(index, entry)| {
        let manager = manager_named(entry.get("name")?.as_str()?)?;
        let version = entry.get("version")?.as_str()?;
        let mut pointer = vec![key("devEngines"), key("packageManager")];
        pointer.extend(index.map(PathSeg::Index));
        pointer.push(key("version"));
        Some(Source {
            tool: Tool::Manager(manager),
            version: version.to_owned(),
            origin: "`devEngines.packageManager`".to_owned(),
            pin: None,
            location: field_location(root, &pointer),
            from_version_file: false,
        })
    })
}

fn tool_name(tool: Tool) -> &'static str {
    match tool {
        Tool::Node => "Node",
        Tool::Manager(manager) => manager_name(manager),
    }
}

fn pin_help(pin: &Pin, source: &Source) -> String {
    let base = format!("change it to {} to match {}", source.version, source.origin);
    match (pin.label.as_str(), &source.pin) {
        ("actions/setup-node node-version", Some((path, _))) => {
            format!("{base}, or replace `node-version` with `node-version-file: {path}`")
        }
        ("pnpm/action-setup version", _) if source.origin == "`packageManager`" => {
            format!("{base}, or remove `version` so the action reads `packageManager`")
        }
        _ => base,
    }
}

fn pin_finding(pin: &Pin, source: &Source) -> Finding {
    let tool = tool_name(pin.tool);
    Finding {
        span: pin.span.clone(),
        message: format!(
            "`{}` pins {tool} {}, which does not match {} from {}",
            pin.label, pin.version, source.version, source.origin
        ),
        help: pin_help(pin, source),
        detail: if pin.tool == Tool::Node {
            NODE_DETAIL
        } else {
            MANAGER_DETAIL
        },
        severity: None,
        related: Some(Related::new(
            format!("the root pins {tool} {} here", source.version),
            source.location.clone(),
        )),
    }
}

fn corepack_finding(span: Range<usize>, manager: Option<&Source>) -> Finding {
    let (name, prepare) = match manager {
        Some(source) => {
            let name = tool_name(source.tool);
            (name, format!("`corepack prepare {name}@{} --activate`", source.version))
        }
        None => (
            "package manager",
            "`corepack prepare <manager>@<version> --activate`".to_owned(),
        ),
    };
    Finding {
        span,
        message: format!("`corepack enable` without `corepack prepare` leaves the {name} version to corepack"),
        help: format!(
            "add {prepare} after it, or copy package.json in before the {name} runs so corepack reads `packageManager`"
        ),
        detail: COREPACK_DETAIL,
        severity: Some(Severity::Info),
        related: None,
    }
}

impl Rule for ToolchainPinDrift {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn options_schema(&self) -> Option<serde_json::Value> {
        Some(options::schema(&DEFAULT_KINDS))
    }

    fn validate_options(&self, options: &toml::Table) -> Result<(), OptionsError> {
        ToolchainOptions::parse(options).map(|_| ())
    }

    fn check(&self, cx: &LintContext<'_>, out: &mut Emitter<'_>) {
        let Ok(options) = ToolchainOptions::of(cx, META.id).resolve(&DEFAULT_KINDS) else {
            return;
        };
        let scan = Scan::of(cx, &self.fs, &rule_ids());
        let root = cx.workspace.root_package();
        let node = node_source(&scan, root);
        let manager = manager_source(root);
        for file in scan.files_for(&options) {
            for pin in &file.parsed.pins {
                let source = match pin.tool {
                    Tool::Node => node.as_ref(),
                    Tool::Manager(_) => manager.as_ref().filter(|source| source.tool == pin.tool),
                };
                let Some(source) = source else { continue };
                if !source.is(file, pin) && compatible(&source.version, &pin.version) == Some(false) {
                    emit(out, cx, file, &META, pin_finding(pin, source));
                }
            }
            if file.kind == ToolchainKind::Dockerfiles
                && let Some(span) = &file.parsed.unpinned_corepack
            {
                emit(out, cx, file, &META, corepack_finding(span.clone(), manager.as_ref()));
            }
        }
        check_engines(cx, out, root, node.as_ref(), &options);
    }
}

/// Reports a root `engines.node` that a root version file, the source of truth, does not satisfy.
fn check_engines(
    cx: &LintContext<'_>,
    out: &mut Emitter<'_>,
    root: &Package,
    node: Option<&Source>,
    options: &options::Resolved,
) {
    let Some(node) = node.filter(|node| node.from_version_file) else {
        return;
    };
    let manifest = root.manifest.path.to_string_lossy().replace('\\', "/");
    if cx.workspace.is_ignored(root.id) || options.ignore_files.matches(&manifest, false) {
        return;
    }
    let Some(engines) = root.manifest.value().pointer("/engines/node").and_then(Value::as_str) else {
        return;
    };
    if compatible(&node.version, engines) != Some(false) {
        return;
    }
    out.emit(
        diagnostic(
            &META,
            format!(
                "`engines.node` allows {engines}, which does not match {} from {}",
                node.version, node.origin
            ),
            field_location(root, &[key("engines"), key("node")]),
        )
        .package(root.display_name())
        .package_id(root.id)
        .help(format!(
            "widen `engines.node` to admit {}, or change {} to a version it allows",
            node.version, node.origin
        ))
        .detail(NODE_DETAIL)
        .related(Related::new(
            format!("the root pins Node {} here", node.version),
            node.location.clone(),
        ))
        .build(),
    );
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::testing::{config, lint, located};

    /// The path of a finding with `/` separators, as on every platform.
    fn path_of(diagnostic: &skif_core::Diagnostic) -> String {
        diagnostic.location.path.to_string_lossy().replace('\\', "/")
    }

    const ROOT: &str = r#"{"name":"root","private":true,"packageManager":"pnpm@9.1.0+sha512.abc","engines":{"node":">=18"},"workspaces":["packages/*"]}"#;
    const MEMBER: &str = r#"{"name":"a","private":true}"#;

    /// `(path, located text, detail)` of each finding, sorted.
    fn findings(files: &[(&str, &str)], toml: &str) -> Vec<(String, String, String)> {
        let mut found: Vec<(String, String, String)> = lint(META.id, files, &config(toml))
            .iter()
            .map(|diagnostic| {
                (
                    path_of(diagnostic),
                    located(files, diagnostic).to_owned(),
                    diagnostic.detail.clone().unwrap_or_default(),
                )
            })
            .collect();
        found.sort();
        found
    }

    fn row(path: &str, text: &str, detail: &str) -> (String, String, String) {
        (path.to_owned(), text.to_owned(), detail.to_owned())
    }

    #[test]
    fn exact_and_range_node_pins_are_checked_against_the_root_version_file() {
        let dockerfile = "\
ARG NODE_VERSION=20.11.1
FROM node:18.19.0-alpine AS old
FROM node:20-slim
FROM node:lts
";
        let workflow = "\
jobs:
  test:
    steps:
      - uses: actions/setup-node@v4
        with:
          node-version: 20.x
      - uses: actions/setup-node@v4
        with:
          node-version: '22'
";
        let files = [
            ("package.json", ROOT),
            (".nvmrc", "v20.11.1\n"),
            (".tool-versions", "nodejs 20.11.1\n"),
            ("Dockerfile", dockerfile),
            (".github/workflows/ci.yml", workflow),
            ("packages/a/package.json", MEMBER),
            ("packages/a/.node-version", "20.11.0\n"),
            ("packages/a/docker/Dockerfile.prod", "FROM node:20.11.1\n"),
        ];
        assert_eq!(
            findings(&files, ""),
            [
                row(".github/workflows/ci.yml", "22", NODE_DETAIL),
                row("Dockerfile", "18.19.0", NODE_DETAIL),
                row("packages/a/.node-version", "20.11.0", NODE_DETAIL),
            ]
        );
        let diagnostics = lint(META.id, &files, &config(""));
        let docker = diagnostics
            .iter()
            .find(|diagnostic| path_of(diagnostic) == "Dockerfile")
            .unwrap();
        assert_eq!(
            docker.message,
            "`FROM node` pins Node 18.19.0, which does not match 20.11.1 from `.nvmrc`"
        );
        assert_eq!(docker.location.span.unwrap().start_line, 2);
        assert_eq!(docker.related[0].location.path, PathBuf::from(".nvmrc"));
        assert_eq!(docker.package.as_deref(), Some("root"));
        let workflow_finding = diagnostics
            .iter()
            .find(|diagnostic| path_of(diagnostic).ends_with("ci.yml"))
            .unwrap();
        assert!(
            workflow_finding
                .help
                .as_deref()
                .unwrap()
                .contains("node-version-file: .nvmrc"),
            "{:?}",
            workflow_finding.help
        );
        let member = diagnostics
            .iter()
            .find(|diagnostic| path_of(diagnostic).starts_with("packages/a"))
            .unwrap();
        assert_eq!(member.package.as_deref(), Some("a"));
    }

    #[test]
    fn engines_node_is_the_source_without_a_version_file() {
        let root = r#"{"name":"root","private":true,"engines":{"node":">=20.10 <21"},"workspaces":["packages/*"]}"#;
        let files = [
            ("package.json", root),
            ("Dockerfile", "FROM node:20\nFROM node:20.9.0\nFROM node:22-alpine\n"),
        ];
        assert_eq!(
            findings(&files, ""),
            [
                row("Dockerfile", "20.9.0", NODE_DETAIL),
                row("Dockerfile", "22", NODE_DETAIL)
            ]
        );
        let diagnostics = lint(META.id, &files, &config(""));
        assert!(
            diagnostics[0]
                .message
                .contains(">=20.10 <21 from `engines.node` of the root package.json")
        );
    }

    #[test]
    fn engines_node_must_admit_the_root_version_file() {
        let root = r#"{"name":"root","private":true,"engines":{"node":">=20"},"workspaces":["packages/*"]}"#;
        let files = [("package.json", root), (".node-version", "18.19.0\n")];
        assert_eq!(
            findings(&files, ""),
            [row("package.json", r#""node":">=20""#, NODE_DETAIL)]
        );
        let agreeing = [("package.json", root), (".node-version", "20\n")];
        assert!(findings(&agreeing, "").is_empty());
    }

    #[test]
    fn without_a_root_pin_nothing_is_compared() {
        let root = r#"{"name":"root","private":true,"workspaces":["packages/*"]}"#;
        let files = [
            ("package.json", root),
            (
                "Dockerfile",
                "FROM node:18\nRUN corepack prepare pnpm@8.15.0 --activate\n",
            ),
            (".nvmrc", "lts/*\n"),
        ];
        assert!(findings(&files, "").is_empty());
    }

    #[test]
    fn package_manager_pins_are_checked_against_package_manager() {
        let dockerfile = "\
FROM node:20.11.1
RUN corepack enable && corepack prepare pnpm@8.15.0 --activate
RUN npm install -g pnpm@9
RUN corepack prepare yarn@4.1.0
";
        let workflow = "\
jobs:
  build:
    steps:
      - uses: pnpm/action-setup@v4
        with:
          version: \"9.0.6\"
";
        let files = [
            ("package.json", ROOT),
            ("Dockerfile", dockerfile),
            (".github/workflows/release.yaml", workflow),
            (".tool-versions", "pnpm 9.1\n"),
            ("mise.toml", "[tools]\npnpm = \"8\"\n"),
        ];
        assert_eq!(
            findings(&files, ""),
            [
                row(".github/workflows/release.yaml", "9.0.6", MANAGER_DETAIL),
                row("Dockerfile", "pnpm@8.15.0", MANAGER_DETAIL),
                row("mise.toml", "8", MANAGER_DETAIL),
            ]
        );
        let diagnostics = lint(META.id, &files, &config(""));
        let docker = diagnostics
            .iter()
            .find(|diagnostic| path_of(diagnostic) == "Dockerfile")
            .unwrap();
        assert_eq!(
            docker.message,
            "`corepack prepare` pins pnpm 8.15.0, which does not match 9.1.0 from `packageManager`"
        );
        let action = diagnostics
            .iter()
            .find(|diagnostic| path_of(diagnostic).ends_with(".yaml"))
            .unwrap();
        assert!(action.help.as_deref().unwrap().contains("remove `version`"));
    }

    #[test]
    fn dev_engines_versions_are_a_manager_source() {
        let root = r#"{"name":"root","private":true,"devEngines":{"packageManager":[{"name":"deno"},{"name":"pnpm","version":"^9.1.0"}]},"workspaces":["packages/*"]}"#;
        let files = [
            ("package.json", root),
            (
                "Dockerfile",
                "RUN corepack prepare pnpm@9.4.0 && corepack prepare pnpm@8.1.0\n",
            ),
        ];
        assert_eq!(findings(&files, ""), [row("Dockerfile", "pnpm@8.1.0", MANAGER_DETAIL)]);
    }

    #[test]
    fn corepack_enable_without_prepare_is_an_info_finding_in_dockerfiles() {
        let files = [
            ("package.json", ROOT),
            (
                "Dockerfile",
                "FROM node:20\nRUN corepack enable\nRUN pnpm install --frozen-lockfile\n",
            ),
            ("Dockerfile.ci", "RUN corepack enable && corepack prepare --activate\n"),
            (
                ".github/workflows/ci.yml",
                "jobs:\n  a:\n    steps:\n      - run: corepack enable\n",
            ),
        ];
        let diagnostics = lint(META.id, &files, &config(""));
        assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
        assert_eq!(located(&files, &diagnostics[0]), "corepack enable");
        assert_eq!(diagnostics[0].detail.as_deref(), Some(COREPACK_DETAIL));
        assert_eq!(diagnostics[0].severity, Severity::Info);
        assert!(
            diagnostics[0]
                .help
                .as_deref()
                .unwrap()
                .contains("corepack prepare pnpm@9.1.0 --activate")
        );
        let configured = lint(META.id, &files, &config("[rules]\ntoolchain-pin-drift = \"error\"\n"));
        assert_eq!(configured[0].severity, Severity::Error);
    }

    #[test]
    fn comments_with_reasons_ignore_files_and_kinds_suppress_findings() {
        let dockerfile = "\
# skif-ignore-next-line toolchain-pin-drift -- the base image is pinned by the platform team
FROM node:18
# skif-ignore-next-line toolchain-pin-drift
FROM node:16
";
        let files = [
            ("package.json", ROOT),
            (".nvmrc", "20.11.1\n"),
            ("Dockerfile", dockerfile),
            ("docker/Dockerfile", "FROM node:16\n"),
            (
                "scripts/old.Dockerfile",
                "# skif-ignore-file -- kept for the 1.x images\nFROM node:14\n",
            ),
        ];
        assert_eq!(
            findings(&files, ""),
            [
                row("Dockerfile", "16", NODE_DETAIL),
                row("docker/Dockerfile", "16", NODE_DETAIL)
            ]
        );
        let diagnostics = lint(META.id, &files, &config(""));
        let unreasoned = diagnostics
            .iter()
            .find(|diagnostic| path_of(diagnostic) == "Dockerfile")
            .unwrap();
        assert!(
            unreasoned
                .help
                .as_deref()
                .unwrap()
                .contains("comment on line 3 does not apply"),
            "{:?}",
            unreasoned.help
        );
        let ignored =
            "[rules.toolchain-pin-drift]\nignore-files = [{ name = \"docker/**\", reason = \"built elsewhere\" }]\n";
        assert_eq!(findings(&files, ignored), [row("Dockerfile", "16", NODE_DETAIL)]);
        let workflows_only = "[rules.toolchain-pin-drift]\nkinds = [\"workflows\"]\n";
        assert!(findings(&files, workflows_only).is_empty());
    }

    #[test]
    fn globs_add_files_but_not_those_of_other_projects() {
        let files = [
            ("package.json", ROOT),
            (".nvmrc", "20.11.1\n"),
            ("deploy/base.Dockerfile", "FROM node:18\n"),
            ("examples/legacy/package.json", r#"{"name":"legacy"}"#),
            ("examples/legacy/Dockerfile", "FROM node:16\n"),
            ("packages/a/package.json", MEMBER),
        ];
        assert!(findings(&files, "").is_empty(), "deploy/ is not a usual place");
        let globbed = "[rules.toolchain-pin-drift]\nfiles = [\"deploy/**\", \"examples/**\"]\n";
        assert_eq!(
            findings(&files, globbed),
            [row("deploy/base.Dockerfile", "18", NODE_DETAIL)]
        );
        let ignored = "[workspace]\nignore-packages = [\"a\"]\n";
        let member = [
            ("package.json", ROOT),
            (".nvmrc", "20.11.1\n"),
            ("packages/a/package.json", MEMBER),
            ("packages/a/.nvmrc", "18\n"),
        ];
        assert_eq!(findings(&member, ""), [row("packages/a/.nvmrc", "18", NODE_DETAIL)]);
        assert!(findings(&member, ignored).is_empty());
    }

    #[test]
    fn versions_compare_by_overlap() {
        let cases = [
            ("20.11.1", "20.11.1", Some(true)),
            ("20.11.1", "20.11.0", Some(false)),
            ("20.11.1", "20", Some(true)),
            ("20.11.1", "^20.12.0", Some(false)),
            (">=20", "18", Some(false)),
            (">=20", "22.1.0", Some(true)),
            ("v20.11.1", "20.x", Some(true)),
            ("lts/*", "20", None),
            ("20", "latest", None),
        ];
        for (source, pin, expected) in cases {
            assert_eq!(compatible(source, pin), expected, "{source} vs {pin}");
        }
    }
}
