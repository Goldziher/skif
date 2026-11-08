//! `skif update` against a registry served from this process on 127.0.0.1: no request leaves the machine.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::{Arc, Mutex};

use support::{run, skif, tree};
use tempfile::TempDir;

/// A request the registry received: its path and `Authorization` header.
type Seen = Arc<Mutex<Vec<(String, Option<String>)>>>;

const OLD: &str = "2020-01-01T00:00:00.000Z";
const FUTURE: &str = "2999-01-01T00:00:00.000Z";

fn packument(versions: &[(&str, &str)]) -> String {
    let list: Vec<String> = versions
        .iter()
        .map(|(version, _)| format!("\"{version}\": {{}}"))
        .collect();
    let times: Vec<String> = versions
        .iter()
        .map(|(version, time)| format!("\"{version}\": \"{time}\""))
        .collect();
    format!(
        "{{\"versions\": {{{}}}, \"time\": {{{}}}}}",
        list.join(", "),
        times.join(", ")
    )
}

fn answer(path: &str) -> Option<String> {
    match path {
        "/react" => Some(packument(&[
            ("18.2.0", OLD),
            ("18.3.1", OLD),
            ("19.0.0", OLD),
            ("20.0.0", FUTURE),
        ])),
        "/@acme%2fui" => Some(packument(&[("1.0.0", OLD), ("1.2.0", OLD)])),
        _ => None,
    }
}

fn respond(mut stream: TcpStream, seen: &Seen) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    let path = request_line.split_whitespace().nth(1).unwrap_or_default().to_owned();
    let mut authorization = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("authorization")
        {
            authorization = Some(value.trim().to_owned());
        }
    }
    seen.lock().unwrap().push((path.clone(), authorization));
    let (status, body) = answer(&path).map_or(("404 Not Found", "{}".to_owned()), |body| ("200 OK", body));
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

/// Serves [`answer`] until the test process ends; returns the base URL and the requests seen.
fn registry() -> (String, Seen) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let seen: Seen = Arc::default();
    let record = Arc::clone(&seen);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            respond(stream, &record);
        }
    });
    (url, seen)
}

/// Answers every request with a 302 to the same path under `target`, until the test process ends; returns its base URL.
fn redirector(target: &str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let target = target.trim_end_matches('/').to_owned();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            if reader.read_line(&mut request_line).is_err() {
                continue;
            }
            let path = request_line.split_whitespace().nth(1).unwrap_or_default().to_owned();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
            }
            let _ = write!(
                stream,
                "HTTP/1.1 302 Found\r\nLocation: {target}{path}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
        }
    });
    url
}

const APP: &str = "{\n    \"name\": \"app\",\n    \"version\": \"1.0.0\",\n    \"dependencies\": {\n        \"react\": \"^18.2.0\",\n        \"@acme/ui\": \"1.0.0\"\n    }\n}\n";

fn workspace(npmrc: &str, app: &str) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("package.json"),
        "{\n  \"name\": \"root\",\n  \"private\": true,\n  \"workspaces\": [\"packages/*\"]\n}\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.path().join("packages/app")).unwrap();
    std::fs::write(dir.path().join("packages/app/package.json"), app).unwrap();
    std::fs::write(dir.path().join(".npmrc"), npmrc).unwrap();
    dir
}

/// `skif update` with no proxy, no user `.npmrc` and no registry from the environment.
fn update(dir: &Path) -> assert_cmd::Command {
    let mut command = skif();
    for name in [
        "HTTP_PROXY",
        "http_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
        "npm_config_registry",
        "NPM_CONFIG_REGISTRY",
    ] {
        command.env_remove(name);
    }
    command
        .env("NPM_CONFIG_USERCONFIG", dir.join("no-such-npmrc"))
        .arg("update")
        .arg(dir);
    command
}

#[test]
fn check_lists_the_updates_exits_one_and_writes_nothing() {
    let (url, seen) = registry();
    let dir = workspace(
        &format!("//127.0.0.1:{}/:_authToken=${{SKIF_TEST_TOKEN}}\n", url_port(&url)),
        APP,
    );
    let before = tree(dir.path());
    // The environment names the registry, so the project's `${VAR}` token may be sent to it; the project's own file
    // sets no http:// registry for the host, which would drop the token.
    let result = run(update(dir.path())
        .env("npm_config_registry", &url)
        .env("SKIF_TEST_TOKEN", "t0ken-value")
        .args(["--check", "--format", "json"]));
    assert_eq!(result.code, 1, "{}{}", result.stdout, result.stderr);
    assert_eq!(tree(dir.path()), before);
    let report = result.json();
    let updates: Vec<(&str, &str, &str)> = report["updates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|update| {
            (
                update["dependency"].as_str().unwrap(),
                update["current"].as_str().unwrap(),
                update["target"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        updates,
        [("react", "^18.2.0", "^19.0.0"), ("@acme/ui", "1.0.0", "1.2.0")],
        "20.0.0 is not old enough"
    );
    assert_eq!(report["updates"][0]["state"], "DiffersToNpmRegistry");
    assert_eq!(report["failures"], serde_json::json!([]));
    assert_eq!(report["notes"], serde_json::json!([]));
    let mut requests = seen.lock().unwrap().clone();
    requests.sort();
    assert_eq!(
        requests,
        [
            ("/@acme%2fui".to_owned(), Some("Bearer t0ken-value".to_owned())),
            ("/react".to_owned(), Some("Bearer t0ken-value".to_owned())),
        ]
    );
    assert!(!result.stdout.contains("t0ken-value") && !result.stderr.contains("t0ken-value"));
    assert_eq!(
        result
            .stderr
            .matches(&format!(
                "warning: sending the registry token for {url} over plain http"
            ))
            .count(),
        1,
        "once per registry: {}",
        result.stderr
    );
}

fn url_port(url: &str) -> &str {
    url.trim_end_matches('/').rsplit(':').next().unwrap()
}

#[test]
fn update_writes_the_new_versions_and_keeps_the_formatting() {
    let (url, _) = registry();
    let dir = workspace(&format!("registry={url}\n"), APP);
    let result = run(update(dir.path()).args(["--target", "minor", "--no-install"]));
    assert_eq!(result.code, 0, "{}{}", result.stdout, result.stderr);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("packages/app/package.json")).unwrap(),
        APP.replace("^18.2.0", "^18.3.1").replace("\"1.0.0\"\n", "\"1.2.0\"\n")
    );
    assert!(
        result.stdout.contains("wrote packages/app/package.json"),
        "{}",
        result.stdout
    );
    let again = run(update(dir.path()).args(["--check", "--target", "minor"]));
    assert_eq!(again.code, 0, "{}{}", again.stdout, again.stderr);
    assert!(again.stdout.starts_with("No updates"), "{}", again.stdout);
}

#[test]
fn a_package_the_registry_does_not_know_makes_the_run_incomplete() {
    let (url, _) = registry();
    let app = APP.replace("\"@acme/ui\"", "\"@private/thing\"");
    let dir = workspace(&format!("registry={url}\n"), &app);
    let result = run(update(dir.path()).args(["--check", "--format", "json", "--dependencies", "@private/*"]));
    assert_eq!(result.code, 2, "{}{}", result.stdout, result.stderr);
    let report = result.json();
    assert_eq!(report["failures"][0]["dependency"], "@private/thing");
    assert_eq!(report["failures"][0]["message"], "not found in the registry");
    assert_eq!(report["queried"], 1, "--dependencies leaves react out");
}

#[test]
fn an_unreachable_registry_exits_two_and_writes_nothing() {
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let dir = workspace(&format!("registry=http://127.0.0.1:{port}/\n"), APP);
    let before = tree(dir.path());
    let result = run(update(dir.path()).arg("--no-install"));
    assert_eq!(result.code, 2, "{}{}", result.stdout, result.stderr);
    assert!(
        result.stdout.contains("Could not check 2 packages"),
        "{}",
        result.stdout
    );
    assert!(result.stdout.contains("cannot reach the registry"), "{}", result.stdout);
    assert_eq!(tree(dir.path()), before);
}

#[test]
fn writing_is_refused_when_ci_is_truthy_but_check_runs() {
    let dir = workspace(
        "",
        "{\"name\": \"app\", \"dependencies\": {\"lib\": \"workspace:*\"}}\n",
    );
    let refused = run(update(dir.path()).env("CI", "true").args(["--format", "json"]));
    assert_eq!(refused.code, 2, "{}{}", refused.stdout, refused.stderr);
    assert_eq!(refused.json()["error"]["kind"], "ci-refusal");
    let check = run(update(dir.path()).env("CI", "true").arg("--check"));
    assert_eq!(
        check.code, 0,
        "nothing to look up, so no request: {}{}",
        check.stdout, check.stderr
    );
    assert!(
        check.stdout.starts_with("No updates: 0 packages checked"),
        "{}",
        check.stdout
    );
}

#[test]
fn a_project_token_from_the_environment_is_not_sent_to_an_http_registry_the_project_sets() {
    let (url, seen) = registry();
    let dir = workspace(
        &format!(
            "registry={url}\n//127.0.0.1:{}/:_authToken=${{SKIF_TEST_TOKEN}}\n",
            url_port(&url)
        ),
        APP,
    );
    let result = run(update(dir.path())
        .env("npm_config_registry", &url)
        .env("SKIF_TEST_TOKEN", "t0ken-value")
        .args(["--check", "--format", "json"]));
    assert_eq!(result.code, 1, "{}{}", result.stdout, result.stderr);
    let requests = seen.lock().unwrap().clone();
    assert_eq!(requests.len(), 2, "{requests:?}");
    assert!(
        requests.iter().all(|(_, authorization)| authorization.is_none()),
        "{requests:?}"
    );
    assert!(
        result.stderr.contains("sets an http:// registry for 127.0.0.1:"),
        "{}",
        result.stderr
    );
    assert!(!result.stdout.contains("t0ken-value") && !result.stderr.contains("t0ken-value"));
}

#[test]
fn a_redirect_to_another_host_does_not_carry_the_token() {
    let (url, seen) = registry();
    let first = redirector(&url);
    let dir = workspace(
        &format!("//127.0.0.1:{}/:_authToken=t0ken-value\n", url_port(&first)),
        APP,
    );
    let result = run(update(dir.path())
        .env("npm_config_registry", &first)
        .args(["--check", "--format", "json"]));
    assert_eq!(
        result.code, 1,
        "the redirect is followed: {}{}",
        result.stdout, result.stderr
    );
    let requests = seen.lock().unwrap().clone();
    assert_eq!(requests.len(), 2, "{requests:?}");
    assert!(
        requests.iter().all(|(_, authorization)| authorization.is_none()),
        "the second port is another origin: {requests:?}"
    );
}

#[test]
fn a_registry_document_over_64_mib_is_refused() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
            }
            // A valid document padded with whitespace one byte past the limit.
            let head = b"{\"versions\": {}}";
            let padding = 64 * 1024 * 1024 + 1 - head.len();
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                head.len() + padding
            );
            let _ = stream.write_all(head);
            let chunk = vec![b' '; 1024 * 1024];
            let mut left = padding;
            while left > 0 {
                let size = left.min(chunk.len());
                if stream.write_all(&chunk[..size]).is_err() {
                    break;
                }
                left -= size;
            }
        }
    });
    let dir = workspace(&format!("registry={url}\n"), APP);
    let result = run(update(dir.path()).args(["--check", "--format", "json", "--dependencies", "react"]));
    assert_eq!(result.code, 2, "{}{}", result.stdout, result.stderr);
    assert_eq!(result.json()["failures"][0]["dependency"], "react");
}

#[test]
fn a_project_token_from_the_environment_is_not_sent_to_a_host_the_user_did_not_configure() {
    let (url, seen) = registry();
    let dir = workspace(
        &format!(
            "registry={url}\n//127.0.0.1:{}/:_authToken=${{SKIF_TEST_TOKEN}}\n",
            url_port(&url)
        ),
        APP,
    );
    let result = run(update(dir.path())
        .env("SKIF_TEST_TOKEN", "t0ken-value")
        .args(["--check", "--format", "json"]));
    assert_eq!(result.code, 1, "{}{}", result.stdout, result.stderr);
    let requests = seen.lock().unwrap().clone();
    assert_eq!(requests.len(), 2, "{requests:?}");
    assert!(
        requests.iter().all(|(_, authorization)| authorization.is_none()),
        "{requests:?}"
    );
    let host = format!("127.0.0.1:{}", url_port(&url));
    assert!(
        result
            .stderr
            .contains(&format!("warning: ignoring the _authToken for {host}")),
        "{}",
        result.stderr
    );
    assert!(!result.stdout.contains("t0ken-value") && !result.stderr.contains("t0ken-value"));
}
