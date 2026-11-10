//! Railway: the build, start and pre-deploy commands of every `service(…)` from `railway/iac` in
//! `.railway/railway.ts` are scripts, run in the service's root directory. Services built from an image or a template,
//! or from another GitHub repository than the one `package.json#repository` names, are left out.

use skif_js_syntax::{ConfigValue, extract_imported_calls};

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{get_in, manifest, script_in, strings};

/// The `railway` plugin.
pub struct Railway;

static SPEC: PluginSpec = PluginSpec {
    id: "railway",
    enablers: &[Enabler::Dependency("railway")],
    config: &[".railway/railway.ts"],
    binaries: &["railway"],
    optional_binaries: true,
    ..PluginSpec::DEFAULT
};

/// The string `key` holds in `value`.
fn text<'v>(value: Option<&'v ConfigValue>, key: &str) -> Option<&'v str> {
    value?.get(key).and_then(ConfigValue::as_str)
}

/// `owner/name` of a GitHub repository URL or shorthand, lowercased. Only a `github.com` host or a `github:` prefix is
/// cut off: a bare `owner/name` is the shorthand, even when it contains "github" (`acme/github-tools`).
fn github_repository(value: &str) -> Option<String> {
    let value = value.trim();
    let value = value.strip_prefix("git+").unwrap_or(value);
    let value = value.trim_end_matches('/');
    let value = value.strip_suffix(".git").unwrap_or(value);
    let lower = value.to_ascii_lowercase();
    let rest = if let Some(at) = lower.rfind("github.com") {
        let host_start = at == 0 || lower[..at].ends_with(['/', '@', '.']);
        let after = lower[at + "github.com".len()..].strip_prefix(['/', ':']);
        after.filter(|_| host_start)?.to_owned()
    } else if let Some(shorthand) = lower.strip_prefix("github:") {
        shorthand.to_owned()
    } else {
        lower
    };
    let mut parts = rest.split('/');
    let (owner, name) = (parts.next()?, parts.next()?);
    (parts.next().is_none() && !owner.is_empty() && !name.is_empty() && !owner.contains(':'))
        .then(|| format!("{owner}/{name}"))
}

/// The directory a service's commands run in, relative to the workspace. Railway writes it from the repository root,
/// often with a leading `/` (`/apps/api`), which is cut off; a directory with `..` is not followed and drops the
/// service's commands.
fn root_directory(dir: &str) -> Option<&str> {
    let dir = dir.trim().trim_start_matches('/');
    (!dir.split(['/', '\\']).any(|part| part == "..")).then_some(dir)
}

impl Plugin for Railway {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    /// The services are read from the `service(…)` calls in the source.
    fn reads_config(&self) -> bool {
        false
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let local: Vec<String> = manifest(cx)
            .and_then(|manifest| {
                let repository = manifest.get("repository")?;
                let url = repository
                    .as_str()
                    .or_else(|| repository.get("url").and_then(ConfigValue::as_str))?;
                github_repository(url)
            })
            .into_iter()
            .collect();
        let mut out = Vec::new();
        for call in extract_imported_calls(config.source, config.path, &["railway/iac"], &[]) {
            let Some(service @ ConfigValue::Object(_)) = call.arguments.get(1).filter(|_| call.export == "service")
            else {
                continue;
            };
            let source = service.get("source");
            let repository = text(source, "repo");
            let kind = text(source, "type").or_else(|| {
                if repository.is_some() {
                    Some("github")
                } else if text(source, "image").is_some() {
                    Some("image")
                } else if text(source, "template").is_some() {
                    Some("template")
                } else {
                    None
                }
            });
            let foreign = kind == Some("github")
                && !local.is_empty()
                && repository
                    .and_then(github_repository)
                    .is_some_and(|repository| !local.contains(&repository));
            if matches!(kind, Some("image" | "template")) || foreign {
                continue;
            }
            let dir = text(source, "rootDirectory")
                .or_else(|| text(Some(service), "root"))
                .or_else(|| text(Some(service), "rootDirectory"))
                .unwrap_or_default();
            let Some(dir) = root_directory(dir) else {
                continue;
            };
            let build = service.get("build");
            let commands = [
                build,
                build.and_then(|build| build.get("buildCommand")),
                service.get("start"),
                service.get("preDeploy"),
                get_in(service, &["deploy", "preDeployCommand"]),
            ];
            for command in commands.into_iter().flatten().flat_map(strings) {
                out.extend(script_in(dir, command.to_owned()));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::{github_repository, root_directory};

    #[test]
    fn github_repositories_are_normalized() {
        for value in [
            "https://github.com/Acme/App.git",
            "git+https://github.com/acme/app",
            "git@github.com:acme/app.git",
            "github:acme/app",
            "acme/app",
        ] {
            assert_eq!(github_repository(value).as_deref(), Some("acme/app"), "{value}");
        }
        assert_eq!(github_repository("https://gitlab.com/a/b/c"), None);
        assert_eq!(
            github_repository("acme/github-tools").as_deref(),
            Some("acme/github-tools")
        );
        assert_eq!(github_repository("https://notgithub.com/a/b"), None);
    }

    #[test]
    fn root_directories_stay_inside_the_repository() {
        assert_eq!(root_directory("/apps/api"), Some("apps/api"));
        assert_eq!(root_directory("apps/web"), Some("apps/web"));
        assert_eq!(root_directory("/"), Some(""));
        assert_eq!(root_directory("../other"), None);
        assert_eq!(root_directory("apps/../../x"), None);
    }
}
