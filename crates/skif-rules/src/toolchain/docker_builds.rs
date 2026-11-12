//! The image builds a workspace's scripts, CI workflows and Compose files run: which Dockerfile each builds and
//! from which build context, so that a Dockerfile outside a directory is known to copy from it.
//!
//! A command is `docker build`, `docker buildx build`, `docker image build` or `podman build` with a static context
//! argument and an optional `-f`/`--file`; both are taken relative to the workspace root, where scripts and workflows
//! are assumed to run. A Compose file at the root (`compose.yaml`, `compose.yml`, `docker-compose.yaml`,
//! `docker-compose.yml`) gives a context per service under `build` (a string, or `context` with an optional
//! `dockerfile` relative to it).

use serde_json::Value;
use skif_core::shell::Word;

use super::commands::{basename, program};

/// The Compose files read at the workspace root.
pub(crate) const COMPOSE_FILES: [&str; 4] = [
    "compose.yaml",
    "compose.yml",
    "docker-compose.yaml",
    "docker-compose.yml",
];

/// The options of `docker build` that take a value as the next word.
const VALUE_OPTIONS: [&str; 24] = [
    "-t",
    "--tag",
    "--build-arg",
    "--build-context",
    "--target",
    "--platform",
    "--label",
    "--secret",
    "--ssh",
    "--cache-from",
    "--cache-to",
    "-o",
    "--output",
    "--network",
    "--progress",
    "--iidfile",
    "--metadata-file",
    "--builder",
    "--add-host",
    "--annotation",
    "--attest",
    "--shm-size",
    "--ulimit",
    "--allow",
];

/// An image build: the Dockerfile it reads and its build context, as written relative to the workspace root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Build {
    /// The Dockerfile; `None` for the context's own `Dockerfile`.
    pub dockerfile: Option<String>,
    pub context: String,
}

/// The index of the first argument of a build command in `words`, past `docker build`, `docker buildx build`,
/// `docker image build` or `podman build`.
fn build_arguments(words: &[Word]) -> Option<usize> {
    let start = program(words)?;
    if !matches!(basename(&words[start].text), "docker" | "podman") {
        return None;
    }
    let text = |index: usize| words.get(index).map(|word| word.text.as_str());
    match (text(start + 1)?, text(start + 2)) {
        ("build", _) => Some(start + 2),
        ("buildx" | "image", Some("build")) => Some(start + 3),
        _ => None,
    }
}

/// The build a command runs, when it is a build with a static context and Dockerfile.
pub(crate) fn command_build(words: &[Word]) -> Option<Build> {
    let mut index = build_arguments(words)?;
    let mut dockerfile = None;
    let mut context = None;
    while let Some(word) = words.get(index) {
        let text = word.text.as_str();
        index += 1;
        if let Some(value) = text.strip_prefix("--file=") {
            dockerfile = Some((value.to_owned(), word.dynamic));
        } else if matches!(text, "-f" | "--file") {
            let value = words.get(index)?;
            dockerfile = Some((value.text.clone(), value.dynamic));
            index += 1;
        } else if let Some(value) = text
            .strip_prefix("-f")
            .filter(|value| !value.is_empty() && !value.starts_with('-'))
        {
            dockerfile = Some((value.to_owned(), word.dynamic));
        } else if VALUE_OPTIONS.contains(&text) {
            index += 1;
        } else if !text.starts_with('-') {
            context = Some((text.to_owned(), word.dynamic));
        }
    }
    let (context, dynamic) = context?;
    let remote = context.contains("://") || context.starts_with("git@");
    if dynamic || remote || dockerfile.as_ref().is_some_and(|(_, dynamic)| *dynamic) {
        return None;
    }
    Some(Build {
        dockerfile: dockerfile.map(|(path, _)| path),
        context,
    })
}

/// The builds of the services of a Compose file: `build: <context>`, or `build:` with `context` and `dockerfile`,
/// the Dockerfile joined to the context it is relative to. A file that is not valid YAML has none.
pub(crate) fn compose_builds(source: &str) -> Vec<Build> {
    let Ok(document) = serde_saphyr::from_str::<Value>(source) else {
        return Vec::new();
    };
    let Some(services) = document.get("services").and_then(Value::as_object) else {
        return Vec::new();
    };
    services
        .values()
        .filter_map(|service| match service.get("build")? {
            Value::String(context) => Some(Build {
                dockerfile: None,
                context: context.clone(),
            }),
            build => {
                let context = build.get("context").and_then(Value::as_str).unwrap_or(".");
                let dockerfile = build
                    .get("dockerfile")
                    .and_then(Value::as_str)
                    .map(|dockerfile| format!("{}/{dockerfile}", context.trim_end_matches('/')));
                Some(Build {
                    dockerfile,
                    context: context.to_owned(),
                })
            }
        })
        .collect()
}

/// `path`, relative to the workspace root, with `.` segments removed and `..` resolved; `None` when it is absolute,
/// leaves the root or is remote. The root is `""`.
pub(crate) fn normalized(path: &str) -> Option<String> {
    if path.starts_with('/') || path.contains("://") {
        return None;
    }
    let mut segments: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop()?;
            }
            segment => segments.push(segment),
        }
    }
    Some(segments.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(line: &str) -> Option<Build> {
        let tokens = skif_core::shell::tokenize(line);
        let words = tokens.segments.into_iter().next()?.words;
        command_build(&words)
    }

    fn expected(dockerfile: Option<&str>, context: &str) -> Build {
        Build {
            dockerfile: dockerfile.map(str::to_owned),
            context: context.to_owned(),
        }
    }

    #[test]
    fn build_commands_give_their_dockerfile_and_context() {
        let cases = [
            (
                "docker build -f docker/web.Dockerfile apps/web",
                Some(expected(Some("docker/web.Dockerfile"), "apps/web")),
            ),
            (
                "docker buildx build --file=docker/web.Dockerfile -t web:latest --push ./apps/web",
                Some(expected(Some("docker/web.Dockerfile"), "./apps/web")),
            ),
            (
                "sudo docker image build -fdocker/api.Dockerfile --build-arg X=1 apps/api",
                Some(expected(Some("docker/api.Dockerfile"), "apps/api")),
            ),
            ("podman build apps/web", Some(expected(None, "apps/web"))),
            ("docker build -t \"$TAG\" .", Some(expected(None, "."))),
            ("docker build -f \"$FILE\" apps/web", None),
            ("docker build \"$CONTEXT\"", None),
            ("docker build https://github.com/example/repo.git#main", None),
            ("docker build - < Dockerfile", None),
            ("docker run -v apps/web:/app node", None),
            ("docker buildx create --use", None),
            ("pnpm build apps/web", None),
        ];
        for (line, build_of) in cases {
            assert_eq!(build(line), build_of, "{line}");
        }
    }

    #[test]
    fn compose_services_give_their_build_contexts() {
        let source = "\
services:
  web:
    build:
      context: apps/web
      dockerfile: ../../docker/web.Dockerfile
  api:
    build: ./apps/api
  db:
    image: postgres:16
  worker:
    build:
      dockerfile: docker/worker.Dockerfile
";
        assert_eq!(
            compose_builds(source),
            [
                expected(Some("apps/web/../../docker/web.Dockerfile"), "apps/web"),
                expected(None, "./apps/api"),
                expected(Some("./docker/worker.Dockerfile"), "."),
            ]
        );
        assert_eq!(compose_builds("services: [unclosed\n"), []);
    }

    #[test]
    fn paths_are_normalized_within_the_root() {
        assert_eq!(normalized("./apps/web/").as_deref(), Some("apps/web"));
        assert_eq!(
            normalized("apps/web/../../docker/web.Dockerfile").as_deref(),
            Some("docker/web.Dockerfile")
        );
        assert_eq!(normalized(".").as_deref(), Some(""));
        assert_eq!(normalized("../outside"), None);
        assert_eq!(normalized("/abs/path"), None);
    }
}
