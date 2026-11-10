use std::sync::Arc;

use skif_config::PackageManager;
use skif_core::{Category, FileSystem, Fixability, RuleMeta, Severity};
use skif_lint::{Emitter, LintContext, OptionsError, Rule};

use super::commands::{Invocation, basename, invocation, manager_name};
use super::options::{self, ToolchainKind, ToolchainOptions};
use super::parse::Command;
use super::scan::Scan;
use super::{Finding, emit, rule_ids};

/// Metadata of `foreign-package-manager-command`.
pub static META: RuleMeta = RuleMeta::new(
    "foreign-package-manager-command",
    Category::Workspace,
    Some(Severity::Warning),
    Fixability::None,
    "Dockerfiles, CI workflows and shell scripts must not run a package manager other than the workspace's.",
);

/// The kinds of file read when the `kinds` option is not set.
const DEFAULT_KINDS: [ToolchainKind; 3] = [
    ToolchainKind::Dockerfiles,
    ToolchainKind::Workflows,
    ToolchainKind::ShellScripts,
];

/// Reports commands in Dockerfile `RUN` instructions, workflow `run:` steps and shell scripts that run a package
/// manager other than the workspace's (`packageManager`, or what detection finds): `npm ci` or `npx` in a pnpm
/// workspace, a bare `yarn` in an npm one.
///
/// Read-only commands (`npm view`, `npm whoami`, `yarn info`), a global install of the workspace's own manager
/// (`npm install -g pnpm@9`) and the files of projects outside the workspace are not reported. The options are those of
/// every toolchain rule: `kinds` (default Dockerfiles, workflows and shell scripts), `files` and `ignore-files`.
pub struct ForeignPackageManagerCommand {
    fs: Arc<dyn FileSystem>,
}

impl ForeignPackageManagerCommand {
    /// The rule, reading toolchain files through `fs`.
    #[must_use]
    pub fn new(fs: Arc<dyn FileSystem>) -> Self {
        Self { fs }
    }
}

impl std::fmt::Debug for ForeignPackageManagerCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ForeignPackageManagerCommand").finish_non_exhaustive()
    }
}

/// The command of `chosen` that does what `quoted` (the foreign program through its subcommand) does.
fn equivalent(chosen: PackageManager, quoted: &[&str]) -> String {
    let program = basename(quoted.first().copied().unwrap_or_default());
    let name = manager_name(chosen);
    let runner = match chosen {
        PackageManager::Npm => "npx",
        PackageManager::Yarn => "yarn dlx",
        PackageManager::Bun => "bunx",
        _ => "pnpm dlx",
    };
    let frozen = match chosen {
        PackageManager::Npm => "npm ci",
        PackageManager::Yarn => "yarn install --immutable",
        PackageManager::Bun => "bun install --frozen-lockfile",
        _ => "pnpm install --frozen-lockfile",
    };
    if matches!(program, "npx" | "pnpx" | "bunx") {
        return runner.to_owned();
    }
    match quoted.last().copied().unwrap_or_default() {
        "dlx" | "x" | "exec" => runner.to_owned(),
        "ci" | "clean-install" | "ic" | "install-clean" | "cit" | "install-ci-test" => frozen.to_owned(),
        "install" | "i" | "in" | "yarn" | "yarnpkg" => format!("{name} install"),
        "add" | "a" => format!("{name} add"),
        _ => format!("{name} run <script>"),
    }
}

fn finding(command: &Command, found: &Invocation, chosen: PackageManager) -> Finding {
    let (first, last) = found.words;
    let words = &command.words[first..=last];
    let quoted: Vec<&str> = words.iter().map(|word| word.text.as_str()).collect();
    let foreign = manager_name(found.manager);
    let chosen_name = manager_name(chosen);
    Finding {
        span: words[0].span.start..words[words.len() - 1].span.end,
        message: format!(
            "`{}` runs {foreign}, but the workspace's package manager is {chosen_name}",
            quoted.join(" ")
        ),
        help: format!(
            "use {chosen_name} (`{}`), or put `# skif-ignore-next-line {} -- <reason>` above the line when {foreign} \
             is deliberate here",
            equivalent(chosen, &quoted),
            META.id
        ),
        detail: foreign,
        severity: None,
        related: None,
    }
}

impl Rule for ForeignPackageManagerCommand {
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
        let Some(chosen) = cx.workspace.package_manager else {
            return;
        };
        let Ok(options) = ToolchainOptions::of(cx, META.id).resolve(&DEFAULT_KINDS) else {
            return;
        };
        let scan = Scan::of(cx, &self.fs, &rule_ids());
        for file in scan.files_for(&options) {
            for command in &file.parsed.commands {
                if let Some(found) = invocation(&command.words, chosen) {
                    emit(out, cx, file, &META, finding(command, &found, chosen));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{config, lint, located};

    const ROOT: &str = r#"{"name":"root","private":true,"packageManager":"pnpm@9.1.0","workspaces":["packages/*"]}"#;
    const MEMBER: &str = r#"{"name":"a","private":true}"#;

    fn path_of(diagnostic: &skif_core::Diagnostic) -> String {
        diagnostic.location.path.to_string_lossy().replace('\\', "/")
    }

    /// `(path, line, located text)` of each finding, sorted.
    fn findings(files: &[(&str, &str)], toml: &str) -> Vec<(String, u32, String)> {
        let mut found: Vec<(String, u32, String)> = lint(META.id, files, &config(toml))
            .iter()
            .map(|diagnostic| {
                (
                    path_of(diagnostic),
                    diagnostic.location.span.unwrap().start_line,
                    located(files, diagnostic).to_owned(),
                )
            })
            .collect();
        found.sort();
        found
    }

    fn row(path: &str, line: u32, text: &str) -> (String, u32, String) {
        (path.to_owned(), line, text.to_owned())
    }

    #[test]
    fn commands_of_another_manager_are_reported_in_every_kind_of_file() {
        let dockerfile = "\
FROM node:20
RUN npm ci
RUN apt-get update && \\
    npx tsc --noEmit
RUN [\"yarn\", \"install\"]
RUN corepack enable && pnpm install --frozen-lockfile
CMD npm start
";
        let workflow = "\
jobs:
  build:
    steps:
      - run: npm run build
      - run: |
          pnpm install
          yarn
";
        let files = [
            ("package.json", ROOT),
            ("Dockerfile", dockerfile),
            (".github/workflows/ci.yml", workflow),
            ("packages/a/package.json", MEMBER),
            ("packages/a/scripts/release.sh", "#!/bin/sh\nset -e\nnpm publish\nnpm test\n"),
        ];
        assert_eq!(
            findings(&files, ""),
            [
                row(".github/workflows/ci.yml", 4, "npm run"),
                row(".github/workflows/ci.yml", 7, "yarn"),
                row("Dockerfile", 2, "npm ci"),
                row("Dockerfile", 4, "npx tsc"),
                row("Dockerfile", 5, "[\"yarn\", \"install\"]"),
                row("packages/a/scripts/release.sh", 4, "npm test"),
            ]
        );
        let diagnostics = lint(META.id, &files, &config(""));
        let ci = diagnostics
            .iter()
            .find(|diagnostic| located(&files, diagnostic) == "npm ci")
            .unwrap();
        assert_eq!(ci.message, "`npm ci` runs npm, but the workspace's package manager is pnpm");
        assert!(ci.help.as_deref().unwrap().contains("`pnpm install --frozen-lockfile`"));
        assert_eq!(ci.detail.as_deref(), Some("npm"));
        assert_eq!(ci.package.as_deref(), Some("root"));
        let script = diagnostics
            .iter()
            .find(|diagnostic| path_of(diagnostic).ends_with("release.sh"))
            .unwrap();
        assert_eq!(script.package.as_deref(), Some("a"));
        let npx = diagnostics
            .iter()
            .find(|diagnostic| located(&files, diagnostic) == "npx tsc")
            .unwrap();
        assert!(npx.help.as_deref().unwrap().contains("`pnpm dlx`"), "{:?}", npx.help);
    }

    #[test]
    fn read_only_commands_global_installs_of_the_manager_and_other_projects_are_not_reported() {
        let dockerfile = "\
RUN npm install -g pnpm@9.1.0 corepack
RUN VERSION=$(npm view pnpm version) && npm whoami
RUN npm --version && yarn --version
RUN npx pnpm@9.1.0 install
";
        let files = [
            ("package.json", ROOT),
            ("Dockerfile", dockerfile),
            ("examples/legacy/package.json", r#"{"name":"legacy"}"#),
            ("examples/legacy/Dockerfile", "RUN npm ci\n"),
            ("examples/legacy/build.sh", "npm run build\n"),
        ];
        let globbed = "[rules.foreign-package-manager-command]\nfiles = [\"examples/**\"]\n";
        assert!(findings(&files, globbed).is_empty());
    }

    #[test]
    fn comments_ignore_files_and_kinds_suppress_findings() {
        let script = "\
#!/usr/bin/env bash
# skif-ignore-next-line foreign-package-manager-command -- fallback for the 1.x release branch
npm ci
# skif-ignore-next-line foreign-package-manager-command
yarn install
";
        let files = [
            ("package.json", ROOT),
            ("deploy.sh", script),
            ("scripts/legacy.sh", "# skif-ignore-file -- kept on npm until the app moves\nnpm ci\n"),
            ("docker/Dockerfile", "RUN npm ci\n"),
        ];
        assert_eq!(
            findings(&files, ""),
            [row("deploy.sh", 5, "yarn install"), row("docker/Dockerfile", 1, "npm ci")]
        );
        let diagnostics = lint(META.id, &files, &config(""));
        let unreasoned = diagnostics
            .iter()
            .find(|diagnostic| path_of(diagnostic) == "deploy.sh")
            .unwrap();
        assert!(unreasoned.help.as_deref().unwrap().contains("comment on line 4 does not apply"));
        let ignored = "[rules.foreign-package-manager-command]\nignore-files = [{ name = \"docker/*\", reason = \"the image is built from the npm branch\" }]\n";
        assert_eq!(findings(&files, ignored), [row("deploy.sh", 5, "yarn install")]);
        let dockerfiles_only = "[rules.foreign-package-manager-command]\nkinds = [\"dockerfiles\"]\n";
        assert_eq!(findings(&files, dockerfiles_only), [row("docker/Dockerfile", 1, "npm ci")]);
        let off = "[[ignore]]\nrules = [\"foreign-package-manager-command\"]\npackages = [\"root\"]\n";
        assert!(findings(&files, off).is_empty());
    }

    #[test]
    fn the_detected_manager_decides_and_without_one_nothing_is_reported() {
        let npm_root = r#"{"name":"root","private":true,"packageManager":"npm@10.8.0","workspaces":["packages/*"]}"#;
        let files = [("package.json", npm_root), ("Dockerfile", "RUN npm ci\nRUN yarn\nRUN pnpm install\n")];
        assert_eq!(
            findings(&files, ""),
            [row("Dockerfile", 2, "yarn"), row("Dockerfile", 3, "pnpm install")]
        );
        let unknown = r#"{"name":"root","private":true,"workspaces":["packages/*"]}"#;
        let files = [("package.json", unknown), ("Dockerfile", "RUN npm ci\nRUN yarn\n")];
        assert!(findings(&files, "").is_empty());
        let lockfile = [
            ("package.json", unknown),
            ("yarn.lock", ""),
            ("Dockerfile", "RUN npm ci\nRUN yarn\n"),
        ];
        assert_eq!(findings(&lockfile, ""), [row("Dockerfile", 1, "npm ci")]);
    }
}
