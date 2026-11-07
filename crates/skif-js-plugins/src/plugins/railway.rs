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
    ..PluginSpec::DEFAULT
};

/// The string `key` holds in `value`.
fn text<'v>(value: Option<&'v ConfigValue>, key: &str) -> Option<&'v str> {
    value?.get(key).and_then(ConfigValue::as_str)
}

/// `owner/name` of a GitHub repository URL or shorthand, lowercased.
fn github_repository(value: &str) -> Option<String> {
    let value = value.trim();
    let value = value.strip_prefix("git+").unwrap_or(value);
    let value = value.trim_end_matches('/');
    let value = value.strip_suffix(".git").unwrap_or(value);
    let lower = value.to_ascii_lowercase();
    let rest = match lower.rfind("github.com").or_else(|| lower.rfind("github")) {
        Some(at) => {
            let after = &lower[at..];
            let after = after
                .strip_prefix("github.com")
                .or_else(|| after.strip_prefix("github"))?;
            after.strip_prefix(['/', ':'])?.to_owned()
        }
        None => lower,
    };
    let mut parts = rest.split('/');
    let (owner, name) = (parts.next()?, parts.next()?);
    (parts.next().is_none() && !owner.is_empty() && !name.is_empty() && !owner.contains(':'))
        .then(|| format!("{owner}/{name}"))
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
    use super::github_repository;

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
    }
}
